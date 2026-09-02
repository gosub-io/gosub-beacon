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
use gosub_engine::events::{DownloadId, EngineEvent, Modifiers, MouseButton, TabCommand};
use gosub_render_pipeline::render::backend::ExternalHandle;
use gosub_render_pipeline::render::{composite_tiles, TileTarget};
use tokio::runtime::Runtime;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod gpu;

/// CPU tiles through Skia — the same path the GTK frontend rasterizes with. The shell gets
/// finished pixels; nothing here needs a GPU or a view.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
type FfiConfig = gosub_engine::DefaultRenderConfig<gosub_renderer_skia::SkiaBackend, gosub_renderer_skia::SkiaFontSystem>;

/// Vello on the GPU, so the page can be blitted into a view the native chrome owns.
#[cfg(any(target_os = "macos", target_os = "windows"))]
type FfiConfig = gosub_engine::DefaultRenderConfig<gosub_renderer_vello::VelloBackend<gpu::FfiWgpuContext>>;

/// Send this library's logging to stderr, once.
///
/// A native shell links a Rust dylib and never installs a logger, so without this every
/// `log::warn!` in here and in the engine goes nowhere -- which is exactly how a failing
/// `beacon_attach_view` came back as a bare "false" with no reason attached. Level comes
/// from `BEACON_LOG` (or `RUST_LOG`), defaulting to warnings.
fn init_logging() {
    use std::io::Write;
    use std::sync::Once;

    struct Stderr(log::LevelFilter);
    impl log::Log for Stderr {
        fn enabled(&self, meta: &log::Metadata) -> bool {
            meta.level() <= self.0
        }
        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                let _ = writeln!(std::io::stderr(), "beacon [{}] {}", record.level(), record.args());
            }
        }
        fn flush(&self) {
            let _ = std::io::stderr().flush();
        }
    }

    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let level = std::env::var("BEACON_LOG")
            .or_else(|_| std::env::var("RUST_LOG"))
            .ok()
            .and_then(|s| s.parse::<log::LevelFilter>().ok())
            .unwrap_or(log::LevelFilter::Warn);
        // An embedder may have installed its own logger; that one wins.
        if log::set_boxed_logger(Box::new(Stderr(level))).is_ok() {
            log::set_max_level(level);
        }
    });
}

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
    /// Fires whenever the compositor has a new frame. This -- not
    /// `EngineEvent::Redraw`, which nothing emits -- is how a frontend learns there is
    /// something to draw; the GTK and egui frontends both repaint from it.
    redraw: Option<tokio::sync::mpsc::UnboundedReceiver<()>>,

    /// Stable `uint64_t` handles for the shell. `TabId` is a UUID, which does not fit in a
    /// C integer, and handing out pointers would invite use-after-free.
    handles: HashMap<u64, TabId>,
    next_handle: u64,

    /// Whether this browser is a private session. Asked for rather than remembered by the
    /// shell, like everything else here.
    private: bool,

    /// Events translated but not yet collected by the shell.
    pending: Vec<BeaconEvent>,
    /// Strings referenced by the last `poll_events` batch, kept alive until the next call.
    strings: Vec<CString>,
    /// The composited frame handed out by `acquire_frame`, owned until `release_frame`.
    frame: Vec<u8>,
    /// The favicon bytes last handed out, kept alive until the next `tab_favicon` call.
    favicon: Vec<u8>,
    /// The last history search, held so the shell can read rows out of it by index rather
    /// than the ABI having to hand back an array of structs.
    history: Vec<gosub_engine::places::VisitedPage>,

    /// What the shell last said about each tab's page area, so zoom can recompute the
    /// viewport without the shell having to resend it -- and so activating a tab can
    /// restore the process-wide raster DPR that another tab may have moved.
    viewports: HashMap<TabId, ViewportState>,
    /// Latest load progress per tab. The engine reports it as an event; the shell wants to
    /// ask, so it is remembered here rather than in every shell.
    progress: HashMap<TabId, f64>,
    /// Downloads the engine has offered and the shell has not answered yet. Keyed by an
    /// offer id handed to the shell in the event, so an answer can never land on the wrong
    /// offer when two arrive together.
    offers: HashMap<u64, PendingOffer>,
    next_offer: u64,

    /// The wgpu device Vello draws through, kept so attached views can share it.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    gpu: std::sync::Arc<gpu::FfiWgpuContext>,
    /// Views the shell has attached, one per tab.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    views: HashMap<TabId, gpu::ViewSurface>,
}

