use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::{Camera, PrepareCtx, RecordCtx, RenderFeature, Role, SetupCtx, ViewId};
use pathon_core::{FeatureStatus, FrameInput, RenderError, Renderer, RendererBuilder, ViewInput};
use pathon_rhi::{
    Access, Backend, Color, ColorAttachment, CommandList, Device, DeviceApi, DeviceRequest, Extent3d, Format,
    PhysicalSize, RenderPassDesc, RhiError, SubresourceRange, SurfaceTarget, TextureDesc, TextureId, TextureUsage,
};
use pathon_test::{GeometryPack, OnDemandConsumer, UnlitMeshPack, UpscalerPack, VendorDummy};

const SIZE: PhysicalSize = PhysicalSize::new(64, 32);

fn null_builder() -> RendererBuilder {
    Renderer::builder().with_device_request(DeviceRequest {
        backend: Some(Backend::Null),
        ..DeviceRequest::default()
    })
}

fn one_view(renderer: &mut Renderer, format: Format) -> FrameInput {
    let target = renderer.add_offscreen_target(SIZE, format).expect("offscreen target");
    FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    }
}

fn status_of(renderer: &Renderer, name: &str) -> FeatureStatus {
    renderer
        .features()
        .into_iter()
        .find(|(feature, _)| *feature == name)
        .map(|(_, status)| status)
        .expect("feature")
}

#[test]
fn renderer_is_send_so_hosts_can_move_it_to_their_render_thread() {
    fn assert_send<T: Send>() {}
    assert_send::<Renderer>();
}

#[test]
fn empty_renderer_clears_an_offscreen_target_under_state_validation() {
    let mut renderer = null_builder().build().expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);

    for frame in 0..3 {
        let report = renderer
            .render_frame(&FrameInput {
                frame_index: frame,
                ..input.clone()
            })
            .expect("frame");
        assert_eq!(report.views.len(), 1);
        assert_eq!(report.views[0].passes_live, 1);
    }
}

#[test]
fn device_loss_rebuilds_the_device_and_asks_for_data_again() {
    let mut renderer = null_builder()
        .with_feature(UnlitMeshPack::new())
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);
    let reuploads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reuploads);
    renderer.on_device_reset(Box::new(move |_device| {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    renderer.render_frame(&input).expect("first frame");

    renderer
        .device()
        .as_null()
        .expect("null backend")
        .simulate_device_loss();
    assert!(matches!(renderer.render_frame(&input), Err(RenderError::DeviceLost)));

    renderer.recover().expect("recover");
    assert_eq!(reuploads.load(Ordering::SeqCst), 1);
    let report = renderer.render_frame(&input).expect("frame after recovery");
    assert_eq!(report.compiled_views, 1);
    assert_eq!(renderer.active_features(), ["unlit mesh"]);
}

#[test]
fn borrowed_device_is_reset_by_the_host_not_recovered() {
    let host_device = Arc::new(Device::null());
    let mut renderer = Renderer::builder()
        .with_device(Arc::clone(&host_device))
        .build()
        .expect("renderer");

    assert!(matches!(renderer.recover(), Err(RenderError::BorrowedDevice)));
    renderer
        .reset(Arc::new(Device::null()))
        .expect("reset onto the host's new device");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);
    renderer.render_frame(&input).expect("frame");
}

#[test]
fn second_geometry_pack_loses_the_role() {
    let renderer = null_builder()
        .with_feature(GeometryPack::new("default geometry", Color::BLACK, 10))
        .with_feature(GeometryPack::new("custom geometry", Color::BLACK, 0))
        .build()
        .expect("renderer");

    assert_eq!(renderer.active_features(), ["custom geometry"]);
    assert!(matches!(
        status_of(&renderer, "default geometry"),
        FeatureStatus::Disabled { code: "F0102", .. }
    ));
}

#[test]
fn upscaler_changes_render_size_for_everyone_else() {
    let upscaler = UpscalerPack::new(0.5);
    let observed = Arc::clone(&upscaler.observed_render_size);
    let mut renderer = null_builder()
        .with_feature(GeometryPack::new("geometry", Color::rgba(1.0, 0.0, 0.0, 1.0), 0))
        .with_feature(upscaler)
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba16Float);

    renderer.render_frame(&input).expect("frame");
    assert_eq!(*observed.lock().expect("mutex"), Some(Extent3d::new_2d(32, 16)));
}

#[test]
fn on_demand_slot_exists_only_when_requested() {
    let live_passes = |with_consumer: bool| {
        let mut builder = null_builder()
            .with_feature(GeometryPack::new("geometry", Color::BLACK, 0))
            .with_feature(UpscalerPack::new(1.0));
        if with_consumer {
            builder = builder.with_feature(OnDemandConsumer);
        }
        let mut renderer = builder.build().expect("renderer");
        let input = one_view(&mut renderer, Format::Rgba16Float);
        let report = renderer.render_frame(&input).expect("frame");
        report.views[0].logical_textures
    };

    assert_eq!(live_passes(false), 1, "hdr only");
    assert_eq!(live_passes(true), 2, "hdr and roughness");
}

