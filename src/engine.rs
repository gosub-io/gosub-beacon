//! Thin wrapper around the new (local `../engine`) `GosubEngine`.
//!
//! The new engine is fully asynchronous and owns networking, cookies, storage and the
//! render pipeline. A browser tab maps onto an engine [`TabHandle`]; rendering is consumed
//! from the [`DefaultCompositor`] as ready-to-blit tiles (see [`draw_frame`]).

use std::sync::Arc;

use gosub_engine::cookies::SqliteCookieStore;
use gosub_engine::events::EngineEvent;
use gosub_engine::storage::{InMemorySessionStore, PartitionPolicy, SqliteLocalStore, StorageService};
use gosub_engine::tab::{TabDefaults, TabHandle};
use gosub_engine::zone::{Zone, ZoneConfig, ZoneId, ZoneServices};
use gosub_engine::{DefaultRenderConfig, GosubEngine};
use gosub_render_pipeline::render::backend::{blend_over_argb_u32, CachedTile, ExternalHandle};
use gosub_render_pipeline::render::DefaultCompositor;
use gosub_renderer_cairo::{CairoBackend, PangoFontSystem};
use parking_lot::RwLock;
use tokio::runtime::Runtime;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use uuid::uuid;

const DEFAULT_ZONE: uuid::Uuid = uuid!("f1234567-abcd-4000-8000-000000000001");

/// The engine is generic over a render configuration; we render through Cairo with Pango text.
type AppConfig = DefaultRenderConfig<CairoBackend, PangoFontSystem>;

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

        // Cache GTK-derived resources while on the main thread (rasterizer threads must
        // never touch GTK globals).
        gosub_renderer_cairo::init_gtk_resources()?;

        let (tx_redraw, rx_redraw) = mpsc::unbounded_channel::<()>();
        let compositor = Arc::new(DefaultCompositor::new(move || {
            let _ = tx_redraw.send(());
        }));

        let backend = CairoBackend::new();
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
    pub fn create_tab(&mut self, rt: &Runtime, title: &str) -> anyhow::Result<TabHandle> {
        let defaults = TabDefaults {
            url: None,
            title: Some(title.to_string()),
            // Viewport is set on the first GTK resize so the DPR is correct.
            viewport: None,
        };

        let tab = rt
            .block_on(self.zone.create_tab(defaults, None))
            .map_err(|e| anyhow::anyhow!("create_tab: {e:?}"))?;
        Ok(tab)
    }
}

/// Blit the latest composited frame for `tab_id` into the cairo context `cr`.
/// Falls back to a light placeholder when no frame is available yet.
pub fn draw_frame(compositor: &Arc<DefaultCompositor>, tab_id: EngineTabId, cr: &gtk4::cairo::Context, w: i32, h: i32) {
    match compositor.frame_for(tab_id) {
        Some(ExternalHandle::TileCache {
            dpr,
            scroll_x,
            scroll_y,
            tiles,
            ..
        }) => draw_tile_cache(cr, w, h, &tiles, dpr, scroll_x, scroll_y),
        Some(ExternalHandle::CpuPixelsOwned {
            width,
            height,
            stride,
            pixels,
            ..
        }) => blit_pixels(cr, w, pixels, width, height, stride),
        Some(ExternalHandle::CpuPixelsPtr {
            width,
            height,
            stride,
            pixel_buf,
        }) => {
            // SAFETY: pixel_buf points at `height * stride` bytes valid for this call.
            let owned = unsafe { std::slice::from_raw_parts(pixel_buf.as_ptr(), (height as usize) * (stride as usize)) }.to_vec();
            blit_pixels(cr, w, owned, width, height, stride);
        }
        _ => draw_placeholder(cr, w, h),
    }
}

fn blit_pixels(cr: &gtk4::cairo::Context, widget_w: i32, pixels: Vec<u8>, width: u32, height: u32, stride: u32) {
    let frame_scale = (width as f64 / widget_w.max(1) as f64).round() as i32;
    match gtk4::cairo::ImageSurface::create_for_data(pixels, gtk4::cairo::Format::ARgb32, width as i32, height as i32, stride as i32) {
        Ok(surface) => {
            surface.flush();
            if frame_scale > 1 {
                surface.set_device_scale(frame_scale as f64, frame_scale as f64);
            }
            cr.set_source_surface(&surface, 0.0, 0.0).unwrap_or_default();
            cr.paint().unwrap_or_default();
        }
        Err(e) => log::warn!("blit surface failed: {e:?}"),
    }
}

