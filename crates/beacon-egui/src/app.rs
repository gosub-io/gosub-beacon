//! The egui frontend: chrome, viewport, and the loop that turns gestures into
//! [`BeaconCommand`]s and [`BeaconEvent`]s into pixels.
//!
//! Everything about *what the browser does* lives in `beacon-core`; this file is only
//! about drawing it and collecting input. Where it reaches past core — sending
//! `TabCommand` straight to a tab handle for pointer and viewport events — that is
//! per-frame input plumbing the command seam has no opinion about yet.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use beacon_core::beacon::{Beacon, DRAW_FPS};
use beacon_core::command::BeaconCommand;
use beacon_core::engine::BrowserEngine;
use beacon_core::event::{BeaconEvent, Cursor};
use beacon_core::tab::{GosubTab, GosubTabManager, TabId};
use eframe::CreationContext;
use gosub_engine::events::{EngineEvent, MouseButton, TabCommand};
use gosub_render_pipeline::render::backend::ExternalHandle;
use gosub_render_pipeline::render::{argb_u32_to_rgba8, composite_tiles, TileTarget};
use gosub_renderer_vello::{VelloBackend, WgpuContextProvider};
use tokio::runtime::Runtime;

use crate::chrome::{self, Favicons};
use crate::context::EguiContextProvider;
use crate::platform::EguiPlatform;

/// Phone layout: no tab strip and no bookmarks bar, which a phone has no room for. Tabs are
/// behind a button that opens a list of them instead.
const COMPACT: bool = cfg!(target_os = "android");

/// Fling (kinetic scroll) tuning. The release speed is measured over the last `FLING_WINDOW`
/// seconds of the swipe; a finger held still that long before lifting does not fling.
const FLING_WINDOW: f64 = 0.1;
/// How much velocity a fling keeps per millisecond: iOS's "normal" deceleration rate.
const FLING_DECAY: f32 = 0.998;
/// Slower than this at release (points per second) is a drag that ended, not a flick.
const FLING_MIN: f32 = 150.0;
/// A fling has stopped once it is slower than this.
const FLING_STOP: f32 = 20.0;
/// Upper bound on a release speed, against a stray sample pair a few microseconds apart.
const FLING_MAX: f32 = 8000.0;

/// The velocity a finger lifted off with, in points per second, from its last positions, or
/// `None` when that is no flick: too slow, or the finger had stopped before it lifted.
fn fling_velocity(trail: &std::collections::VecDeque<(f64, egui::Pos2)>, now: f64) -> Option<egui::Vec2> {
    let recent: Vec<_> = trail.iter().filter(|(t, _)| now - t <= FLING_WINDOW).collect();
    let (&&(t0, p0), &&(t1, p1)) = (recent.first()?, recent.last()?);
    // The finger must still have been moving when it lifted, over a span long enough to
    // measure: two samples in one frame say nothing about speed.
    if now - t1 > FLING_WINDOW / 2.0 || t1 - t0 < 0.01 {
        return None;
    }
    let velocity = (p1 - p0) / (t1 - t0) as f32;
    let speed = velocity.length();
    (speed >= FLING_MIN).then(|| velocity * (speed.min(FLING_MAX) / speed))
}

/// Bookmark bar text size, in points. A phone needs it big enough to hit with a finger.
const BOOKMARK_TEXT: f32 = if cfg!(target_os = "android") { 16.0 } else { 12.0 };

/// Link-hover status text size, in points.
const STATUS_TEXT: f32 = if cfg!(target_os = "android") { 14.0 } else { 11.0 };

/// This frontend's render configuration: Vello on egui's own wgpu device.
pub type EguiConfig = gosub_engine::DefaultRenderConfig<VelloBackend<EguiContextProvider>>;

/// Per-tab drawing state. Scroll lives here rather than in core because it is a property of
/// how this frontend is presenting the page, and the engine owns the authoritative value.
#[derive(Default)]
struct TabView {
    /// CPU tile-cache path.
    cpu_texture: Option<egui::TextureHandle>,
    /// (engine wgpu texture id, egui handle) for the GPU path. Keyed on the texture id so it
    /// is re-registered only when the texture itself changes — on resize, not every frame.
    gpu_texture: Option<(u64, egui::TextureId)>,
    scroll_x: f32,
    scroll_y: f32,
    page_height: f32,
    viewport: Option<(u32, u32)>,
}

pub struct BeaconApp {
    rt: &'static Runtime,
    /// The texture registry Vello renders into; the GPU path resolves ids through it.
    context: Arc<EguiContextProvider>,
    engine: BrowserEngine<EguiConfig>,
    beacon: Beacon,
    tabs: Arc<Mutex<GosubTabManager>>,
    event_rx: tokio::sync::broadcast::Receiver<EngineEvent>,
    resource_rx: tokio::sync::broadcast::Receiver<gosub_engine::events::ResourceUpdate>,