#[test]
fn vendor_dummy_runs_its_external_command() {
    let vendor = VendorDummy::default();
    let called: Arc<AtomicBool> = Arc::clone(&vendor.called);
    let mut renderer = null_builder()
        .with_feature(GeometryPack::new("geometry", Color::BLACK, 0))
        .with_feature(vendor)
        .with_feature(UpscalerPack::new(1.0))
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba16Float);

    renderer.render_frame(&input).expect("frame");
    assert!(called.load(Ordering::SeqCst));
}

#[test]
fn record_frame_refuses_window_surfaces() {
    let mut renderer = null_builder().build().expect("renderer");
    let window = SurfaceTarget::new(Arc::new(NoWindow), SIZE, 1.0);
    let target = renderer.add_window(window).expect("surface");
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    };

    assert!(matches!(
        renderer.record_frame(&input),
        Err(RenderError::SurfaceInRecordFrame)
    ));
    renderer
        .render_frame(&input)
        .expect("window frames go through render_frame");
}

fn host_texture(device: &Device) -> TextureId {
    device
        .create_texture(&pathon_rhi::TextureDesc::new_2d(
            "host color",
            SIZE.width,
            SIZE.height,
            Format::Rgba8Unorm,
            pathon_rhi::TextureUsage::RENDER_TARGET | pathon_rhi::TextureUsage::COPY_SRC,
        ))
        .expect("host texture")
}

// Records mode A into a host texture and counts the passes that clear it.
fn output_clears_in_mode_a(feature: impl RenderFeature + 'static) -> usize {
    let device = Arc::new(Device::null());
    let texture = host_texture(&device);
    let mut renderer = Renderer::builder()
        .with_device(Arc::clone(&device))
        .with_feature(feature)
        .build()
        .expect("renderer");
    let target = renderer
        .import_texture(texture, Access::COLOR_ATTACHMENT, Access::COPY_SRC)
        .expect("import");
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    };
    let commands = renderer.record_frame(&input).expect("record");
    commands
        .commands()
        .iter()
        .filter(|command| matches!(command, pathon_rhi::Command::PushDebugGroup("clear output")))
        .count()
}

#[test]
fn imported_host_texture_keeps_its_contents_for_mode_a() {
    assert_eq!(
        output_clears_in_mode_a(UnlitMeshPack::new()),
        0,
        "pathon must not clear what the host rendered"
    );
}

struct ReadsUnwrittenTexture;

impl RenderFeature for ReadsUnwrittenTexture {
    fn name(&self) -> &'static str {
        "reads unwritten"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let graph = ctx.graph();
        let texture = graph.create_texture("never written", GraphTextureDesc::render(Format::Rgba8Unorm));
        let mut pass = graph.add_pass(PassDesc::new("reads unwritten", Stage::PostProcess).side_effect());
        pass.read(texture, Access::SAMPLED);
        Ok(())
    }
}

#[test]
fn a_broken_graph_leaves_the_host_image_untouched_in_mode_a() {
    assert_eq!(
        output_clears_in_mode_a(ReadsUnwrittenTexture),
        0,
        "the fallback must not clear what the host rendered"
    );
}

struct NoWindow;

impl raw_window_handle::HasWindowHandle for NoWindow {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::Unavailable)
    }
}

impl raw_window_handle::HasDisplayHandle for NoWindow {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::Unavailable)
    }
}

#[test]
fn stable_scene_compiles_its_graph_once() {
    let mut renderer = null_builder()
        .with_feature(GeometryPack::new("geometry", Color::BLACK, 0))
        .with_feature(UpscalerPack::new(0.5))
        .with_feature(UnlitMeshPack::new())
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba16Float);

    let compiled: Vec<usize> = (0..5)
        .map(|frame| {
            let input = FrameInput {
                frame_index: frame,
                time_seconds: frame as f64 * 0.016,
                ..input.clone()
            };
            renderer.render_frame(&input).expect("frame").compiled_views
        })
        .collect();
    assert_eq!(compiled, [1, 0, 0, 0, 0]);

    renderer
        .resize_target(input.views[0].target, PhysicalSize::new(48, 48))
        .expect("resize");
    assert_eq!(renderer.render_frame(&input).expect("frame").compiled_views, 1);
}

#[test]
fn capture_is_opt_in_and_feature_details_explain_disabled_packs() {
    let mut renderer = null_builder()
        .with_feature(GeometryPack::new("geometry", Color::BLACK, 0))
        .with_feature(GeometryPack::new("fallback geometry", Color::BLACK, 10))
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba16Float);
    assert_eq!(renderer.render_frame(&input).expect("frame").captures.len(), 0);

    renderer.set_capture(true);
    let report = renderer.render_frame(&input).expect("frame");
    assert_eq!(report.captures.len(), report.views.len());
    let live = report.captures[0]
        .passes
        .iter()
        .filter(|pass| pass.order.is_some())
        .count();
    assert_eq!(live, report.views[0].passes_live);

    let details = renderer.feature_details();
    let fallback = details
        .iter()
        .find(|info| info.name == "fallback geometry")
        .expect("listed");
    assert_eq!(fallback.order, None);
    assert!(matches!(fallback.status, FeatureStatus::Disabled { code: "F0102", .. }));
    assert!(
        details
            .iter()
            .any(|info| info.name == "geometry" && info.order.is_some())
    );
}