/// Composite all tiles into a single physical-resolution surface, then paint it once.
///
/// Per-tile `set_source_surface`+`paint` at fractional positions produces 1-pixel seams at
/// tile boundaries (bilinear filtering / AA at each source surface edge). CPU-blitting every
/// tile into one buffer at integer pixel offsets avoids those seams entirely.
fn draw_tile_cache(cr: &gtk4::cairo::Context, w: i32, h: i32, tiles: &[CachedTile], dpr: u32, scroll_x: f32, scroll_y: f32) {
    let dpr_i = dpr as i32;
    let dpr_f = dpr as f64;

    let w_phys = w * dpr_i;
    let h_phys = h * dpr_i;
    if w_phys <= 0 || h_phys <= 0 {
        return;
    }

    let Ok(mut dst) = gtk4::cairo::ImageSurface::create(gtk4::cairo::Format::ARgb32, w_phys, h_phys) else {
        return;
    };
    let stride = dst.stride() as usize;

    {
        let Ok(mut data) = dst.data() else {
            return;
        };

        // White background (ARGB32 premultiplied little-endian = 0xFFFF_FFFF).
        for b in data.chunks_exact_mut(4) {
            b[0] = 0xFF;
            b[1] = 0xFF;
            b[2] = 0xFF;
            b[3] = 0xFF;
        }

        let sx = (scroll_x * dpr as f32).round() as i64;
        let sy = (scroll_y * dpr as f32).round() as i64;

        for tile in tiles.iter() {
            let px = (tile.page_x * dpr as f32).round() as i64 - sx;
            let py = (tile.page_y * dpr as f32).round() as i64 - sy;
            let tw = tile.width as i64;
            let th = tile.height as i64;

            if px >= w_phys as i64 || py >= h_phys as i64 || px + tw <= 0 || py + th <= 0 {
                continue;
            }

            let tile_col0 = (-px).max(0) as usize;
            let tile_row0 = (-py).max(0) as usize;
            let dst_x = px.max(0) as usize;
            let dst_y0 = py.max(0) as usize;
            let tw_usize = tw as usize;
            let th_usize = th as usize;

            for tile_row in tile_row0..th_usize {
                let dst_y = dst_y0 + (tile_row - tile_row0);
                if dst_y >= h_phys as usize {
                    break;
                }
                let copy_w = (tw_usize - tile_col0).min(w_phys as usize - dst_x);
                if copy_w == 0 {
                    break;
                }
                let src_off = (tile_row * tw_usize + tile_col0) * 4;
                let dst_off = dst_y * stride + dst_x * 4;
                // Source-over blend so transparent pixels of an upper-layer tile reveal
                // whatever was drawn beneath it.
                for col in 0..copy_w {
                    let s = src_off + col * 4;
                    let d = dst_off + col * 4;
                    let src_px = u32::from_le_bytes([tile.data[s], tile.data[s + 1], tile.data[s + 2], tile.data[s + 3]]);
                    let src_argb = tile.format.pixel_to_argb_u32(src_px);
                    let dst_px = u32::from_le_bytes([data[d], data[d + 1], data[d + 2], data[d + 3]]);
                    let out = blend_over_argb_u32(src_argb, dst_px);
                    data[d..d + 4].copy_from_slice(&out.to_le_bytes());
                }
            }
        }
    }

    // Device scale so GTK maps 1 CSS px → dpr physical px; `Good` keeps any resample smooth.
    dst.set_device_scale(dpr_f, dpr_f);
    cr.set_source_surface(&dst, 0.0, 0.0).unwrap_or_default();
    cr.source().set_filter(gtk4::cairo::Filter::Good);
    cr.paint().unwrap_or_default();
}

fn draw_placeholder(cr: &gtk4::cairo::Context, w: i32, h: i32) {
    cr.set_source_rgb(0.92, 0.92, 0.92);
    cr.rectangle(0.0, 0.0, w as f64, h as f64);
    cr.fill().unwrap_or_default();
}