    views: HashMap<TabId, TabView>,
    address_bar: String,
    /// True while the address bar is being edited, so engine updates do not fight the caret.
    address_bar_focused: bool,
    status: String,
    cursor: Cursor,
    log: Vec<String>,
    favicons: Favicons,
    /// Bookmarks, read from the engine's places store once at startup.
    bookmarks: Vec<(String, String)>,
    /// The finger scrolling the page, and where it was last seen.
    touch: Option<(egui::TouchId, egui::Pos2)>,
    /// The activity strip over the page, while it is switched on (Show activity).
    activity: Option<chrome::ActivityStrip>,
    /// The loading bar along the top of the page.
    progress: chrome::LoadingBar,
    /// Per tab, the queue that forwards [`Self::send_active`]'s commands in order.
    senders: std::cell::RefCell<HashMap<TabId, tokio::sync::mpsc::UnboundedSender<TabCommand>>>,
    /// The tab list is showing in place of the page (compact layout).
    tab_list_open: bool,
    /// The finger's recent positions, with their times, for the speed it lifts off at.
    touch_trail: std::collections::VecDeque<(f64, egui::Pos2)>,
    /// A scroll still going after the finger let go: velocity in points per second, and when
    /// it was last advanced.
    fling: Option<(egui::Vec2, f64)>,
    /// Where the mouse was over the page last frame, so only real movement reaches the engine.
    last_pointer: Option<egui::Pos2>,
}

impl BeaconApp {
    pub fn new(cc: &CreationContext<'_>, rt: &'static Runtime, urls: Vec<String>) -> anyhow::Result<Self> {
        // Deliberately NOT holding `rt.enter()` across this function. `BrowserEngine::new`
        // enters the runtime for its own setup, and `create_tab` below does a `block_on` --
        // which panics if it runs while a runtime context is already entered. Holding a
        // guard here stalls startup before the first frame, with an empty log.
        #[cfg(target_os = "android")]
        crate::android::touch_style(&cc.egui_ctx);

        let context = Arc::new(
            EguiContextProvider::from_eframe(cc)
                .ok_or_else(|| anyhow::anyhow!("eframe is not running its wgpu renderer; Beacon's egui frontend needs it"))?,
        );
        // On a phone, Vello rendering the whole HiDPI viewport takes longer than a frame (~55 ms
        // on a Fairphone 6), so every scroll step lagged. The tile pipeline rasterizes once and
        // scrolls by compositing (~12 ms). The desktop keeps the scene path for now.
        let backend = VelloBackend::new(context.clone())
            .map_err(|e| anyhow::anyhow!("Vello backend: {e:?}"))?
            .with_gpu_tiles(cfg!(target_os = "android"));

        let mut engine = BrowserEngine::<EguiConfig>::new(rt, false, Arc::new(backend))?;
        let event_rx = engine
            .take_event_rx()
            .ok_or_else(|| anyhow::anyhow!("engine event stream already taken"))?;
        let resource_rx = engine
            .take_resource_rx()
            .ok_or_else(|| anyhow::anyhow!("engine resource stream already taken"))?;

        // Repaint whenever a frame is composited. The compositor's notification is the only
        // thing that knows a page changed, so without this egui would idle and the page
        // would appear frozen until the pointer moved.
        if let Some(mut redraw_rx) = engine.take_redraw_rx() {
            let ctx = cc.egui_ctx.clone();
            rt.spawn(async move {
                while redraw_rx.recv().await.is_some() {
                    ctx.request_repaint();
                }
            });
        }

        let tabs = Arc::new(Mutex::new(GosubTabManager::new()));
        let beacon = Beacon::new(tabs.clone(), rt.handle().clone(), Rc::new(EguiPlatform::new(cc.egui_ctx.clone())));

        let mut app = Self {
            rt,
            context,
            engine,
            beacon,
            tabs,
            event_rx,
            resource_rx,
            views: HashMap::new(),
            address_bar: String::new(),
            address_bar_focused: false,
            status: String::new(),
            cursor: Cursor::Default,
            log: Vec::new(),
            favicons: Favicons::default(),
            bookmarks: Vec::new(),
            touch: None,
            last_pointer: None,
            touch_trail: Default::default(),
            fling: None,
            tab_list_open: false,
            senders: Default::default(),
            progress: Default::default(),
            activity: None,
        };
        app.bookmarks = app.engine.places().bookmarks().into_iter().map(|b| (b.title, b.url)).collect();

        let urls = if urls.is_empty() { vec!["gosub://home".to_string()] } else { urls };
        for url in urls {
            app.open_tab(&url);
        }
        log::info!("beacon-egui ready with {} tab(s)", app.tabs.lock().unwrap().tab_count());
        Ok(app)
    }

    /// Open a tab on `url`, creating the engine tab behind it.
    fn open_tab(&mut self, url: &str) -> Option<TabId> {
        let (_mode, url) = beacon_core::address_parser::GosubAddressParser::parse(url).ok()?;

        let mut tab = GosubTab::new(url.clone(), url.as_str());
        let handle = match self.engine.create_tab(self.rt, url.as_str(), Some((1024, 768))) {
            Ok(handle) => handle,
            Err(e) => {
                self.log.push(format!("could not create engine tab: {e}"));
                return None;
            }
        };
        let engine_id = handle.tab_id;
        tab.set_tab_handle(handle.clone());
        tab.set_loading(true);

        let tab_id = tab.id();
        self.tabs.lock().unwrap().add_tab(tab, None);
        self.beacon.bind_engine_tab(engine_id, tab_id);
        self.beacon.mru_mut().insert_unused(tab_id);
        self.views.insert(tab_id, TabView::default());

        let target = url.to_string();
        log::debug!("open_tab: navigating engine tab {engine_id:?} to {target}");
        self.rt.spawn(async move {
            if let Err(e) = handle.send(TabCommand::Navigate { url: target }).await {
                log::warn!("navigate command was not delivered: {e:?}");
            }
            let _ = handle.send(TabCommand::ResumeDrawing { fps: DRAW_FPS }).await;
        });
        Some(tab_id)
    }