#[test]
fn a_failed_frame_gives_back_the_window_frames_it_acquired() {
    let mut renderer = null_builder().build().expect("renderer");
    let window = renderer
        .add_window(SurfaceTarget::new(Arc::new(NoWindow), SIZE, 1.0))
        .expect("surface");
    let removed = renderer
        .add_offscreen_target(SIZE, Format::Rgba8Unorm)
        .expect("offscreen");
    renderer.remove_target(removed).expect("remove");
    let view = |target| ViewInput {
        target,
        camera: Camera::default(),
    };

    let failing = FrameInput {
        views: vec![view(window), view(removed)],
        ..FrameInput::default()
    };
    assert!(matches!(
        renderer.render_frame(&failing),
        Err(RenderError::UnknownTarget)
    ));

    let next = FrameInput {
        views: vec![view(window)],
        ..FrameInput::default()
    };
    renderer.render_frame(&next).expect("the window can be acquired again");
}

// Fails to record, and leaves a pass that reports the device lost, so the same frame fails.
struct FailsThenLosesDevice {
    already_failed: bool,
}

impl RenderFeature for FailsThenLosesDevice {
    fn name(&self) -> &'static str {
        "fails then loses device"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        if self.already_failed {
            return Ok(());
        }
        self.already_failed = true;
        let pass = ctx
            .graph()
            .add_pass(PassDesc::new("loses device", Stage::PostProcess).side_effect());
        pass.record(|_| Err(RhiError::DeviceLost));
        Err(RhiError::InvalidHandle("broken pack"))
    }
}

struct FailsToRecord {
    calls: Arc<AtomicUsize>,
}

impl RenderFeature for FailsToRecord {
    fn name(&self) -> &'static str {
        "fails to record"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn record(&mut self, _ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(RhiError::InvalidHandle("broken pack"))
    }
}

#[test]
fn a_pack_that_fails_to_record_is_disabled_and_not_called_again() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut renderer = null_builder()
        .with_feature(FailsToRecord {
            calls: Arc::clone(&calls),
        })
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);

    for _ in 0..3 {
        renderer
            .render_frame(&input)
            .expect("the rest of the frame still renders");
    }

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        status_of(&renderer, "fails to record"),
        FeatureStatus::Disabled { code: "F0105", .. }
    ));
    assert_eq!(renderer.active_features(), [] as [&str; 0]);
}

#[test]
fn a_failing_pack_is_reported_once_per_frame() {
    let mut renderer = null_builder()
        .with_feature(FailsToRecord {
            calls: Arc::new(AtomicUsize::new(0)),
        })
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);

    let report = renderer.render_frame(&input).expect("the frame still renders");

    let failures = report.diagnostics.iter().filter(|d| d.code == "F0105").count();
    assert_eq!(failures, 1);
}

// Fails to record and also leaves a pass that fails to record, so one frame carries both kinds of F0105.
struct FailsAndBreaksPass;

impl RenderFeature for FailsAndBreaksPass {
    fn name(&self) -> &'static str {
        "fails and breaks a pass"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let pass = ctx
            .graph()
            .add_pass(PassDesc::new("broken pass", Stage::PostProcess).side_effect());
        pass.record(|_| Err(RhiError::InvalidHandle("broken pass")));
        Err(RhiError::InvalidHandle("broken pack"))
    }
}

#[test]
fn a_failed_pack_and_a_failed_pass_are_each_reported_once() {
    let mut renderer = null_builder()
        .with_feature(FailsAndBreaksPass)
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);

    let report = renderer.render_frame(&input).expect("the frame still renders");

    let messages: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "F0105")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.contains("'fails and breaks a pass'"))
            .count(),
        1
    );
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.contains("a pass failed to record"))
            .count(),
        1
    );
}

#[test]
fn a_failed_frame_does_not_leak_its_diagnostics_into_the_next_report() {
    let mut renderer = null_builder()
        .with_feature(FailsThenLosesDevice { already_failed: false })
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);
    assert!(matches!(renderer.render_frame(&input), Err(RenderError::DeviceLost)));

    renderer.recover().expect("recover");
    let report = renderer.render_frame(&input).expect("frame after recovery");

    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

