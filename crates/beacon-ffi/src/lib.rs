//! A C ABI over [`beacon_core`], so a chrome written in Swift, C# or anything else that
//! speaks C can drive the browser.
//!
//! The rules this boundary is built on, all of which exist to stop a native shell growing
//! its own idea of what the browser is:
//!
//! - **The shell keeps no state.** It asks — `tab_count`, `tab_at`, `tab_title`. Two lists
//!   that can disagree is a bug we have already had, when a GTK stack and the tab manager
//!   both believed they knew the active tab.
//! - **Events are pulled, never pushed.** [`beacon_poll_events`] is called from the shell's
//!   own run loop. A callback would fire on whichever Rust thread noticed, and AppKit and
//!   WinUI both insist on the UI thread.
//! - **Nothing Rust crosses.** Opaque pointers, `uint64_t` handles, C strings the caller
//!   frees with [`beacon_string_free`].
//! - **Single-threaded.** Every function here must be called from the same thread — the
//!   shell's UI thread. The engine's own work happens on a tokio runtime underneath, and
//!   never touches these types.
//!
//! The header in `include/beacon.h` is written by hand rather than generated, so it can
//! carry this reasoning. `examples/smoke.c` compiles against it, which is what keeps the
//! two from drifting.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Arc, Mutex, OnceLock};

use beacon_core::beacon::{Beacon, DRAW_FPS};
use beacon_core::command::BeaconCommand;
use beacon_core::engine::BrowserEngine;
use beacon_core::event::{BeaconEvent, Cursor};
use beacon_core::tab::{GosubTab, GosubTabManager, TabId};
use gosub_engine::events::{EngineEvent, MouseButton, TabCommand};
use gosub_render_pipeline::render::backend::ExternalHandle;
use gosub_render_pipeline::render::{composite_tiles, TileTarget};
use tokio::runtime::Runtime;

/// CPU tiles through Skia — the same path the GTK frontend rasterizes with. The shell gets
/// finished pixels; nothing here needs a GPU surface.
type FfiConfig = gosub_engine::DefaultRenderConfig<gosub_renderer_skia::SkiaBackend, gosub_renderer_skia::SkiaFontSystem>;

fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("tokio runtime"))
}

// ── C-visible types ──────────────────────────────────────────────────────────

/// Opaque browser handle.
pub struct BeaconBrowser {
    engine: BrowserEngine<FfiConfig>,
    beacon: Beacon,
    tabs: Arc<Mutex<GosubTabManager>>,
    events: tokio::sync::broadcast::Receiver<EngineEvent>,

    /// Stable `uint64_t` handles for the shell. `TabId` is a UUID, which does not fit in a
    /// C integer, and handing out pointers would invite use-after-free.
    handles: HashMap<u64, TabId>,
    next_handle: u64,

    /// Events translated but not yet collected by the shell.
    pending: Vec<BeaconEvent>,
    /// Strings referenced by the last `poll_events` batch, kept alive until the next call.
    strings: Vec<CString>,
    /// The composited frame handed out by `acquire_frame`, owned until `release_frame`.
    frame: Vec<u8>,
}

#[repr(C)]
pub struct BeaconConfig {
    /// Profile directory, or NULL for the platform default.
    pub user_data_dir: *const c_char,
    pub private_mode: bool,
}

#[repr(u32)]
#[derive(Clone, Copy)]
pub enum BeaconEventKind {
    Redraw = 0,
    TabsChanged = 1,
    ActiveTabChanged = 2,
    TitleChanged = 3,
    UrlChanged = 4,
    LoadingChanged = 5,
    Progress = 6,
    FaviconChanged = 7,
    NavStateChanged = 8,
    HoverUrl = 9,
    CursorChanged = 10,
    DownloadOffered = 11,
    TabCrashed = 12,
    Log = 13,
}

