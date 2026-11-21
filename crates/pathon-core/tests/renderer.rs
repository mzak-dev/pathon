use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::{Camera, RecordCtx, RenderFeature, Role, SetupCtx};
use pathon_core::{FeatureStatus, FrameInput, RenderError, Renderer, RendererBuilder, ViewInput};
use pathon_rhi::{
    Access, Backend, Color, Device, DeviceApi, DeviceRequest, Extent3d, Format, PhysicalSize, RhiError, SurfaceTarget,
    TextureId,
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
    renderer.render_frame(&input).expect("frame after recovery");
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
        pass.read(texture, Access::SHADER_READ);
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