#[test]
fn a_minimised_window_is_skipped_until_it_has_a_size_again() {
    let mut renderer = null_builder().build().expect("renderer");
    let window = renderer
        .add_window(SurfaceTarget::new(Arc::new(NoWindow), SIZE, 1.0))
        .expect("surface");
    let input = FrameInput {
        views: vec![ViewInput {
            target: window,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    };
    assert_eq!(renderer.render_frame(&input).expect("frame").skipped_views, 0);

    renderer
        .resize_target(window, PhysicalSize::new(0, 0))
        .expect("minimising is not an error");
    let report = renderer.render_frame(&input).expect("frame");
    assert_eq!(report.skipped_views, 1);

    renderer.resize_target(window, SIZE).expect("restore");
    assert_eq!(renderer.render_frame(&input).expect("frame").skipped_views, 0);
}

struct ClaimsRole {
    name: &'static str,
    priority: i32,
    setup_fails: bool,
}

impl RenderFeature for ClaimsRole {
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
        Some(Role::new("test.exclusive"))
    }

    fn setup(&mut self, _ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        if self.setup_fails {
            Err(RhiError::InvalidHandle("setup gave up"))
        } else {
            Ok(())
        }
    }

    fn record(&mut self, _ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        Ok(())
    }
}

#[test]
fn a_role_passes_to_the_next_claimant_when_its_holder_fails_setup() {
    let renderer = null_builder()
        .with_feature(ClaimsRole {
            name: "first claimant",
            priority: 0,
            setup_fails: true,
        })
        .with_feature(ClaimsRole {
            name: "second claimant",
            priority: 1,
            setup_fails: false,
        })
        .build()
        .expect("renderer");

    assert_eq!(renderer.active_features(), ["second claimant"]);
    assert!(matches!(
        status_of(&renderer, "first claimant"),
        FeatureStatus::Disabled { code: "F0104", .. }
    ));
}

#[test]
fn reordering_views_does_not_recompile_their_graphs() {
    let mut renderer = null_builder().build().expect("renderer");
    let a = renderer
        .add_offscreen_target(PhysicalSize::new(64, 32), Format::Rgba8Unorm)
        .expect("target a");
    let b = renderer
        .add_offscreen_target(PhysicalSize::new(32, 16), Format::Rgba8Unorm)
        .expect("target b");
    let view = |target| ViewInput {
        target,
        camera: Camera::default(),
    };

    let first = renderer
        .render_frame(&FrameInput {
            views: vec![view(a), view(b)],
            ..FrameInput::default()
        })
        .expect("first frame");
    assert_eq!(first.compiled_views, 2);

    let second = renderer
        .render_frame(&FrameInput {
            views: vec![view(b), view(a)],
            ..FrameInput::default()
        })
        .expect("reordered frame");
    assert_eq!(second.compiled_views, 0);
}

#[test]
fn a_second_view_of_the_same_offscreen_target_is_skipped() {
    let mut renderer = null_builder().build().expect("renderer");
    let target = renderer
        .add_offscreen_target(SIZE, Format::Rgba8Unorm)
        .expect("offscreen target");
    let view = ViewInput {
        target,
        camera: Camera::default(),
    };

    let report = renderer
        .render_frame(&FrameInput {
            views: vec![view, view],
            ..FrameInput::default()
        })
        .expect("frame");
    assert_eq!(report.views.len(), 1);
    assert_eq!(report.skipped_views, 1);
    assert!(report.diagnostics.iter().any(|d| d.code == "F0106"));
}

#[test]
fn a_second_view_of_the_same_window_is_skipped_with_f0106() {
    let mut renderer = null_builder().build().expect("renderer");
    let window = renderer
        .add_window(SurfaceTarget::new(Arc::new(NoWindow), SIZE, 1.0))
        .expect("surface");
    let view = ViewInput {
        target: window,
        camera: Camera::default(),
    };
    let input = FrameInput {
        views: vec![view, view],
        ..FrameInput::default()
    };

    let report = renderer.render_frame(&input).expect("frame");
    assert_eq!(report.skipped_views, 1);
    assert_eq!(report.views.len(), 1);
    assert_eq!(report.diagnostics.iter().filter(|d| d.code == "F0106").count(), 1);

    renderer.render_frame(&input).expect("the next frame is fine too");
}

// Mode A has no `FrameReport`, so the observable effect is that the target is written once.
#[test]
fn a_second_view_of_the_same_imported_texture_is_skipped_in_mode_a() {
    let device = Arc::new(Device::null());
    let texture = host_texture(&device);
    let mut renderer = Renderer::builder()
        .with_device(Arc::clone(&device))
        .with_feature(UnlitMeshPack::new())
        .build()
        .expect("renderer");
    let target = renderer
        .import_texture(texture, Access::COLOR_ATTACHMENT, Access::COPY_SRC)
        .expect("import");
    let view = ViewInput {
        target,
        camera: Camera::default(),
    };
    let mut command_count = |views: Vec<ViewInput>| {
        let input = FrameInput {
            views,
            ..FrameInput::default()
        };
        renderer.record_frame(&input).expect("record").commands().len()
    };

    let single = command_count(vec![view]);
    assert!(single > 0);
    assert_eq!(command_count(vec![view, view]), single);
}

struct RecordsViewIds {
    seen: Arc<Mutex<Vec<ViewId>>>,
}

impl RenderFeature for RecordsViewIds {
    fn name(&self) -> &'static str {
        "records view ids"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn prepare_view(&mut self, ctx: &mut PrepareCtx<'_>) {
        self.seen.lock().expect("lock").push(ctx.view().view);
    }

    fn record(&mut self, _ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        Ok(())
    }
}

#[test]
fn a_view_keeps_its_id_while_its_target_lives() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut renderer = null_builder()
        .with_feature(RecordsViewIds {
            seen: Arc::clone(&seen),
        })
        .build()
        .expect("renderer");
    let a = renderer
        .add_offscreen_target(SIZE, Format::Rgba8Unorm)
        .expect("target a");
    let b = renderer
        .add_offscreen_target(SIZE, Format::Rgba8Unorm)
        .expect("target b");
    let view = |target| ViewInput {
        target,
        camera: Camera::default(),
    };
    let frame = |renderer: &mut Renderer, targets: [_; 2]| {
        seen.lock().expect("lock").clear();
        renderer
            .render_frame(&FrameInput {
                views: targets.map(view).to_vec(),
                ..FrameInput::default()
            })
            .expect("frame");
        seen.lock().expect("lock").clone()
    };

    let ab = frame(&mut renderer, [a, b]);
    let ba = frame(&mut renderer, [b, a]);
    assert_eq!(ab[0], ba[1], "target a keeps its id when it moves in the list");
    assert_eq!(ab[1], ba[0]);
    assert_ne!(ab[0], ab[1]);

    renderer.remove_target(a).expect("remove a");
    let a2 = renderer
        .add_offscreen_target(SIZE, Format::Rgba8Unorm)
        .expect("target a2");
    let recreated = frame(&mut renderer, [a2, b]);
    assert_ne!(recreated[0], ab[0], "a re-created target is a new view");
    assert_eq!(recreated[1], ab[1]);
}

