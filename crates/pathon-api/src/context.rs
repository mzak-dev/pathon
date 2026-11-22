use std::sync::Arc;

use glam::Vec2;
use pathon_diag::Diagnostic;
use pathon_graph::{FrameGraph, GraphTextureDesc, TextureHandle};
use pathon_rhi_types::{Caps, DeviceApi, PhysicalSize, Resource};

use crate::graph_builder::GraphBuilder;
use crate::shared::Shared;
use crate::slot::{Slot, Slots};
use crate::task_pool::TaskPool;
use crate::view::ViewCtx;

pub struct SetupCtx<'frame> {
    device: &'frame Arc<dyn DeviceApi>,
    task_pool: &'frame Arc<dyn TaskPool>,
    shared: &'frame mut Shared,
}

impl std::fmt::Debug for SetupCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupCtx")
            .field("caps", self.device.caps())
            .finish_non_exhaustive()
    }
}

impl<'frame> SetupCtx<'frame> {
    #[doc(hidden)]
    pub fn new(
        device: &'frame Arc<dyn DeviceApi>,
        task_pool: &'frame Arc<dyn TaskPool>,
        shared: &'frame mut Shared,
    ) -> Self {
        Self {
            device,
            task_pool,
            shared,
        }
    }

    pub fn device(&self) -> &'frame dyn DeviceApi {
        self.device.as_ref()
    }

    pub fn caps(&self) -> &'frame Caps {
        self.device.caps()
    }

    pub fn task_pool(&self) -> &'frame dyn TaskPool {
        self.task_pool.as_ref()
    }

    // Owned handles for work that outlives the frame, such as background compilation (ADR-0024).
    pub fn device_handle(&self) -> Arc<dyn DeviceApi> {
        Arc::clone(self.device)
    }

    pub fn task_pool_handle(&self) -> Arc<dyn TaskPool> {
        Arc::clone(self.task_pool)
    }

    pub fn shared(&mut self) -> &mut Shared {
        self.shared
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TemporalSettings {
    pub render_size: PhysicalSize,
    pub jitter: Vec2,
    pub mip_bias: f32,
}

#[derive(Debug)]
pub struct PrepareCtx<'frame> {
    view: &'frame mut ViewCtx,
    owns_temporal: bool,
    shared: &'frame Shared,
}

impl<'frame> PrepareCtx<'frame> {
    #[doc(hidden)]
    pub fn new(view: &'frame mut ViewCtx, owns_temporal: bool, shared: &'frame Shared) -> Self {
        Self {
            view,
            owns_temporal,
            shared,
        }
    }

    pub fn view(&self) -> &ViewCtx {
        self.view
    }

    pub fn shared(&self) -> &Shared {
        self.shared
    }

    pub fn request(&mut self, slot: Slot) {
        if !self.view.requested.contains(&slot) {
            self.view.requested.push(slot);
        }
    }

    // Only the holder of the TemporalResolve role may change render size and jitter.
    pub fn set_temporal(&mut self, settings: TemporalSettings) -> bool {
        if !self.owns_temporal {
            return false;
        }
        self.view.render_size = settings.render_size;
        self.view.jitter = settings.jitter;
        self.view.mip_bias = settings.mip_bias;
        true
    }
}

pub struct RecordCtx<'frame> {
    graph: GraphBuilder<'frame>,
    slots: &'frame mut Slots,
    view: &'frame ViewCtx,
    device: &'frame dyn DeviceApi,
    diagnostics: &'frame mut Vec<Diagnostic>,
    released: &'frame mut Vec<Resource>,
    shared: &'frame mut Shared,
}

impl std::fmt::Debug for RecordCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordCtx")
            .field("view", &self.view)
            .field("slots", &self.slots)
            .finish_non_exhaustive()
    }
}

impl<'frame> RecordCtx<'frame> {
    #[doc(hidden)]
    pub fn new(
        graph: &'frame mut FrameGraph,
        slots: &'frame mut Slots,
        view: &'frame ViewCtx,
        device: &'frame dyn DeviceApi,
        diagnostics: &'frame mut Vec<Diagnostic>,
        released: &'frame mut Vec<Resource>,
        shared: &'frame mut Shared,
    ) -> Self {
        Self {
            graph: GraphBuilder::new(graph),
            slots,
            view,
            device,
            diagnostics,
            released,
            shared,
        }
    }

    pub fn shared(&self) -> &Shared {
        self.shared
    }

    pub fn shared_mut(&mut self) -> &mut Shared {
        self.shared
    }