/// The page area as the shell described it, in the shell's own terms: logical (CSS-ish)
/// size, the display's backing scale, and the user's zoom. The engine viewport is derived
/// from all three, which is why none of them is stored pre-multiplied.
#[derive(Clone, Copy)]
struct ViewportState {
    logical_width: u32,
    logical_height: u32,
    scale: f32,
    zoom: f32,
}

/// A download the engine offered, waiting for the shell to name a file or decline.
struct PendingOffer {
    tab_id: TabId,
    url: String,
    suggested_filename: String,
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
    DownloadChanged = 14,
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
        // Frames first. Many notifications can pile up between polls, but they all mean the
        // same thing -- "draw the latest" -- so they coalesce into one event rather than
        // making the shell repaint once per notification.
        if let Some(redraw) = self.redraw.as_mut() {
            let mut any = false;
            while redraw.try_recv().is_ok() {
                any = true;
            }
            if any {
                self.pending.push(BeaconEvent::Redraw);
            }
        }

        loop {
            match self.events.try_recv() {
                Ok(event) => {
                    let out = self.beacon.on_engine_event(event);
                    for e in &out {
                        match e {
                            // Progress arrives as an event but is asked for as a value, so it
                            // is caught on the way past rather than made the shell's problem.
                            BeaconEvent::LoadProgress(t, fraction) => {
                                self.progress.insert(*t, fraction.unwrap_or(-1.0));
                            }
                            BeaconEvent::LoadingChanged(t, false) => {
                                self.progress.remove(t);
                            }
                            _ => {}
                        }
                    }
                    self.pending.extend(out);
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    }

    /// Push the engine viewport derived from what the shell last said about this tab.
    ///
    /// Three numbers collapse into two here. The engine lays out in CSS pixels and
    /// rasterizes at `DEVICE_PIXEL_RATIO`, so zoom is expressed as a *smaller viewport* at a
    /// *higher* raster ratio -- the page then lays out as though the window were smaller and
    /// is drawn large, which is what zoom means. Sending a scaled-up viewport instead would
    /// reflow the page rather than magnify it.
    fn apply_viewport(&mut self, tab_id: TabId) {
        let Some(vs) = self.viewports.get(&tab_id).copied() else { return };

        // Process-wide in the engine, so it belongs to whichever tab last drew. Re-storing it
        // on activation and on every viewport change is what keeps a zoomed background tab
        // from deciding the foreground tab's resolution.
        let effective = (vs.scale * vs.zoom).max(0.1);
        let raster_dpr = (effective.ceil() as u32).clamp(1, 4);
        gosub_render_pipeline::render::DEVICE_PIXEL_RATIO.store(raster_dpr, std::sync::atomic::Ordering::Relaxed);

        let width = ((vs.logical_width as f32) / vs.zoom).round().max(1.0) as u32;
        let height = ((vs.logical_height as f32) / vs.zoom).round().max(1.0) as u32;
        self.send_and_draw(tab_id, TabCommand::SetViewport { x: 0, y: 0, width, height });
    }

    fn zoom_of(&self, tab_id: TabId) -> f32 {
        self.viewports.get(&tab_id).map(|v| v.zoom).unwrap_or(1.0)
    }

    /// Open a tab on `url`, optionally at a given strip position, and return its handle.
    /// Shared by `beacon_open_tab` and `beacon_reopen_closed_tab` so a reopened tab is in
    /// every respect an ordinary one.
    fn open_tab_at(&mut self, url: &str, position: Option<usize>) -> u64 {
        let Ok((_mode, url)) = beacon_core::address_parser::GosubAddressParser::parse(url) else {
            return 0;
        };

        let mut tab = GosubTab::new(url.clone(), url.as_str());
        let Ok(engine_handle) = self.engine.create_tab(runtime(), url.as_str(), Some((1024, 768))) else {
            return 0;
        };
        let engine_id = engine_handle.tab_id;
        tab.set_tab_handle(engine_handle.clone());
        tab.set_loading(true);

        let tab_id = tab.id();
        self.tabs.lock().unwrap().add_tab(tab, position);
        self.beacon.bind_engine_tab(engine_id, tab_id);
        self.beacon.mru_mut().insert_unused(tab_id);

        let target = url.to_string();
        runtime().spawn(async move {
            let _ = engine_handle.send(TabCommand::Navigate { url: target }).await;
            let _ = engine_handle.send(TabCommand::ResumeDrawing { fps: DRAW_FPS }).await;
        });
        self.handle_for(tab_id)
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
    init_logging();
    let private = unsafe { config.as_ref() }.map(|c| c.private_mode).unwrap_or(false);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let backend = Arc::new(gosub_renderer_skia::SkiaBackend::new());

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let (gpu_context, backend) = {
        let context = match gpu::FfiWgpuContext::new(runtime()) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                log::error!("beacon_new: {e}");
                return std::ptr::null_mut();
            }
        };
        let backend = match gosub_renderer_vello::VelloBackend::new(context.clone()) {
            Ok(b) => Arc::new(b),
            Err(e) => {
                log::error!("beacon_new: Vello backend: {e:?}");
                return std::ptr::null_mut();
            }
        };
        (context, backend)
    };

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
    let redraw = engine.take_redraw_rx();

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
        redraw,
        handles: HashMap::new(),
        next_handle: 1,
        favicon: Vec::new(),
        history: Vec::new(),
        private,
        viewports: HashMap::new(),
        progress: HashMap::new(),
        offers: HashMap::new(),
        next_offer: 1,
        pending: Vec::new(),
        strings: Vec::new(),
        frame: Vec::new(),
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        gpu: gpu_context,
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        views: HashMap::new(),
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
    b.open_tab_at(url, None)
}

/// Reopen the most recently closed tab, at the position it was closed from. Returns its
/// handle, or 0 when nothing has been closed.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_reopen_closed_tab(browser: *mut BeaconBrowser) -> u64 {
    let b = browser!(browser, 0);
    let Some(closed) = b.beacon.closed_mut().pop() else { return 0 };
    b.open_tab_at(&closed.url, closed.position)
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
    // Remember it before it goes, so Cmd+Shift+T has something to bring back.
    let closed = {
        let manager = b.tabs.lock().unwrap();
        let position = manager.order().iter().position(|id| *id == tab_id);
        manager.get_tab(tab_id).map(|t| (t.url().to_string(), position))
    };
    if let Some((url, position)) = closed {
        b.beacon.closed_mut().push(url, position);
    }

