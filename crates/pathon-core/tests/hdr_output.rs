// M4-16 (RHI and core part): the output colour space reaches the packs through `ViewCtx::output` (ADR-0034).

use std::sync::{Arc, Mutex};

use pathon_api::graph::Stage;
use pathon_api::{Camera, OutputInfo, PrepareCtx, RecordCtx, RenderFeature};
use pathon_core::{FrameInput, RenderError, Renderer, TargetId, ViewInput};
use pathon_rhi::{
    Device, DisplayHdr, Format, NullDevice, PhysicalSize, RhiError, SurfaceCaps, SurfaceColorSpace, SurfaceColorSpaces,
    SurfaceFormatCaps, SurfaceTarget,
};

const SIZE: PhysicalSize = PhysicalSize::new(64, 32);
const PEAK_NITS: f32 = 1000.0;

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

struct SeesOutput {
    seen: Arc<Mutex<Vec<OutputInfo>>>,
}

impl RenderFeature for SeesOutput {
    fn name(&self) -> &'static str {
        "sees output"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn prepare_view(&mut self, ctx: &mut PrepareCtx<'_>) {
        self.seen.lock().expect("lock").push(ctx.view().output);
    }

    fn record(&mut self, _ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        Ok(())
    }
}

fn hdr_display() -> SurfaceCaps {
    let mut caps = SurfaceCaps {
        formats: vec![SurfaceFormatCaps {
            format: Format::Bgra8UnormSrgb,
            color_spaces: SurfaceColorSpaces::SRGB,
        }],
        display: DisplayHdr {
            peak_nits: Some(PEAK_NITS),
            sdr_white_nits: Some(200.0),
            headroom: Some(5.0),
            ..DisplayHdr::default()
        },
    };
    caps.formats.push(SurfaceFormatCaps {
        format: Format::Rgba16Float,
        color_spaces: SurfaceColorSpaces::SRGB | SurfaceColorSpaces::EXTENDED_SRGB_LINEAR,
    });
    caps
}

fn renderer_on(caps: Option<SurfaceCaps>) -> (Renderer, Arc<Mutex<Vec<OutputInfo>>>) {
    let null = caps.map_or_else(NullDevice::new, |caps| NullDevice::new().with_surface_caps(caps));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let renderer = Renderer::builder()
        .with_device(Arc::new(Device::from(null)))
        .with_feature(SeesOutput {
            seen: Arc::clone(&seen),
        })
        .build()
        .expect("renderer");
    (renderer, seen)
}

fn window(renderer: &mut Renderer, color_space: SurfaceColorSpace) -> TargetId {
    renderer
        .add_window_with_color_space(SurfaceTarget::new(Arc::new(NoWindow), SIZE, 1.0), color_space)
        .expect("window")
}

fn render_one(renderer: &mut Renderer, target: TargetId) {
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    };
    renderer.render_frame(&input).expect("frame");
}

fn last(seen: &Arc<Mutex<Vec<OutputInfo>>>) -> OutputInfo {
    *seen.lock().expect("lock").last().expect("a view was prepared")
}

#[test]
fn auto_stays_sdr_even_on_an_hdr_display() {
    let (mut renderer, seen) = renderer_on(Some(hdr_display()));
    let target = window(&mut renderer, SurfaceColorSpace::Auto);
    render_one(&mut renderer, target);

    let output = last(&seen);
    assert!(!output.is_hdr());
    assert_eq!(output.color_space, SurfaceColorSpace::Srgb);
    assert_eq!(renderer.target_output(target), Some(output));
}

#[test]
fn an_offered_hdr_space_reaches_the_packs_with_the_peak_luminance() {
    let (mut renderer, seen) = renderer_on(Some(hdr_display()));
    let target = window(&mut renderer, SurfaceColorSpace::ExtendedSrgbLinear);
    render_one(&mut renderer, target);

    let output = last(&seen);
    assert!(output.is_hdr());
    assert_eq!(output.color_space, SurfaceColorSpace::ExtendedSrgbLinear);
    assert_eq!(output.peak_nits(), Some(PEAK_NITS));
    assert_eq!(output.display.headroom, Some(5.0));
}

