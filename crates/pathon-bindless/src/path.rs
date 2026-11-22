use pathon_api::rhi::{Caps, Features};

use crate::BindlessError;
use crate::error::BindlessShortfall;

pub const DEFAULT_CAPACITY: u32 = 1024;
/// Below this a texture array is not worth a path of its own.
pub const MIN_BINDLESS_CAPACITY: u32 = 64;
/// What indexing a texture array with a per-pixel value needs (the Desktop profile has both).
pub const BINDLESS_FEATURES: Features = Features::TEXTURE_BINDING_ARRAY.union(Features::NON_UNIFORM_INDEXING);

/// How the application wants textures to reach shaders.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum PathPreference {
    /// Bindless when the device can, a bind group per material otherwise (`B0001` says why).
    #[default]
    Auto,
    /// Bindless or an error (`B0002`); for tests and tools that must not fall back silently.
    Bindless,
    /// A bind group per material even on a device that could do bindless (to compare the paths, or to match Web).
    PerMaterial,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TexturePath {
    /// One bind group with a `binding_array` of `capacity` textures, indexed by the material's slot.
    Bindless { capacity: u32 },
    /// One bind group per material with up to [`MAX_MATERIAL_TEXTURES`](crate::MAX_MATERIAL_TEXTURES) textures.
    PerMaterial,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PathChoice {
    pub path: TexturePath,
    /// Set when [`PathPreference::Auto`] wanted bindless and the device could not.
    pub shortfall: Option<BindlessShortfall>,
}

/// Picks the path from what the device can do, not from a profile name: a pure function of `Caps`, so tables of
/// fabricated `Caps` test it.
pub fn choose_path(caps: &Caps, preference: PathPreference, capacity: u32) -> Result<PathChoice, BindlessError> {
    if preference == PathPreference::PerMaterial {
        return Ok(PathChoice {
            path: TexturePath::PerMaterial,
            shortfall: None,
        });
    }
    match bindless_capacity(caps, capacity) {
        Ok(capacity) => Ok(PathChoice {
            path: TexturePath::Bindless { capacity },
            shortfall: None,
        }),
        Err(shortfall) if preference == PathPreference::Auto => Ok(PathChoice {
            path: TexturePath::PerMaterial,
            shortfall: Some(shortfall),
        }),
        Err(shortfall) => Err(BindlessError::BindlessUnavailable {
            reason: shortfall.to_string(),
        }),
    }
}

// The array length is clamped to the device limit: the shader is generated for the final length.
fn bindless_capacity(caps: &Caps, requested: u32) -> Result<u32, BindlessShortfall> {
    let missing = caps.missing(BINDLESS_FEATURES);
    if !missing.is_empty() {
        return Err(BindlessShortfall::MissingFeatures(missing));
    }
    let limit = caps.limits.max_binding_array_elements_per_shader_stage;
    let capacity = requested.min(limit);
    if capacity < MIN_BINDLESS_CAPACITY.min(requested) {
        return Err(BindlessShortfall::CapacityAboveLimit { requested, limit });
    }
    Ok(capacity)
}

#[cfg(test)]
mod tests {
    use pathon_api::rhi::{Limits, Profile};

    use super::*;

    fn caps(features: Features, limit: u32) -> Caps {
        let device = pathon_rhi::NullDevice::with_features(features);
        let mut caps = pathon_api::rhi::DeviceApi::caps(&device).clone();
        caps.limits = Limits {
            max_binding_array_elements_per_shader_stage: limit,
            ..caps.limits
        };
        caps
    }

    #[test]
    fn the_profiles_pick_their_path() {
        let web = caps(Profile::Web.features(), 0);
        let desktop = caps(Profile::Desktop.features(), 1 << 20);

        assert_eq!(
            choose_path(&web, PathPreference::Auto, 1024).map(|choice| choice.path),
            Ok(TexturePath::PerMaterial)
        );
        assert_eq!(
            choose_path(&desktop, PathPreference::Auto, 1024).map(|choice| choice.path),
            Ok(TexturePath::Bindless { capacity: 1024 })
        );
    }

    #[test]
    fn web_falls_back_with_the_missing_features_named() {
        let web = caps(Features::empty(), 0);

        let choice = choose_path(&web, PathPreference::Auto, 1024).expect("auto never fails");

        assert_eq!(
            choice.shortfall,
            Some(BindlessShortfall::MissingFeatures(BINDLESS_FEATURES))
        );
    }

    #[test]
    fn one_missing_feature_is_enough_to_fall_back() {
        let half = caps(Features::TEXTURE_BINDING_ARRAY, 1 << 20);

        let choice = choose_path(&half, PathPreference::Auto, 1024).expect("auto never fails");

        assert_eq!(choice.path, TexturePath::PerMaterial);
        assert_eq!(
            choice.shortfall,
            Some(BindlessShortfall::MissingFeatures(Features::NON_UNIFORM_INDEXING))
        );
    }

    #[test]
    fn forced_bindless_without_support_is_b0002() {
        let web = caps(Features::empty(), 0);

        let error = choose_path(&web, PathPreference::Bindless, 1024).expect_err("cannot");

        assert_eq!(error.code(), "B0002");
    }

    #[test]
    fn the_capacity_is_clamped_to_the_device_limit_down_to_a_floor() {
        let features = BINDLESS_FEATURES;

        assert_eq!(
            choose_path(&caps(features, 500), PathPreference::Auto, 1024).map(|choice| choice.path),
            Ok(TexturePath::Bindless { capacity: 500 })
        );
        let tiny = choose_path(&caps(features, 16), PathPreference::Auto, 1024).expect("auto");
        assert_eq!(tiny.path, TexturePath::PerMaterial);
        assert_eq!(
            tiny.shortfall,
            Some(BindlessShortfall::CapacityAboveLimit {
                requested: 1024,
                limit: 16
            })
        );
        // A table that asked for 8 textures is fine with a limit of 16.
        assert_eq!(
            choose_path(&caps(features, 16), PathPreference::Auto, 8).map(|choice| choice.path),
            Ok(TexturePath::Bindless { capacity: 8 })
        );
    }

    #[test]
    fn per_material_can_be_forced_on_a_capable_device() {
        let desktop = caps(Profile::High.features(), 1 << 20);

        let choice = choose_path(&desktop, PathPreference::PerMaterial, 1024).expect("always possible");

        assert_eq!(choice.path, TexturePath::PerMaterial);
        assert_eq!(choice.shortfall, None);
    }
}