    b.tabs.lock().unwrap().remove_tab(tab_id);
    b.beacon.mru_mut().forget(tab_id);
    b.viewports.remove(&tab_id);
    b.progress.remove(&tab_id);
    b.handles.remove(&tab);
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_activate_tab(browser: *mut BeaconBrowser, tab: u64) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    let previous = b.tabs.lock().unwrap().active();
    if previous == Some(tab_id) {
        return;
    }

    // A background tab that keeps drawing at 30fps is a laptop-fan bug, and with one shared
    // GPU context it also competes with the tab the user is actually looking at.
    if let Some(previous) = previous {
        b.send(previous, TabCommand::SuspendDrawing);
    }

    b.tabs.lock().unwrap().mark_active(tab_id);
    b.beacon.mru_mut().touch(tab_id);

    // Re-assert this tab's viewport: the raster DPR is process-wide, so whatever the last
    // active tab wanted is still in force until something says otherwise.
    b.apply_viewport(tab_id);
    b.send(tab_id, TabCommand::ResumeDrawing { fps: DRAW_FPS });
}

/// Whether this browser is a private session: cookies and storage in memory only, and no
/// visited history recorded. Bookmarks and settings are still the shared, persistent ones,
/// which is what mainstream browsers do too.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_is_private(browser: *mut BeaconBrowser) -> bool {
    let b = browser!(browser, false);
    b.private
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

/// Tell the engine how big the page area is, in CSS pixels, and how many device pixels
/// there are per CSS pixel.
///
/// `scale` is what makes text sharp on a HiDPI display: without it the page is rasterized
/// at 1x and then stretched onto a 2x surface, which looks exactly like bad font
/// rendering and is not.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_set_viewport(browser: *mut BeaconBrowser, tab: u64, width: u32, height: u32, scale: f32) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    if width == 0 || height == 0 {
        return;
    }

    // The rasterizer reads this global rather than taking it per-tab, so it is re-stored
    // whenever a viewport lands -- which is also what happens on activation and resize.
    // With several tabs at different scales the last one to be sized wins, exactly as in
    // the GTK frontend; a per-tab DPR is an engine-side change.
    let zoom = b.zoom_of(tab_id);
    b.viewports.insert(
        tab_id,
        ViewportState {
            logical_width: width,
            logical_height: height,
            scale: scale.max(0.1),
            zoom,
        },
    );
    b.apply_viewport(tab_id);
}

