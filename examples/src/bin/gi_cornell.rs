// K12: a Cornell box lit by one area light in the ceiling, rendered twice by the viewer's stack: with the environment
// only (what `PbrPack` does without a probe volume) and with `GiPack`. The light that reaches the walls in the shadow of
// the boxes, and the colour the red and the green wall bleed onto the white ones, exist only in the second picture.
//
//   cargo run --release -p pathon-examples --bin gi_cornell -- [prefix] [SIZE]
//
// writes `<prefix>_environment.png` and `<prefix>_gi.png` (defaults: `gi_cornell`, 512).

use pathon::api::glam::Vec3;
use pathon_examples::cornell::{ROOM, cornell_scene};
use pathon_examples::viewer::screenshot::{ScreenshotRequest, render_screenshot, write_png};
use pathon_examples::viewer::stack::ViewerConfig;
use pathon_gi::GiSettings;

#[expect(clippy::print_stderr, reason = "the command line tool reports errors to the console")]
fn main() -> std::process::ExitCode {
    pathon_examples::init_logging();
    let mut arguments = std::env::args().skip(1);
    let prefix = arguments.next().unwrap_or_else(|| "gi_cornell".to_owned());
    let size: u32 = arguments.next().and_then(|text| text.parse().ok()).unwrap_or(512);
    let request = ScreenshotRequest {
        width: size,
        height: size,
        eye_and_target: Some((
            Vec3::new(0.0, ROOM * 0.5, ROOM * 1.45),
            Vec3::new(0.0, ROOM * 0.45, 0.0),
        )),
        vertical_fov_degrees: Some(45.0),
        bloom: false,
        environment_intensity: Some(0.0),
        sky_intensity: Some(0.0),
        ..ScreenshotRequest::default()
    };
    let scene = cornell_scene();
    for (suffix, gi) in [("environment", None), ("gi", Some(GiSettings::default()))] {
        let config = ViewerConfig {
            gi,
            ..ViewerConfig::default()
        };
        let image = match render_screenshot(&scene, &config, &request) {
            Ok(image) => image,
            Err(message) => {
                eprintln!("gi_cornell: {message}");
                return std::process::ExitCode::FAILURE;
            }
        };
        let path = format!("{prefix}_{suffix}.png");
        if let Err(message) = write_png(std::path::Path::new(&path), &image) {
            eprintln!("gi_cornell: {message}");
            return std::process::ExitCode::FAILURE;
        }
        log::info!("wrote {path}");
    }
    std::process::ExitCode::SUCCESS
}
