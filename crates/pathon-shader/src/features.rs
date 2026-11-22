use std::collections::BTreeSet;

use crate::{ShaderError, is_identifier};

/// The set of enabled WESL flags; a disabled flag is simply absent, so equal permutations compare equal.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct ShaderFeatures {
    enabled: BTreeSet<String>,
}

impl ShaderFeatures {
    pub fn with(mut self, name: &str) -> Result<Self, ShaderError> {
        self.set(name, true)?;
        Ok(self)
    }

    pub fn set(&mut self, name: &str, enabled: bool) -> Result<(), ShaderError> {
        if !is_identifier(name) {
            return Err(ShaderError::InvalidFeatureName { name: name.to_owned() });
        }
        if enabled {
            self.enabled.insert(name.to_owned());
        } else {
            self.enabled.remove(name);
        }
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.enabled.iter().map(String::as_str)
    }
}
