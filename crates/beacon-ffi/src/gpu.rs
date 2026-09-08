//! The GPU path: Vello renders the page into a wgpu texture, and that texture is blitted
//! into a view the native shell owns.
//!
//! This is the arrangement a Swift or WinUI chrome wants. The shell lays out a view among
//! its own widgets and hands the pointer over; the page is drawn straight into it, with no
//! copy and no readback. `beacon_acquire_frame` still exists for anything headless — the C
//! smoke test, screenshots — and on this path it reads the texture back, which is the slow
//! route by design rather than by accident.
//!
//! Only compiled where a native surface makes sense. Elsewhere the CPU rasterizer is both
//! simpler and sufficient.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use gosub_renderer_vello::WgpuContextProvider;
use parking_lot::RwLock;
use vello::wgpu;

/// The wgpu device Vello renders through, plus the registry of textures it has created.
///
/// Built without a surface: a browser exists before any view does, and tabs can render
/// while detached. The surface arrives later, in [`ViewSurface`].
pub struct FfiWgpuContext {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    instance: wgpu::Instance,
    /// Held so an attached view can ask what its surface supports.
    adapter: wgpu::Adapter,
    textures: RwLock<HashMap<u64, (wgpu::Texture, wgpu::TextureView)>>,
    next_id: AtomicU64,
}

impl FfiWgpuContext {
    /// Pick an adapter and device with no surface in hand. On macOS this is Metal, which
    /// every Mac has — there is no software-fallback question to answer here.
    pub fn new(rt: &tokio::runtime::Runtime) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = rt
            .block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                // No surface yet; one is attached later and must be compatible with this
                // adapter. On macOS that is safe because there is a single Metal adapter.
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
            .map_err(|e| anyhow::anyhow!("no wgpu adapter: {e}"))?;