/// Set the page zoom for a tab, as a factor (1.0 = 100%). Clamped to 0.25--5.0, the same
/// range mainstream browsers offer.
///
/// The shell keeps sending the view's *unzoomed* size to [`beacon_set_viewport`]; zoom is
/// applied here, so the two never have to be kept in step by the caller.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_set_zoom(browser: *mut BeaconBrowser, tab: u64, zoom: f32) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    let zoom = zoom.clamp(0.25, 5.0);
    let Some(vs) = b.viewports.get_mut(&tab_id) else { return };
    if (vs.zoom - zoom).abs() < 1e-3 {
        return;
    }
    vs.zoom = zoom;
    b.apply_viewport(tab_id);
}

/// The tab's zoom factor; 1.0 when it has never been set.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_zoom(browser: *mut BeaconBrowser, tab: u64) -> f32 {
    let b = browser!(browser, 1.0);
    match b.tab(tab) {
        Some(tab_id) => b.zoom_of(tab_id),
        None => 1.0,
    }
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
pub unsafe extern "C" fn beacon_mouse_up(browser: *mut BeaconBrowser, tab: u64, x: f32, y: f32, button: BeaconButton) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    let button = match button {
        BeaconButton::Left => MouseButton::Left,
        BeaconButton::Middle => MouseButton::Middle,
        BeaconButton::Right => MouseButton::Right,
    };
    b.send(tab_id, TabCommand::MouseUp { x, y, button });
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_scroll(browser: *mut BeaconBrowser, tab: u64, delta_x: f32, delta_y: f32) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    b.send(tab_id, TabCommand::MouseScroll { delta_x, delta_y });
}

// ── keyboard ─────────────────────────────────────────────────────────────────

/// Bit flags matching the engine's `Modifiers`, and the web's `KeyboardEvent` modifiers.
/// `META` is Command on macOS and the Windows key elsewhere.
pub const BEACON_MOD_SHIFT: u32 = 1;
pub const BEACON_MOD_CONTROL: u32 = 2;
pub const BEACON_MOD_ALT: u32 = 4;
pub const BEACON_MOD_META: u32 = 8;

fn modifiers_from_bits(bits: u32) -> Modifiers {
    let mut m = Modifiers::empty();
    if bits & BEACON_MOD_SHIFT != 0 {
        m |= Modifiers::SHIFT;
    }
    if bits & BEACON_MOD_CONTROL != 0 {
        m |= Modifiers::CONTROL;
    }
    if bits & BEACON_MOD_ALT != 0 {
        m |= Modifiers::ALT;
    }
    if bits & BEACON_MOD_META != 0 {
        m |= Modifiers::META;
    }
    m
}

/// A key went down in the page.
///
/// `key` and `code` are the web's own names -- `KeyboardEvent.key` (`"a"`, `"Enter"`,
/// `"ArrowLeft"`) and `KeyboardEvent.code` (the physical key, `"KeyA"`). The shell maps its
/// platform's key events onto those, because only the shell knows the keyboard layout: a
/// French AZERTY `A` is `KeyQ`, and nothing on this side of the boundary can work that out.
///
/// Pass NULL for `code` when the shell has no physical-key information; `key` is what the
/// engine reads today.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`]; `key` and `code` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn beacon_key_down(browser: *mut BeaconBrowser, tab: u64, key: *const c_char, code: *const c_char, modifiers: u32) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    let Some(key) = to_str(key) else { return };
    let code = to_str(code).unwrap_or(key).to_string();
    b.send_and_draw(
        tab_id,
        TabCommand::KeyDown {
            key: key.to_string(),
            code,
            modifiers: modifiers_from_bits(modifiers),
        },
    );
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`]; `key` and `code` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn beacon_key_up(browser: *mut BeaconBrowser, tab: u64, key: *const c_char, code: *const c_char, modifiers: u32) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    let Some(key) = to_str(key) else { return };
    let code = to_str(code).unwrap_or(key).to_string();
    b.send(
        tab_id,
        TabCommand::KeyUp {
            key: key.to_string(),
            code,
            modifiers: modifiers_from_bits(modifiers),
        },
    );
}

/// Committed text -- what an input method produced, which is not the same as which keys
/// were pressed. On macOS this is what `NSTextInputClient.insertText` hands you, and it is
/// the only way CJK, dead keys and emoji reach the page correctly.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`]; `text` NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn beacon_text_input(browser: *mut BeaconBrowser, tab: u64, text: *const c_char) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    let Some(text) = to_str(text) else { return };
    if text.is_empty() {
        return;
    }
    b.send_and_draw(tab_id, TabCommand::TextInput { text: text.to_string() });
}

