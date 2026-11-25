//! M3-06 end to end: the shader files of the `hot_reload` example are edited while frames run, and the picture follows
//! them within two seconds, keeps its last colour while a shader does not compile, and follows the fix. The null device
//! runs the same script without looking at pixels; the GPU variant (feature `gpu-tests`, `PATHON_TEST_BACKEND` picks
//! the backend) checks the colours.

#![expect(clippy::print_stdout, reason = "the measured delays are the point of this test")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use std::{fs, thread};

use pathon::rhi::Device;
use pathon_examples::hot_quad::{HotFrame, HotQuad};
use pathon_hot::HotConfig;

const QUAD: &str = include_str!("../shaders/hot_reload/shaders/quad.wgsl");
const LIMIT: Duration = Duration::from_secs(2);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "pathon-hot-e2e-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("shaders")).expect("shaders dir");
        fs::create_dir_all(path.join("modules")).expect("modules dir");
        Self(path)
    }

    fn palette(&self) -> PathBuf {
        self.0.join("modules/palette.wgsl")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.0));
    }
}

fn palette(red: f32, green: f32, blue: f32) -> String {
    format!("fn tint() -> vec4<f32> {{\n    return vec4<f32>({red:?}, {green:?}, {blue:?}, 1.0);\n}}\n")
}

fn write_atomically(path: &Path, text: &str) {
    let staging = path.with_extension("wgsl.tmp");
    fs::write(&staging, text).expect("staging file");
    fs::rename(&staging, path).expect("replace");
}

// Frames run like a host's: pumping, drawing, sleeping a little. Returns when `done` accepts a frame, with how long it
// took.
fn run_frames_until(quad: &mut HotQuad, limit: Duration, mut done: impl FnMut(&HotFrame) -> bool) -> Option<Duration> {
    let started = Instant::now();
    while started.elapsed() < limit {
        let frame = quad.frame();
        if done(&frame) {
            return Some(started.elapsed());
        }
        thread::sleep(Duration::from_millis(4));
    }
    None
}

fn dominant(pixel: [u8; 4]) -> char {
    match (pixel[0] > 150, pixel[1] > 150, pixel[2] > 150) {
        (true, false, false) => 'r',
        (false, true, false) => 'g',
        (false, false, true) => 'b',
        (true, true, false) => 'y',
        _ => '?',
    }
}

fn scenario(device: Arc<Device>, pixels_mean_something: bool) {
    let dir = TempDir::new();
    fs::write(dir.0.join("shaders/quad.wgsl"), QUAD).expect("quad");
    fs::write(dir.palette(), palette(0.9, 0.1, 0.1)).expect("palette");
    let config = HotConfig::new()
        .with_shader_dir(dir.0.join("shaders"))
        .with_module_dir(dir.0.join("modules"))
        .with_debounce(Duration::from_millis(30));
    let mut quad = HotQuad::with_config(device, &config).expect("hot reload starts");
    // Without pixels to look at (the null device) a reload counts as the picture changing.
    let showing = |colour: char| {
        let mut reloaded = false;
        move |frame: &HotFrame| {
            reloaded |= frame.report.changed();
            if pixels_mean_something {
                frame.pixel.is_some_and(|pixel| dominant(pixel) == colour)
            } else {
                reloaded && frame.pixel.is_some()
            }
        }
    };

    run_frames_until(&mut quad, Duration::from_secs(30), |frame| {
        frame
            .pixel
            .is_some_and(|pixel| !pixels_mean_something || dominant(pixel) == 'r')
    })
    .expect("the first picture appears");

    fs::write(dir.palette(), palette(0.1, 0.9, 0.1)).expect("plain write");
    let plain = run_frames_until(&mut quad, LIMIT, showing('g'));
    println!("plain write visible after {plain:?}");
    assert!(plain.is_some(), "a plain write shows within {LIMIT:?}");

    write_atomically(&dir.palette(), &palette(0.1, 0.1, 0.9));
    let atomic = run_frames_until(&mut quad, LIMIT, showing('b'));
    println!("atomic replace visible after {atomic:?}");
    assert!(atomic.is_some(), "an atomic replace shows within {LIMIT:?}");

    fs::write(
        dir.palette(),
        "fn tint() -> vec4<f32> {\n    return vec4<f32>(oops, 0.0, 0.0, 1.0);\n}\n",
    )
    .expect("break");
    let mut diagnostics = Vec::new();
    let mut blanked = false;
    run_frames_until(&mut quad, Duration::from_secs(10), |frame| {
        blanked |=
            frame.pixel.is_none() || (pixels_mean_something && frame.pixel.is_some_and(|pixel| dominant(pixel) != 'b'));
        diagnostics.extend(
            frame
                .report
                .diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.code, diagnostic.message.clone())),
        );
        diagnostics.iter().any(|(code, _)| *code == "K0006")
    })
    .expect("the broken shader is reported");
    for _ in 0..30 {
        let frame = quad.frame();
        blanked |=
            frame.pixel.is_none() || (pixels_mean_something && frame.pixel.is_some_and(|pixel| dominant(pixel) != 'b'));
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!blanked, "the picture kept its colour while the shader was broken");
    let located = diagnostics
        .iter()
        .find(|(code, _)| *code == "S0107")
        .expect("the located error is reported");
    assert!(located.1.contains("palette.wgsl:2:"), "file and line: {}", located.1);

    fs::write(dir.palette(), palette(0.9, 0.9, 0.1)).expect("fix");
    let fixed = run_frames_until(&mut quad, LIMIT, showing('y'));
    println!("fix visible after {fixed:?}");
    assert!(fixed.is_some(), "the fix shows within {LIMIT:?}");
    assert!(quad.has_pipeline());
}

#[test]
fn editing_a_shader_file_changes_the_picture_without_a_restart_on_the_null_device() {
    scenario(Arc::new(Device::null()), false);
}

#[cfg(feature = "gpu-tests")]
#[test]
fn editing_a_shader_file_changes_the_picture_without_a_restart_on_a_real_gpu() {
    use pathon::rhi::{Backend, DeviceRequest};

    let backend = match std::env::var("PATHON_TEST_BACKEND").ok().as_deref() {
        Some("vulkan") => Some(Backend::Vulkan),
        Some("dx12") => Some(Backend::Dx12),
        Some("metal") => Some(Backend::Metal),
        _ => None,
    };
    let device = Device::build(&DeviceRequest {
        label: "hot reload test",
        backend,
        ..Default::default()
    })
    .expect("an adapter is required for gpu-tests");
    scenario(Arc::new(device), true);
}
