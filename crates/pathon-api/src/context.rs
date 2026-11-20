use glam::Vec2;
use pathon_diag::Diagnostic;
use pathon_graph::{FrameGraph, GraphTextureDesc, TextureHandle};
use pathon_rhi_types::{Caps, DeviceApi, PhysicalSize};

use crate::slot::{Slot, Slots};
use crate::task_pool::TaskPool;
use crate::view::ViewCtx;

pub struct SetupCtx<'frame> {
    device: &'frame dyn DeviceApi,
    task_pool: &'frame dyn TaskPool,
}

impl std::fmt::Debug for SetupCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupCtx")
            .field("caps", self.device.caps())
            .finish_non_exhaustive()
    }
}

impl<'frame> SetupCtx<'frame> {
    pub fn new(device: &'frame dyn DeviceApi, task_pool: &'frame dyn TaskPool) -> Self {
        Self { device, task_pool }
    }

    pub fn device(&self) -> &'frame dyn DeviceApi {
        self.device
    }

    pub fn caps(&self) -> &'frame Caps {
        self.device.caps()
    }

    pub fn task_pool(&self) -> &'frame dyn TaskPool {
        self.task_pool
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
}

impl<'frame> PrepareCtx<'frame> {
    pub fn new(view: &'frame mut ViewCtx, owns_temporal: bool) -> Self {
        Self { view, owns_temporal }
    }

    pub fn view(&self) -> &ViewCtx {
        self.view
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
    graph: &'frame mut FrameGraph,
    slots: &'frame mut Slots,
    view: &'frame ViewCtx,
    device: &'frame dyn DeviceApi,
    diagnostics: &'frame mut Vec<Diagnostic>,
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
    pub fn new(
        graph: &'frame mut FrameGraph,
        slots: &'frame mut Slots,
        view: &'frame ViewCtx,
        device: &'frame dyn DeviceApi,
        diagnostics: &'frame mut Vec<Diagnostic>,
    ) -> Self {
        Self {
            graph,
            slots,
            view,
            device,
            diagnostics,
        }
    }

    pub fn graph(&mut self) -> &mut FrameGraph {
        self.graph
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
                Diagnostic::error("G0005", format!("slot {slot:?} expects {expected:?}, got {actual:?}"))
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
}

#[cfg(test)]
mod tests {
    use pathon_rhi_null::NullDevice;
    use pathon_rhi_types::Format;

    use super::*;
    use crate::view::{Camera, ViewId};

    #[test]
    fn every_slot_but_output_has_a_contract_format() {
        for slot in Slot::ALL {
            assert_eq!(slot.format().is_none(), slot == Slot::Output, "{slot:?}");
        }
    }

    #[test]
    fn slot_with_wrong_format_is_rejected_with_g0005() {
        let device = NullDevice::new();
        let mut graph = FrameGraph::new();
        let mut slots = Slots::new();
        let view = ViewCtx::new(ViewId(0), PhysicalSize::new(4, 4), Camera::default());
        let mut diagnostics = Vec::new();
        let mut ctx = RecordCtx::new(&mut graph, &mut slots, &view, &device, &mut diagnostics);

        let wrong = ctx
            .graph()
            .create_texture("ao", GraphTextureDesc::render(Format::Rgba8Unorm));
        assert!(!ctx.set_slot(Slot::Ao, wrong));
        let right = ctx.create_slot_texture(Slot::Ao, "ao").expect("ao has a format");
        assert!(ctx.set_slot(Slot::Ao, right));

        assert_eq!(slots.get(Slot::Ao), Some(right));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "G0005");
    }

    #[test]
    fn only_the_temporal_owner_changes_render_size() {
        let mut view = ViewCtx::new(ViewId(0), PhysicalSize::new(100, 100), Camera::default());
        let settings = TemporalSettings {
            render_size: PhysicalSize::new(67, 67),
            jitter: Vec2::new(0.25, -0.25),
            mip_bias: -0.5,
        };

        assert!(!PrepareCtx::new(&mut view, false).set_temporal(settings));
        assert_eq!(view.render_size, PhysicalSize::new(100, 100));
        assert!(PrepareCtx::new(&mut view, true).set_temporal(settings));
        assert_eq!(view.render_size, PhysicalSize::new(67, 67));
    }
}
