// M2-18: one unlit mesh through the whole stack — platform → core → graph → RHI — in two windows.
// `--devtools` streams every frame's graph to `cargo run -p pathon-devtools` (docs/devtools.md).

use pathon::platform::WindowDesc;
use pathon::rhi::Color;
use pathon::{Renderer, ViewInput, WinitPlatform};
use pathon_devtools::{DEFAULT_ADDRESS, DevtoolsServer};
use pathon_test::UnlitMeshPack;

fn start_devtools() -> Option<DevtoolsServer> {
    DevtoolsServer::start(DEFAULT_ADDRESS)
        .inspect_err(|error| log::error!("devtools unavailable on {DEFAULT_ADDRESS}: {error}"))
        .ok()
}

fn main() -> Result<(), pathon::runtime::RunError> {
    pathon_examples::init_logging();
    let mut devtools = std::env::args()
        .any(|arg| arg == "--devtools")
        .then(start_devtools)
        .flatten();
    let builder = Renderer::builder()
        .with_clear_color(Color::rgba(0.05, 0.05, 0.08, 1.0))
        .with_feature(UnlitMeshPack::new())
        .with_task_pool(std::sync::Arc::new(pathon::runtime::StdTaskPool::new()));
    let windows = vec![
        WindowDesc::new("pathon — slice, left", 640.0, 480.0),
        WindowDesc::new("pathon — slice, right", 480.0, 480.0),
    ];
    pathon::runtime::run_with_hook(
        WinitPlatform::new(),
        builder,
        windows,
        |frame| {
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
        },
        move |renderer, report| {
            if let Some(devtools) = &mut devtools {
                devtools.frame(renderer, report);
            }
        },
    )
}
