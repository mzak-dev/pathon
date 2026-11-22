#![doc = include_str!("../../../docs/crates/pathon-shader.md")]

mod background;
mod features;
mod library;
mod pipeline_cache;
mod reflect;

use std::collections::BTreeMap;

pub use background::{BackgroundCompiler, BackgroundConfig, BuildError, BuildFailure, Lookup, MissPolicy, PumpReport};
pub use features::ShaderFeatures;
pub use library::{ShaderKey, ShaderLibrary};
use pathon_rhi_types::RhiError;
pub use pipeline_cache::PipelineCache;
pub use reflect::reflect_bind_groups;
use wesl::resolver::VirtualResolver;
use wesl::syntax::ModulePath;
use wesl::{Compiler, Error as WeslError, Feature, Features};

const RESERVED_SEGMENTS: &[&str] = &[
    "_",
    "alias",
    "as",
    "break",
    "case",
    "const",
    "const_assert",
    "continue",
    "continuing",
    "default",
    "diagnostic",
    "discard",
    "else",
    "enable",
    "false",
    "fn",
    "for",
    "if",
    "import",
    "let",
    "loop",
    "override",
    "package",
    "requires",
    "return",
    "self",
    "struct",
    "super",
    "switch",
    "true",
    "var",
    "while",
];

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ShaderError {
    #[error("invalid module path `{path}`: {message}")]
    InvalidModulePath { path: String, message: String },
    #[error("`{name}` is not a WGSL identifier, so it cannot be a shader flag")]
    InvalidFeatureName { name: String },
    #[error("no shader is registered under `{name}`")]
    UnknownRoot { name: String },
    #[error("creating the shader module failed: {0}")]
    Rhi(#[source] RhiError),
    #[error("{file_path}: {message}")]
    Compose { file_path: String, message: String },
    #[error("invalid WGSL: {message}")]
    InvalidWgsl { message: String },
    #[error("unknown entry point `{name}`")]
    UnknownEntryPoint { name: String },
    #[error("entry point `{entry_point}` has a stage that bind group layouts cannot describe")]
    UnsupportedStage { entry_point: String },
    #[error("@group({group}) @binding({binding}): {reason}")]
    UnsupportedBinding { group: u32, binding: u32, reason: String },
    #[error("@group({group}) @binding({binding}) is declared as different resources by the given entry points")]
    BindingConflict { group: u32, binding: u32 },
}

#[derive(Clone, Debug)]
struct Module {
    path: ModulePath,
    source: String,
}

#[derive(Clone, Debug, Default)]
pub struct ShaderComposer {
    modules: BTreeMap<String, Module>,
}

impl ShaderComposer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `source` as `package::<path>`; a module added again under the same path replaces the earlier one.
    pub fn add_module(&mut self, path: &str, source: &str) -> Result<(), ShaderError> {
        let module = Module {
            path: module_path(path)?,
            source: without_byte_order_mark(source).to_owned(),
        };
        self.modules.insert(path.to_owned(), module);
        Ok(())
    }

    /// Resolves the imports of `source` against the current modules with every flag disabled; `file_path` only labels
    /// errors.
    pub fn compose(&self, source: &str, file_path: &str) -> Result<String, ShaderError> {
        self.compose_with(source, file_path, &ShaderFeatures::default())
    }

    /// Like [`Self::compose`], with the flags of `features` enabled for `@if` in the shader and its modules.
    pub fn compose_with(
        &self,
        source: &str,
        file_path: &str,
        features: &ShaderFeatures,
    ) -> Result<String, ShaderError> {
        let mut resolver = VirtualResolver::new();
        resolver.add_module(ModulePath::new_root(), without_byte_order_mark(source).into());
        for module in self.modules.values() {
            resolver.add_module(module.path.clone(), module.source.as_str().into());
        }
        let mut compiler = Compiler::default().with_resolver(resolver);
        // Imported declarations nothing reaches are dropped; the main shader is kept whole, because
        // pipelines address its overrides by name even when no entry point reads them.
        compiler.options.strip = true;
        compiler.options.keep_main = true;
        compiler.options.features = Features {
            default: Feature::Disable,
            flags: features.iter().map(|name| (name.to_owned(), Feature::Enable)).collect(),
        };
        compiler
            .compile_root()
            .map(|result| result.to_string())
            .map_err(|error| ShaderError::Compose {
                file_path: file_path.to_owned(),
                message: plain_message(error),
            })
    }
}

// wesl decides on ANSI colours from the terminal and the environment, which must not leak into messages.
fn plain_message(error: WeslError) -> String {
    match error {
        WeslError::Error(diagnostic) => diagnostic.colored(false).to_string(),
        other => other.to_string(),
    }
}

fn without_byte_order_mark(source: &str) -> &str {
    source.strip_prefix('\u{feff}').unwrap_or(source)
}

fn is_identifier(segment: &str) -> bool {
    let mut characters = segment.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn module_path(path: &str) -> Result<ModulePath, ShaderError> {
    let invalid = |message: String| ShaderError::InvalidModulePath {
        path: path.to_owned(),
        message,
    };
    if !path.split("::").all(is_identifier) {
        return Err(invalid("expected identifiers separated by `::`".to_owned()));
    }
    if let Some(keyword) = path.split("::").find(|segment| RESERVED_SEGMENTS.contains(segment)) {
        return Err(invalid(format!("`{keyword}` is a reserved word")));
    }
    format!("package::{path}")
        .parse::<ModulePath>()
        .map_err(|error| invalid(error.to_string()))
}