struct RecordsHistoryValidity {
    seen: Arc<Mutex<Vec<bool>>>,
}

impl RenderFeature for RecordsHistoryValidity {
    fn name(&self) -> &'static str {
        "history recorder"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let history = ctx
            .graph()
            .history_texture("test history", GraphTextureDesc::render(Format::Rgba8Unorm));
        self.seen.lock().expect("mutex").push(history.previous_valid);
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("history writer", Stage::PostProcess));
        pass.read(history.previous, Access::SAMPLED);
        let current = pass.write(history.current, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let view = pass.view(current)?;
            drop(pass.commands().render_pass(RenderPassDesc {
                label: "history writer",
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
                timestamps: None,
            }));
            Ok(())
        });
        Ok(())
    }
}

#[test]
fn history_survives_a_skipped_frame_and_resets_with_the_device() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut renderer = null_builder()
        .with_feature(RecordsHistoryValidity {
            seen: Arc::clone(&seen),
        })
        .build()
        .expect("renderer");
    let with_view = one_view(&mut renderer, Format::Rgba8Unorm);

    renderer.render_frame(&with_view).expect("first frame");
    renderer.render_frame(&FrameInput::default()).expect("skipped frame");
    renderer.render_frame(&with_view).expect("frame after the skip");
    assert_eq!(*seen.lock().expect("mutex"), [false, true]);

    renderer
        .device()
        .as_null()
        .expect("null backend")
        .simulate_device_loss();
    assert!(matches!(
        renderer.render_frame(&with_view),
        Err(RenderError::DeviceLost)
    ));
    renderer.recover().expect("recover");
    seen.lock().expect("mutex").clear();
    renderer.render_frame(&with_view).expect("frame after recovery");
    assert_eq!(*seen.lock().expect("mutex"), [false]);
}

fn history_renderer() -> (Renderer, Arc<Mutex<Vec<bool>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let renderer = null_builder()
        .with_feature(RecordsHistoryValidity {
            seen: Arc::clone(&seen),
        })
        .build()
        .expect("renderer");
    (renderer, seen)
}

// The second view names a removed target, so the frame fails after the first view has recorded.
fn failing_second_view(renderer: &mut Renderer, healthy: &FrameInput) -> FrameInput {
    let removed = renderer
        .add_offscreen_target(SIZE, Format::Rgba8Unorm)
        .expect("offscreen");
    renderer.remove_target(removed).expect("remove");
    let mut views = healthy.views.clone();
    views.push(ViewInput {
        target: removed,
        camera: Camera::default(),
    });
    FrameInput {
        views,
        ..FrameInput::default()
    }
}

#[test]
fn history_written_in_a_frame_that_failed_later_is_not_valid_in_the_next() {
    let (mut renderer, seen) = history_renderer();
    let healthy = one_view(&mut renderer, Format::Rgba8Unorm);
    let failing = failing_second_view(&mut renderer, &healthy);

    renderer.render_frame(&healthy).expect("first frame");
    assert!(renderer.render_frame(&failing).is_err());
    renderer.render_frame(&healthy).expect("frame after the failure");

    assert_eq!(*seen.lock().expect("mutex"), [false, true, false]);
}

#[test]
fn history_written_in_a_record_frame_that_failed_later_is_not_valid_in_the_next() {
    let (mut renderer, seen) = history_renderer();
    let healthy = one_view(&mut renderer, Format::Rgba8Unorm);
    let failing = failing_second_view(&mut renderer, &healthy);

    renderer.record_frame(&healthy).expect("first frame");
    assert!(renderer.record_frame(&failing).is_err());
    renderer.record_frame(&healthy).expect("frame after the failure");

    assert_eq!(*seen.lock().expect("mutex"), [false, true, false]);
}

#[test]
fn history_stays_valid_across_frames_that_succeed() {
    let (mut renderer, seen) = history_renderer();
    let healthy = one_view(&mut renderer, Format::Rgba8Unorm);

    for _ in 0..3 {
        renderer.render_frame(&healthy).expect("frame");
    }

    assert_eq!(*seen.lock().expect("mutex"), [false, true, true]);
}