// ── tab state the shell displays ─────────────────────────────────────────────

/// The tab's favicon, as the bytes the site served (usually PNG or ICO -- the shell
/// decodes). Returns NULL and writes 0 when the tab has none.
///
/// The bytes are borrowed until the next call to this function, like the strings in
/// [`beacon_poll_events`]. Copy them if you keep them.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`]; `out_len` a valid `size_t*`.
#[no_mangle]
pub unsafe extern "C" fn beacon_tab_favicon(browser: *mut BeaconBrowser, tab: u64, out_len: *mut usize) -> *const u8 {
    let b = browser!(browser, std::ptr::null());
    if !out_len.is_null() {
        unsafe { std::ptr::write(out_len, 0) };
    }
    let Some(tab_id) = b.tab(tab) else {
        return std::ptr::null();
    };
    let bytes = b.tabs.lock().unwrap().get_tab(tab_id).and_then(|t| t.favicon().map(|f| f.to_vec()));
    let Some(bytes) = bytes else {
        return std::ptr::null();
    };
    if bytes.is_empty() {
        return std::ptr::null();
    }
    b.favicon = bytes;
    if !out_len.is_null() {
        unsafe { std::ptr::write(out_len, b.favicon.len()) };
    }
    b.favicon.as_ptr()
}

/// Load progress as a fraction 0.0--1.0, or -1.0 when nothing is loading or the server never
/// said how big the response is. A determinate bar wants the fraction; -1.0 means show a
/// barber pole, or nothing.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_tab_progress(browser: *mut BeaconBrowser, tab: u64) -> f64 {
    let b = browser!(browser, -1.0);
    match b.tab(tab) {
        Some(tab_id) => b.progress.get(&tab_id).copied().unwrap_or(-1.0),
        None => -1.0,
    }
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_tab_is_pinned(browser: *mut BeaconBrowser, tab: u64) -> bool {
    let b = browser!(browser, false);
    let Some(tab_id) = b.tab(tab) else { return false };
    b.tabs.lock().unwrap().get_tab(tab_id).map(|t| t.is_pinned()).unwrap_or(false)
}

/// Pin or unpin a tab. Pinned tabs are held at the left of the strip and are not closable
/// by the ordinary close gesture -- that rule lives in `beacon-core`, not in the shell.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_set_tab_pinned(browser: *mut BeaconBrowser, tab: u64, pinned: bool) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    let mut manager = b.tabs.lock().unwrap();
    if pinned {
        manager.pin_tab(tab_id);
    } else {
        manager.unpin_tab(tab_id);
    }
}

/// Move a tab to `index` in strip order -- what a drag in the tab bar means.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_move_tab(browser: *mut BeaconBrowser, tab: u64, index: usize) {
    let b = browser!(browser);
    let Some(tab_id) = b.tab(tab) else { return };
    b.tabs.lock().unwrap().reorder(tab_id, index);
}

// ── bookmarks ────────────────────────────────────────────────────────────────

/// Whether the tab's current page is bookmarked.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_tab_is_bookmarked(browser: *mut BeaconBrowser, tab: u64) -> bool {
    let b = browser!(browser, false);
    let Some(tab_id) = b.tab(tab) else { return false };
    let url = b.tabs.lock().unwrap().get_tab(tab_id).map(|t| t.url().to_string());
    match url {
        Some(url) => b.engine.places().is_bookmarked(&url),
        None => false,
    }
}

