use pathon_shader::{ShaderFeatures, ShaderKey};

use crate::standard::{AlphaMode, DiffuseModel, StandardMaterial};

/// WESL flags of the standard material shader (`@if(...)` in `package::material::standard`).
pub mod flags {
    pub const MASKED: &str = "masked";
    pub const TRANSPARENT: &str = "transparent";
    pub const SUBSURFACE: &str = "subsurface";
    pub const TRANSMISSION: &str = "transmission";
    pub const CLEARCOAT: &str = "clearcoat";
    pub const BURLEY: &str = "burley";
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum AlphaKind {
    #[default]
    Opaque,
    Masked,
    Transparent,
}

/// The shader permutation a material needs: which optional code its pixels run. Opaque is the absence of `masked` and
/// `transparent`; those two exclude each other, so there are 3 * 2^4 = 48 variants.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one independent WESL flag; an enum per flag would only rename it"
)]
pub struct MaterialVariant {
    pub alpha: AlphaKind,
    pub subsurface: bool,
    pub transmission: bool,
    pub clearcoat: bool,
    pub burley_diffuse: bool,
}

impl MaterialVariant {
    pub const COUNT: usize = 48;

    pub fn of(material: &StandardMaterial) -> Self {
        Self {
            alpha: match material.alpha_mode {
                AlphaMode::Opaque => AlphaKind::Opaque,
                AlphaMode::Mask { .. } => AlphaKind::Masked,
                AlphaMode::Blend => AlphaKind::Transparent,
            },
            subsurface: material.diffuse_transmission > 0.0,
            transmission: material.transmission > 0.0,
            clearcoat: material.clearcoat > 0.0,
            burley_diffuse: material.diffuse_model == DiffuseModel::Burley,
        }
    }

    /// Every variant, for warm-up and for tests that compile all permutations.
    pub fn all() -> impl Iterator<Item = Self> {
        (0..Self::COUNT).map(|index| {
            let extensions = index / 3;
            Self {
                alpha: [AlphaKind::Opaque, AlphaKind::Masked, AlphaKind::Transparent][index % 3],
                subsurface: extensions & 1 != 0,
                transmission: extensions & 2 != 0,
                clearcoat: extensions & 4 != 0,
                burley_diffuse: extensions & 8 != 0,
            }
        })
    }

    /// The variant that draws everything either of the two draws; `None` when alpha kind or diffuse model differ,
    /// because those need separate pipelines (blend state, sorting).
    pub fn merge(self, other: Self) -> Option<Self> {
        (self.alpha == other.alpha && self.burley_diffuse == other.burley_diffuse).then_some(Self {
            alpha: self.alpha,
            subsurface: self.subsurface || other.subsurface,
            transmission: self.transmission || other.transmission,
            clearcoat: self.clearcoat || other.clearcoat,
            burley_diffuse: self.burley_diffuse,
        })
    }

    /// Whether a pipeline built for `self` draws a material that needs `needed` correctly.
    pub fn covers(self, needed: Self) -> bool {
        self.merge(needed) == Some(self)
    }

    pub fn features(self) -> ShaderFeatures {
        let enabled = [
            (flags::MASKED, self.alpha == AlphaKind::Masked),
            (flags::TRANSPARENT, self.alpha == AlphaKind::Transparent),
            (flags::SUBSURFACE, self.subsurface),
            (flags::TRANSMISSION, self.transmission),
            (flags::CLEARCOAT, self.clearcoat),
            (flags::BURLEY, self.burley_diffuse),
        ];
        enabled
            .into_iter()
            .filter(|(_, on)| *on)
            .fold(ShaderFeatures::default(), |features, (name, _)| {
                features.with(name).expect("material flag names are WGSL identifiers")
            })
    }

    /// Key for `root`, the shader that imports `package::material::standard`.
    pub fn shader_key(self, root: &str) -> ShaderKey {
        ShaderKey {
            root: root.to_owned(),
            features: self.features(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn all_lists_forty_eight_distinct_variants_with_distinct_keys() {
        let variants: Vec<_> = MaterialVariant::all().collect();

        assert_eq!(variants.len(), MaterialVariant::COUNT);
        assert_eq!(variants.iter().collect::<HashSet<_>>().len(), MaterialVariant::COUNT);
        let keys: HashSet<_> = variants.iter().map(|variant| variant.shader_key("lit")).collect();
        assert_eq!(keys.len(), MaterialVariant::COUNT);
    }

    #[test]
    fn a_plain_opaque_material_enables_no_flag() {
        let variant = MaterialVariant::of(&StandardMaterial::default());

        assert_eq!(variant.features().iter().count(), 0);
    }

    #[test]
    fn extensions_a_material_uses_become_flags() {
        let variant = MaterialVariant::of(&StandardMaterial {
            alpha_mode: AlphaMode::Blend,
            clearcoat: 1.0,
            transmission: 0.5,
            ..StandardMaterial::default()
        });

        let flags: Vec<_> = variant.features().iter().map(str::to_owned).collect();
        assert_eq!(flags, ["clearcoat", "transmission", "transparent"]);
    }

    #[test]
    fn merging_unions_extensions_but_not_alpha_kinds() {
        let coated = MaterialVariant {
            clearcoat: true,
            ..MaterialVariant::default()
        };
        let transmissive = MaterialVariant {
            transmission: true,
            ..MaterialVariant::default()
        };
        let masked = MaterialVariant {
            alpha: AlphaKind::Masked,
            ..MaterialVariant::default()
        };

        let both = coated.merge(transmissive).expect("same alpha kind");
        assert!(both.clearcoat && both.transmission);
        assert!(both.covers(coated) && both.covers(transmissive));
        assert!(!coated.covers(transmissive));
        assert_eq!(coated.merge(masked), None);
    }
}
