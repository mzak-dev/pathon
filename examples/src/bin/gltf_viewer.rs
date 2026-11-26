// M4-10: opens a glTF scene (the classroom of docs/scenes.md) in a window, or renders one frame of it to a PNG. The
// scene is lit by the pathon packs (IblPack, ShadowPack, PbrPack, PostPack); `--simple` keeps the viewer's own
// one-feature lighting for comparison.
//
//   cargo run --release -p pathon-examples --bin gltf_viewer -- [scene.glb] [options]
//   PATHON_TEST_SCENE=test-assets/classroom/classroom-lite.glb cargo run --release -p pathon-examples --bin gltf_viewer
//
// Options: --screenshot out.png [--camera N | --camera-name NAME | --eye x,y,z --target x,y,z [--fov DEG]] [--size WxH]
//          [--supersample N] [--ev STOPS] [--no-shadows] [--no-local-shadows] [--area-shadows]
//          [--local-shadow-lights N] [--auto-exposure] [--no-bloom] [--neutral]
//          [--area-as-spot]  (area lights as the hemisphere spots of the file, as before K05)
//          [--gi [--gi-spacing M] [--gi-voxel M] [--gi-rays N] [--gi-hysteresis H] [--gi-period FRAMES] [--gi-warmup
// FRAMES]]  (pathon-gi probes)          [--dump-hdr out.npy] [--reference-like lights-only|ibl]  (the measuring mode,
// docs/porownanie-obrazu.md) [--brute-force-lights]          [--env file.hdr | --env none] [--environment-intensity X]
// [--sky-intensity X]          [--simple] [--per-material] [--bench FRAMES] [--backend vulkan|dx12|metal]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use pathon::WinitPlatform;
use pathon::api::glam::Vec3;
use pathon::platform::Platform;
use pathon::rhi::Backend;
use pathon_bindless::PathPreference;
use pathon_examples::gltf_loader::{AreaLights, LoadOptions, load_path_with};
use pathon_examples::viewer::bench::{format_report, run_benchmark};
use pathon_examples::viewer::environment::{default_environment_path, load_environment};
use pathon_examples::viewer::linear_dump::{render_linear, write_npy};
use pathon_examples::viewer::screenshot::{ScreenshotRequest, render_screenshot, write_png};
use pathon_examples::viewer::settings::{LightingMode, ReferenceLighting};
use pathon_examples::viewer::stack::{DEFAULT_GI_WARMUP, ViewerConfig};
use pathon_examples::viewer::window::ViewerWindow;
use pathon_gi::GiSettings;
use pathon_post::Tonemapper;

const SCENE_VARIABLE: &str = "PATHON_TEST_SCENE";

enum EnvironmentChoice {
    /// The `env/Garage.hdr` next to the scene when there is one, otherwise the synthetic sky.
    Default,
    Synthetic,
    File(PathBuf),
}