#[test]
fn an_unoffered_hdr_space_falls_back_to_sdr_with_a_diagnostic() {
    let (mut renderer, seen) = renderer_on(None);
    let target = window(&mut renderer, SurfaceColorSpace::Hdr10St2084);
    let report = {
        let input = FrameInput {
            views: vec![ViewInput {
                target,
                camera: Camera::default(),
            }],
            ..FrameInput::default()
        };
        renderer.render_frame(&input).expect("the fallback is not an error")
    };

    assert_eq!(last(&seen), OutputInfo::SDR);
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.code == "H0001"));
}

#[test]
fn auto_never_reports_a_fallback() {
    let (mut renderer, _seen) = renderer_on(None);
    let target = window(&mut renderer, SurfaceColorSpace::Auto);
    let input = FrameInput {
        views: vec![ViewInput {
            target,
            camera: Camera::default(),
        }],
        ..FrameInput::default()
    };
    let report = renderer.render_frame(&input).expect("frame");
    assert!(report.diagnostics.iter().all(|diagnostic| diagnostic.code != "H0001"));
}

#[test]
fn the_color_space_can_change_after_the_window_exists() {
    let (mut renderer, seen) = renderer_on(Some(hdr_display()));
    let target = window(&mut renderer, SurfaceColorSpace::Auto);
    render_one(&mut renderer, target);
    assert!(!last(&seen).is_hdr());

    renderer
        .set_target_color_space(target, SurfaceColorSpace::ExtendedSrgbLinear)
        .expect("reconfigure");
    render_one(&mut renderer, target);
    assert!(last(&seen).is_hdr());

    renderer
        .set_target_color_space(target, SurfaceColorSpace::Auto)
        .expect("reconfigure");
    render_one(&mut renderer, target);
    assert!(!last(&seen).is_hdr());
}

#[test]
fn the_requested_space_survives_a_resize_and_a_device_reset() {
    let (mut renderer, _seen) = renderer_on(Some(hdr_display()));
    let target = window(&mut renderer, SurfaceColorSpace::ExtendedSrgbLinear);

    renderer
        .resize_target(target, PhysicalSize::new(128, 64))
        .expect("resize");
    assert!(renderer.target_output(target).expect("known").is_hdr());

    let replacement = Arc::new(Device::from(NullDevice::new().with_surface_caps(hdr_display())));
    renderer.reset(replacement).expect("reset");
    let output = renderer.target_output(target).expect("window kept after a reset");
    assert_eq!(output.color_space, SurfaceColorSpace::ExtendedSrgbLinear);
}

#[test]
fn surface_caps_report_what_the_surface_offers() {
    let (mut renderer, _seen) = renderer_on(Some(hdr_display()));
    let target = window(&mut renderer, SurfaceColorSpace::Auto);

    let caps = renderer.target_surface_caps(target).expect("caps");
    assert!(caps.color_spaces().supports(SurfaceColorSpace::ExtendedSrgbLinear));
    assert!(!caps.color_spaces().supports(SurfaceColorSpace::Hdr10St2084));
    assert_eq!(caps.display.peak_nits, Some(PEAK_NITS));
}

#[test]
fn offscreen_targets_are_sdr_and_have_no_surface_caps() {
    let (mut renderer, _seen) = renderer_on(Some(hdr_display()));
    let target = renderer
        .add_offscreen_target(SIZE, Format::Rgba16Float)
        .expect("offscreen");

    assert_eq!(renderer.target_output(target), Some(OutputInfo::SDR));
    assert!(matches!(
        renderer.target_surface_caps(target),
        Err(RenderError::Rhi(RhiError::Unsupported(_)))
    ));
    assert!(matches!(
        renderer.set_target_color_space(target, SurfaceColorSpace::Hdr10St2084),
        Err(RenderError::Rhi(RhiError::Unsupported(_)))
    ));
}
