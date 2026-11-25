// M3-06: a triangle that covers the picture, drawn from shader files that are reloaded while it runs. Edit
// `examples/shaders/hot_reload/shaders/quad.wgsl` (or the module `modules/palette.wgsl`) and save: the log says what
// was reloaded and the colour printed below follows within a fraction of a second. Break the shader and the old colour
// stays, with the error and its line in the log.
//
//   hot_reload [SECONDS] [SHADER_DIR MODULE_DIR]     (SECONDS defaults to 60; 0 runs until interrupted)

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use pathon::rhi::{Backend, Device, DeviceRequest};
use pathon_examples::hot_quad::HotQuad;

const FRAME: Duration = Duration::from_millis(16);

fn main() {
    pathon_examples::init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seconds: u64 = args.first().and_then(|text| text.parse().ok()).unwrap_or(60);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders/hot_reload");
    let shader_dir = args.get(1).map_or_else(|| root.join("shaders"), PathBuf::from);
    let module_dir = args.get(2).map_or_else(|| root.join("modules"), PathBuf::from);

    let device = Arc::new(
        Device::build(&DeviceRequest {
            label: "hot reload",
            backend: std::env::var("PATHON_BACKEND")
                .ok()
                .and_then(|name| (name.eq_ignore_ascii_case("vulkan")).then_some(Backend::Vulkan)),
            ..Default::default()
        })
        .expect("a GPU adapter"),
    );
    let mut quad = HotQuad::new(device, &shader_dir, &module_dir).unwrap_or_else(|error| panic!("{error}"));
    log::info!(
        "watching {} and {}; edit and save a shader",
        shader_dir.display(),
        module_dir.display()
    );

    let started = Instant::now();
    let mut shown = None;
    while seconds == 0 || started.elapsed() < Duration::from_secs(seconds) {
        let frame = quad.frame();
        if frame.pixel != shown {
            if let Some(pixel) = frame.pixel {
                log::info!("the picture is now rgba {pixel:?}");
            }
            shown = frame.pixel;
        }
        thread::sleep(FRAME);
    }
}
