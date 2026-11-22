// M0-10: one window cleared by an empty renderer, through the runner (platform on the main thread, renderer on the
// render thread).

use pathon::platform::WindowDesc;
use pathon::rhi::Color;
use pathon::{Renderer, ViewInput, WinitPlatform};

fn main() -> Result<(), pathon::runtime::RunError> {
    pathon_examples::init_logging();
    let builder = Renderer::builder().with_clear_color(Color::rgba(0.1, 0.2, 0.35, 1.0));
    let windows = vec![WindowDesc::new("pathon — clear", 960.0, 540.0)];
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
    let hook = pathon_examples::report_frames_hook(pathon_examples::frame_limit_from_env());
    pathon::runtime::run_with_hook(WinitPlatform::new(), builder, windows, scene, hook)
}
