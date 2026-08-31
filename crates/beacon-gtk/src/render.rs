//! Getting the engine's frames onto a `GtkGLArea`.
//!
//! Skia rasterizes pages into CPU tile buffers; the shared `DefaultCompositor` delivers
//! them as `ExternalHandle::TileCache` frames, which [`render_frame_gl`] composites on the
//! GPU into the framebuffer GTK bound for the current render pass.
//!
//! This is the half of the old `engine.rs` that could not move to `beacon-core`: it is
//! GTK and GL all the way down. The engine setup it used to sit next to now lives in
//! [`beacon_core::engine`].

use std::sync::Arc;

use gtk4::prelude::*;

use beacon_core::engine::EngineTabId;
use gosub_render_pipeline::render::backend::{anchored_tile_pos, ExternalHandle};
use gosub_render_pipeline::render::DefaultCompositor;

/// The render configuration this frontend runs: Skia rasterization, Skia font system.
pub type GtkConfig = gosub_engine::DefaultRenderConfig<gosub_renderer_skia::SkiaBackend, gosub_renderer_skia::SkiaFontSystem>;

/// The engine, specialised for this frontend.
pub type BrowserEngine = beacon_core::engine::BrowserEngine<GtkConfig>;

/// Build the rasterizer this frontend hands to the engine.
pub fn backend() -> Arc<gosub_renderer_skia::SkiaBackend> {
    Arc::new(gosub_renderer_skia::SkiaBackend::new())
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
    target_scale: f64,
) {
    if phys_w <= 0 || phys_h <= 0 {
        return;
    }

    // We share this GL context with GTK, which mutates state Skia caches (scissor, blend, bound
    // FBO, viewport). Without this, Skia keeps drawing against a stale idea of that state, which
    // shows up as region-shaped artifacts -- e.g. a corner that stays unpainted.
    dc.reset(None);

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

        // `target_scale` is the physical-px-per-CSS-px the shell wants on screen (display
        // scale × page zoom). Tiles arrive rasterized at `tile_dpr`; the difference is bridged
        // here, which also keeps stale tiles (rasterized at the previous zoom's dpr) at the
        // correct on-screen size until fresh ones land.
        let correction = match compositor.frame_for(tab_id) {
            Some(ExternalHandle::TileCache { dpr: tile_dpr, .. }) => target_scale / tile_dpr.max(1) as f64,
            _ => 1.0,
        };
        if (correction - 1.0).abs() > 1e-3 {
            canvas.scale((correction as f32, correction as f32));
        }

        // Cull bounds live in CANVAS space, which the scale above has divorced from physical
        // pixels: a tile at canvas x maps to screen x * correction. Comparing raw canvas
        // coordinates against the physical surface size drops every tile past
        // `phys_w`, even though anything up to `phys_w / correction` is still on screen --
        // which is why zooming in used to leave a blank band down the right-hand side.
        let cull_w = (phys_w as f64 / correction).ceil() as i32;
        let cull_h = (phys_h as f64 / correction).ceil() as i32;

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

                if px >= cull_w || py >= cull_h || px + tw <= 0 || py + th <= 0 {
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
