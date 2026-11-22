// M4-16 (RHI and core part): reports which colour spaces the window's surface offers and what it presents.
// PATHON_COLOR_SPACE=auto|srgb|display-p3|scrgb|extended-srgb|extended-display-p3|hdr10|hlg asks for a space;
// the default `auto` never gives HDR (ADR-0034). PATHON_RECONFIGURE=1 asks for it on the live window instead of at
// creation. PATHON_BACKEND=dx12|vulkan|metal picks the backend to compare them. Nothing is tonemapped yet: this checks capabilities, not pictures.

use pathon::platform::WindowDesc;
use pathon::rhi::{Backend, Color, DeviceRequest, SurfaceColorSpace};
use pathon::{Renderer, ViewInput, WinitPlatform};

const COLOR_SPACE: &str = "PATHON_COLOR_SPACE";

fn parse_color_space(name: &str) -> Option<SurfaceColorSpace> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "auto" => SurfaceColorSpace::Auto,
        "srgb" => SurfaceColorSpace::Srgb,
        "display-p3" => SurfaceColorSpace::DisplayP3,
        "scrgb" => SurfaceColorSpace::ExtendedSrgbLinear,
        "extended-srgb" => SurfaceColorSpace::ExtendedSrgb,
        "extended-display-p3" => SurfaceColorSpace::ExtendedDisplayP3,
        "hdr10" => SurfaceColorSpace::Hdr10St2084,
        "hlg" => SurfaceColorSpace::Hlg,
        _ => return None,
    })
}

fn requested_color_space() -> SurfaceColorSpace {
    let name = std::env::var(COLOR_SPACE).unwrap_or_default();
    if name.is_empty() {
        return SurfaceColorSpace::Auto;
    }
    parse_color_space(&name).unwrap_or_else(|| {
        log::warn!("{COLOR_SPACE}={name} is not a colour space name; using auto");
        SurfaceColorSpace::Auto
    })
}

fn requested_backend() -> Option<Backend> {
    match std::env::var("PATHON_BACKEND").ok()?.to_ascii_lowercase().as_str() {
        "dx12" => Some(Backend::Dx12),
        "vulkan" => Some(Backend::Vulkan),
        "metal" => Some(Backend::Metal),
        _ => None,
    }
}

fn report_targets(renderer: &mut Renderer, requested: SurfaceColorSpace) {
    for target in renderer.target_ids() {
        if std::env::var_os("PATHON_RECONFIGURE").is_some()
            && let Err(error) = renderer.set_target_color_space(target, requested)
        {
            log::warn!("{target:?}: cannot switch to {requested:?}: {error}");
            continue;
        }
        match renderer.target_surface_caps(target) {
            Ok(caps) => {
                for format in &caps.formats {
                    let spaces: Vec<_> = format.color_spaces.spaces().collect();
                    log::info!("surface offers {:?}: {spaces:?}", format.format);
                }
                log::info!("display: {:?}", caps.display);
            }
            Err(error) => log::warn!("{target:?}: no surface caps: {error}"),
        }
        log::info!(
            "requested {requested:?}, presenting {:?}",
            renderer.target_output(target)
        );
    }
}

fn main() -> Result<(), pathon::runtime::RunError> {
    pathon_examples::init_logging();
    let requested = requested_color_space();
    let builder = Renderer::builder()
        .with_device_request(DeviceRequest {
            backend: requested_backend(),
            ..DeviceRequest::default()
        })
        .with_color_space(requested)
        .with_clear_color(Color::rgba(0.1, 0.2, 0.35, 1.0));
    let windows = vec![WindowDesc::new("pathon — hdr output", 960.0, 540.0)];
    let scene = |frame: &pathon::runtime::FrameContext<'_>| {
        frame
            .windows
            .iter()
            .map(|(target, _)| ViewInput {
                target: *target,
                camera: pathon::api::Camera::default(),
            })
            .collect()
    };
    let mut frames_hook = pathon_examples::report_frames_hook(pathon_examples::frame_limit_from_env());
    let mut reported = false;
    let hook = move |renderer: &mut Renderer, report: &pathon::FrameReport| {
        if !reported {
            reported = true;
            report_targets(renderer, requested);
        }
        frames_hook(renderer, report);
    };
    pathon::runtime::run_with_hook(WinitPlatform::new(), builder, windows, scene, hook)
}