/// Add or remove a bookmark for the tab's current page; returns the state it ended in.
///
/// Internal `gosub://` pages are not bookmarkable and always return false -- the same rule
/// the GTK shell applies, kept here so every shell gets it for free.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_toggle_bookmark(browser: *mut BeaconBrowser, tab: u64) -> bool {
    let b = browser!(browser, false);
    let Some(tab_id) = b.tab(tab) else { return false };
    let entry = b
        .tabs
        .lock()
        .unwrap()
        .get_tab(tab_id)
        .map(|t| (t.url().to_string(), t.title().to_string()));
    let Some((url, title)) = entry else { return false };
    // Internal pages only. The GTK shell tests for `http`, which also rejects `file://` --
    // a local page is a perfectly reasonable thing to bookmark, and the demo corpus is
    // full of them.
    if url.starts_with("gosub:") || url.starts_with("about:") {
        return false;
    }
    let places = b.engine.places();
    if places.is_bookmarked(&url) {
        places.remove_bookmark(&url);
        false
    } else {
        places.add_bookmark(&url, if title.is_empty() { &url } else { &title });
        true
    }
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_bookmark_count(browser: *mut BeaconBrowser) -> usize {
    let b = browser!(browser, 0);
    b.engine.places().bookmarks().len()
}

/// The bookmark at `index`. Free with [`beacon_string_free`]; NULL when out of range.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_bookmark_url(browser: *mut BeaconBrowser, index: usize) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    match b.engine.places().bookmarks().get(index) {
        Some(bookmark) => to_c_string(&bookmark.url),
        None => std::ptr::null_mut(),
    }
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_bookmark_title(browser: *mut BeaconBrowser, index: usize) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    match b.engine.places().bookmarks().get(index) {
        Some(bookmark) => to_c_string(&bookmark.title),
        None => std::ptr::null_mut(),
    }
}

// ── history ──────────────────────────────────────────────────────────────────

/// Search visited pages for `query` and return how many matched, capped at `limit`.
///
/// Read the rows with the functions below. Results are held until the next search, which is
/// what lets an address bar re-query on every keystroke without allocating a list each time.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`]; `query` NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn beacon_history_search(browser: *mut BeaconBrowser, query: *const c_char, limit: usize) -> usize {
    let b = browser!(browser, 0);
    let Some(query) = to_str(query) else {
        b.history.clear();
        return 0;
    };
    if query.is_empty() {
        b.history.clear();
        return 0;
    }
    b.history = b.engine.places().query_visited(query, limit.max(1));
    b.history.len()
}

/// A row from the last [`beacon_history_search`]. Free with [`beacon_string_free`]; NULL
/// when out of range.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_history_url(browser: *mut BeaconBrowser, index: usize) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    match b.history.get(index) {
        Some(page) => to_c_string(&page.url),
        None => std::ptr::null_mut(),
    }
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_history_title(browser: *mut BeaconBrowser, index: usize) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    match b.history.get(index) {
        Some(page) => to_c_string(&page.title),
        None => std::ptr::null_mut(),
    }
}

/// How often the page has been visited -- the obvious thing to rank suggestions by.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_history_visit_count(browser: *mut BeaconBrowser, index: usize) -> u64 {
    let b = browser!(browser, 0);
    b.history.get(index).map(|page| page.visit_count).unwrap_or(0)
}

// ── downloads ────────────────────────────────────────────────────────────────

/// What a download is doing. Matches `beacon_core::download::DownloadState`.
#[repr(u32)]
#[derive(Clone, Copy)]
pub enum BeaconDownloadState {
    Running = 0,
    Finished = 1,
    Failed = 2,
}

/// The URL a pending offer would fetch. Free with [`beacon_string_free`]; NULL if the offer
/// id is unknown (it has already been answered, or never existed).
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_download_offer_url(browser: *mut BeaconBrowser, offer: u64) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    match b.offers.get(&offer) {
        Some(pending) => to_c_string(&pending.url),
        None => std::ptr::null_mut(),
    }
}

