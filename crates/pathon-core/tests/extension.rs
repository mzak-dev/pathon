// Extension points a pack has without touching the core: its own slots, its own roles, data shared by type, and
// packs that bundle several features.

use std::sync::{Arc, Mutex};

use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::rhi::{
    Access, Backend, Color, ColorAttachment, DeviceRequest, Format, PhysicalSize, RenderPassDesc, RhiError,
};
use pathon_api::{Camera, Pack, PackRegistrar, RecordCtx, RenderFeature, Role, Slot};
use pathon_core::{FeatureStatus, FrameInput, Renderer, RendererBuilder, ViewInput};

// Declared by the pack, not by pathon.
const MASK: Slot = Slot::new("outline.mask", Format::R8Unorm);
const OUTLINE: Role = Role::new("outline");

#[derive(Debug, Clone, Copy, PartialEq)]
struct OutlineWidth(f32);

fn clear_pass(
    name: &'static str,
    target: pathon_api::graph::TextureHandle,
) -> impl FnOnce(&mut pathon_api::graph::PassContext<'_>) -> Result<(), RhiError> {
    move |pass| {
        let view = pass.view(target)?;
        drop(pass.commands().render_pass(RenderPassDesc {
            label: name,
            color: vec![ColorAttachment::clear(view, Color::BLACK)],
            depth: None,
            timestamps: None,
        }));
        Ok(())
    }
}

struct MaskProducer;

impl RenderFeature for MaskProducer {
    fn name(&self) -> &'static str {
        "outline mask"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let mask = ctx
            .graph()
            .create_texture("outline mask", GraphTextureDesc::output(Format::R8Unorm));
        let mut pass = ctx.graph().add_pass(PassDesc::new("outline mask", Stage::Opaque));
        let mask = pass.write(mask, Access::COLOR_ATTACHMENT);
        pass.record(clear_pass("outline mask", mask));
        ctx.set_slot(MASK, mask);
        Ok(())
    }
}

struct OutlineComposite {
    seen_widths: Arc<Mutex<Vec<f32>>>,
}

impl RenderFeature for OutlineComposite {
    fn name(&self) -> &'static str {
        "outline composite"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn role(&self) -> Option<Role> {
        Some(OUTLINE)
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let width = ctx.shared().get::<OutlineWidth>().map_or(0.0, |width| width.0);
        self.seen_widths.lock().expect("test mutex").push(width);
        let (Some(mask), Some(output)) = (ctx.slot(MASK), ctx.slot(Slot::OUTPUT)) else {
            return Ok(());
        };
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("outline composite", Stage::PostProcess));
        pass.read(mask, Access::SHADER_READ);
        let output = pass.modify(output, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let view = pass.view(output)?;
            drop(pass.commands().render_pass(RenderPassDesc {
                label: "outline composite",
                color: vec![ColorAttachment::load(view)],
                depth: None,
                timestamps: None,
            }));
            Ok(())
        });
        ctx.set_slot(Slot::OUTPUT, output);
        Ok(())
    }
}

// One installable unit: two features plus the setting they share.
struct OutlinePack {
    seen_widths: Arc<Mutex<Vec<f32>>>,
}

impl Pack for OutlinePack {
    fn register(self, registrar: &mut PackRegistrar) {
        registrar
            .add_feature(MaskProducer)
            .add_feature(OutlineComposite {
                seen_widths: self.seen_widths,
            })
            .insert_shared(OutlineWidth(1.0));
    }
}

fn null_builder() -> RendererBuilder {
    Renderer::builder().with_device_request(DeviceRequest {
        backend: Some(Backend::Null),
        ..DeviceRequest::default()
    })
}

fn frame(renderer: &mut Renderer) -> FrameInput {
    let target = renderer
        .add_offscreen_target(PhysicalSize::new(32, 32), Format::Rgba8Unorm)
        .expect("target");
    FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    }
}

#[test]
fn pack_bundles_features_custom_slot_and_shared_data() {
    let seen_widths = Arc::new(Mutex::new(Vec::new()));
    let mut renderer = null_builder()
        .with_pack(OutlinePack {
            seen_widths: Arc::clone(&seen_widths),
        })
        .build()
        .expect("renderer");
    let input = frame(&mut renderer);

    let report = renderer.render_frame(&input).expect("frame");
    assert_eq!(renderer.active_features(), ["outline mask", "outline composite"]);
    assert_eq!(
        report.views[0].passes_live, 3,
        "clear, mask producer and composite stay; the custom slot links them"
    );

    renderer
        .shared_mut()
        .get_mut::<OutlineWidth>()
        .expect("installed by the pack")
        .0 = 3.0;
    renderer.render_frame(&input).expect("frame");
    assert_eq!(*seen_widths.lock().expect("test mutex"), [1.0, 3.0]);
}

#[test]
fn custom_slot_without_a_consumer_costs_nothing() {
    let mut renderer = null_builder().with_feature(MaskProducer).build().expect("renderer");
    let input = frame(&mut renderer);

    let report = renderer.render_frame(&input).expect("frame");
    assert_eq!(report.views[0].passes_live, 1, "only the output clear survives culling");
    assert_eq!(report.views[0].logical_textures, 0);
}

#[test]
fn custom_role_has_one_holder() {
    let first = Arc::new(Mutex::new(Vec::new()));
    let renderer = null_builder()
        .with_feature(OutlineComposite {
            seen_widths: Arc::clone(&first),
        })
        .with_feature(OutlineComposite { seen_widths: first })
        .build()
        .expect("renderer");

    let statuses: Vec<FeatureStatus> = renderer.features().into_iter().map(|(_, status)| status).collect();
    assert!(matches!(statuses[1], FeatureStatus::Disabled { code: "F0102", .. }));
    assert_eq!(renderer.active_features().len(), 1);
}

#[test]
fn application_provides_shared_data_up_front() {
    let seen_widths = Arc::new(Mutex::new(Vec::new()));
    let mut renderer = null_builder()
        .with_shared(OutlineWidth(5.0))
        .with_feature(OutlineComposite {
            seen_widths: Arc::clone(&seen_widths),
        })
        .build()
        .expect("renderer");
    let input = frame(&mut renderer);

    renderer.render_frame(&input).expect("frame");
    assert_eq!(*seen_widths.lock().expect("test mutex"), [5.0]);
}