#[repr(C)]
pub struct BeaconCEvent {
    pub kind: BeaconEventKind,
    /// The tab it concerns, or 0.
    pub tab: u64,
    /// Borrowed until the next `beacon_poll_events`; NULL when the event carries no text.
    pub text: *const c_char,
    /// Progress fraction, loading flag as 0/1, cursor shape — per event kind.
    pub number: f64,
}

#[repr(C)]
pub struct BeaconFrame {
    /// BGRA, premultiplied. Borrowed until `beacon_release_frame`.
    pub pixels: *const u8,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub dpr: u32,
}

#[repr(u32)]
#[derive(Clone, Copy)]
pub enum BeaconButton {
    Left = 0,
    Middle = 1,
    Right = 2,
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// `&mut BeaconBrowser` from a caller pointer, or return `$ret` if it is NULL.
macro_rules! browser {
    ($ptr:expr, $ret:expr) => {
        match unsafe { $ptr.as_mut() } {
            Some(b) => b,
            None => return $ret,
        }
    };
    ($ptr:expr) => {
        match unsafe { $ptr.as_mut() } {
            Some(b) => b,
            None => return,
        }
    };
}

fn to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

/// Hand a Rust string to C. Freed with [`beacon_string_free`].
fn to_c_string(value: &str) -> *mut c_char {
    match CString::new(value) {
        Ok(s) => s.into_raw(),
        // An interior NUL cannot be represented; an empty string is better than a crash.
        Err(_) => CString::new("").unwrap().into_raw(),
    }
}

impl BeaconBrowser {
    fn tab(&self, handle: u64) -> Option<TabId> {
        self.handles.get(&handle).copied()
    }

    fn handle_for(&mut self, tab_id: TabId) -> u64 {
        if let Some((h, _)) = self.handles.iter().find(|(_, id)| **id == tab_id) {
            return *h;
        }
        let handle = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(handle, tab_id);
        handle
    }

    /// Drain the engine and translate. Called by `poll_events` before it serves the shell.
    fn pump(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(event) => {
                    let out = self.beacon.on_engine_event(event);
                    self.pending.extend(out);
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    }

    fn send(&self, tab_id: TabId, command: TabCommand) {
        let handle = self.tabs.lock().unwrap().get_tab(tab_id).and_then(|t| t.tab_handle());
        let Some(handle) = handle else { return };
        runtime().spawn(async move {
            let _ = handle.send(command).await;
        });
    }

    fn send_and_draw(&self, tab_id: TabId, command: TabCommand) {
        let handle = self.tabs.lock().unwrap().get_tab(tab_id).and_then(|t| t.tab_handle());
        let Some(handle) = handle else { return };
        runtime().spawn(async move {
            let _ = handle.send(command).await;
            // Drawing stays suspended until asked; navigating does not resume it.
            let _ = handle.send(TabCommand::ResumeDrawing { fps: DRAW_FPS }).await;
        });
    }
}

// ── lifecycle ────────────────────────────────────────────────────────────────

/// Create a browser. Returns NULL if the engine could not start.
///
/// # Safety
/// `config` may be NULL for defaults; if not, it must point at a valid `BeaconConfig`
/// whose `user_data_dir` is NULL or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn beacon_new(config: *const BeaconConfig) -> *mut BeaconBrowser {
    let private = unsafe { config.as_ref() }.map(|c| c.private_mode).unwrap_or(false);

    let backend = Arc::new(gosub_renderer_skia::SkiaBackend::new());
    let mut engine = match BrowserEngine::<FfiConfig>::new(runtime(), private, backend) {
        Ok(engine) => engine,
        Err(e) => {
            log::error!("beacon_new: {e}");
            return std::ptr::null_mut();
        }
    };
    let Some(events) = engine.take_event_rx() else {
        return std::ptr::null_mut();
    };

    let tabs = Arc::new(Mutex::new(GosubTabManager::new()));
    let beacon = Beacon::new(
        tabs.clone(),
        runtime().handle().clone(),
        std::rc::Rc::new(beacon_core::platform::NullPlatform),
    );