    fn active(&self) -> Option<TabId> {
        self.tabs.lock().unwrap().active()
    }

    fn active_handle(&self) -> Option<gosub_engine::tab::TabHandle> {
        let id = self.active()?;
        self.tabs.lock().unwrap().get_tab(id)?.tab_handle()
    }

    /// Send a raw engine command to the active tab. Used for pointer, key and viewport
    /// traffic, which is this frontend's own input plumbing rather than a browser decision.
    ///
    /// In order: a tap is a move, a press and a release, and the engine follows the link under
    /// the pointer as of the press. Sent from a task each, they raced, and a press that beat its
    /// move followed whatever link the pointer was last over -- or none. So each tab gets one
    /// task that forwards its commands as they were queued.
    fn send_active(&self, command: TabCommand) {
        let Some(tab_id) = self.active() else { return };
        let mut senders = self.senders.borrow_mut();
        if let std::collections::hash_map::Entry::Vacant(entry) = senders.entry(tab_id) {
            let Some(handle) = self.active_handle() else { return };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TabCommand>();
            self.rt.spawn(async move {
                while let Some(command) = rx.recv().await {
                    let _ = handle.send(command).await;
                }
            });
            entry.insert(tx);
        }
        if let Some(tx) = senders.get(&tab_id) {
            let _ = tx.send(command);
        }
    }

    /// Send a command and then make sure the tab is actually drawing. Drawing is suspended
    /// until asked, and neither navigating nor resizing resumes it on its own.
    fn send_active_and_draw(&self, command: TabCommand) {
        self.send_active(command);
        self.send_active(TabCommand::ResumeDrawing { fps: DRAW_FPS });
    }

    /// Switch to a tab: record it, promote it in the MRU list, and follow the address bar.
    fn activate(&mut self, tab_id: TabId) {
        // A fling belongs to the page it started on, and so does a loading bar.
        self.fling = None;
        self.progress = Default::default();
        self.tabs.lock().unwrap().mark_active(tab_id);
        self.beacon.mru_mut().touch(tab_id);
        if let Some(tab) = self.tabs.lock().unwrap().get_tab(tab_id) {
            self.address_bar = tab.url().to_string();
        }
    }

    /// Close a tab, shutting down its engine worker. Refuses the last one, as GTK does --
    /// a browser with no tabs has nothing to show and no way back.
    fn close_tab(&mut self, tab_id: TabId) {
        if self.tabs.lock().unwrap().tab_count() <= 1 {
            return;
        }
        if let Some(handle) = self.tabs.lock().unwrap().get_tab(tab_id).and_then(|t| t.tab_handle()) {
            self.beacon.unbind_engine_tab(handle.tab_id);
            self.rt.spawn(async move {
                let _ = handle.send(TabCommand::CloseTab).await;
            });
        }
        if let Some(tab) = self.tabs.lock().unwrap().get_tab(tab_id) {
            self.beacon.closed_mut().push(tab.url().to_string(), None);
        }
        self.tabs.lock().unwrap().remove_tab(tab_id);
        self.beacon.mru_mut().forget(tab_id);
        self.views.remove(&tab_id);
        // Dropping the sender ends the tab's forwarding task.
        self.senders.borrow_mut().remove(&tab_id);
        self.favicons.forget(tab_id);
        // remove_tab hands over to a neighbour; follow it so the address bar agrees.
        let next = self.tabs.lock().unwrap().active();
        if let Some(next) = next {
            self.activate(next);
        }
    }

    fn dispatch(&mut self, command: BeaconCommand) {
        let events = self.beacon.apply(command);
        self.absorb(events);
    }

