//! The wgpu device Vello renders through, borrowed from eframe.
//!
//! `VelloBackend` is generic over a [`WgpuContextProvider`] so the embedder decides where
//! the GPU comes from. Here it is egui's own wgpu render state, which means the page and
//! the chrome share one device and one queue — the page's texture can be handed straight
//! to egui with no readback.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use eframe::CreationContext;
use gosub_renderer_vello::WgpuContextProvider;
use parking_lot::RwLock;
use vello::wgpu;

pub struct EguiContextProvider {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    textures: RwLock<HashMap<u64, (wgpu::Texture, wgpu::TextureView)>>,
    next_id: AtomicU64,
}

impl EguiContextProvider {
    /// `None` when eframe is not running its wgpu renderer, which the caller must treat as
    /// fatal — there is no software fallback here.
    pub fn from_eframe(cc: &CreationContext) -> Option<Self> {
        let state = cc.wgpu_render_state.as_ref()?;
        Some(Self {
            device: Arc::new(state.device.clone()),
            queue: Arc::new(state.queue.clone()),
            textures: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    pub fn device_ref(&self) -> &wgpu::Device {
        &self.device
    }
}

impl WgpuContextProvider for EguiContextProvider {
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
            label: Some("beacon-vello-texture"),
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