struct FailsToRecordOnFirstDevice {
    on_first_device: Arc<AtomicBool>,
    setups: Arc<AtomicUsize>,
}

impl RenderFeature for FailsToRecordOnFirstDevice {
    fn name(&self) -> &'static str {
        "fails on first device"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn setup(&mut self, _ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        self.setups.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn record(&mut self, _ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        if self.on_first_device.load(Ordering::SeqCst) {
            Err(RhiError::InvalidHandle("first device only"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn a_pack_disabled_while_recording_comes_back_after_reset() {
    let on_first_device = Arc::new(AtomicBool::new(true));
    let setups = Arc::new(AtomicUsize::new(0));
    let mut renderer = null_builder()
        .with_feature(FailsToRecordOnFirstDevice {
            on_first_device: Arc::clone(&on_first_device),
            setups: Arc::clone(&setups),
        })
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);
    renderer.render_frame(&input).expect("frame");
    assert!(matches!(
        status_of(&renderer, "fails on first device"),
        FeatureStatus::Disabled { code: "F0105", .. }
    ));

    on_first_device.store(false, Ordering::SeqCst);
    renderer
        .device()
        .as_null()
        .expect("null backend")
        .simulate_device_loss();
    assert!(matches!(renderer.render_frame(&input), Err(RenderError::DeviceLost)));
    renderer.recover().expect("recover");

    assert_eq!(renderer.active_features(), ["fails on first device"]);
    assert_eq!(setups.load(Ordering::SeqCst), 2);
    renderer.render_frame(&input).expect("frame after recovery");
    assert_eq!(renderer.active_features(), ["fails on first device"]);
}

#[test]
fn a_pack_disabled_for_other_reasons_stays_off_after_reset() {
    let mut renderer = null_builder()
        .with_feature(GeometryPack::new("default geometry", Color::BLACK, 10))
        .with_feature(GeometryPack::new("custom geometry", Color::BLACK, 0))
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);
    renderer.render_frame(&input).expect("frame");

    renderer
        .device()
        .as_null()
        .expect("null backend")
        .simulate_device_loss();
    assert!(matches!(renderer.render_frame(&input), Err(RenderError::DeviceLost)));
    renderer.recover().expect("recover");

    assert_eq!(renderer.active_features(), ["custom geometry"]);
    assert!(matches!(
        status_of(&renderer, "default geometry"),
        FeatureStatus::Disabled { code: "F0102", .. }
    ));
}

#[derive(Clone, Default)]
struct ProbeControl {
    record_fails: Arc<AtomicBool>,
    setup_fails: Arc<AtomicBool>,
    claims_role: Arc<AtomicBool>,
    setups: Arc<AtomicUsize>,
    resets: Arc<AtomicUsize>,
}

struct Probe {
    name: &'static str,
    priority: i32,
    control: ProbeControl,
}

impl Probe {
    fn new(name: &'static str, priority: i32, control: &ProbeControl) -> Self {
        Self {
            name,
            priority,
            control: control.clone(),
        }
    }
}

impl RenderFeature for Probe {
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
        self.control
            .claims_role
            .load(Ordering::SeqCst)
            .then(|| Role::new("test.exclusive"))
    }

    fn setup(&mut self, _ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        self.control.setups.fetch_add(1, Ordering::SeqCst);
        if self.control.setup_fails.load(Ordering::SeqCst) {
            Err(RhiError::InvalidHandle("setup gave up"))
        } else {
            Ok(())
        }
    }

    fn on_device_reset(&mut self, _ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        self.control.resets.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn record(&mut self, _ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        if self.control.record_fails.load(Ordering::SeqCst) {
            Err(RhiError::InvalidHandle("probe failed"))
        } else {
            Ok(())
        }
    }
}

fn lose_device_and_recover(renderer: &mut Renderer, input: &FrameInput) {
    renderer
        .device()
        .as_null()
        .expect("null backend")
        .simulate_device_loss();
    assert!(matches!(renderer.render_frame(input), Err(RenderError::DeviceLost)));
    renderer.recover().expect("recover");
}

#[test]
fn a_restored_pack_is_set_up_and_a_pack_that_stayed_active_is_only_reset() {
    let restored = ProbeControl::default();
    let stayed = ProbeControl::default();
    restored.record_fails.store(true, Ordering::SeqCst);
    let mut renderer = null_builder()
        .with_feature(Probe::new("restored", 0, &restored))
        .with_feature(Probe::new("stayed", 1, &stayed))
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);
    renderer.render_frame(&input).expect("frame");
    restored.record_fails.store(false, Ordering::SeqCst);

    lose_device_and_recover(&mut renderer, &input);

    assert_eq!(renderer.active_features(), ["restored", "stayed"]);
    assert_eq!(restored.setups.load(Ordering::SeqCst), 2);
    assert_eq!(restored.resets.load(Ordering::SeqCst), 0);
    assert_eq!(stayed.setups.load(Ordering::SeqCst), 1);
    assert_eq!(stayed.resets.load(Ordering::SeqCst), 1);
}

#[test]
fn a_restored_pack_does_not_take_a_role_from_a_pack_that_stayed_active() {
    let restored = ProbeControl::default();
    let holder = ProbeControl::default();
    holder.claims_role.store(true, Ordering::SeqCst);
    restored.record_fails.store(true, Ordering::SeqCst);
    let mut renderer = null_builder()
        .with_feature(Probe::new("restored claimant", 0, &restored))
        .with_feature(Probe::new("holder", 1, &holder))
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);
    renderer.render_frame(&input).expect("frame");
    restored.record_fails.store(false, Ordering::SeqCst);
    restored.claims_role.store(true, Ordering::SeqCst);

    lose_device_and_recover(&mut renderer, &input);

    assert_eq!(renderer.active_features(), ["holder"]);
    assert!(matches!(
        status_of(&renderer, "restored claimant"),
        FeatureStatus::Disabled { code: "F0102", .. }
    ));
    assert_eq!(restored.setups.load(Ordering::SeqCst), 1);
}

#[test]
fn a_restored_pack_whose_setup_fails_on_the_new_device_is_disabled_with_f0104() {
    let restored = ProbeControl::default();
    restored.record_fails.store(true, Ordering::SeqCst);
    let mut renderer = null_builder()
        .with_feature(Probe::new("restored", 0, &restored))
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);
    renderer.render_frame(&input).expect("frame");
    restored.setup_fails.store(true, Ordering::SeqCst);

    lose_device_and_recover(&mut renderer, &input);

    assert_eq!(renderer.active_features(), [] as [&str; 0]);
    assert!(matches!(
        status_of(&renderer, "restored"),
        FeatureStatus::Disabled { code: "F0104", .. }
    ));
}

// Writes a pooled transient every frame; `draw_without_pipeline` makes the submit fail halfway through the list.
struct ScratchFeature {
    draw_without_pipeline: Arc<AtomicBool>,
}

impl RenderFeature for ScratchFeature {
    fn name(&self) -> &'static str {
        "scratch"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let graph = ctx.graph();
        let scratch = graph.create_texture("scratch", GraphTextureDesc::render(Format::Rgba8Unorm));
        let mut pass = graph.add_pass(PassDesc::new("fill scratch", Stage::PostProcess));
        let written = pass.write(scratch, Access::COLOR_ATTACHMENT);
        let broken = self.draw_without_pipeline.load(Ordering::SeqCst);
        pass.record(move |pass| {
            let view = pass.view(written)?;
            let mut encoder = pass.commands().render_pass(RenderPassDesc {
                label: "fill scratch",
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
                timestamps: None,
            });
            if broken {
                encoder.draw(0..3, 0..1);
            }
            Ok(())
        });
        let mut pass = graph.add_pass(PassDesc::new("use scratch", Stage::PostProcess).side_effect());
        pass.read(written, Access::SAMPLED);
        pass.record(|_| Ok(()));
        Ok(())
    }
}