    pub fn graph(&mut self) -> &mut GraphBuilder<'frame> {
        &mut self.graph
    }

    pub fn view(&self) -> &ViewCtx {
        self.view
    }

    pub fn device(&self) -> &dyn DeviceApi {
        self.device
    }

    pub fn slot(&self, slot: Slot) -> Option<TextureHandle> {
        self.slots.get(slot)
    }

    // Rejects textures that break the slot contract (G0005), so consumers can rely on the format.
    pub fn set_slot(&mut self, slot: Slot, handle: TextureHandle) -> bool {
        let actual = self.graph.texture_format(handle);
        if let Some(expected) = slot.format()
            && actual != Some(expected)
        {
            self.diagnostics.push(
                Diagnostic::error(
                    "G0005",
                    format!("slot '{}' expects {expected:?}, got {actual:?}", slot.name()),
                )
                .with_hint("create the texture with Slot::format() or RecordCtx::create_slot_texture()"),
            );
            return false;
        }
        self.slots.insert(slot, handle);
        true
    }

    pub fn create_slot_texture(&mut self, slot: Slot, name: &'static str) -> Option<TextureHandle> {
        let format = slot.format()?;
        Some(self.graph.create_texture(name, GraphTextureDesc::render(format)))
    }

    pub fn report(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    // The core destroys the object once the GPU is done with every frame that could still use it (ADR-0022).
    pub fn release_later(&mut self, resource: impl Into<Resource>) {
        self.released.push(resource.into());
    }
}

#[cfg(test)]
mod tests {
    use pathon_rhi_null::NullDevice;
    use pathon_rhi_types::{BufferDesc, BufferUsage, Format};

    use super::*;
    use crate::view::{Camera, ViewId};

    #[test]
    fn setup_hands_out_owned_handles_to_the_same_device_and_pool() {
        let device: Arc<dyn DeviceApi> = Arc::new(NullDevice::new());
        let pool: Arc<dyn TaskPool> = Arc::new(crate::SerialTaskPool);
        let mut shared = Shared::new();
        let ctx = SetupCtx::new(&device, &pool, &mut shared);

        let (device_handle, pool_handle) = (ctx.device_handle(), ctx.task_pool_handle());

        assert!(Arc::ptr_eq(&device_handle, &device));
        assert!(Arc::ptr_eq(&pool_handle, &pool));
        assert_eq!(ctx.task_pool().thread_count(), 1);
    }

    #[test]
    fn every_slot_but_output_has_a_contract_format() {
        for slot in Slot::BUILT_IN {
            assert_eq!(slot.format().is_none(), slot == Slot::OUTPUT, "{slot:?}");
        }
    }

    #[test]
    fn slot_with_wrong_format_is_rejected_with_g0005() {
        let device = NullDevice::new();
        let mut graph = FrameGraph::new();
        let mut slots = Slots::new();
        let view = ViewCtx::new(ViewId(0), PhysicalSize::new(4, 4), Camera::default());
        let mut diagnostics = Vec::new();
        let mut released = Vec::new();
        let mut shared = Shared::new();
        let mut ctx = RecordCtx::new(
            &mut graph,
            &mut slots,
            &view,
            &device,
            &mut diagnostics,
            &mut released,
            &mut shared,
        );

        let wrong = ctx
            .graph()
            .create_texture("ao", GraphTextureDesc::render(Format::Rgba8Unorm));
        assert!(!ctx.set_slot(Slot::AO, wrong));
        let right = ctx.create_slot_texture(Slot::AO, "ao").expect("ao has a format");
        assert!(ctx.set_slot(Slot::AO, right));

        assert_eq!(slots.get(Slot::AO), Some(right));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "G0005");
    }

    #[test]
    fn release_later_queues_the_resource_for_the_core() {
        let device = NullDevice::new();
        let buffer = device
            .create_buffer(&BufferDesc {
                label: "old",
                size: 4,
                usage: BufferUsage::UNIFORM,
            })
            .expect("buffer");
        let mut graph = FrameGraph::new();
        let mut slots = Slots::new();
        let view = ViewCtx::new(ViewId(0), PhysicalSize::new(4, 4), Camera::default());
        let mut diagnostics = Vec::new();
        let mut released = Vec::new();
        let mut shared = Shared::new();
        let mut ctx = RecordCtx::new(
            &mut graph,
            &mut slots,
            &view,
            &device,
            &mut diagnostics,
            &mut released,
            &mut shared,
        );

        ctx.release_later(buffer);

        assert_eq!(released, [Resource::Buffer(buffer)]);
        assert!(
            device.write_buffer(buffer, 0, &[0; 4]).is_ok(),
            "queued, not destroyed here"
        );
    }

    #[test]
    fn only_the_temporal_owner_changes_render_size() {
        let mut view = ViewCtx::new(ViewId(0), PhysicalSize::new(100, 100), Camera::default());
        let settings = TemporalSettings {
            render_size: PhysicalSize::new(67, 67),
            jitter: Vec2::new(0.25, -0.25),
            mip_bias: -0.5,
        };

        assert!(!PrepareCtx::new(&mut view, false, &Shared::new()).set_temporal(settings));
        assert_eq!(view.render_size, PhysicalSize::new(100, 100));
        assert!(PrepareCtx::new(&mut view, true, &Shared::new()).set_temporal(settings));
        assert_eq!(view.render_size, PhysicalSize::new(67, 67));
    }
}