/// Accept a download offer and write it to `path`. Returns the download id to track it
/// with, or 0 if the offer is unknown.
///
/// The shell chooses the path -- an `NSSavePanel` on macOS -- because where a file goes is a
/// platform question, and the engine should not be inventing a Downloads folder.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`]; `path` NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn beacon_download_accept(browser: *mut BeaconBrowser, offer: u64, path: *const c_char) -> u64 {
    let b = browser!(browser, 0);
    let Some(path) = to_str(path) else { return 0 };
    let Some(pending) = b.offers.remove(&offer) else { return 0 };

    let target = std::path::PathBuf::from(path);
    let filename = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| pending.suggested_filename.clone());

    let id = b.beacon.downloads_mut().next_id();
    b.beacon.downloads_mut().start(id, filename, target.clone());
    b.send(
        pending.tab_id,
        TabCommand::StartDownload {
            id: DownloadId(id),
            url: pending.url,
            target_path: target,
        },
    );
    id
}

/// Decline an offer. Doing nothing at all leaks the offer, so a shell that dismisses its
/// save panel should call this.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_download_reject(browser: *mut BeaconBrowser, offer: u64) {
    let b = browser!(browser);
    b.offers.remove(&offer);
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_download_count(browser: *mut BeaconBrowser) -> usize {
    let b = browser!(browser, 0);
    b.beacon.downloads().entries().len()
}

/// The download id at `index`, newest last, or 0 out of range.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_download_at(browser: *mut BeaconBrowser, index: usize) -> u64 {
    let b = browser!(browser, 0);
    b.beacon.downloads().entries().get(index).map(|e| e.id).unwrap_or(0)
}

/// The download's file name. Free with [`beacon_string_free`]; NULL for an unknown id.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_download_filename(browser: *mut BeaconBrowser, id: u64) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    match b.beacon.downloads().get(id) {
        Some(entry) => to_c_string(&entry.filename),
        None => std::ptr::null_mut(),
    }
}

/// Where the download is being written. Free with [`beacon_string_free`].
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_download_path(browser: *mut BeaconBrowser, id: u64) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    match b.beacon.downloads().get(id) {
        Some(entry) => to_c_string(&entry.path.to_string_lossy()),
        None => std::ptr::null_mut(),
    }
}

/// Fraction complete 0.0--1.0, or -1.0 when the server gave no total -- show bytes received
/// rather than a bar in that case.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_download_progress(browser: *mut BeaconBrowser, id: u64) -> f64 {
    let b = browser!(browser, -1.0);
    b.beacon.downloads().get(id).and_then(|e| e.fraction()).unwrap_or(-1.0)
}

/// Bytes written so far.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_download_received(browser: *mut BeaconBrowser, id: u64) -> u64 {
    let b = browser!(browser, 0);
    b.beacon.downloads().get(id).map(|e| e.received).unwrap_or(0)
}

/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_download_state(browser: *mut BeaconBrowser, id: u64) -> BeaconDownloadState {
    let b = browser!(browser, BeaconDownloadState::Failed);
    use beacon_core::download::DownloadState;
    match b.beacon.downloads().get(id).map(|e| &e.state) {
        Some(DownloadState::Running) => BeaconDownloadState::Running,
        Some(DownloadState::Finished) => BeaconDownloadState::Finished,
        _ => BeaconDownloadState::Failed,
    }
}

/// Open a finished download in the desktop's default application.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_download_open(browser: *mut BeaconBrowser, id: u64) {
    let b = browser!(browser);
    let events = b.beacon.apply(BeaconCommand::OpenDownload(id));
    b.pending.extend(events);
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
            // An offer is parked here with an id rather than pushed at the shell whole: the
            // shell answers asynchronously (a save panel is modal and takes as long as the
            // user does), and by then a second offer may have arrived. The id is what keeps
            // an answer attached to the offer it answers.
            BeaconEvent::DownloadOffered {
                tab_id,
                url,
                suggested_filename,
                ..
            } => {
                let offer = b.next_offer;
                b.next_offer += 1;
                b.offers.insert(
                    offer,
                    PendingOffer {
                        tab_id,
                        url,
                        suggested_filename: suggested_filename.clone(),
                    },
                );
                (
                    BeaconEventKind::DownloadOffered,
                    Some(tab_id),
                    Some(suggested_filename),
                    offer as f64,
                )
            }
            BeaconEvent::DownloadChanged(id) => (BeaconEventKind::DownloadChanged, None, None, id as f64),
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
/// On the GPU path this reads the page texture back off the card, which is deliberately the
/// slow route: a shell that cares about speed attaches a view with
/// [`beacon_attach_view`] and never calls this. It exists for headless use — tests,
/// screenshots, thumbnails.
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

    let (width, height, dpr) = match &handle {
        ExternalHandle::TileCache {
            dpr,
            viewport_width,
            viewport_height,
            ..
        } => ((viewport_width * dpr) as usize, (viewport_height * dpr) as usize, *dpr),
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        ExternalHandle::WgpuTextureId { id, .. } => match b.gpu.read_back(*id) {
            Some((pixels, w, h)) => {
                b.frame = pixels;
                unsafe {
                    std::ptr::write(
                        out,
                        BeaconFrame {
                            pixels: b.frame.as_ptr(),
                            width: w,
                            height: h,
                            stride: w * 4,
                            dpr: 1,
                        },
                    );
                }
                return true;
            }
            None => return false,
        },
        _ => return false,
    };

    let ExternalHandle::TileCache {
        tiles, scroll_x, scroll_y, ..
    } = handle
    else {
        return false;
    };
    if width == 0 || height == 0 {
        return false;
    }

    // Composite onto opaque white at the frame's own scroll position, exactly as the other
    // frontends do — going through the shared compositor is what gets `fixed` and `sticky`
    // right, and the offset is what makes scrolling visible at all.
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