fn scratch_renderer() -> (Renderer, Arc<AtomicBool>) {
    let broken = Arc::new(AtomicBool::new(false));
    let renderer = null_builder()
        .with_feature(ScratchFeature {
            draw_without_pipeline: Arc::clone(&broken),
        })
        .build()
        .expect("renderer");
    (renderer, broken)
}

#[test]
fn an_offscreen_target_recovers_after_its_first_frame_was_abandoned() {
    let mut renderer = null_builder().build().expect("renderer");
    let healthy = one_view(&mut renderer, Format::Rgba8Unorm);
    let failing = failing_second_view(&mut renderer, &healthy);

    assert!(renderer.render_frame(&failing).is_err());
    renderer
        .render_frame(&healthy)
        .expect("the abandoned frame left no state behind");
}

#[test]
fn a_pooled_transient_recovers_after_its_first_frame_was_abandoned() {
    let device = Arc::new(Device::null());
    let host_texture = device
        .create_texture(&TextureDesc::new_2d(
            "host color",
            SIZE.width,
            SIZE.height,
            Format::Rgba8Unorm,
            TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
        ))
        .expect("host texture");
    let mut renderer = Renderer::builder()
        .with_device(Arc::clone(&device))
        .with_feature(ScratchFeature {
            draw_without_pipeline: Arc::new(AtomicBool::new(false)),
        })
        .build()
        .expect("renderer");
    let target = renderer
        .import_texture(host_texture, Access::empty(), Access::empty())
        .expect("import");
    let healthy = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    };
    let failing = failing_second_view(&mut renderer, &healthy);

    assert!(renderer.render_frame(&failing).is_err());
    renderer
        .render_frame(&healthy)
        .expect("only the pool remembered states");
}

#[test]
fn frames_recover_after_one_submit_was_rejected_midway() {
    let (mut renderer, broken) = scratch_renderer();
    let input = one_view(&mut renderer, Format::Rgba8Unorm);
    renderer.render_frame(&input).expect("first frame");

    broken.store(true, Ordering::SeqCst);
    assert!(renderer.render_frame(&input).is_err());
    broken.store(false, Ordering::SeqCst);
    for _ in 0..3 {
        renderer.render_frame(&input).expect("a rejected list changed nothing");
    }
}

#[test]
fn a_record_frame_that_failed_later_leaves_no_state_behind() {
    let (mut renderer, _) = scratch_renderer();
    let healthy = one_view(&mut renderer, Format::Rgba8Unorm);
    let failing = failing_second_view(&mut renderer, &healthy);

    assert!(renderer.record_frame(&failing).is_err());
    let list = renderer.record_frame(&healthy).expect("recording after the failure");
    renderer
        .device()
        .submit(list)
        .expect("the list starts from the state the GPU is really in");
}