    Box::into_raw(Box::new(BeaconBrowser {
        engine,
        beacon,
        tabs,
        events,
        handles: HashMap::new(),
        next_handle: 1,
        pending: Vec::new(),
        strings: Vec::new(),
        frame: Vec::new(),
    }))
}

/// Destroy a browser. Safe to call with NULL.
///
/// # Safety
/// `browser` must have come from [`beacon_new`] and must not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn beacon_free(browser: *mut BeaconBrowser) {
    if !browser.is_null() {
        drop(unsafe { Box::from_raw(browser) });
    }
}

/// Free a string returned by this library. Safe to call with NULL.
///
/// # Safety
/// `s` must have come from one of this library's string-returning functions.
#[no_mangle]
pub unsafe extern "C" fn beacon_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

// ── tabs ─────────────────────────────────────────────────────────────────────

/// Open a tab on `url` and return its handle, or 0 on failure.
///
/// # Safety
/// `url` must be a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn beacon_open_tab(browser: *mut BeaconBrowser, url: *const c_char) -> u64 {
    let b = browser!(browser, 0);
    let Some(url) = to_str(url) else { return 0 };
    let Ok((_mode, url)) = beacon_core::address_parser::GosubAddressParser::parse(url) else {
        return 0;
    };

    let mut tab = GosubTab::new(url.clone(), url.as_str());
    let Ok(engine_handle) = b.engine.create_tab(runtime(), url.as_str(), Some((1024, 768))) else {
        return 0;
    };
    let engine_id = engine_handle.tab_id;
    tab.set_tab_handle(engine_handle.clone());
    tab.set_loading(true);

    let tab_id = tab.id();
    b.tabs.lock().unwrap().add_tab(tab, None);
    b.beacon.bind_engine_tab(engine_id, tab_id);
    b.beacon.mru_mut().insert_unused(tab_id);

    let target = url.to_string();
    runtime().spawn(async move {
        let _ = engine_handle.send(TabCommand::Navigate { url: target }).await;
        let _ = engine_handle.send(TabCommand::ResumeDrawing { fps: DRAW_FPS }).await;
    });
    b.handle_for(tab_id)
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_close_tab(browser: *mut BeaconBrowser, tab: u64) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    if b.tabs.lock().unwrap().tab_count() <= 1 {
        return;
    }
    if let Some(handle) = b.tabs.lock().unwrap().get_tab(tab_id).and_then(|t| t.tab_handle()) {
        b.beacon.unbind_engine_tab(handle.tab_id);
        runtime().spawn(async move {
            let _ = handle.send(TabCommand::CloseTab).await;
        });
    }
    b.tabs.lock().unwrap().remove_tab(tab_id);
    b.beacon.mru_mut().forget(tab_id);
    b.handles.remove(&tab);
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_activate_tab(browser: *mut BeaconBrowser, tab: u64) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    b.tabs.lock().unwrap().mark_active(tab_id);
    b.beacon.mru_mut().touch(tab_id);
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_tab_count(browser: *mut BeaconBrowser) -> usize {
    let b = browser!(browser, 0);
    b.tabs.lock().unwrap().tab_count()
}

/// The tab at `index` in strip order, or 0 if out of range.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_tab_at(browser: *mut BeaconBrowser, index: usize) -> u64 {
    let b = browser!(browser, 0);
    let order = b.tabs.lock().unwrap().order();
    match order.get(index).copied() {
        Some(tab_id) => b.handle_for(tab_id),
        None => 0,
    }
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_active_tab(browser: *mut BeaconBrowser) -> u64 {
    let b = browser!(browser, 0);
    let active = b.tabs.lock().unwrap().active();
    match active {
        Some(tab_id) => b.handle_for(tab_id),
        None => 0,
    }
}