    /// Drain everything the engine has said since the last frame.
    fn pump_engine(&mut self) {
        loop {
            match self.resource_rx.try_recv() {
                Ok(update) => self.beacon.on_resource_update(update),
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        loop {
            match self.event_rx.try_recv() {
                Ok(event) => {
                    log::debug!("engine event: {}", engine_event_name(&event));
                    let out = self.beacon.on_engine_event(event);
                    self.absorb(out);
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    }

    /// Reflect what core said changed.
    fn absorb(&mut self, events: Vec<BeaconEvent>) {
        for event in events {
            match event {
                BeaconEvent::UrlChanged(tab_id, url) => {
                    if self.active() == Some(tab_id) && !self.address_bar_focused {
                        self.address_bar = url.to_string();
                    }
                }
                BeaconEvent::HoverUrl(tab_id, url) => {
                    if self.active() == Some(tab_id) {
                        self.status = url.unwrap_or_default();
                    }
                }
                BeaconEvent::CursorChanged(tab_id, cursor) => {
                    if self.active() == Some(tab_id) {
                        self.cursor = cursor;
                    }
                }
                BeaconEvent::Log(message) => {
                    log::warn!("{message}");
                    self.log.push(message);
                }
                BeaconEvent::TabCrashed(tab_id, _) => {
                    self.views.remove(&tab_id);
                    self.senders.borrow_mut().remove(&tab_id);
                    self.favicons.forget(tab_id);
                }
                BeaconEvent::FaviconChanged(tab_id) => self.favicons.forget(tab_id),
                // Only the active tab's load is reported. `None` is the load ending, which
                // runs the bar to the end before it goes.
                BeaconEvent::LoadProgress(tab_id, fraction) => {
                    if self.active() == Some(tab_id) {
                        self.progress.set(fraction.map(|f| f as f32));
                    }
                }
                // The tab strip, the buttons and the viewport are all rebuilt from current
                // state every frame, so these need no separate handling in an immediate-mode
                // UI -- unlike GTK, where each one has a widget to poke.
                BeaconEvent::Redraw
                | BeaconEvent::PickerRequested { .. }
                | BeaconEvent::TabsChanged
                | BeaconEvent::ActiveTabChanged(_)
                | BeaconEvent::TitleChanged(..)
                | BeaconEvent::LoadingChanged(..)
                | BeaconEvent::NavStateChanged(_)
                | BeaconEvent::NavigationFailed(..)
                | BeaconEvent::DownloadOffered { .. }
                | BeaconEvent::DownloadChanged(_) => {}
            }
        }
    }

    /// Turn the latest composited frame for `tab_id` into something egui can paint.
    fn refresh_texture(&mut self, tab_id: TabId, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let Some(engine_id) = self.tabs.lock().unwrap().get_tab(tab_id).and_then(|t| t.engine_tab_id()) else {
            return;
        };
        let Some(handle) = self.engine.compositor.frame_for(engine_id) else {
            log::debug!("no composited frame yet for engine tab {engine_id:?}");
            return;
        };
        let Some(view) = self.views.get_mut(&tab_id) else { return };

        match handle {
            ExternalHandle::TileCache {
                tiles,
                dpr,
                viewport_width,
                viewport_height,
                page_height,
                ..
            } => {
                log::debug!(
                    "frame: TileCache {} tile(s) {viewport_width}x{viewport_height} dpr={dpr} page_height={page_height}",
                    tiles.len()
                );
                view.page_height = page_height;
                let w = (viewport_width * dpr) as usize;
                let h = (viewport_height * dpr) as usize;
                if w == 0 || h == 0 {
                    return;
                }
                // Composite onto opaque white at the local (immediate) scroll, then convert to
                // RGBA8 for egui. Going through the shared compositor rather than doing the
                // scroll maths here is what gets `sticky` and `fixed` right.
                let mut buf = vec![0xFFFF_FFFFu32; w * h];
                composite_tiles(
                    &tiles,
                    dpr,
                    (view.scroll_x, view.scroll_y),
                    &mut TileTarget {
                        buf: &mut buf,
                        stride: w,
                        origin_x: 0,
                        origin_y: 0,
                        width: w,
                        height: h,
                    },
                );
                let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &argb_u32_to_rgba8(&buf));
                match &mut view.cpu_texture {
                    Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                    None => view.cpu_texture = Some(ctx.load_texture("page", image, egui::TextureOptions::LINEAR)),
                }
            }

            ExternalHandle::WgpuTextureId { id, .. } => {
                log::debug!("frame: WgpuTextureId {id}");
                // Re-register only when the wgpu texture itself changes. The engine renders
                // into the same texture every frame, so keying on anything per-frame would
                // churn a bind-group free+register each time and wreck the frame rate.
                if view.gpu_texture.as_ref().map(|(known, _)| *known == id).unwrap_or(false) {
                    return;
                }
                let Some(state) = frame.wgpu_render_state() else { return };
                let Some((_, texture_view)) = self.context.get_texture(id) else {
                    return;
                };
                if let Some((_, old)) = view.gpu_texture.take() {
                    state.renderer.write().free_texture(&old);
                }
                let registered = state.renderer.write().register_native_texture(
                    self.context.device_ref(),
                    &texture_view,
                    eframe::wgpu::FilterMode::Linear,
                );
                view.gpu_texture = Some((id, registered));
            }

            other => log::debug!("frame: unhandled handle {other:?}"),
        }
    }
}

/// Variant name only -- engine events are chatty and their payloads are large.
fn engine_event_name(event: &EngineEvent) -> &'static str {
    match event {
        EngineEvent::Redraw { .. } => "Redraw",
        EngineEvent::Navigation { .. } => "Navigation",
        EngineEvent::TitleChanged { .. } => "TitleChanged",
        EngineEvent::FavIconChanged { .. } => "FavIconChanged",
        EngineEvent::HoverUrl { .. } => "HoverUrl",
        EngineEvent::CursorChanged { .. } => "CursorChanged",
        EngineEvent::TabCrashed { .. } => "TabCrashed",
        _ => "other",
    }
}

impl eframe::App for BeaconApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Read once: the panel frames below need this while `ui` is borrowed mutably.
        let faint = ui.visuals().faint_bg_color;
        self.pump_engine();

        let Some(active) = self.active() else { return };

        // View > Show Activity's shortcut in the GTK frontend.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::A)) {
            self.toggle_activity();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::BrowserBack)) {
            self.back_key(&ctx, active);
        }

