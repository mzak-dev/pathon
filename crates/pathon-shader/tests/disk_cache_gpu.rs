#![cfg(feature = "gpu-tests")]
#![expect(clippy::print_stderr, reason = "the measurement is the point of this test")]

//! Measures what the saved pipeline cache buys a second start (M3-05, `docs/bench/pipeline-cache.md`).
//!
//! Each "start" is a separate process of this very test binary (`start_in_this_process` is the child entry point).
//! Drivers keep their own shader caches, in memory and on disk, that outlive a device and a process, so a second start
//! can be fast without the blob. The children therefore run with the drivers' own caches turned off (see
//! `isolate_driver_caches`); setting `PATHON_KEEP_DRIVER_CACHES=1` leaves them on to measure the default setup.
//! The parent runs three starts of the same shaders: an empty cache, the cache the first one saved, and a control
//! with no cache at all.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pathon_rhi::{
    Backend, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingType, ComputePipelineDesc, Device, DeviceApi,
    DeviceRequest, DeviceType, Features, PipelineCacheData, RhiError, ShaderModuleDesc, ShaderStage, ShaderStages,
    StorageAccess,
};
use pathon_shader::{FileCacheStore, PersistentPipelineCache, SaveOutcome};

const VARIANTS: u32 = 8;
const OPERATIONS: u32 = 400;
// The second start must compile at least this much faster than the first, or the cache did nothing measurable.
const REQUIRED_SPEEDUP: f64 = 1.25;

const CHILD_CACHE_FILE: &str = "PATHON_CACHE_CHILD_FILE";
const CHILD_SALT: &str = "PATHON_CACHE_CHILD_SALT";
const KEEP_DRIVER_CACHES: &str = "PATHON_KEEP_DRIVER_CACHES";

fn request(seed: Option<PipelineCacheData>) -> DeviceRequest {
    DeviceRequest {
        label: "pipeline cache start",
        backend: Some(Backend::Vulkan),
        required_features: Features::PIPELINE_CACHE,
        pipeline_cache: seed,
        ..Default::default()
    }
}

// Straight-line transcendental code: long enough for the driver's compiler to take milliseconds, so the cache has
// something to save. The salt makes every run's shaders new to any driver-level cache that outlives the run.
fn heavy_shader(salt: u64, variant: u32) -> String {
    let mut body = String::new();
    for step in 0..OPERATIONS {
        let a = 1.0 + f64::from((step * 7 + variant) % 97) / 1000.0;
        let b = f64::from((step * 13 + variant) % 89) / 10.0;
        writeln!(
            body,
            "    x = fma(x, {a:.4}, sin(y + {b:.1})); y = y * 0.9993 + cos(x) * {b:.1};"
        )
        .expect("writing to a String does not fail");
    }
    format!(
        "@group(0) @binding(0) var<storage, read_write> out: array<f32>;\n\
         @compute @workgroup_size(64)\n\
         fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
         \x20   var x = f32(id.x) + {salt}.0;\n    var y = x * 0.5 + {variant}.0;\n{body}\
         \x20   out[id.x] = x + y;\n}}\n"
    )
}

struct Start {
    pipelines: Duration,
    saved: &'static str,
    adapter: String,
    software: bool,
}

// One start: build the device (seeded from `cache_file` when it holds a cache), compile every pipeline, save.
fn start_with(cache_file: &Path, salt: u64) -> Option<Start> {
    let cache = PersistentPipelineCache::new(Arc::new(FileCacheStore::new(cache_file)));
    let seed = cache.load();
    assert!(seed.warnings().is_empty(), "{:?}", seed.warnings());
    let device = match Device::build(&request(seed.data().cloned())) {
        Ok(device) => device,
        Err(error @ (RhiError::NoAdapter(_) | RhiError::MissingFeatures(_))) => {
            eprintln!("skipped: no Vulkan adapter with PIPELINE_CACHE ({error})");
            return None;
        }
        Err(error) => panic!("device creation failed: {error}"),
    };
    assert!(seed.adapter_mismatch(device.caps()).is_none());
    let modules: Vec<_> = (0..VARIANTS)
        .map(|variant| {
            device
                .create_shader_module(&ShaderModuleDesc {
                    label: "heavy",
                    wgsl: &heavy_shader(salt, variant),
                })
                .expect("module")
        })
        .collect();
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "heavy",
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::StorageBuffer {
                    access: StorageAccess::ReadWrite,
                },
                count: None,
            }],
        })
        .expect("layout");
    let started = Instant::now();
    for module in modules {
        device
            .create_compute_pipeline(&ComputePipelineDesc {
                label: "heavy",
                bind_group_layouts: vec![layout],
                shader: ShaderStage {
                    module,
                    entry_point: "main",
                    constants: vec![],
                },
                immediate_size: 0,
            })
            .expect("pipeline");
    }
    let pipelines = started.elapsed();
    let saved = match cache.save(&device).expect("save") {
        SaveOutcome::Saved { .. } => "saved",
        SaveOutcome::Unchanged => "unchanged",
        SaveOutcome::Empty => "empty",
        SaveOutcome::Unsupported => "unsupported",
        other => panic!("unexpected outcome {other:?}"),
    };
    let adapter = &device.caps().adapter;
    Some(Start {
        pipelines,
        saved,
        adapter: format!(
            "{} ({:?}, driver {})",
            adapter.name, adapter.device_type, adapter.driver
        ),
        software: adapter.device_type == DeviceType::Cpu,
    })
}

