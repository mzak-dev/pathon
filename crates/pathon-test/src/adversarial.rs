// Packs that push on the core/API boundary (M2-14). Each must work without changes to pathon-core or pathon-api.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use pathon_api::glam::Vec2;
use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::{
    Access, Color, ColorAttachment, DeclaredAccess, Extent3d, ExternalCommand, PhysicalSize, RenderPassDesc,
    ResourceRef, RhiError, SubresourceRange, TextureCopy,
};
use pathon_api::{PrepareCtx, RecordCtx, RenderFeature, Role, Slot, TemporalSettings};

// Claims the Geometry role and fills `Hdr`; produces `Roughness` only when someone asks for it.
#[derive(Debug)]
pub struct GeometryPack {
    name: &'static str,
    color: Color,
    priority: i32,
}

impl GeometryPack {
    pub fn new(name: &'static str, color: Color, priority: i32) -> Self {
        Self { name, color, priority }
    }
}

impl RenderFeature for GeometryPack {
    fn name(&self) -> &'static str {
        self.name
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    fn role(&self) -> Option<Role> {
        Some(Role::GEOMETRY)
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let hdr = ctx
            .create_slot_texture(Slot::HDR, "hdr")
            .expect("hdr has a contract format");
        let produce_roughness = ctx.view().is_requested(Slot::ROUGHNESS);
        let roughness = produce_roughness
            .then(|| ctx.create_slot_texture(Slot::ROUGHNESS, "roughness"))
            .flatten();
        let color = self.color;
        let mut pass = ctx.graph().add_pass(PassDesc::new(self.name, Stage::Opaque));
        let hdr = pass.write(hdr, Access::COLOR_ATTACHMENT);
        let roughness = roughness.map(|texture| pass.write(texture, Access::COLOR_ATTACHMENT));
        pass.record(move |pass| {
            let mut color_attachments = vec![ColorAttachment::clear(pass.view(hdr)?, color)];
            if let Some(roughness) = roughness {
                color_attachments.push(ColorAttachment::clear(
                    pass.view(roughness)?,
                    Color::rgba(0.5, 0.0, 0.0, 1.0),
                ));
            }
            drop(pass.commands().render_pass(RenderPassDesc {
                label: "geometry",
                color: color_attachments,
                depth: None,
                timestamps: None,
            }));
            Ok(())
        });
        ctx.set_slot(Slot::HDR, hdr);
        if let Some(roughness) = roughness {
            ctx.set_slot(Slot::ROUGHNESS, roughness);
        }
        Ok(())
    }
}

// Holds TemporalResolve, renders at a fraction of the output size and copies `Hdr` into the corner of `Output`.
// Writes the render size it saw into `observed_render_size` for tests.
#[derive(Debug)]
pub struct UpscalerPack {
    scale: f32,
    pub observed_render_size: Arc<Mutex<Option<Extent3d>>>,
}

impl UpscalerPack {
    pub fn new(scale: f32) -> Self {
        Self {
            scale,
            observed_render_size: Arc::default(),
        }
    }
}

impl RenderFeature for UpscalerPack {
    fn name(&self) -> &'static str {
        "test upscaler"
    }

    fn stage(&self) -> Stage {
        Stage::Resolve
    }

    fn role(&self) -> Option<Role> {
        Some(Role::TEMPORAL_RESOLVE)
    }

    fn prepare_view(&mut self, ctx: &mut PrepareCtx<'_>) {
        let output = ctx.view().output_size;
        let scaled = |value: u32| {
            (f64::from(value) * f64::from(self.scale))
                .round()
                .clamp(1.0, f64::from(u32::MAX))
        };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "rounded and clamped above"
        )]
        let render_size = PhysicalSize::new(scaled(output.width) as u32, scaled(output.height) as u32);
        ctx.set_temporal(TemporalSettings {
            render_size,
            jitter: Vec2::new(0.5, -0.5),
            mip_bias: -1.0,
        });
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let (Some(hdr), Some(output)) = (ctx.slot(Slot::HDR), ctx.slot(Slot::OUTPUT)) else {
            return Ok(());
        };
        let observed = Arc::clone(&self.observed_render_size);
        let mut pass = ctx.graph().add_pass(PassDesc::new("test upscale", Stage::Resolve));
        let source = pass.read(hdr, Access::COPY_SRC);
        let destination = pass.modify(output, Access::COPY_DST);
        pass.record(move |pass| {
            let size = pass.desc(source).size;
            *observed.lock().expect("test mutex is never poisoned") = Some(size);
            let (source, destination) = (pass.texture(source), pass.texture(destination));
            pass.commands()
                .copy_texture_to_texture(TextureCopy::whole(source), TextureCopy::whole(destination), size);
            Ok(())
        });
        ctx.set_slot(Slot::OUTPUT, destination);
        Ok(())
    }
}

// Asks for the on-demand `Roughness` slot and consumes it.
#[derive(Debug, Default)]
pub struct OnDemandConsumer;

impl RenderFeature for OnDemandConsumer {
    fn name(&self) -> &'static str {
        "roughness consumer"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn prepare_view(&mut self, ctx: &mut PrepareCtx<'_>) {
        ctx.request(Slot::ROUGHNESS);
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let (Some(roughness), Some(hdr)) = (ctx.slot(Slot::ROUGHNESS), ctx.slot(Slot::HDR)) else {
            return Ok(());
        };
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("roughness consumer", Stage::PostProcess));
        pass.read(roughness, Access::SAMPLED);
        let hdr = pass.modify(hdr, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let view = pass.view(hdr)?;
            drop(pass.commands().render_pass(RenderPassDesc {
                label: "roughness consumer",
                color: vec![ColorAttachment::load(view)],
                depth: None,
                timestamps: None,
            }));
            Ok(())
        });
        ctx.set_slot(Slot::HDR, hdr);
        Ok(())
    }
}

// Stands in for a vendor SDK: an external command with declared accesses on `Hdr`.
#[derive(Debug, Default)]
pub struct VendorDummy {
    pub called: Arc<AtomicBool>,
}

impl RenderFeature for VendorDummy {
    fn name(&self) -> &'static str {
        "vendor dummy"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn priority(&self) -> i32 {
        10
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(hdr) = ctx.slot(Slot::HDR) else {
            return Ok(());
        };
        let called = Arc::clone(&self.called);
        let access = Access::STORAGE_READ | Access::STORAGE_WRITE;
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("vendor dummy", Stage::PostProcess).priority(10));
        let hdr = pass.modify(hdr, access);
        pass.record(move |pass| {
            let texture = pass.texture(hdr);
            pass.commands().external(ExternalCommand {
                label: "vendor sdk",
                accesses: vec![DeclaredAccess {
                    resource: ResourceRef::Texture(texture, SubresourceRange::ALL),
                    access,
                }],
                callback: Box::new(move |_native: &mut dyn std::any::Any| called.store(true, Ordering::SeqCst)),
            });
            Ok(())
        });
        ctx.set_slot(Slot::HDR, hdr);
        Ok(())
    }
}