        // The window runs under the status and navigation bars on Android; keep the chrome
        // and the page out from under them. Added first, so they are the outermost panels.
        #[cfg(target_os = "android")]
        if let Some((_, top, _, bottom)) = crate::android::content_rect() {
            let ppp = ctx.pixels_per_point();
            let screen = ctx.content_rect().height() * ppp;
            inset(ui, egui::Panel::top("inset-top"), top as f32 / ppp, faint);
            inset(ui, egui::Panel::bottom("inset-bottom"), (screen - bottom as f32) / ppp, faint);
        }

        // ── scroll ────────────────────────────────────────────────────────
        // Raw wheel events, not egui's smoothed delta: the engine smooths scrolling itself,
        // and forwarding an already-smoothed value double-smooths it into a slow ramp.
        // Trackpad deltas are already smooth and go to the engine as precise, unanimated.
        let (scroll, precise) = ctx.input(|i| {
            let mut acc = egui::Vec2::ZERO;
            let mut precise = true;
            for event in &i.events {
                if let egui::Event::MouseWheel { unit, delta, .. } = event {
                    let scale = match unit {
                        egui::MouseWheelUnit::Line => 134.0,
                        // A wheel notch arrives as a whole-number Point delta; a trackpad
                        // sends fractional ones. Scale the former, pass the latter through.
                        egui::MouseWheelUnit::Point => {
                            if delta.x.fract() == 0.0 && delta.y.fract() == 0.0 {
                                134.0
                            } else {
                                1.0
                            }
                        }
                        egui::MouseWheelUnit::Page => 800.0,
                    };
                    precise &= scale == 1.0;
                    acc += *delta * scale;
                }
            }
            (acc, precise)
        });
        if scroll != egui::Vec2::ZERO {
            let (dx, dy) = (-scroll.x, -scroll.y);
            if let Some(view) = self.views.get_mut(&active) {
                let max_y = (view.page_height - view.viewport.map(|(_, h)| h as f32).unwrap_or(0.0)).max(0.0);
                view.scroll_x = (view.scroll_x + dx).max(0.0);
                view.scroll_y = (view.scroll_y + dy).clamp(0.0, max_y);
            }
            self.send_active(TabCommand::MouseScroll {
                delta_x: dx,
                delta_y: dy,
                precise,
            });
        }

        self.refresh_texture(active, &ctx, frame);

