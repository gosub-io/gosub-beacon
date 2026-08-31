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
}

impl BeaconApp {
    pub fn new(cc: &CreationContext<'_>, rt: &'static Runtime, urls: Vec<String>) -> anyhow::Result<Self> {
        // Deliberately NOT holding `rt.enter()` across this function. `BrowserEngine::new`
        // enters the runtime for its own setup, and `create_tab` below does a `block_on` --
        // which panics if it runs while a runtime context is already entered. Holding a
        // guard here stalls startup before the first frame, with an empty log.
        let context = Arc::new(
            EguiContextProvider::from_eframe(cc)
                .ok_or_else(|| anyhow::anyhow!("eframe is not running its wgpu renderer; Beacon's egui frontend needs it"))?,
        );
        let backend = VelloBackend::new(context.clone()).map_err(|e| anyhow::anyhow!("Vello backend: {e:?}"))?;

        let mut engine = BrowserEngine::<EguiConfig>::new(rt, false, Arc::new(backend))?;
        let event_rx = engine
            .take_event_rx()
            .ok_or_else(|| anyhow::anyhow!("engine event stream already taken"))?;

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
            views: HashMap::new(),
            address_bar: String::new(),
            address_bar_focused: false,
            status: String::new(),
            cursor: Cursor::Default,
            log: Vec::new(),
            favicons: Favicons::default(),
            bookmarks: Vec::new(),
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
    fn send_active(&self, command: TabCommand) {
        let Some(handle) = self.active_handle() else { return };
        self.rt.spawn(async move {
            let _ = handle.send(command).await;
        });
    }

    /// Send a command and then make sure the tab is actually drawing. Drawing is suspended
    /// until asked, and neither navigating nor resizing resumes it on its own.
    fn send_active_and_draw(&self, command: TabCommand) {
        let Some(handle) = self.active_handle() else { return };
        self.rt.spawn(async move {
            let _ = handle.send(command).await;
            let _ = handle.send(TabCommand::ResumeDrawing { fps: DRAW_FPS }).await;
        });
    }

    /// Switch to a tab: record it, promote it in the MRU list, and follow the address bar.
    fn activate(&mut self, tab_id: TabId) {
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
                    self.favicons.forget(tab_id);
                }
                BeaconEvent::FaviconChanged(tab_id) => self.favicons.forget(tab_id),
                // The tab strip, the buttons and the viewport are all rebuilt from current
                // state every frame, so these need no separate handling in an immediate-mode
                // UI -- unlike GTK, where each one has a widget to poke.
                BeaconEvent::Redraw
                | BeaconEvent::TabsChanged
                | BeaconEvent::ActiveTabChanged(_)
                | BeaconEvent::TitleChanged(..)
                | BeaconEvent::LoadingChanged(..)
                | BeaconEvent::LoadProgress(..)
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

        // ── scroll ────────────────────────────────────────────────────────
        // Raw wheel events, not egui's smoothed delta: the engine smooths scrolling itself,
        // and forwarding an already-smoothed value double-smooths it into a slow ramp.
        let scroll = ctx.input(|i| {
            let mut acc = egui::Vec2::ZERO;
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
                    acc += *delta * scale;
                }
            }
            acc
        });
        if scroll != egui::Vec2::ZERO {
            let (dx, dy) = (-scroll.x, -scroll.y);
            if let Some(view) = self.views.get_mut(&active) {
                let max_y = (view.page_height - view.viewport.map(|(_, h)| h as f32).unwrap_or(0.0)).max(0.0);
                view.scroll_x = (view.scroll_x + dx).max(0.0);
                view.scroll_y = (view.scroll_y + dy).clamp(0.0, max_y);
            }
            self.send_active(TabCommand::MouseScroll { delta_x: dx, delta_y: dy });
        }

        self.refresh_texture(active, &ctx, frame);

        // ── tab strip ─────────────────────────────────────────────────────
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

                    if chrome::tool_button(ui, "\u{2190}", "Back", can_back).clicked() {
                        self.dispatch(BeaconCommand::Back);
                    }
                    if chrome::tool_button(ui, "\u{2192}", "Forward", can_forward).clicked() {
                        self.dispatch(BeaconCommand::Forward(None));
                    }
                    if loading {
                        if chrome::tool_button(ui, "\u{2715}", "Stop", true).clicked() {
                            self.dispatch(BeaconCommand::Stop);
                        }
                    } else if chrome::tool_button(ui, "\u{21bb}", "Reload", true).clicked() {
                        self.dispatch(BeaconCommand::Reload { ignore_cache: false });
                    }
                    if chrome::tool_button(ui, "\u{2302}", "Home", true).clicked() {
                        self.navigate_active("gosub://home");
                    }
                    ui.add_space(4.0);

                    // The address bar takes the room left after the trailing controls, so
                    // they stay put instead of drifting with the URL length.
                    let trailing = 30.0;
                    let response = ui.add_sized(
                        [ui.available_width() - trailing, ui.spacing().interact_size.y],
                        egui::TextEdit::singleline(&mut self.address_bar)
                            .hint_text("Search or enter address")
                            .vertical_align(egui::Align::Center),
                    );
                    self.address_bar_focused = response.has_focus();
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let target = self.address_bar.clone();
                        self.navigate_active(&target);
                    }

                    let bookmarked = self.bookmarks.iter().any(|(_, b)| b.as_str() == url.as_str());
                    let star = if bookmarked { "\u{2605}" } else { "\u{2606}" };
                    chrome::tool_button(ui, star, "Bookmark this page", true);
                });
            });

        // ── bookmarks bar ─────────────────────────────────────────────────
        if !self.bookmarks.is_empty() {
            egui::Panel::top("bookmarks")
                .frame(egui::Frame::default().inner_margin(egui::Margin {
                    left: 10,
                    right: 8,
                    top: 0,
                    bottom: 5,
                }))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let mut go = None;
                        for (title, url) in &self.bookmarks {
                            if ui
                                .add(egui::Button::new(egui::RichText::new(title).size(12.0)).frame(false))
                                .on_hover_text(url)
                                .clicked()
                            {
                                go = Some(url.clone());
                            }
                        }
                        if let Some(url) = go {
                            self.navigate_active(&url);
                        }
                    });
                });
        }

        // ── status: only while a link is under the pointer ────────────────
        if !self.status.is_empty() {
            egui::Panel::bottom("status")
                .frame(egui::Frame::default().fill(faint).inner_margin(egui::Margin::symmetric(8, 3)))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(&self.status).size(11.0).color(ui.visuals().weak_text_color()));
                });
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
            ui.painter().image(
                texture,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );

            if let Some(pos) = ctx.pointer_latest_pos() {
                if rect.contains(pos) {
                    let rel = pos - rect.min;
                    self.send_active(TabCommand::MouseMove { x: rel.x, y: rel.y });
                    ui.ctx().set_cursor_icon(match self.cursor {
                        Cursor::Pointer => egui::CursorIcon::PointingHand,
                        Cursor::Text => egui::CursorIcon::Text,
                        Cursor::Default => egui::CursorIcon::Default,
                    });
                }
            }

            if response.clicked() {
                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                    let rel = pos - rect.min;
                    self.send_active(TabCommand::MouseDown {
                        x: rel.x,
                        y: rel.y,
                        button: MouseButton::Left,
                    });
                }
            }
        });
    }
}

impl BeaconApp {
    /// Navigate the active tab, running the address through the same parser the GTK
    /// frontend uses so `example.com` and `/etc/hosts` behave the same in both.
    fn navigate_active(&mut self, address: &str) {
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