/// The tab's title. Free with [`beacon_string_free`].
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_tab_title(browser: *mut BeaconBrowser, tab: u64) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    let Some(tab_id) = b.tab(tab) else {
        return std::ptr::null_mut();
    };
    let title = b.tabs.lock().unwrap().get_tab(tab_id).map(|t| t.title().to_string());
    match title {
        Some(t) => to_c_string(&t),
        None => std::ptr::null_mut(),
    }
}

/// The tab's URL. Free with [`beacon_string_free`].
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_tab_url(browser: *mut BeaconBrowser, tab: u64) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    let Some(tab_id) = b.tab(tab) else {
        return std::ptr::null_mut();
    };
    let url = b.tabs.lock().unwrap().get_tab(tab_id).map(|t| t.url().to_string());
    match url {
        Some(u) => to_c_string(&u),
        None => std::ptr::null_mut(),
    }
}

macro_rules! tab_flag {
    ($name:ident, $method:ident) => {
        /// # Safety
        /// `browser` must be a live handle from [`beacon_new`].
        #[no_mangle]
        pub unsafe extern "C" fn $name(browser: *mut BeaconBrowser, tab: u64) -> bool {
            let b = browser!(browser, false);
            let Some(tab_id) = b.tab(tab) else { return false };
            let tabs = b.tabs.lock().unwrap();
            tabs.get_tab(tab_id).map(|t| t.$method()).unwrap_or(false)
        }
    };
}

tab_flag!(beacon_tab_is_loading, is_loading);

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_tab_can_go_back(browser: *mut BeaconBrowser, tab: u64) -> bool {
    let b = browser!(browser, false);
    let Some(tab_id) = b.tab(tab) else { return false };
    let tabs = b.tabs.lock().unwrap();
    tabs.get_tab(tab_id).map(|t| t.history().can_go_back()).unwrap_or(false)
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_tab_can_go_forward(browser: *mut BeaconBrowser, tab: u64) -> bool {
    let b = browser!(browser, false);
    let Some(tab_id) = b.tab(tab) else { return false };
    let tabs = b.tabs.lock().unwrap();
    tabs.get_tab(tab_id).map(|t| t.history().can_go_forward()).unwrap_or(false)
}

// ── commands ─────────────────────────────────────────────────────────────────

/// # Safety
/// `browser` must be a live handle; `url` a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn beacon_navigate(browser: *mut BeaconBrowser, tab: u64, url: *const c_char) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    let Some(url) = to_str(url) else { return };
    let Ok((_mode, url)) = beacon_core::address_parser::GosubAddressParser::parse(url) else {
        return;
    };
    {
        let mut tabs = b.tabs.lock().unwrap();
        if let Some(mut t) = tabs.get_tab(tab_id) {
            t.set_url(url.clone());
            t.set_loading(true);
            tabs.update_tab(tab_id, &t);
        }
    }
    b.send_and_draw(tab_id, TabCommand::Navigate { url: url.to_string() });
}

macro_rules! active_command {
    ($name:ident, $command:expr) => {
        /// # Safety
        /// `browser` must be a live handle from [`beacon_new`].
        #[no_mangle]
        pub unsafe extern "C" fn $name(browser: *mut BeaconBrowser) {
            let b = browser!(browser);
            let events = b.beacon.apply($command);
            b.pending.extend(events);
        }
    };
}

active_command!(beacon_back, BeaconCommand::Back);
active_command!(beacon_forward, BeaconCommand::Forward(None));
active_command!(beacon_stop, BeaconCommand::Stop);

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_reload(browser: *mut BeaconBrowser, ignore_cache: bool) {
    let b = browser!(browser);
    let events = b.beacon.apply(BeaconCommand::Reload { ignore_cache });
    b.pending.extend(events);
}

// ── input ────────────────────────────────────────────────────────────────────