        // ── tab strip ─────────────────────────────────────────────────────
        if !COMPACT {
            egui::Panel::top("tabs")
                .frame(egui::Frame::default().fill(faint).inner_margin(egui::Margin {
                    left: 6,
                    right: 6,
                    top: 4,
                    bottom: 0,
                }))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let order = self.tabs.lock().unwrap().order();
                        // Share the strip between tabs, down to a floor -- past that they would
                        // be unreadable, and a scrolling strip is the lesser evil.
                        let count = order.len().max(1) as f32;
                        let room = ui.available_width() - 34.0;
                        let width = (room / count).clamp(90.0, 240.0);

                        let mut action = None;
                        for tab_id in order {
                            let Some(tab) = self.tabs.lock().unwrap().get_tab(tab_id) else {
                                continue;
                            };
                            let icon = self.favicons.get(&ctx, tab_id, tab.favicon());
                            let title = if tab.title().is_empty() { tab.url().as_str() } else { tab.title() };
                            let (response, closed) =
                                chrome::tab(ui, title, icon.as_ref(), tab.is_loading(), Some(tab_id) == self.active(), width);
                            let response = response.on_hover_text(tab.url().as_str());
                            if closed {
                                action = Some(chrome::TabAction::Close(tab_id));
                            } else if response.clicked() {
                                action = Some(chrome::TabAction::Activate(tab_id));
                            }
                        }
                        if ui
                            .add(egui::Button::new(egui::RichText::new("+").size(16.0)).frame(false))
                            .on_hover_text("New tab")
                            .clicked()
                        {
                            if let Some(id) = self.open_tab("gosub://home") {
                                action = Some(chrome::TabAction::Activate(id));
                            }
                        }

                        match action {
                            Some(chrome::TabAction::Activate(tab_id)) => self.activate(tab_id),
                            Some(chrome::TabAction::Close(tab_id)) => self.close_tab(tab_id),
                            None => {}
                        }
                    });
                });
        }

        // ── toolbar ───────────────────────────────────────────────────────
        egui::Panel::top("toolbar")
            .frame(egui::Frame::default().inner_margin(egui::Margin::symmetric(8, 6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (can_back, can_forward, loading, url) = {
                        let tabs = self.tabs.lock().unwrap();
                        match tabs.get_tab(active) {
                            Some(tab) => (
                                tab.history().can_go_back(),
                                tab.history().can_go_forward(),
                                tab.is_loading(),
                                tab.url().clone(),
                            ),
                            None => return,
                        }
                    };

                    if chrome::tool_button(ui, "\u{23f4}", "Back", can_back).clicked() {
                        self.dispatch(BeaconCommand::Back);
                    }
                    // Compact keeps back and reload; forward, home and the star give their
                    // room to the address bar.
                    if !COMPACT && chrome::tool_button(ui, "\u{23f5}", "Forward", can_forward).clicked() {
                        self.dispatch(BeaconCommand::Forward(None));
                    }
                    if loading {
                        if chrome::tool_button(ui, "\u{1f5d9}", "Stop", true).clicked() {
                            self.dispatch(BeaconCommand::Stop);
                        }
                    } else if chrome::tool_button(ui, "\u{21bb}", "Reload", true).clicked() {
                        self.dispatch(BeaconCommand::Reload { ignore_cache: false });
                    }
                    if !COMPACT && chrome::tool_button(ui, "\u{1f3e0}", "Home", true).clicked() {
                        self.navigate_active("gosub://home");
                    }
                    ui.add_space(4.0);

                    // The address bar takes the room left after the trailing controls, so
                    // they stay put instead of drifting with the URL length.
                    let trailing = if COMPACT {
                        2.0 * (ui.spacing().interact_size.y + ui.spacing().item_spacing.x)
                    } else {
                        30.0
                    };
                    let response = ui.add_sized(
                        [ui.available_width() - trailing, ui.spacing().interact_size.y],
                        egui::TextEdit::singleline(&mut self.address_bar)
                            .hint_text("Search or enter address")
                            .vertical_align(egui::Align::Center),
                    );
                    self.address_bar_focused = response.has_focus();
                    // Select the whole address on focus, as browsers do, so typing replaces it.
                    if response.gained_focus() {
                        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                            let end = egui::text::CCursor::new(self.address_bar.chars().count());
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
                            state.store(ui.ctx(), response.id);
                        }
                    }
                    #[cfg(target_os = "android")]
                    if response.gained_focus() || response.lost_focus() {
                        crate::android::show_keyboard(response.gained_focus());
                    }
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let target = self.address_bar.clone();
                        self.navigate_active(&target);
                    }

                    if COMPACT {
                        let count = self.tabs.lock().unwrap().tab_count();
                        if chrome::tab_count_button(ui, count).clicked() {
                            self.tab_list_open = !self.tab_list_open;
                        }
                        // Frameless like the other toolbar buttons.
                        let menu = egui::Button::new(egui::RichText::new("\u{2630}").size(18.0))
                            .frame(false)
                            .min_size(egui::Vec2::splat(ui.spacing().interact_size.y));
                        egui::containers::menu::MenuButton::from_button(menu).ui(ui, |ui| {
                            let mut showing = self.activity.is_some();
                            if ui.checkbox(&mut showing, "Show activity").changed() {
                                self.toggle_activity();
                                ui.close();
                            }
                        });
                    } else {
                        let bookmarked = self.bookmarks.iter().any(|(_, b)| b.as_str() == url.as_str());
                        let star = if bookmarked { "\u{2605}" } else { "\u{2606}" };
                        chrome::tool_button(ui, star, "Bookmark this page", true);
                    }
                });
            });

        // ── bookmarks bar ─────────────────────────────────────────────────
        if !COMPACT && !self.bookmarks.is_empty() {
            egui::Panel::top("bookmarks")
                .frame(egui::Frame::default().inner_margin(egui::Margin {
                    left: 10,
                    right: 8,
                    top: 0,
                    bottom: 5,
                }))
                .show(ui, |ui| {
                    // Scrolls sideways once the bookmarks outgrow the window, which on a
                    // phone is after three or four of them.
                    let mut go = None;
                    egui::ScrollArea::horizontal()
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                for (title, url) in &self.bookmarks {
                                    if ui
                                        .add(egui::Button::new(egui::RichText::new(title).size(BOOKMARK_TEXT)).frame(false))
                                        .on_hover_text(url)
                                        .clicked()
                                    {
                                        go = Some(url.clone());
                                    }
                                }
                            });
                        });
                    if let Some(url) = go {
                        self.navigate_active(&url);
                    }
                });
        }

        // ── tab list, in place of the page (compact) ──────────────────────
        if self.tab_list_open {
            self.tab_list(ui, &ctx);
            return;
        }

        // ── page ──────────────────────────────────────────────────────────
        egui::CentralPanel::default().show(ui, |ui| {
            let size = ui.available_size();
            if size.x > 1.0 && size.y > 1.0 {
                let wanted = (size.x as u32, size.y as u32);
                let changed = self.views.get(&active).and_then(|v| v.viewport) != Some(wanted);
                if changed {
                    if let Some(view) = self.views.get_mut(&active) {
                        view.viewport = Some(wanted);
                    }
                    // Rasterize at the display's real resolution. Without this the page is
                    // drawn at 1x and stretched onto a HiDPI surface, which reads as blurry
                    // text rather than as the scaling bug it is.
                    let raster_dpr = (ctx.pixels_per_point().max(1.0).ceil() as u32).clamp(1, 4);
                    gosub_render_pipeline::render::DEVICE_PIXEL_RATIO.store(raster_dpr, std::sync::atomic::Ordering::Relaxed);
                    self.send_active_and_draw(TabCommand::SetViewport {
                        x: 0,
                        y: 0,
                        width: wanted.0,
                        height: wanted.1,
                    });
                }
            }

            let texture = self.views.get(&active).and_then(|view| {
                view.cpu_texture
                    .as_ref()
                    .map(|t| t.id())
                    .or_else(|| view.gpu_texture.as_ref().map(|(_, id)| *id))
            });

            let Some(texture) = texture else {
                ui.centered_and_justified(|ui| {
                    ui.label(egui::RichText::new("Loading…").italics().color(egui::Color32::GRAY));
                });
                return;
            };

            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

            // A finger dragging the page scrolls it, as on any phone. From the raw touch
            // events rather than egui's drag, which drops in and out during a swipe. Mouse
            // drags are left alone: there the wheel above scrolls. The movement is already
            // in points and tracks the finger, so it goes through as precise.
            let mut drag = egui::Vec2::ZERO;
            let now = ctx.input(|i| i.time);
            ctx.input(|i| {
                for event in &i.events {
                    let egui::Event::Touch { id, phase, pos, .. } = *event else {
                        continue;
                    };
                    let ours = self.touch.is_some_and(|(touch, _)| touch == id);
                    match phase {
                        egui::TouchPhase::Start if rect.contains(pos) => {
                            self.touch = Some((id, pos));
                            // A finger on the page catches a fling, as on any phone.
                            self.fling = None;
                            self.touch_trail.clear();
                            self.touch_trail.push_back((now, pos));
                        }
                        egui::TouchPhase::Move if ours => {
                            if let Some((_, last)) = self.touch.as_mut() {
                                drag += pos - *last;
                                *last = pos;
                            }
                            self.touch_trail.push_back((now, pos));
                        }
                        egui::TouchPhase::End if ours => {
                            self.touch = None;
                            self.fling = fling_velocity(&self.touch_trail, now).map(|v| (v, now));
                        }
                        egui::TouchPhase::Cancel if ours => self.touch = None,
                        _ => {}
                    }
                }
            });
            while self.touch_trail.front().is_some_and(|(t, _)| now - t > FLING_WINDOW) {
                self.touch_trail.pop_front();
            }

            // A released fling keeps the page going at the finger's speed, slowing down until
            // it stops. Each frame moves it as far as its velocity carried it since the last.
            if let (None, Some((velocity, last))) = (self.touch, self.fling) {
                let dt = (now - last) as f32;
                drag += velocity * dt;
                let velocity = velocity * FLING_DECAY.powf(dt * 1000.0);
                self.fling = (velocity.length() > FLING_STOP).then_some((velocity, now));
                ctx.request_repaint();
            }
            if drag != egui::Vec2::ZERO {
                let (dx, dy) = (-drag.x, -drag.y);
                if let Some(view) = self.views.get_mut(&active) {
                    let max_y = (view.page_height - view.viewport.map(|(_, h)| h as f32).unwrap_or(0.0)).max(0.0);
                    view.scroll_x = (view.scroll_x + dx).max(0.0);
                    view.scroll_y = (view.scroll_y + dy).clamp(0.0, max_y);
                }
                self.send_active(TabCommand::MouseScroll {
                    delta_x: dx,
                    delta_y: dy,
                    precise: true,
                });
            }

            ui.painter().image(
                texture,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            self.progress.paint(ui, rect);
            let active_tab = self.active();
            if let Some(strip) = self.activity.as_mut() {
                strip.paint(ui, rect, active_tab);
            }

            // ── status: only while a link is under the pointer ────────────
            // Drawn over the page's bottom-left corner, not as a panel: a panel coming and
            // going resizes the page, and each resize is a re-layout with the old frame
            // stretched to the new size until it lands. A swipe crossing links did that on
            // every link.
            if !self.status.is_empty() {
                let padding = egui::vec2(8.0, 3.0);
                let color = ui.visuals().weak_text_color();
                let mut job = egui::text::LayoutJob::simple_singleline(self.status.clone(), egui::FontId::proportional(STATUS_TEXT), color);
                job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 2.0 * padding.x);
                let galley = ui.painter().layout_job(job);
                let size = galley.size() + 2.0 * padding;
                let bubble = egui::Rect::from_min_size(egui::pos2(rect.min.x, rect.max.y - size.y), size);
                let radius = egui::CornerRadius {
                    ne: 4,
                    ..Default::default()
                };
                ui.painter().rect_filled(bubble, radius, faint);
                ui.painter().galley(bubble.min + padding, galley, color);
            }

            // Hover follows a mouse, and only when it actually moved: every move is a hit test,
            // and a hover change can restyle and re-layout the page. A finger has no hover, and
            // a swipe forwarded as moves re-laid the page out for every link it crossed. A tap
            // sends its own move below.
            let touching = self.touch.is_some() || ctx.input(|i| i.any_touches());
            let pointer = ctx.pointer_latest_pos().filter(|pos| rect.contains(*pos) && !touching);
            if let Some(pos) = pointer {
                if self.last_pointer != Some(pos) {
                    let rel = pos - rect.min;
                    self.send_active(TabCommand::MouseMove { x: rel.x, y: rel.y });
                }
                ui.ctx().set_cursor_icon(match self.cursor {
                    Cursor::Pointer => egui::CursorIcon::PointingHand,
                    Cursor::Text => egui::CursorIcon::Text,
                    Cursor::Resize => egui::CursorIcon::ResizeNwSe,
                    Cursor::Default => egui::CursorIcon::Default,
                });
            }
            self.last_pointer = pointer;

            if response.clicked() {
                // The click's own position: a finger lifting takes the pointer away with it,
                // so by now the context may have none.
                if let Some(pos) = response.interact_pointer_pos() {
                    let rel = pos - rect.min;
                    // The engine follows the link under the pointer, and for a tap this is the
                    // first it hears of where the finger is. For a mouse it is a no-op.
                    self.send_active(TabCommand::MouseMove { x: rel.x, y: rel.y });
                    // Press and release together: a press without its release would leave
                    // the pointer holding whatever it went down on.
                    self.send_active(TabCommand::MouseDown {
                        x: rel.x,
                        y: rel.y,
                        button: MouseButton::Left,
                    });
                    self.send_active(TabCommand::MouseUp {
                        x: rel.x,
                        y: rel.y,
                        button: MouseButton::Left,
                    });
                }
            }
        });
    }
}

