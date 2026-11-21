#![doc = include_str!("../../../docs/crates/pathon-shader.md")]

use naga::back::wgsl::WriterFlags;
use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga_oil::compose::{ComposableModuleDescriptor, Composer, NagaModuleDescriptor};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ShaderError {
    /// Message comes from `naga_oil` and already carries the file name and line.
    #[error("{message}")]
    Compose { message: String },
    #[error("cannot write composed WGSL of {file_path}: {message}")]
    WriteWgsl { file_path: String, message: String },
}

#[derive(Debug, Default)]
pub struct ShaderComposer {
    composer: Composer,
}

impl ShaderComposer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a module declaring `#define_import_path`. Everything it imports must already be added.
    /// A module added again under the same import path replaces the earlier one for later compositions.
    pub fn add_module(&mut self, source: &str, file_path: &str) -> Result<(), ShaderError> {
        self.composer
            .add_composable_module(ComposableModuleDescriptor {
                source,
                file_path,
                ..Default::default()
            })
            .map(|_| ())
            .map_err(|error| ShaderError::Compose {
                message: error.emit_to_string(&self.composer),
            })
    }

    /// Resolves the imports of `source` and returns self-contained WGSL.
    pub fn compose(&mut self, source: &str, file_path: &str) -> Result<String, ShaderError> {
        let module = self
            .composer
            .make_naga_module(NagaModuleDescriptor {
                source,
                file_path,
                ..Default::default()
            })
            .map_err(|error| ShaderError::Compose {
                message: error.emit_to_string(&self.composer),
            })?;
        write_wgsl(&module, file_path)
    }
}

fn write_wgsl(module: &naga::Module, file_path: &str) -> Result<String, ShaderError> {
    let write_error = |message: String| ShaderError::WriteWgsl {
        file_path: file_path.to_owned(),
        message,
    };
    // The writer needs validation info; the composer's own validation does not expose it.
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(module)
        .map_err(|error| write_error(error.to_string()))?;
    naga::back::wgsl::write_string(module, &info, WriterFlags::empty()).map_err(|error| write_error(error.to_string()))
}
