#![doc = include_str!("../../../docs/crates/pathon-shader.md")]

use std::collections::BTreeMap;

use wesl::Compiler;
use wesl::resolver::VirtualResolver;
use wesl::syntax::ModulePath;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ShaderError {
    #[error("invalid module path `{path}`: {message}")]
    InvalidModulePath { path: String, message: String },
    #[error("{file_path}: {message}")]
    Compose { file_path: String, message: String },
}

#[derive(Debug, Default)]
pub struct ShaderComposer {
    modules: BTreeMap<String, String>,
}

impl ShaderComposer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `source` as `package::<path>`; a module added again under the same path replaces the earlier one.
    pub fn add_module(&mut self, path: &str, source: &str) -> Result<(), ShaderError> {
        module_path(path)?;
        self.modules.insert(path.to_owned(), source.to_owned());
        Ok(())
    }

    /// Resolves the imports of `source` against the current modules; `file_path` only labels errors.
    pub fn compose(&self, source: &str, file_path: &str) -> Result<String, ShaderError> {
        let compose_error = |message: String| ShaderError::Compose {
            file_path: file_path.to_owned(),
            message,
        };
        let mut resolver = VirtualResolver::new();
        resolver.add_module(ModulePath::new_root(), source.into());
        for (path, module_source) in &self.modules {
            resolver.add_module(module_path(path)?, module_source.as_str().into());
        }
        let mut compiler = Compiler::default().with_resolver(resolver);
        // Stripping would drop overrides the main shader declares but does not read, which pipelines address by name.
        compiler.options.strip = false;
        compiler
            .compile_root()
            .map(|result| result.to_string())
            .map_err(|error| compose_error(error.to_string()))
    }
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
    format!("package::{path}")
        .parse::<ModulePath>()
        .map_err(|error| invalid(error.to_string()))
}