/// An empty strip `size` points deep, keeping what comes after it off a system bar.
#[cfg(target_os = "android")]
fn inset(ui: &mut egui::Ui, panel: egui::Panel, size: f32, fill: egui::Color32) {
    if size > 0.0 {
        panel
            .exact_size(size)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::default().fill(fill))
            .show(ui, |_| {});
    }
}

impl BeaconApp {
    /// Show or hide the activity strip. Showing it subscribes to the engine's telemetry,
    /// which is what makes the engine announce its stages; hiding drops the subscription.
    fn toggle_activity(&mut self) {
        self.activity = match self.activity.take() {
            Some(_) => None,
            None => Some(chrome::ActivityStrip::new()),
        };
    }

    /// The Back key (Android's Back button or gesture, a keyboard's Back key). It unwinds
    /// whatever the chrome has open before it touches the page: the address bar's focus,
    /// a menu, the tab list. Then it goes back in the tab's history, and with no history
    /// left Beacon goes to the background on Android.
    fn back_key(&mut self, ctx: &egui::Context, active: TabId) {
        if self.address_bar_focused {
            ctx.memory_mut(|m| m.stop_text_input());
        } else if egui::Popup::is_any_open(ctx) {
            egui::Popup::close_all(ctx);
        } else if self.tab_list_open {
            self.tab_list_open = false;
        } else if self
            .tabs
            .lock()
            .unwrap()
            .get_tab(active)
            .is_some_and(|tab| tab.history().can_go_back())
        {
            self.dispatch(BeaconCommand::Back);
        } else {
            #[cfg(target_os = "android")]
            crate::android::move_to_background();
        }
    }