struct Options {
    scene: PathBuf,
    screenshot: Option<PathBuf>,
    dump_hdr: Option<PathBuf>,
    bench_frames: Option<u32>,
    request: ScreenshotRequest,
    lighting: LightingMode,
    environment: EnvironmentChoice,
    texture_path: PathPreference,
    tonemapper: Tonemapper,
    backend: Option<Backend>,
    load: LoadOptions,
    gi: Option<GiSettings>,
    gi_warmup: u32,
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
    let mut options = Options {
        scene: PathBuf::new(),
        screenshot: None,
        dump_hdr: None,
        bench_frames: None,
        request: ScreenshotRequest::default(),
        lighting: LightingMode::Pbr,
        environment: EnvironmentChoice::Default,
        texture_path: PathPreference::Auto,
        tonemapper: Tonemapper::AcesFitted,
        backend: None,
        load: LoadOptions::default(),
        gi: None,
        gi_warmup: DEFAULT_GI_WARMUP,
    };
    let (mut eye, mut target, mut fov_degrees) = (None, None, None);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        let request = &mut options.request;
        match arg.as_str() {
            "--screenshot" => options.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--dump-hdr" => options.dump_hdr = Some(PathBuf::from(value("--dump-hdr")?)),
            "--reference-like" => {
                request.reference_like = Some(match value("--reference-like")?.as_str() {
                    "lights-only" => ReferenceLighting::LightsOnly,
                    "ibl" => ReferenceLighting::Ibl,
                    other => return Err(format!("--reference-like takes lights-only or ibl, got {other}")),
                });
            }
            "--area-as-spot" => options.load.area_lights = AreaLights::AsSpot,
            "--camera-name" => request.camera_name = Some(value("--camera-name")?),
            "--bench" => options.bench_frames = Some(parse_number("--bench", &value("--bench")?)?),
            "--camera" => request.camera = Some(parse_number("--camera", &value("--camera")?)?),
            "--size" => (request.width, request.height) = parse_size(&value("--size")?)?,
            "--supersample" => request.supersample = parse_number("--supersample", &value("--supersample")?)?,
            "--ev" => request.exposure_stops = parse_number("--ev", &value("--ev")?)?,
            "--eye" => eye = Some(parse_vec3("--eye", &value("--eye")?)?),
            "--target" => target = Some(parse_vec3("--target", &value("--target")?)?),
            "--fov" => fov_degrees = Some(parse_number("--fov", &value("--fov")?)?),
            "--no-shadows" => request.sun_shadows = false,
            "--no-local-shadows" => request.local_shadows = false,
            "--area-shadows" => request.area_shadows = true,
            "--redraw-local-shadows" => request.redraw_local_shadows = true,
            "--local-shadow-lights" => {
                request.local_shadow_lights = Some(parse_number("--local-shadow-lights", &value(&arg)?)?);
            }
            "--auto-exposure" => request.auto_exposure = true,
            "--no-bloom" => request.bloom = false,
            "--brute-force-lights" => request.brute_force_lights = true,
            "--environment-intensity" => {
                request.environment_intensity = Some(parse_number("--environment-intensity", &value(&arg)?)?);
            }
            "--sky-intensity" => request.sky_intensity = Some(parse_number("--sky-intensity", &value(&arg)?)?),
            "--neutral" => options.tonemapper = Tonemapper::PbrNeutral,
            "--backend" => {
                options.backend = Some(match value("--backend")?.as_str() {
                    "vulkan" => Backend::Vulkan,
                    "dx12" => Backend::Dx12,
                    "metal" => Backend::Metal,
                    other => return Err(format!("unknown backend {other}")),
                });
            }
            "--simple" => options.lighting = LightingMode::Simple,
            "--gi" => {
                options.gi.get_or_insert_with(GiSettings::default);
            }
            "--gi-warmup" => options.gi_warmup = parse_number("--gi-warmup", &value("--gi-warmup")?)?,
            "--gi-spacing" => {
                options.gi.get_or_insert_with(GiSettings::default).probe_spacing =
                    parse_number("--gi-spacing", &value("--gi-spacing")?)?;
            }
            "--gi-voxel" => {
                options.gi.get_or_insert_with(GiSettings::default).voxel_size =
                    parse_number("--gi-voxel", &value("--gi-voxel")?)?;
            }
            "--gi-rays" => {
                options.gi.get_or_insert_with(GiSettings::default).rays_per_probe =
                    parse_number("--gi-rays", &value("--gi-rays")?)?;
            }
            "--gi-period" => {
                options.gi.get_or_insert_with(GiSettings::default).voxel_relight_period =
                    parse_number("--gi-period", &value("--gi-period")?)?;
            }
            "--gi-hysteresis" => {
                options.gi.get_or_insert_with(GiSettings::default).hysteresis =
                    parse_number("--gi-hysteresis", &value("--gi-hysteresis")?)?;
            }
            "--per-material" => options.texture_path = PathPreference::PerMaterial,
            "--env" => {
                let path = value("--env")?;
                options.environment = if path == "none" {
                    EnvironmentChoice::Synthetic
                } else {
                    EnvironmentChoice::File(PathBuf::from(path))
                };
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            path => scene = Some(PathBuf::from(path)),
        }
    }
    if options.dump_hdr.is_some() {
        options.tonemapper = Tonemapper::Linear;
    }
    options.request.eye_and_target = match (eye, target) {
        (Some(eye), Some(target)) => Some((eye, target)),
        (None, None) => None,
        _ => return Err("--eye and --target go together".into()),
    };
    options.request.vertical_fov_degrees = fov_degrees;
    options.scene = scene.ok_or_else(|| format!("give a .glb file or set {SCENE_VARIABLE}"))?;
    Ok(options)
}

fn viewer_config(options: &Options) -> Result<ViewerConfig, String> {
    let path = match &options.environment {
        EnvironmentChoice::Default => default_environment_path(&options.scene),
        EnvironmentChoice::Synthetic => None,
        EnvironmentChoice::File(path) => Some(path.clone()),
    };
    let environment = match path {
        Some(path) if options.lighting == LightingMode::Pbr => {
            let started = Instant::now();
            let image = load_environment(&path)?;
            log::info!(
                "environment {} ({}x{}) in {:.1} s",
                path.display(),
                image.width(),
                image.height(),
                started.elapsed().as_secs_f32()
            );
            Some(image)
        }
        _ => None,
    };
    Ok(ViewerConfig {
        lighting: options.lighting,
        environment,
        texture_path: options.texture_path,
        tonemapper: options.tonemapper,
        gpu_timestamps: false,
        backend: options.backend,
        gi: options.gi.filter(|_| options.lighting == LightingMode::Pbr),
        gi_warmup: options.gi_warmup,
    })
}

fn run(options: &Options) -> Result<(), String> {
    let started = Instant::now();
    let scene = load_path_with(&options.scene, options.load).map_err(|error| error.to_string())?;
    log::info!(
        "loaded {} in {:.1} s: {} instances, {} images, {} lights, {} cameras",
        options.scene.display(),
        started.elapsed().as_secs_f32(),
        scene.instances.len(),
        scene.images.len(),
        scene.lights.len(),
        scene.cameras.len(),
    );
    let config = viewer_config(options)?;
    if let Some(frames) = options.bench_frames {
        let report = run_benchmark(&scene, &config, &options.request, frames)?;
        log::info!("benchmark of {frames} frames:\n{}", format_report(&report));
        return Ok(());
    }
    if let Some(path) = &options.dump_hdr {
        let image = render_linear(&scene, &config, &options.request)?;
        write_npy(path, &image)?;
        log::info!(
            "wrote {} ({}x{} linear float32)",
            path.display(),
            image.width,
            image.height
        );
        return Ok(());
    }
    if let Some(path) = &options.screenshot {
        let image = render_screenshot(&scene, &config, &options.request)?;
        write_png(path, &image)?;
        log::info!("wrote {} ({}x{})", path.display(), image.width(), image.height());
        return Ok(());
    }
    let failure = Arc::new(Mutex::new(None));
    let title = format!("pathon - {}", options.scene.display());
    let app = ViewerWindow::new(scene, title, Arc::clone(&failure), config);
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