        let (device, queue) = rt
            .block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("beacon-ffi"),
                required_features: wgpu::Features::empty(),
                // Vello's compute shaders need more than the downlevel defaults; asking for
                // the adapter's own limits avoids the "max_storage_buffers_per_shader_stage
                // is 0" failure that the downlevel set produces.
                required_limits: adapter.limits(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            }))
            .map_err(|e| anyhow::anyhow!("no wgpu device: {e}"))?;

        Ok(Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
            instance,
            adapter,
            textures: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    /// Copy a page texture off the GPU as BGRA bytes.
    ///
    /// Deliberately the slow path: a shell that cares attaches a view instead. This exists
    /// so headless callers -- tests, screenshots, thumbnails -- still work on the GPU
    /// platforms, and so `beacon_acquire_frame` means the same thing everywhere.
    pub fn read_back(&self, texture_id: u64) -> Option<(Vec<u8>, u32, u32)> {
        let (texture, _) = self.get_texture(texture_id)?;
        let (width, height) = (texture.width(), texture.height());

        // wgpu requires each row of a texture-to-buffer copy to start on a 256-byte
        // boundary, so the buffer is padded and the padding stripped after mapping.
        let unpadded = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded.div_ceil(align) * align;

        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("beacon-readback"),
            size: (padded as u64) * (height as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("beacon-readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        // Block until the copy lands: the caller asked for pixels, synchronously.
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv().ok()?.ok()?;

        let mapped = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((unpadded * height) as usize);
        for row in 0..height {
            let start = (row * padded) as usize;
            pixels.extend_from_slice(&mapped[start..start + unpadded as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Some((pixels, width, height))
    }
}

impl WgpuContextProvider for FfiWgpuContext {
    fn device(&self) -> &wgpu::Device {
        &self.device
    }

    fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    fn device_arc(&self) -> Arc<wgpu::Device> {
        Arc::clone(&self.device)
    }

    fn queue_arc(&self) -> Arc<wgpu::Queue> {
        Arc::clone(&self.queue)
    }

    fn create_texture(&self, width: u32, height: u32, format: wgpu::TextureFormat) -> u64 {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("beacon-page"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.textures.write().insert(id, (texture, view));
        id
    }

    fn get_texture(&self, id: u64) -> Option<(wgpu::Texture, wgpu::TextureView)> {
        self.textures.read().get(&id).map(|(t, v)| (t.clone(), v.clone()))
    }

    fn remove_texture(&self, id: u64) {
        self.textures.write().remove(&id);
    }
}

/// A surface over a view the shell owns, plus the pipeline that blits a page texture onto
/// it. One per attached tab.
pub struct ViewSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl ViewSurface {
    /// Wrap a native view. `handle` is an `NSView*` on macOS and an `HWND` on Windows.
    ///
    /// # Safety
    /// `handle` must be a valid pointer to a view of the platform's expected type, and it
    /// must outlive this surface — the shell must call `beacon_detach_view` before the view
    /// goes away.
    pub unsafe fn new(context: &FfiWgpuContext, handle: *mut std::ffi::c_void, width: u32, height: u32) -> anyhow::Result<Self> {
        let ptr = std::ptr::NonNull::new(handle).ok_or_else(|| anyhow::anyhow!("null view handle"))?;

        // Both handles are required, even though AppKit's display handle is an empty
        // struct carrying nothing. wgpu-hal matches on the *pair* -- `(AppKit(_),
        // AppKit(handle))` -- so passing None for the display drops through to
        // "not a Metal-compatible handle" and the view never attaches.
        #[cfg(target_os = "macos")]
        let raw_display = raw_window_handle::RawDisplayHandle::AppKit(raw_window_handle::AppKitDisplayHandle::new());
        #[cfg(target_os = "windows")]
        let raw_display = raw_window_handle::RawDisplayHandle::Windows(raw_window_handle::WindowsDisplayHandle::new());

        #[cfg(target_os = "macos")]
        let raw_window = raw_window_handle::RawWindowHandle::AppKit(raw_window_handle::AppKitWindowHandle::new(ptr));
        #[cfg(target_os = "windows")]
        let raw_window = {
            let mut handle = raw_window_handle::Win32WindowHandle::new(
                std::num::NonZeroIsize::new(ptr.as_ptr() as isize).ok_or_else(|| anyhow::anyhow!("null HWND"))?,
            );
            handle.hinstance = None;
            raw_window_handle::RawWindowHandle::Win32(handle)
        };

        let surface = unsafe {
            context.instance().create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(raw_display),
                raw_window_handle: raw_window,
            })?
        };

        let caps = surface.get_capabilities(&context.adapter);

        // Non-sRGB where possible: Vello's texture already holds sRGB-encoded bytes, and an
        // sRGB surface format would encode them twice — washing colours out and thinning
        // glyph edges.
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: caps.present_modes[0],
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(context.device(), &config);

        let (pipeline, layout, sampler) = build_blit_pipeline(context.device(), format);
        Ok(Self {
            surface,
            config,
            pipeline,
            layout,
            sampler,
        })
    }

    pub fn resize(&mut self, context: &FfiWgpuContext, width: u32, height: u32) {
        if width == 0 || height == 0 || (width == self.config.width && height == self.config.height) {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(context.device(), &self.config);
    }

    /// Draw a page texture onto the view. A dropped frame is not worth reporting: the next
    /// redraw is milliseconds away, and a shell cannot do anything useful about one.
    pub fn present(&self, context: &FfiWgpuContext, page: &wgpu::TextureView) {
        // A frame that is not Success is not worth reporting: the next redraw is
        // milliseconds away, and a shell cannot do anything useful about one.
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => return,
        };
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = context.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("beacon-blit"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(page),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let mut encoder = context.device().create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("beacon-present"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("beacon-present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        context.queue().submit(Some(encoder.finish()));
        frame.present();
    }
}

/// A full-screen triangle that samples the page texture. Three vertices generated in the
/// shader rather than a quad in a buffer: no vertex data to manage, and no seam down the
/// diagonal where two triangles would meet.
fn build_blit_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout, wgpu::Sampler) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("beacon-blit"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    out.uv = vec2<f32>(x, y);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

@group(0) @binding(0) var page: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(page, samp, in.uv);
}
"#
            .into(),
        ),
    });

    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("beacon-blit"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("beacon-blit"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("beacon-blit"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            targets: &[Some(format.into())],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("beacon-blit"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    (pipeline, layout, sampler)
}
