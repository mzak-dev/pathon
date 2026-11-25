#![expect(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use std::{fs, thread};

use pathon_api::SerialTaskPool;
use pathon_hot::{HotConfig, HotReloader, HotReport};
use pathon_rhi::{Device, DeviceApi};
use pathon_shader::{BackgroundCompiler, BackgroundConfig, ShaderFeatures, ShaderKey};

// A directory of its own under the system's temporary one, removed when the test ends.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "pathon-hot-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temp dir");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.path));
    }
}

pub fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir");
    }
    fs::write(path, text).expect("write");
}

// What editors and `git checkout` do: the new text lands under another name and replaces the file in one step.
pub fn write_atomically(path: &Path, text: &str) {
    let staging = path.with_extension("wgsl.tmp");
    fs::write(&staging, text).expect("write staging file");
    fs::rename(&staging, path).expect("replace");
}

pub fn key(root: &str) -> ShaderKey {
    ShaderKey {
        root: root.to_owned(),
        features: ShaderFeatures::default(),
    }
}

pub const WAIT: Duration = Duration::from_secs(20);

pub struct Rig {
    pub dir: TempDir,
    pub device: Arc<Device>,
    pub compiler: BackgroundCompiler,
    pub reloader: HotReloader,
}

impl Rig {
    // `shaders` and `modules` are (relative path without extension, source).
    pub fn new(shaders: &[(&str, &str)], modules: &[(&str, &str)]) -> Self {
        Self::with_config(shaders, modules, |config| config)
    }

    pub fn with_config(
        shaders: &[(&str, &str)],
        modules: &[(&str, &str)],
        configure: impl FnOnce(HotConfig) -> HotConfig,
    ) -> Self {
        Self::on_device(Arc::new(Device::null()), shaders, modules, configure)
    }

    pub fn on_device(
        device: Arc<Device>,
        shaders: &[(&str, &str)],
        modules: &[(&str, &str)],
        configure: impl FnOnce(HotConfig) -> HotConfig,
    ) -> Self {
        let dir = TempDir::new("rig");
        for (name, source) in shaders {
            write(&dir.join(&format!("shaders/{name}.wgsl")), source);
        }
        for (name, source) in modules {
            write(&dir.join(&format!("modules/{name}.wgsl")), source);
        }
        fs::create_dir_all(dir.join("shaders")).expect("shaders dir");
        fs::create_dir_all(dir.join("modules")).expect("modules dir");
        let config = configure(
            HotConfig::new()
                .with_shader_dir(dir.join("shaders"))
                .with_module_dir(dir.join("modules"))
                .with_debounce(Duration::from_millis(30)),
        );
        let handle: Arc<dyn DeviceApi> = Arc::<Device>::clone(&device);
        let compiler = BackgroundCompiler::new(
            handle,
            Arc::new(SerialTaskPool),
            BackgroundConfig::new().with_inline_jobs_per_pump(16),
        );
        let reloader = HotReloader::new(&config).expect("hot reload starts");
        let mut rig = Self {
            dir,
            device,
            compiler,
            reloader,
        };
        rig.pump();
        rig
    }

    pub fn pump(&mut self) -> HotReport {
        self.compiler.pump();
        self.reloader.pump(&mut self.compiler)
    }

    // Pumps, like frames do, until `done` says the reports so far are what the test waits for.
    pub fn pump_until(&mut self, what: &str, mut done: impl FnMut(&[HotReport]) -> bool) -> Vec<HotReport> {
        let deadline = Instant::now() + WAIT;
        let mut reports = Vec::new();
        loop {
            reports.push(self.pump());
            if done(&reports) {
                return reports;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn pump_until_reloaded(&mut self, what: &str) -> Vec<HotReport> {
        self.pump_until(what, |reports| reports.iter().any(HotReport::changed))
    }

    // Asks for the permutation like a pack does every frame, until it is built.
    pub fn build(&mut self, key: &ShaderKey) -> pathon_rhi::ShaderModuleId {
        let deadline = Instant::now() + WAIT;
        loop {
            self.compiler.pump();
            if let Some(module) = self.compiler.shader_module(key).ready() {
                return module;
            }
            assert!(Instant::now() < deadline, "the permutation never finished");
            thread::sleep(Duration::from_millis(2));
        }
    }
}

pub fn codes(reports: &[HotReport]) -> Vec<&'static str> {
    reports
        .iter()
        .flat_map(|report| report.diagnostics.iter().map(|diagnostic| diagnostic.code))
        .collect()
}
