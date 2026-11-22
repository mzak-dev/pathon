#![doc = include_str!("../../../docs/crates/pathon-shader.md")]

mod background;
mod diagnostic;
mod features;
mod issues;
mod library;
mod locate;
mod overrides;
mod pipeline_cache;
mod reflect;
mod text;

use std::collections::BTreeMap;
use std::sync::Arc;

pub use background::{BackgroundCompiler, BackgroundConfig, BuildError, BuildFailure, Lookup, MissPolicy, PumpReport};
pub use diagnostic::{ShaderDiagnostic, SourceLocation};
pub use features::ShaderFeatures;
pub use library::{ShaderKey, ShaderLibrary};
pub use overrides::check_pipeline_constants;
use pathon_rhi_types::RhiError;
pub use pipeline_cache::PipelineCache;
pub use reflect::reflect_bind_groups;
use wesl::resolver::VirtualResolver;
use wesl::syntax::ModulePath;
use wesl::{Compiler, Feature, Features};

use crate::locate::{SourceFile, Sources};

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
    /// Composition failed: a syntax error, an import that does not resolve, a cycle (S0101 to S0105, S0111).
    #[error("{diagnostic}")]
    Compose {
        file_path: String,
        message: String,
        diagnostic: Box<ShaderDiagnostic>,
    },
    /// naga rejected the WGSL: unknown names, types, bindings (S0101, S0107 to S0109).
    #[error("{diagnostic}")]
    InvalidWgsl {
        message: String,
        diagnostic: Box<ShaderDiagnostic>,
    },
    /// A pipeline description that does not fit its shader (S0110).
    #[error("{0}")]
    Diagnostic(Box<ShaderDiagnostic>),
    #[error("unknown entry point `{name}`")]
    UnknownEntryPoint { name: String },
    #[error("entry point `{entry_point}` has a stage that bind group layouts cannot describe")]
    UnsupportedStage { entry_point: String },
    #[error("@group({group}) @binding({binding}): {reason}")]
    UnsupportedBinding { group: u32, binding: u32, reason: String },
    #[error("@group({group}) @binding({binding}) is declared as different resources by the given entry points")]
    BindingConflict { group: u32, binding: u32 },
}

impl ShaderError {
    /// The located diagnostic of an error that has one.
    #[must_use]
    pub fn diagnostic(&self) -> Option<&ShaderDiagnostic> {
        match self {
            Self::Compose { diagnostic, .. } | Self::InvalidWgsl { diagnostic, .. } | Self::Diagnostic(diagnostic) => {
                Some(diagnostic)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct Module {
    path: ModulePath,
    source: String,
    file: Option<String>,
}

/// A composed shader that naga accepts, with what composition noticed on the way.
#[derive(Clone, Debug)]
pub(crate) struct Composed {
    pub(crate) wgsl: Arc<str>,
    pub(crate) warnings: Vec<ShaderDiagnostic>,
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
        self.insert_module(path, source, None)
    }

    /// Like [`Self::add_module`], with the file the source came from, which diagnostics show instead of the module path.
    pub fn add_module_file(&mut self, path: &str, source: &str, file: &str) -> Result<(), ShaderError> {
        self.insert_module(path, source, Some(file))
    }

    /// Forgets the module registered under `path`, if any.
    pub fn remove_module(&mut self, path: &str) {
        self.modules.remove(path);
    }

    fn insert_module(&mut self, path: &str, source: &str, file: Option<&str>) -> Result<(), ShaderError> {
        let module = Module {
            path: module_path(path)?,
            source: without_byte_order_mark(source).to_owned(),
            file: file.map(str::to_owned),
        };
        self.modules.insert(path.to_owned(), module);
        Ok(())
    }

    /// Resolves the imports of `source` against the current modules with every flag disabled; `file_path` labels the
    /// shader in diagnostics.
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
        let source = without_byte_order_mark(source);
        self.compile(source, file_path, features)
            .map_err(|diagnostic| compose_error(file_path, diagnostic))
    }

    /// [`Self::compose_with`] followed by naga's parsing and validation, so a mistake in a type, a name or a binding
    /// is reported at its line in the file that holds it rather than at a line of the composed text. Warnings (S0106)
    /// travel with the result.
    pub(crate) fn compose_checked(
        &self,
        source: &str,
        file_path: &str,
        features: &ShaderFeatures,
    ) -> Result<Composed, ShaderError> {
        let source = without_byte_order_mark(source);
        let wgsl = self
            .compile(source, file_path, features)
            .map_err(|diagnostic| compose_error(file_path, diagnostic))?;
        let sources = self.sources(source, file_path);
        if let Err(issue) = issues::parse_and_validate(&wgsl) {
            let diagnostic = Box::new(issues::naga_diagnostic(&issue, &wgsl, Some(&sources)));
            let diagnostic = name_composed_shader(diagnostic, file_path);
            return Err(ShaderError::InvalidWgsl {
                message: diagnostic.message.clone(),
                diagnostic,
            });
        }
        Ok(Composed {
            warnings: features::flag_warnings(&sources, features),
            wgsl: Arc::from(wgsl),
        })
    }

    fn compile(
        &self,
        source: &str,
        file_path: &str,
        features: &ShaderFeatures,
    ) -> Result<String, Box<ShaderDiagnostic>> {
        let mut resolver = VirtualResolver::new();
        resolver.add_module(ModulePath::new_root(), source.into());
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
            .map_err(|error| Box::new(issues::wesl_diagnostic(&error, &self.sources(source, file_path))))
    }

    fn sources<'a>(&'a self, root: &'a str, file_path: &'a str) -> Sources<'a> {
        Sources {
            root: SourceFile {
                module: file_path.to_owned(),
                file: file_path.to_owned(),
                source: root,
            },
            modules: self
                .modules
                .iter()
                .map(|(path, module)| {
                    let module_name = format!("package::{path}");
                    let file = SourceFile {
                        file: module.file.clone().unwrap_or_else(|| module_name.clone()),
                        module: module_name,
                        source: module.source.as_str(),
                    };
                    (path.clone(), file)
                })
                .collect(),
        }
    }
}

// A problem in a module says which shader pulled the module in; a problem in the shader itself already names it.
fn name_composed_shader(mut diagnostic: Box<ShaderDiagnostic>, file_path: &str) -> Box<ShaderDiagnostic> {
    let in_the_shader = diagnostic
        .location
        .as_ref()
        .is_some_and(|location| location.file == file_path);
    if !in_the_shader {
        diagnostic.notes.push(format!("while composing `{file_path}`"));
    }
    diagnostic
}

fn compose_error(file_path: &str, diagnostic: Box<ShaderDiagnostic>) -> ShaderError {
    let diagnostic = name_composed_shader(diagnostic, file_path);
    ShaderError::Compose {
        file_path: file_path.to_owned(),
        message: diagnostic.message.clone(),
        diagnostic,
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
