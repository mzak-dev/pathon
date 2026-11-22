use std::any::Any;
use std::sync::Arc;

use pathon_api::graph::FrameObserver;
use pathon_api::{Pack, PackRegistrar, RenderFeature, SerialTaskPool, Shared, TaskPool};
use pathon_rhi::{Color, Device, DeviceApi, DeviceRequest, MaybeSend};

use crate::error::RenderError;
use crate::registry::Registry;
use crate::renderer::{DeviceOwnership, Renderer};

pub struct RendererBuilder {
    request: DeviceRequest,
    features: Vec<Box<dyn RenderFeature>>,
    shared: Shared,
    task_pool: Arc<dyn TaskPool>,
    device: Option<Arc<Device>>,
    frames_in_flight: u64,
    clear_color: Color,
    observer: Option<Arc<dyn FrameObserver>>,
}

impl std::fmt::Debug for RendererBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RendererBuilder")
            .field("request", &self.request)
            .field("features", &self.features.len())
            .field("borrowed_device", &self.device.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for RendererBuilder {
    fn default() -> Self {
        Self {
            request: DeviceRequest {
                label: "pathon",
                ..DeviceRequest::default()
            },
            features: Vec::new(),
            shared: Shared::new(),
            task_pool: Arc::new(SerialTaskPool),
            device: None,
            frames_in_flight: 2,
            clear_color: Color::BLACK,
            observer: None,
        }
    }
}

struct Prepared {
    registry: Registry,
    shared: Shared,
    request: DeviceRequest,
    borrowed: Option<Arc<Device>>,
    task_pool: Arc<dyn TaskPool>,
    frames_in_flight: u64,
    clear_color: Color,
    observer: Option<Arc<dyn FrameObserver>>,
}

impl Prepared {
    fn finish(mut self, device: Arc<Device>, ownership: DeviceOwnership) -> Renderer {
        let handle: Arc<dyn DeviceApi> = Arc::<Device>::clone(&device);
        self.registry.activate(&handle, &self.task_pool, &mut self.shared);
        let mut renderer = Renderer::new(
            device,
            ownership,
            self.registry,
            self.task_pool,
            self.frames_in_flight,
            self.clear_color,
            self.shared,
        );
        renderer.set_frame_observer(self.observer);
        renderer
    }
}

impl RendererBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_feature(mut self, feature: impl RenderFeature + 'static) -> Self {
        self.features.push(Box::new(feature));
        self
    }

    // A pack may bring several features and the data they share (e.g. PBR: prepass, lighting, IBL).
    #[must_use]
    pub fn with_pack(mut self, pack: impl Pack) -> Self {
        let mut registrar = PackRegistrar::new();
        pack.register(&mut registrar);
        registrar.merge_into(&mut self.features, &mut self.shared);
        self
    }

    // Data for packs that the application provides up front; it can change it later through `Renderer::shared_mut`.
    #[must_use]
    pub fn with_shared<T: Any + MaybeSend>(mut self, value: T) -> Self {
        self.shared.insert(value);
        self
    }

    #[must_use]
    pub fn with_boxed_feature(mut self, feature: Box<dyn RenderFeature>) -> Self {
        self.features.push(feature);
        self
    }

    #[must_use]
    pub fn with_device_request(self, request: DeviceRequest) -> Self {
        Self { request, ..self }
    }

    // Picks an adapter that can present to this window; keeps the rest of the device request.
    #[must_use]
    pub fn with_compatible_surface(mut self, target: pathon_rhi::SurfaceTarget) -> Self {
        self.request.compatible_surface = Some(target);
        self
    }

    // Integration modes A and B: the host owns the device, so pack requirements can no longer shape it.
    #[must_use]
    pub fn with_device(self, device: Arc<Device>) -> Self {
        Self {
            device: Some(device),
            ..self
        }
    }

    #[must_use]
    pub fn with_task_pool(self, task_pool: Arc<dyn TaskPool>) -> Self {
        Self { task_pool, ..self }
    }

    #[must_use]
    pub fn with_frames_in_flight(self, frames_in_flight: u64) -> Self {
        Self {
            frames_in_flight,
            ..self
        }
    }

    // Profilers (ADR-0026); `Renderer::set_frame_observer` changes it later.
    #[must_use]
    pub fn with_frame_observer(self, observer: Arc<dyn FrameObserver>) -> Self {
        Self {
            observer: Some(observer),
            ..self
        }
    }

    #[must_use]
    pub fn with_clear_color(self, clear_color: Color) -> Self {
        Self { clear_color, ..self }
    }

    fn prepare(self) -> Prepared {
        let mut registry = Registry::new(self.features);
        let mut request = self.request;
        registry.merge_requirements(&mut request);
        Prepared {
            registry,
            shared: self.shared,
            request,
            borrowed: self.device,
            task_pool: self.task_pool,
            frames_in_flight: self.frames_in_flight,
            clear_color: self.clear_color,
            observer: self.observer,
        }
    }

    pub async fn build_async(self) -> Result<Renderer, RenderError> {
        let mut prepared = self.prepare();
        if let Some(device) = prepared.borrowed.take() {
            return Ok(prepared.finish(device, DeviceOwnership::Borrowed));
        }
        #[cfg_attr(
            target_arch = "wasm32",
            expect(clippy::arc_with_non_send_sync, reason = "one thread on the web (ADR-0004)")
        )]
        let device = Arc::new(Device::build_async(&prepared.request).await?);
        let ownership = DeviceOwnership::Owned(prepared.request.clone());
        Ok(prepared.finish(device, ownership))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn build(self) -> Result<Renderer, RenderError> {
        let mut prepared = self.prepare();
        if let Some(device) = prepared.borrowed.take() {
            return Ok(prepared.finish(device, DeviceOwnership::Borrowed));
        }
        let device = Arc::new(Device::build(&prepared.request)?);
        let ownership = DeviceOwnership::Owned(prepared.request.clone());
        Ok(prepared.finish(device, ownership))
    }
}
