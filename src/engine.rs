//! Thin wrapper around the new (local `../engine`) `GosubEngine`.
//!
//! The new engine is fully asynchronous and owns networking, cookies, storage and the
//! render pipeline. A browser tab maps onto an engine [`TabHandle`]. Skia rasterizes
//! pages into CPU tile buffers; the [`DefaultCompositor`] delivers them as
//! `ExternalHandle::TileCache` frames, which [`render_frame_gl`] composites on the GPU
//! into a `GtkGLArea`'s framebuffer.

use std::sync::Arc;

use gtk4::prelude::*;

use gosub_engine::cookies::SqliteCookieStore;
use gosub_engine::events::EngineEvent;
use gosub_engine::storage::{InMemorySessionStore, PartitionPolicy, SqliteLocalStore, StorageService};
use gosub_engine::tab::{TabDefaults, TabHandle};
use gosub_engine::zone::{Zone, ZoneConfig, ZoneId, ZoneServices};
use gosub_engine::{DefaultRenderConfig, GosubEngine};
use gosub_render_pipeline::render::backend::{anchored_tile_pos, ExternalHandle};
use gosub_render_pipeline::render::DefaultCompositor;
use gosub_renderer_skia::{SkiaBackend, SkiaFontSystem};
use tokio::runtime::Runtime;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use uuid::uuid;

const DEFAULT_ZONE: uuid::Uuid = uuid!("f1234567-abcd-4000-8000-000000000001");

/// The engine is generic over a render configuration; we rasterize through Skia.
type AppConfig = DefaultRenderConfig<SkiaBackend, SkiaFontSystem>;

/// Engine TabId, re-exported for callers that need to key on the engine's identifier.
pub type EngineTabId = gosub_engine::tab::TabId;

/// Owns the running engine, a single default zone and the shared compositor.
///
/// Created once per browser window. The engine itself runs on the shared tokio runtime;
/// this struct lives on the GTK main thread.
pub struct BrowserEngine {
    /// Kept alive so the engine keeps running for the lifetime of the window.
    #[allow(dead_code)]
    engine: GosubEngine<AppConfig>,
    zone: Zone<AppConfig>,
    /// Shared compositor; clone the `Arc` into draw callbacks to read frames.
    pub compositor: Arc<DefaultCompositor>,
    /// Fires (after `take_redraw_rx`) whenever a new frame is composited.
    redraw_rx: Option<mpsc::UnboundedReceiver<()>>,
    /// Engine event stream. Subscribed before the zone is created (the engine emits
    /// `ZoneCreated` immediately, which fails if no receiver is alive yet).
    event_rx: Option<broadcast::Receiver<EngineEvent>>,
}

impl BrowserEngine {
    /// Build and start the engine. Must be called with `rt` as the active tokio runtime
    /// for engine tasks to spawn correctly.
    pub fn new(rt: &Runtime) -> anyhow::Result<Self> {
        let _guard = rt.enter();

        let (tx_redraw, rx_redraw) = mpsc::unbounded_channel::<()>();
        let compositor = Arc::new(DefaultCompositor::new(move || {
            let _ = tx_redraw.send(());
        }));

        let backend = SkiaBackend::new();
        let mut engine = GosubEngine::<AppConfig>::new(None, Arc::new(backend), compositor.clone());
        // start() hands back the engine main-loop future; it only runs once spawned.
        let engine_loop = engine.start().map_err(|e| anyhow::anyhow!("engine start: {e:?}"))?;
        tokio::spawn(engine_loop);

        // Subscribe before creating the zone: `create_zone` emits `ZoneCreated` on the
        // event channel, which errors out ("channel closed") if there is no live receiver.
        let event_rx = engine.subscribe_events();

        let zone_cfg = ZoneConfig::builder()
            .do_not_track(true)
            .build()
            .map_err(|e| anyhow::anyhow!("ZoneConfig: {e:?}"))?;

        let cookie_store: gosub_engine::cookies::CookieStoreHandle = SqliteCookieStore::new(".gosub-beacon-cookies.db".into())
            .map_err(|e| anyhow::anyhow!("cookie store: {e:?}"))?
            .into();

        let zone_services = ZoneServices {
            storage: Arc::new(StorageService::new(
                Arc::new(SqliteLocalStore::new(".gosub-beacon-local.db").map_err(|e| anyhow::anyhow!("local store: {e:?}"))?),
                Arc::new(InMemorySessionStore::new()),
            )),
            cookie_store: Some(cookie_store),
            cookie_jar: None,
            partition_policy: PartitionPolicy::None,
        };

        let zone = engine
            .create_zone(Some(zone_cfg), zone_services, Some(ZoneId::from(DEFAULT_ZONE)))
            .map_err(|e| anyhow::anyhow!("create_zone: {e:?}"))?;

        Ok(Self {
            engine,
            zone,
            compositor,
            redraw_rx: Some(rx_redraw),
            event_rx: Some(event_rx),
        })
    }

    /// Take the engine event stream (navigation, redraw, hover, …). Only the first
    /// caller receives the receiver that was subscribed before zone creation.
    pub fn take_event_rx(&mut self) -> Option<broadcast::Receiver<EngineEvent>> {
        self.event_rx.take()
    }

    /// Take the redraw notification receiver (drains compositor frame notifications).
    /// Only the first caller receives it.
    pub fn take_redraw_rx(&mut self) -> Option<mpsc::UnboundedReceiver<()>> {
        self.redraw_rx.take()
    }