#[test]
fn the_host_returns_an_offscreen_target_to_sampled_after_reading_it_back() {
    let mut renderer = null_builder().build().expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);
    renderer.render_frame(&input).expect("first frame");

    let texture = renderer.target_texture(input.views[0].target).expect("texture");
    let mut readback = CommandList::new();
    readback.texture_access(texture, SubresourceRange::ALL, Access::SAMPLED, Access::COPY_SRC);
    readback.texture_access(texture, SubresourceRange::ALL, Access::COPY_SRC, Access::SAMPLED);
    renderer.device().submit(readback).expect("host barriers");

    renderer.render_frame(&input).expect("frame after the readback");
}

// Overwrites a window's `Output` without drawing while `active`, so the list is valid but presenting fails (R0106).
struct OverwritesWindowOutput {
    active: Arc<AtomicBool>,
}

impl RenderFeature for OverwritesWindowOutput {
    fn name(&self) -> &'static str {
        "overwrites window output"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(output) = ctx.slot(pathon_api::Slot::OUTPUT) else {
            return Ok(());
        };
        let is_window = ctx.graph().texture_format(output) != Some(Format::Rgba8Unorm);
        if !self.active.load(Ordering::SeqCst) || !is_window {
            return Ok(());
        }
        let mut pass = ctx.graph().add_pass(PassDesc::new("overwrite", Stage::PostProcess));
        let written = pass.write(output, Access::COLOR_ATTACHMENT);
        pass.record(|_| Ok(()));
        ctx.set_slot(pathon_api::Slot::OUTPUT, written);
        Ok(())
    }
}

#[test]
fn frames_recover_after_present_failed_following_a_successful_submit() {
    let active = Arc::new(AtomicBool::new(true));
    let mut renderer = null_builder()
        .with_feature(ScratchFeature {
            draw_without_pipeline: Arc::new(AtomicBool::new(false)),
        })
        .with_feature(OverwritesWindowOutput {
            active: Arc::clone(&active),
        })
        .build()
        .expect("renderer");
    let offscreen = renderer
        .add_offscreen_target(SIZE, Format::Rgba8Unorm)
        .expect("offscreen");
    let window = renderer
        .add_window(SurfaceTarget::new(Arc::new(NoWindow), SIZE, 1.0))
        .expect("window");
    let view = |target| ViewInput {
        target,
        camera: Camera::default(),
    };
    let input = FrameInput {
        views: vec![view(offscreen), view(window)],
        ..FrameInput::default()
    };

    assert!(renderer.render_frame(&input).is_err(), "presenting must fail");
    active.store(false, Ordering::SeqCst);
    for _ in 0..3 {
        renderer
            .render_frame(&input)
            .expect("the failed present did not roll back states of a list that ran");
    }
}

// `UnlitMeshPack` takes one uniform slot per record; its ring holds 64 KiB.
fn unlit_ring_capacity(renderer: &Renderer) -> usize {
    let alignment = u64::from(renderer.device().caps().limits.min_uniform_buffer_offset_alignment);
    usize::try_from((64 << 10) / alignment).expect("ring capacity fits usize")
}

#[test]
#[ignore = "red until the frame clock lands"]
fn a_pack_with_a_per_frame_ring_survives_a_host_that_never_numbers_its_frames() {
    let mut renderer = null_builder()
        .with_feature(UnlitMeshPack::new())
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);

    for _ in 0..unlit_ring_capacity(&renderer) * 2 {
        renderer.render_frame(&input).expect("frame");
    }

    assert_eq!(renderer.features(), [("unlit mesh", FeatureStatus::Active)]);
}

#[test]
#[ignore = "red until the frame clock lands"]
fn a_pack_with_a_per_frame_ring_survives_unnumbered_frames_in_mode_a() {
    let mut renderer = null_builder()
        .with_feature(UnlitMeshPack::new())
        .build()
        .expect("renderer");
    let input = one_view(&mut renderer, Format::Rgba8Unorm);

    for _ in 0..unlit_ring_capacity(&renderer) * 2 {
        let list = renderer.record_frame(&input).expect("frame");
        renderer.device().submit(list).expect("host submit");
    }

    assert_eq!(renderer.features(), [("unlit mesh", FeatureStatus::Active)]);
}

#[test]
#[ignore = "red until the frame clock lands"]
fn retrying_an_abandoned_frame_does_not_exhaust_a_per_frame_ring() {
    let mut renderer = null_builder()
        .with_feature(UnlitMeshPack::new())
        .build()
        .expect("renderer");
    let healthy = one_view(&mut renderer, Format::Rgba8Unorm);
    let failing = failing_second_view(&mut renderer, &healthy);

    for _ in 0..unlit_ring_capacity(&renderer) * 2 {
        assert!(renderer.render_frame(&failing).is_err());
    }

    renderer.render_frame(&healthy).expect("frame after the retries");
    assert_eq!(renderer.features(), [("unlit mesh", FeatureStatus::Active)]);
}