// ── native views (GPU path) ──────────────────────────────────────────────────

/// Draw `tab` directly into a view the shell owns: an `NSView*` on macOS, an `HWND` on
/// Windows. The page is rendered into that view with no copy and no readback, which is what
/// a native chrome wants — it lays the view out among its own widgets and Beacon fills it.
///
/// Returns false on platforms without this path, or if the view cannot be wrapped.
///
/// # Safety
/// `view` must be a valid pointer of the platform's expected type, and must outlive the
/// attachment: call [`beacon_detach_view`] before the view is destroyed.
#[no_mangle]
pub unsafe extern "C" fn beacon_attach_view(browser: *mut BeaconBrowser, tab: u64, view: *mut c_void, width: u32, height: u32) -> bool {
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (browser, tab, view, width, height);
        false
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let b = browser!(browser, false);
        let Some(tab_id) = b.tab(tab) else { return false };
        match unsafe { gpu::ViewSurface::new(&b.gpu, view, width, height) } {
            Ok(surface) => {
                b.views.insert(tab_id, surface);
                true
            }
            Err(e) => {
                log::warn!("beacon_attach_view: {e}");
                false
            }
        }
    }
}

/// Stop drawing into the tab's view. Call this before the view is destroyed.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_detach_view(browser: *mut BeaconBrowser, tab: u64) {
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (browser, tab);
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let b = browser!(browser);
        if let Some(tab_id) = b.tab(tab) {
            b.views.remove(&tab_id);
        }
    }
}

/// Tell Beacon the attached view changed size, in device pixels.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_resize_view(browser: *mut BeaconBrowser, tab: u64, width: u32, height: u32) {
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (browser, tab, width, height);
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let b = browser!(browser);
        let Some(tab_id) = b.tab(tab) else { return };
        let gpu = b.gpu.clone();
        if let Some(surface) = b.views.get_mut(&tab_id) {
            surface.resize(&gpu, width, height);
        }
    }
}

/// Draw the tab's latest frame into its attached view. Call this when a `BEACON_REDRAW`
/// event arrives, from the shell's own draw cycle.
///
/// Returns false if the tab has no view attached or nothing has been rendered yet.
///
/// # Safety
/// `browser` must be a live handle from [`beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_draw_view(browser: *mut BeaconBrowser, tab: u64) -> bool {
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (browser, tab);
        false
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let b = browser!(browser, false);
        let Some(tab_id) = b.tab(tab) else { return false };
        let engine_id = b.tabs.lock().unwrap().get_tab(tab_id).and_then(|t| t.engine_tab_id());
        let Some(engine_id) = engine_id else { return false };
        let Some(ExternalHandle::WgpuTextureId { id, .. }) = b.engine.compositor.frame_for(engine_id) else {
            return false;
        };
        let Some((_, page)) = gosub_renderer_vello::WgpuContextProvider::get_texture(&*b.gpu, id) else {
            return false;
        };
        let gpu = b.gpu.clone();
        match b.views.get(&tab_id) {
            Some(surface) => {
                surface.present(&gpu, &page);
                true
            }
            None => false,
        }
    }
}