    /// Create a fresh engine tab in the default zone. Blocks on the runtime.
    ///
    /// `viewport` is the initial size in CSS px. Pass the currently visible content
    /// area's size: a hidden `GtkStack` page is never allocated, so its GLArea's
    /// resize handler (the only other `SetViewport` source) does not fire until the
    /// tab is first shown — without an initial viewport, background tabs lay out and
    /// rasterize at the engine's default size and must fully re-render on switch.
    pub fn create_tab(&mut self, rt: &Runtime, title: &str, viewport: Option<(u32, u32)>) -> anyhow::Result<TabHandle> {
        let defaults = TabDefaults {
            url: None,
            title: Some(title.to_string()),
            // Falls back to the first GTK resize when the window has no allocation yet.
            viewport: viewport.map(|(w, h)| gosub_render_pipeline::render::Viewport::new(0, 0, w, h)),
        };

        let tab = rt
            .block_on(self.zone.create_tab(defaults, None))
            .map_err(|e| anyhow::anyhow!("create_tab: {e:?}"))?;
        Ok(tab)
    }
}

/// Resolve the device-pixel ratio to render at for `widget`.
///
/// `GtkWidget::scale_factor()` only ever reports an integer, so on a fractionally
/// scaled display (e.g. 1.25× or 1.5×, common on Wayland) it returns 1 and the page
/// is rasterized at logical resolution — the compositor then upscales the whole
/// surface, blurring text. `GdkSurface::scale()` exposes the true fractional scale;
/// round it *up* and render at that resolution: downscaling a slightly-too-large
/// buffer stays sharp, upscaling a too-small one does not.
pub fn render_dpr(widget: &impl IsA<gtk4::Widget>) -> u32 {
    let widget = widget.upcast_ref::<gtk4::Widget>();
    let fractional = widget
        .native()
        .and_then(|n| n.surface())
        .map(|s| s.scale())
        .filter(|s| *s > 0.0)
        .unwrap_or_else(|| widget.scale_factor() as f64);
    fractional.ceil().max(1.0) as u32
}

// Link libGL so glGetIntegerv resolves (used to query GTK4's bound FBO).
#[link(name = "GL")]
extern "C" {}

/// Query the framebuffer GTK4 bound for the current `GLArea` render pass.
fn bound_framebuffer() -> u32 {
    extern "C" {
        fn glGetIntegerv(pname: u32, data: *mut i32);
    }
    let mut fbo = 0i32;
    unsafe {
        glGetIntegerv(0x8CA6 /* GL_DRAW_FRAMEBUFFER_BINDING */, &mut fbo)
    };
    fbo as u32
}

/// Composite the latest tile frame for `tab_id` into the currently bound GL framebuffer.
///
/// Must run on the GTK main thread with the `GLArea`'s GL context current (i.e. from
/// `connect_render`), with `dc` the Skia `DirectContext` created on that same context.
/// `phys_w`/`phys_h` are the framebuffer size in physical pixels. Clears to white when
/// no frame is available yet.
pub fn render_frame_gl(
    compositor: &Arc<DefaultCompositor>,
    tab_id: EngineTabId,
    dc: &mut skia_safe::gpu::DirectContext,
    phys_w: i32,
    phys_h: i32,
) {
    if phys_w <= 0 || phys_h <= 0 {
        return;
    }

    let fb_info = skia_safe::gpu::gl::FramebufferInfo {
        fboid: bound_framebuffer(),
        format: 0x8058, // GL_RGBA8
        protected: skia_safe::gpu::Protected::No,
    };
    let target = skia_safe::gpu::backend_render_targets::make_gl((phys_w, phys_h), None, 8, fb_info);
    let Some(mut surface) = skia_safe::gpu::surfaces::wrap_backend_render_target(
        dc,
        &target,
        skia_safe::gpu::SurfaceOrigin::BottomLeft,
        skia_safe::ColorType::RGBA8888,
        None,
        None,
    ) else {
        log::warn!("failed to wrap GTK framebuffer as Skia surface");
        return;
    };

    {
        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color4f::new(1.0, 1.0, 1.0, 1.0));

        if let Some(ExternalHandle::TileCache {
            dpr,
            scroll_x,
            scroll_y,
            tiles,
            ..
        }) = compositor.frame_for(tab_id)
        {
            for tile in tiles.iter() {
                // anchored_tile_pos handles scroll / fixed / sticky uniformly (in CSS px);
                // scale the result to physical pixels for the GL surface.
                let (vx, vy) = anchored_tile_pos(
                    tile.page_x as f64,
                    tile.page_y as f64,
                    scroll_x as f64,
                    scroll_y as f64,
                    tile.anchor,
                );
                let px = (vx * dpr as f64).round() as i32;
                let py = (vy * dpr as f64).round() as i32;
                let tw = tile.width as i32;
                let th = tile.height as i32;

                if px >= phys_w || py >= phys_h || px + tw <= 0 || py + th <= 0 {
                    continue;
                }

                // Tile data is BGRA premultiplied (Cairo ARGB32 LE byte order).
                let info = skia_safe::ImageInfo::new((tw, th), skia_safe::ColorType::BGRA8888, skia_safe::AlphaType::Premul, None);
                let stride = (tw * 4) as usize;
                if tile.data.len() < th as usize * stride {
                    continue;
                }
                if let Some(image) = skia_safe::images::raster_from_data(&info, skia_safe::Data::new_copy(&tile.data), stride) {
                    // Fade the whole tile by its layer's group opacity.
                    let paint = (tile.opacity < 1.0).then(|| {
                        let mut p = skia_safe::Paint::default();
                        p.set_alpha_f(tile.opacity);
                        p
                    });
                    canvas.draw_image(&image, (px as f32, py as f32), paint.as_ref());
                }
            }
        }
    }

    dc.flush_surface(&mut surface);
    dc.submit(skia_safe::gpu::SyncCpu::No);
}
