use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use pathon_api::graph::{GraphTextureDesc, PassDesc, Stage};
use pathon_api::{Camera, RecordCtx, RenderFeature};
use pathon_core::{FrameInput, Renderer, ViewInput};
use pathon_rhi::{Access, Backend, Color, DeviceRequest, Format, PhysicalSize, RhiError};
use pathon_test::{GeometryPack, OnDemandConsumer, UnlitMeshPack, UpscalerPack, VendorDummy};

use crate::capture::snapshot;
use crate::protocol::Snapshot;
use crate::server::ClientEvent;

// Writes a texture nobody reads, so the demo shows a culled pass.
struct DebugMask;

impl RenderFeature for DebugMask {
    fn name(&self) -> &'static str {
        "debug mask"
    }

    fn stage(&self) -> Stage {
        Stage::Overlay
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let mask = ctx
            .graph()
            .create_texture("debug mask", GraphTextureDesc::output(Format::R8Unorm));
        let mut pass = ctx.graph().add_pass(PassDesc::new("debug mask", Stage::Overlay));
        pass.write(mask, Access::COLOR_ATTACHMENT);
        Ok(())
    }
}

// The packs of pathon-test on the null backend: no GPU, no application, two views.
pub(crate) fn demo_renderer() -> (Renderer, FrameInput) {
    let mut renderer = Renderer::builder()
        .with_device_request(DeviceRequest {
            backend: Some(Backend::Null),
            ..DeviceRequest::default()
        })
        .with_feature(GeometryPack::new("geometry", Color::BLACK, 0))
        .with_feature(GeometryPack::new("fallback geometry", Color::BLACK, 10))
        .with_feature(OnDemandConsumer)
        .with_feature(VendorDummy::default())
        .with_feature(UpscalerPack::new(0.67))
        .with_feature(UnlitMeshPack::new())
        .with_feature(DebugMask)
        .build()
        .expect("the null backend always builds");
    renderer.set_capture(true);
    let views = [PhysicalSize::new(1920, 1080), PhysicalSize::new(1280, 720)]
        .into_iter()
        .map(|size| {
            let target = renderer
                .add_offscreen_target(size, Format::Rgba16Float)
                .expect("offscreen target");
            ViewInput {
                target,
                camera: Camera::default(),
            }
        })
        .collect();
    (
        renderer,
        FrameInput {
            views,
            ..FrameInput::default()
        },
    )
}

pub(crate) fn demo_snapshot(renderer: &mut Renderer, input: &mut FrameInput) -> Snapshot {
    let start = Instant::now();
    input.frame_index += 1;
    #[expect(clippy::cast_precision_loss, reason = "frame counters stay far below f64 precision")]
    let time = input.frame_index as f64 / 30.0;
    input.time_seconds = time;
    let report = renderer.render_frame(input).expect("the demo scene renders");
    #[expect(clippy::cast_possible_truncation, reason = "a frame time in milliseconds fits f32")]
    let frame_ms = (start.elapsed().as_secs_f64() * 1000.0) as f32;
    snapshot(input.frame_index, frame_ms, renderer, &report)
}

pub fn spawn_demo(events: Sender<ClientEvent>) {
    let spawned = thread::Builder::new()
        .name("pathon-devtools-demo".into())
        .spawn(move || {
            let (mut renderer, mut input) = demo_renderer();
            if events
                .send(ClientEvent::Connected("demo · backend null".into()))
                .is_err()
            {
                return;
            }
            loop {
                let snapshot = demo_snapshot(&mut renderer, &mut input);
                if events.send(ClientEvent::Snapshot(Box::new(snapshot))).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
    if let Err(error) = spawned {
        log::error!("cannot start the devtools demo thread: {error}");
    }
}
