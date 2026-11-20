// M2-18: one unlit mesh through the whole stack — platform → core → graph → RHI — in two windows.

use pathon::platform::WindowDesc;
use pathon::rhi::Color;
use pathon::{Renderer, ViewInput, WinitPlatform};
use pathon_test::UnlitMeshPack;

fn main() -> Result<(), pathon::runtime::RunError> {
    pathon_examples::init_logging();
    let builder = Renderer::builder()
        .with_clear_color(Color::rgba(0.05, 0.05, 0.08, 1.0))
        .with_feature(UnlitMeshPack::new())
        .with_task_pool(std::sync::Arc::new(pathon::runtime::StdTaskPool::new()));
    let windows = vec![
        WindowDesc::new("pathon — slice, left", 640.0, 480.0),
        WindowDesc::new("pathon — slice, right", 480.0, 480.0),
    ];
    pathon::runtime::run(WinitPlatform::new(), builder, windows, |frame| {
        frame
            .windows
            .iter()
            .enumerate()
            .map(|(index, (target, size))| {
                let aspect = size.width as f32 / size.height.max(1) as f32;
                let time = frame.time_seconds + index as f64 * 2.0;
                ViewInput {
                    target: *target,
                    camera: pathon_examples::orbit_camera(time, aspect),
                }
            })
            .collect()
    })
}
