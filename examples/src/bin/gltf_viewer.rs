// M4-10: opens a glTF scene (the classroom of docs/scenes.md) in a window, or renders one frame of it to a PNG.
//
//   cargo run --release -p pathon-examples --bin gltf_viewer -- [scene.glb] [options]
//   PATHON_TEST_SCENE=test-assets/classroom/classroom-lite.glb cargo run --release -p pathon-examples --bin gltf_viewer
//
// Options: --screenshot out.png [--camera N | --eye x,y,z --target x,y,z [--fov DEG]] [--size WxH]
//          [--supersample N] [--ev STOPS] [--no-shadows]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use pathon::WinitPlatform;
use pathon::api::glam::Vec3;
use pathon::platform::Platform;
use pathon_examples::gltf_loader::load_path;
use pathon_examples::viewer::screenshot::{ScreenshotRequest, render_screenshot, write_png};
use pathon_examples::viewer::window::ViewerWindow;

const SCENE_VARIABLE: &str = "PATHON_TEST_SCENE";

struct Options {
    scene: PathBuf,
    screenshot: Option<PathBuf>,
    request: ScreenshotRequest,
}

fn parse_size(text: &str) -> Result<(u32, u32), String> {
    let (width, height) = text.split_once('x').ok_or("the size is WIDTHxHEIGHT")?;
    let parse = |part: &str| {
        part.parse::<u32>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or("the size is WIDTHxHEIGHT")
    };
    Ok((parse(width)?, parse(height)?))
}

fn parse_number<T: std::str::FromStr>(name: &str, text: &str) -> Result<T, String> {
    text.parse().map_err(|_| format!("{name} takes a number, got '{text}'"))
}

fn parse_vec3(name: &str, text: &str) -> Result<Vec3, String> {
    let parts: Vec<f32> = text
        .split(',')
        .map(|part| parse_number(name, part.trim()))
        .collect::<Result<_, _>>()?;
    match parts[..] {
        [x, y, z] => Ok(Vec3::new(x, y, z)),
        _ => Err(format!("{name} takes x,y,z")),
    }
}

fn parse_options(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut args = args;
    let mut scene = std::env::var_os(SCENE_VARIABLE).map(PathBuf::from);
    let mut screenshot = None;
    let mut request = ScreenshotRequest::default();
    let (mut eye, mut target, mut fov_degrees) = (None, None, None);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--screenshot" => screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--camera" => request.camera = Some(parse_number("--camera", &value("--camera")?)?),
            "--size" => (request.width, request.height) = parse_size(&value("--size")?)?,
            "--supersample" => request.supersample = parse_number("--supersample", &value("--supersample")?)?,
            "--ev" => request.exposure_stops = parse_number("--ev", &value("--ev")?)?,
            "--eye" => eye = Some(parse_vec3("--eye", &value("--eye")?)?),
            "--target" => target = Some(parse_vec3("--target", &value("--target")?)?),
            "--fov" => fov_degrees = Some(parse_number("--fov", &value("--fov")?)?),
            "--no-shadows" => request.sun_shadows = false,
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            path => scene = Some(PathBuf::from(path)),
        }
    }
    request.eye_and_target = match (eye, target) {
        (Some(eye), Some(target)) => Some((eye, target)),
        (None, None) => None,
        _ => return Err("--eye and --target go together".into()),
    };
    request.vertical_fov_degrees = fov_degrees;
    let scene = scene.ok_or_else(|| format!("give a .glb file or set {SCENE_VARIABLE}"))?;
    Ok(Options {
        scene,
        screenshot,
        request,
    })
}

fn run(options: &Options) -> Result<(), String> {
    let started = Instant::now();
    let scene = load_path(&options.scene).map_err(|error| error.to_string())?;
    log::info!(
        "loaded {} in {:.1} s: {} instances, {} images, {} lights, {} cameras",
        options.scene.display(),
        started.elapsed().as_secs_f32(),
        scene.instances.len(),
        scene.images.len(),
        scene.lights.len(),
        scene.cameras.len(),
    );
    if let Some(path) = &options.screenshot {
        let image = render_screenshot(&scene, &options.request)?;
        write_png(path, &image)?;
        log::info!("wrote {} ({}x{})", path.display(), image.width(), image.height());
        return Ok(());
    }
    let failure = Arc::new(Mutex::new(None));
    let title = format!("pathon - {}", options.scene.display());
    let app = ViewerWindow::new(scene, title, Arc::clone(&failure));
    WinitPlatform::new()
        .run(Box::new(app))
        .map_err(|error| error.to_string())?;
    match failure.lock().map(|mut slot| slot.take()) {
        Ok(Some(reason)) => Err(reason),
        _ => Ok(()),
    }
}

#[expect(clippy::print_stderr, reason = "the command line tool reports errors to the console")]
fn main() -> ExitCode {
    pathon_examples::init_logging();
    let options = match parse_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("gltf_viewer: {message}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("gltf_viewer: {message}");
            ExitCode::FAILURE
        }
    }
}