/// Tell the engine how big the page area is, in CSS pixels.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_set_viewport(browser: *mut BeaconBrowser, tab: u64, width: u32, height: u32) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    if width == 0 || height == 0 {
        return;
    }
    b.send_and_draw(tab_id, TabCommand::SetViewport { x: 0, y: 0, width, height });
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_mouse_move(browser: *mut BeaconBrowser, tab: u64, x: f32, y: f32) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    b.send(tab_id, TabCommand::MouseMove { x, y });
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_mouse_down(browser: *mut BeaconBrowser, tab: u64, x: f32, y: f32, button: BeaconButton) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    let button = match button {
        BeaconButton::Left => MouseButton::Left,
        BeaconButton::Middle => MouseButton::Middle,
        BeaconButton::Right => MouseButton::Right,
    };
    b.send(tab_id, TabCommand::MouseDown { x, y, button });
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_scroll(browser: *mut BeaconBrowser, tab: u64, delta_x: f32, delta_y: f32) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    b.send(tab_id, TabCommand::MouseScroll { delta_x, delta_y });
}

// ── events ───────────────────────────────────────────────────────────────────

/// Collect up to `max` events into `out`; returns how many were written. Call until it
/// returns 0. Text in the returned events is borrowed until the next call.
///
/// # Safety
/// `out` must point at space for `max` `BeaconCEvent`s.
#[no_mangle]
pub unsafe extern "C" fn beacon_poll_events(browser: *mut BeaconBrowser, out: *mut BeaconCEvent, max: usize) -> usize {
    let b = browser!(browser, 0);
    if out.is_null() || max == 0 {
        return 0;
    }
    b.pump();
    // The previous batch's strings stop being valid here, which is what the header
    // promises: borrowed until the next poll.
    b.strings.clear();

    let taking = b.pending.len().min(max);
    let batch: Vec<BeaconEvent> = b.pending.drain(..taking).collect();

    for (i, event) in batch.into_iter().enumerate() {
        let (kind, tab_id, text, number) = match event {
            BeaconEvent::Redraw => (BeaconEventKind::Redraw, None, None, 0.0),
            BeaconEvent::TabsChanged => (BeaconEventKind::TabsChanged, None, None, 0.0),
            BeaconEvent::ActiveTabChanged(t) => (BeaconEventKind::ActiveTabChanged, Some(t), None, 0.0),
            BeaconEvent::TitleChanged(t, title) => (BeaconEventKind::TitleChanged, Some(t), Some(title), 0.0),
            BeaconEvent::UrlChanged(t, url) => (BeaconEventKind::UrlChanged, Some(t), Some(url.to_string()), 0.0),
            BeaconEvent::LoadingChanged(t, loading) => (BeaconEventKind::LoadingChanged, Some(t), None, if loading { 1.0 } else { 0.0 }),
            BeaconEvent::LoadProgress(t, fraction) => (BeaconEventKind::Progress, Some(t), None, fraction.unwrap_or(-1.0)),
            BeaconEvent::FaviconChanged(t) => (BeaconEventKind::FaviconChanged, Some(t), None, 0.0),
            BeaconEvent::NavStateChanged(t) => (BeaconEventKind::NavStateChanged, Some(t), None, 0.0),
            BeaconEvent::NavigationFailed(t, url, error) => (BeaconEventKind::Log, Some(t), Some(format!("{url}: {error}")), 0.0),
            BeaconEvent::TabCrashed(t, error) => (BeaconEventKind::TabCrashed, Some(t), Some(error), 0.0),
            BeaconEvent::HoverUrl(t, url) => (BeaconEventKind::HoverUrl, Some(t), url, 0.0),
            BeaconEvent::CursorChanged(t, cursor) => (
                BeaconEventKind::CursorChanged,
                Some(t),
                None,
                match cursor {
                    Cursor::Default => 0.0,
                    Cursor::Pointer => 1.0,
                    Cursor::Text => 2.0,
                },
            ),
            BeaconEvent::DownloadOffered {
                tab_id,
                suggested_filename,
                ..
            } => (BeaconEventKind::DownloadOffered, Some(tab_id), Some(suggested_filename), 0.0),
            BeaconEvent::DownloadChanged(id) => (BeaconEventKind::Log, None, None, id as f64),
            BeaconEvent::Log(message) => (BeaconEventKind::Log, None, Some(message), 0.0),
        };

        let tab = tab_id.map(|t| b.handle_for(t)).unwrap_or(0);
        let text_ptr = match text {
            Some(t) => {
                let c = CString::new(t).unwrap_or_else(|_| CString::new("").unwrap());
                let ptr = c.as_ptr();
                b.strings.push(c);
                ptr
            }
            None => std::ptr::null(),
        };

        unsafe {
            std::ptr::write(
                out.add(i),
                BeaconCEvent {
                    kind,
                    tab,
                    text: text_ptr,
                    number,
                },
            );
        }
    }
    taking
}