    /// The tab list: every tab as a card to switch to or close, and a way to open a new one.
    /// Shown instead of the page while the tab button is toggled on.
    fn tab_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ui, |ui| {
            let order = self.tabs.lock().unwrap().order();
            let mut action = None;
            ui.horizontal(|ui| {
                let label = if order.len() == 1 {
                    "1 tab".to_owned()
                } else {
                    format!("{} tabs", order.len())
                };
                ui.heading(label);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Done").clicked() {
                        self.tab_list_open = false;
                    }
                    if ui.button("+ New tab").clicked() {
                        if let Some(id) = self.open_tab("gosub://home") {
                            action = Some(chrome::TabAction::Activate(id));
                        }
                    }
                });
            });
            ui.add_space(8.0);
            // Two columns of cards, each the top of its page as last drawn. The page textures
            // are already on the GPU and registered with egui, so a card costs no copy.
            let gap = 12.0;
            let width = ((ui.available_width() - gap) / 2.0).floor();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for row in order.chunks(2) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for &tab_id in row {
                            let Some(tab) = self.tabs.lock().unwrap().get_tab(tab_id) else {
                                continue;
                            };
                            let icon = self.favicons.get(ctx, tab_id, tab.favicon());
                            let title = if tab.title().is_empty() { tab.url().as_str() } else { tab.title() };
                            let thumbnail = self.views.get(&tab_id).and_then(|view| {
                                let texture = view
                                    .gpu_texture
                                    .as_ref()
                                    .map(|(_, id)| *id)
                                    .or_else(|| view.cpu_texture.as_ref().map(|t| t.id()))?;
                                let (w, h) = view.viewport?;
                                Some(chrome::Thumbnail {
                                    texture,
                                    aspect: w as f32 / h.max(1) as f32,
                                })
                            });
                            let active = Some(tab_id) == self.active();
                            let (response, closed) = chrome::tab_card(ui, title, icon.as_ref(), thumbnail, active, width);
                            if closed {
                                action = Some(chrome::TabAction::Close(tab_id));
                            } else if response.clicked() {
                                action = Some(chrome::TabAction::Activate(tab_id));
                            }
                        }
                    });
                    ui.add_space(gap);
                }
            });
            match action {
                // Picking a tab (or opening one) is done with the list.
                Some(chrome::TabAction::Activate(tab_id)) => {
                    self.activate(tab_id);
                    self.tab_list_open = false;
                }
                Some(chrome::TabAction::Close(tab_id)) => self.close_tab(tab_id),
                None => {}
            }
        });
    }

    /// Navigate the active tab, running the address through the same parser the GTK
    /// frontend uses so `example.com` and `/etc/hosts` behave the same in both.
    fn navigate_active(&mut self, address: &str) {
        self.fling = None;
        let Ok((_mode, url)) = beacon_core::address_parser::GosubAddressParser::parse(address) else {
            self.log.push(format!("cannot parse address: {address}"));
            return;
        };
        let Some(active) = self.active() else { return };
        {
            let mut tabs = self.tabs.lock().unwrap();
            if let Some(mut tab) = tabs.get_tab(active) {
                tab.set_url(url.clone());
                tab.set_loading(true);
                tabs.update_tab(active, &tab);
            }
        }
        self.address_bar = url.to_string();
        self.send_active_and_draw(TabCommand::Navigate { url: url.to_string() });
    }
}