// The child entry point: does nothing when the test binary runs normally.
#[test]
fn start_in_this_process() {
    let (Some(file), Some(salt)) = (std::env::var_os(CHILD_CACHE_FILE), std::env::var(CHILD_SALT).ok()) else {
        return;
    };
    let salt: u64 = salt.parse().expect("a numeric salt");
    // The harness prints "test name ... " before the test runs, so the result starts on a line of its own.
    match start_with(Path::new(&file), salt) {
        Some(start) => println!(
            "\nRESULT\t{:.3}\t{}\t{}\t{}",
            millis(start.pipelines),
            start.saved,
            start.software,
            start.adapter
        ),
        None => println!("\nRESULT\tskipped"),
    }
}

struct ChildStart {
    pipelines_ms: f64,
    saved: String,
    software: bool,
    adapter: String,
}

// Fresh per-user directories (where NVIDIA and Mesa keep caches) and the driver switches known to turn the
// drivers' own caches off: AMD's proprietary Vulkan driver keeps one that survives processes and is not under the
// per-user directories, `AMD_VK_USE_PIPELINE_CACHE=0` disables it. Without this the control run is as fast as the
// seeded one and the blob's effect is invisible.
fn isolate_driver_caches(command: &mut Command, home: &Path) {
    command
        .env("LOCALAPPDATA", home)
        .env("APPDATA", home)
        .env("XDG_CACHE_HOME", home)
        .env("MESA_SHADER_CACHE_DIR", home)
        .env("MESA_SHADER_CACHE_DISABLE", "true")
        .env("AMD_VK_USE_PIPELINE_CACHE", "0");
}

fn run_child(cache_file: &Path, salt: u64, home: &Path) -> Option<ChildStart> {
    std::fs::create_dir_all(home).expect("home");
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        .args(["--exact", "start_in_this_process", "--nocapture", "--test-threads=1"])
        .env(CHILD_CACHE_FILE, cache_file)
        .env(CHILD_SALT, salt.to_string());
    if std::env::var_os(KEEP_DRIVER_CACHES).is_none() {
        isolate_driver_caches(&mut command, home);
    }
    let output = command.output().expect("spawn the child start");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "child failed:\n{stdout}\n{stderr}");
    let line = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RESULT\t"))
        .unwrap_or_else(|| panic!("the child printed no result:\n{stdout}\n{stderr}"));
    if line == "skipped" {
        eprintln!("child start skipped: {}", stderr.trim());
        return None;
    }
    let mut fields = line.splitn(4, '\t');
    let mut next = || fields.next().expect("a result field").to_owned();
    Some(ChildStart {
        pipelines_ms: next().parse().expect("milliseconds"),
        saved: next(),
        software: next() == "true",
        adapter: next(),
    })
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn scratch_directory(salt: u64) -> PathBuf {
    std::env::temp_dir().join(format!("pathon-pipeline-cache-{}-{salt}", std::process::id()))
}

#[test]
fn the_second_start_compiles_the_same_pipelines_faster_from_the_saved_cache() {
    let salt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |elapsed| {
            u64::try_from(elapsed.as_nanos() % 1_000_000).expect("fits")
        });
    let directory = scratch_directory(salt);
    let cache_file = directory.join("pipelines.bin");

    let Some(first) = run_child(&cache_file, salt, &directory.join("driver-first")) else {
        return;
    };
    let second = run_child(&cache_file, salt, &directory.join("driver-second")).expect("second start");
    let control = run_child(&directory.join("no-cache.bin"), salt, &directory.join("driver-control")).expect("control");
    drop(std::fs::remove_dir_all(&directory));

    let speedup = first.pipelines_ms / second.pipelines_ms;
    let against_control = control.pipelines_ms / second.pipelines_ms;
    eprintln!(
        "pipeline cache on {}: {VARIANTS} compute pipelines of {OPERATIONS} operations, each start in a fresh process\n\
         \x20 first start (empty cache):     {:8.1} ms   {}\n\
         \x20 second start (seeded):         {:8.1} ms   {speedup:.1}x faster than the first\n\
         \x20 control (no cache, fresh run): {:8.1} ms   {against_control:.1}x slower than the second",
        first.adapter, first.pipelines_ms, first.saved, second.pipelines_ms, control.pipelines_ms,
    );
    if first.software {
        eprintln!("software driver: it keeps no pipeline cache, so no speed-up is expected");
        return;
    }
    assert_eq!(first.saved, "saved", "the driver put nothing in the cache");
    assert!(
        speedup >= REQUIRED_SPEEDUP,
        "the second start is not measurably faster ({speedup:.2}x: {:.1} ms then {:.1} ms)",
        first.pipelines_ms,
        second.pipelines_ms
    );
    if against_control < REQUIRED_SPEEDUP {
        eprintln!(
            "note: the control is as fast as the seeded start, so a driver-level cache is giving the same gain and \
             the blob's own effect is not isolated on this driver"
        );
    }
}
