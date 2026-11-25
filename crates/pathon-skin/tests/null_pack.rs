// `SkinPack` in a real renderer on the null device, which checks every command list the pack records: barriers, usage
// flags, bind groups against their layout.

mod support;

use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::{Access, Backend, RhiError};
use pathon_api::{RecordCtx, RenderFeature};
use pathon_skin::{MorphTarget, MotionHistory, SkinFrame, SkinInstance, SkinOutput};
use support::{Pose, Rig};

fn skin_passes(report: &pathon_core::FrameReport) -> usize {
    report
        .captures
        .iter()
        .flat_map(|capture| &capture.passes)
        .filter(|pass| pass.name == "skin" && pass.order.is_some())
        .count()
}

#[test]
fn a_frame_with_morphs_and_two_instances_is_valid_and_alternates_banks() {
    let mut rig = Rig::build(Some(Backend::Null), 2, 1).expect("rig");
    let vertices = rig.arm.vertex_count() as usize;
    let target = MorphTarget::positions(vec![[0.0, 0.1, 0.0]; vertices]);
    rig.add_morph_targets(&[target.clone(), target]);
    let poses = |history| {
        [
            Pose {
                weights: vec![0.5, 0.25],
                ..Pose::elbow(0.3, history)
            },
            Pose {
                weights: vec![0.0, 1.0],
                padding_before: 3,
                ..Pose::elbow(-0.3, history)
            },
        ]
    };

    let report = rig.frame(&poses(MotionHistory::Fresh));
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let first = rig.output();
    let report = rig.frame(&poses(MotionHistory::Continuing));
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let second = rig.output();

    assert_eq!((first.frame, second.frame), (0, 1));
    assert_eq!(first.bank_capacity, second.bank_capacity);
}

#[test]
fn two_views_run_the_pass_once_and_both_see_the_output() {
    let mut rig = Rig::build(Some(Backend::Null), 1, 2).expect("rig");
    rig.renderer.set_capture(true);
    let report = rig.frame(&[Pose::elbow(0.2, MotionHistory::Fresh)]);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert_eq!(report.captures.len(), 2);
    assert_eq!(skin_passes(&report), 1);
    let _: SkinOutput = rig.output();
}

#[test]
fn without_a_frame_or_without_instances_nothing_is_published() {
    let mut rig = Rig::build(Some(Backend::Null), 1, 1).expect("rig");
    let report = rig.frame(&[]);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert!(rig.renderer.shared().get::<SkinOutput>().is_none());

    rig.frame(&[Pose::elbow(0.0, MotionHistory::Fresh)]);
    assert!(rig.renderer.shared().get::<SkinOutput>().is_some());
    rig.renderer.shared_mut().remove::<SkinFrame>();
    let input = pathon_core::FrameInput {
        views: vec![pathon_core::ViewInput {
            target: rig
                .renderer
                .add_offscreen_target(
                    pathon_api::rhi::PhysicalSize::new(8, 8),
                    pathon_api::rhi::Format::Rgba8Unorm,
                )
                .expect("target"),
            camera: pathon_api::Camera::default(),
        }],
        ..pathon_core::FrameInput::default()
    };
    rig.renderer.render_frame(&input).expect("frame");
    assert!(
        rig.renderer.shared().get::<SkinOutput>().is_none(),
        "a stale output is withdrawn"
    );
}

#[test]
fn a_broken_instance_is_skipped_and_reported_once() {
    let mut rig = Rig::build(Some(Backend::Null), 2, 1).expect("rig");
    let good = Pose::elbow(0.1, MotionHistory::Fresh);
    let source = pathon_skin::SkinSource::from_pool(&rig.pool, rig.mesh).expect("source");
    let ranges = rig.ranges.clone();
    let short = rig.positions.allocate(3).expect("a short range");
    let mut codes = Vec::new();
    for _ in 0..3 {
        let report = rig
            .frame_with(|frame| {
                frame.bone_matrices.extend_from_slice(&good.bones);
                frame
                    .instances
                    .push(SkinInstance::new(source, ranges[0]).with_bones(0, 2));
                // Wrong vertex count in the range, bones past the palette, and a skinned mesh without bones.
                frame.instances.push(SkinInstance::new(source, short).with_bones(0, 2));
                frame
                    .instances
                    .push(SkinInstance::new(source, ranges[1]).with_bones(1, 9));
                frame.instances.push(SkinInstance::new(source, ranges[1]));
            })
            .expect("frame");
        codes.push(
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
        );
        assert!(
            rig.renderer.shared().get::<SkinOutput>().is_some(),
            "the good instance still runs"
        );
    }
    assert_eq!(codes[0], ["W0001", "W0001", "W0001"]);
    assert!(codes[1].is_empty() && codes[2].is_empty(), "{codes:?}");
}

#[test]
fn a_consumer_pass_reads_the_output_after_the_skin_pass() {
    // Stands in for the prepass: reads the deformed positions and surface as vertex buffers.
    #[derive(Debug)]
    struct Consumer;
    impl RenderFeature for Consumer {
        fn name(&self) -> &'static str {
            "consumer"
        }

        fn stage(&self) -> Stage {
            Stage::Prepass
        }

        fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
            let Some(output) = ctx.shared().get::<SkinOutput>().copied() else {
                return Ok(());
            };
            let mut pass = ctx
                .graph()
                .add_pass(PassDesc::new("consume", Stage::Prepass).side_effect());
            pass.read_buffer(output.positions, Access::VERTEX);
            pass.read_buffer(output.surface, Access::VERTEX);
            pass.record(|_| Ok(()));
            Ok(())
        }
    }

    let mut rig = Rig::build_with(Some(Backend::Null), 1, 1, |builder| builder.with_feature(Consumer)).expect("rig");
    rig.renderer.set_capture(true);
    let report = rig.frame(&[Pose::elbow(0.2, MotionHistory::Fresh)]);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let capture = &report.captures[0];
    let index = |name| capture.passes.iter().position(|pass| pass.name == name).expect(name);
    let (skin, consume) = (index("skin"), index("consume"));
    assert!(capture.passes[skin].order < capture.passes[consume].order);
    assert!(capture.edges.contains(&(skin, consume)), "{:?}", capture.edges);
}