// ── frames ───────────────────────────────────────────────────────────────────

/// Composite the tab's latest frame and lend it to the caller. Returns false when nothing
/// has been rendered yet. Call [`beacon_release_frame`] when done with the pixels.
///
/// # Safety
/// `out` must point at a valid `BeaconFrame`.
#[no_mangle]
pub unsafe extern "C" fn beacon_acquire_frame(browser: *mut BeaconBrowser, tab: u64, out: *mut BeaconFrame) -> bool {
    let b = browser!(browser, false);
    if out.is_null() {
        return false;
    }
    let Some(tab_id) = b.tab(tab) else { return false };
    let engine_id = b.tabs.lock().unwrap().get_tab(tab_id).and_then(|t| t.engine_tab_id());
    let Some(engine_id) = engine_id else { return false };
    let Some(handle) = b.engine.compositor.frame_for(engine_id) else {
        return false;
    };

    let ExternalHandle::TileCache {
        tiles,
        dpr,
        scroll_x,
        scroll_y,
        viewport_width,
        viewport_height,
        ..
    } = handle
    else {
        // A GPU-texture frame cannot be lent as CPU pixels; that is the shared-surface
        // path, and this backend does not produce them.
        return false;
    };

    let width = (viewport_width * dpr) as usize;
    let height = (viewport_height * dpr) as usize;
    if width == 0 || height == 0 {
        return false;
    }

    // Composite onto opaque white at the frame's own scroll position, exactly as the other
    // frontends do — going through the shared compositor is what gets `fixed` and `sticky`
    // right, and the offset is what makes scrolling visible at all. Passing (0,0) here drew
    // the top of the page forever, however far the engine had scrolled.
    let mut argb = vec![0xFFFF_FFFFu32; width * height];
    composite_tiles(
        &tiles,
        dpr,
        (scroll_x, scroll_y),
        &mut TileTarget {
            buf: &mut argb,
            stride: width,
            origin_x: 0,
            origin_y: 0,
            width,
            height,
        },
    );

    // ARGB u32 in native order is BGRA bytes on little-endian, which is what the header
    // promises. Copy once into a buffer we own for the loan.
    b.frame.clear();
    b.frame.reserve(argb.len() * 4);
    for pixel in &argb {
        b.frame.extend_from_slice(&pixel.to_le_bytes());
    }

    unsafe {
        std::ptr::write(
            out,
            BeaconFrame {
                pixels: b.frame.as_ptr(),
                width: width as u32,
                height: height as u32,
                stride: (width * 4) as u32,
                dpr,
            },
        );
    }
    true
}

/// Return a frame lent by [`beacon_acquire_frame`].
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_release_frame(browser: *mut BeaconBrowser, _tab: u64) {
    let b = browser!(browser);
    // The loan ends here. The buffer is kept allocated for the next frame rather than
    // freed: acquire/release runs at frame rate.
    b.frame.clear();
}

/// Unused today; present so the header can promise a stable ABI while the surface grows.
#[no_mangle]
pub extern "C" fn beacon_reserved(_: *mut c_void) {}
