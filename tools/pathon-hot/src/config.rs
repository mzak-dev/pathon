use std::path::PathBuf;
use std::time::Duration;

/// What the files of a watched directory are.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum DirRole {
    /// Every file is a shader root, named by its path under the directory without the extension (`fx/blur.wgsl` is
    /// `fx/blur`).
    Shaders,
    /// Every file is a module other shaders import, registered under its path with `::` in place of `/`
    /// (`lighting/pbr.wgsl` is `package::lighting::pbr`).
    Modules,
}

/// What a source file may be before hot reload refuses it. The defaults keep wesl, which walks the syntax tree
/// recursively, far from overflowing a 1 MiB stack in a debug build (`docs/crates/pathon-hot.md`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct SourceLimits {
    pub max_file_bytes: u64,
    /// Depth of nested `(`, `{` and `[`.
    pub max_nesting_depth: usize,
    /// Binary operators in one statement, which wesl nests left to right.
    pub max_operators_per_statement: usize,
    /// Modules importing modules importing modules.
    pub max_import_depth: usize,
}

impl SourceLimits {
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_file_bytes: 1 << 20,
            max_nesting_depth: 32,
            max_operators_per_statement: 128,
            max_import_depth: 32,
        }
    }

    #[must_use]
    pub fn with_max_file_bytes(self, max_file_bytes: u64) -> Self {
        Self { max_file_bytes, ..self }
    }

    #[must_use]
    pub fn with_max_nesting_depth(self, max_nesting_depth: usize) -> Self {
        Self {
            max_nesting_depth,
            ..self
        }
    }

    #[must_use]
    pub fn with_max_operators_per_statement(self, max_operators_per_statement: usize) -> Self {
        Self {
            max_operators_per_statement,
            ..self
        }
    }

    #[must_use]
    pub fn with_max_import_depth(self, max_import_depth: usize) -> Self {
        Self {
            max_import_depth,
            ..self
        }
    }
}

impl Default for SourceLimits {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct HotConfig {
    pub dirs: Vec<(PathBuf, DirRole)>,
    /// Quiet time after the last file event before the directories are read again; editors save in several steps.
    pub debounce: Duration,
    pub limits: SourceLimits,
    /// Pumps with nothing pending before the replaced modules and pipelines are handed back for destruction.
    pub settle_pumps: u32,
    /// How often the polling watcher looks, when the system's own file notification is unavailable.
    pub poll_interval: Duration,
}

impl HotConfig {
    #[must_use]
    pub fn new() -> Self {
        Self {
            dirs: Vec::new(),
            debounce: Duration::from_millis(60),
            limits: SourceLimits::new(),
            settle_pumps: 2,
            poll_interval: Duration::from_millis(250),
        }
    }

    /// Watches `dir`, whose files are shader roots.
    #[must_use]
    pub fn with_shader_dir(self, dir: impl Into<PathBuf>) -> Self {
        self.with_dir(dir, DirRole::Shaders)
    }

    /// Watches `dir`, whose files are modules shaders import.
    #[must_use]
    pub fn with_module_dir(self, dir: impl Into<PathBuf>) -> Self {
        self.with_dir(dir, DirRole::Modules)
    }

    #[must_use]
    pub fn with_dir(mut self, dir: impl Into<PathBuf>, role: DirRole) -> Self {
        self.dirs.push((dir.into(), role));
        self
    }

    #[must_use]
    pub fn with_debounce(self, debounce: Duration) -> Self {
        Self { debounce, ..self }
    }

    #[must_use]
    pub fn with_limits(self, limits: SourceLimits) -> Self {
        Self { limits, ..self }
    }

    #[must_use]
    pub fn with_settle_pumps(self, settle_pumps: u32) -> Self {
        Self { settle_pumps, ..self }
    }

    #[must_use]
    pub fn with_poll_interval(self, poll_interval: Duration) -> Self {
        Self { poll_interval, ..self }
    }
}

impl Default for HotConfig {
    fn default() -> Self {
        Self::new()
    }
}
