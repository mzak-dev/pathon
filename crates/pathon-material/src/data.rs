use std::mem::{offset_of, size_of};

use crate::standard::{AlphaMode, DiffuseModel, StandardMaterial, TextureSlot};
use crate::{MaterialError, MaterialVariant};

/// Value of a texture slot field that holds no texture.
pub const NO_TEXTURE: u32 = u32::MAX;

/// Bits of [`MaterialData::flags`]; shaders that cannot use a permutation (a ray-tracing hit shader) branch on them.
pub mod flag_bits {
    /// Two bits: 0 opaque, 1 mask, 2 blend.
    pub const ALPHA_MODE_MASK: u32 = 0b11;
    pub const ALPHA_MODE_MASKED: u32 = 1;
    pub const ALPHA_MODE_BLEND: u32 = 2;
    pub const DOUBLE_SIDED: u32 = 1 << 2;
    pub const CLEARCOAT: u32 = 1 << 3;
    pub const TRANSMISSION: u32 = 1 << 4;
    pub const SUBSURFACE: u32 = 1 << 5;
    pub const BURLEY_DIFFUSE: u32 = 1 << 6;
}

/// One element of the material array, laid out like `MaterialData` in `package::material::standard`: 144 bytes, 16
/// aligned, valid as a uniform and as a storage buffer element.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaterialData {
    pub base_color: [f32; 4],
    /// `emissive * emissive_strength`; alpha unused.
    pub emissive: [f32; 4],
    /// `specular_color`, alpha is `specular_factor`.
    pub specular_color: [f32; 4],
    /// `attenuation_color`, alpha is the attenuation distance (0 is infinite).
    pub attenuation_color: [f32; 4],
    /// `diffuse_transmission_color`, alpha is `diffuse_transmission`.
    pub diffuse_transmission_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub ior: f32,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub clearcoat: f32,
    pub clearcoat_roughness: f32,
    pub transmission: f32,
    pub thickness: f32,
    pub alpha_cutoff: f32,
    pub flags: u32,
    pub base_color_texture: u32,
    pub metallic_roughness_texture: u32,
    pub normal_texture: u32,
    pub occlusion_texture: u32,
    pub emissive_texture: u32,
}

const _: () = assert!(size_of::<MaterialData>() == 144);

/// Field names and byte offsets in the order `MaterialData` declares them in WGSL; tests compare it with the reflected
/// shader layout.
pub const MATERIAL_DATA_FIELDS: &[(&str, usize)] = &[
    ("base_color", offset_of!(MaterialData, base_color)),
    ("emissive", offset_of!(MaterialData, emissive)),
    ("specular_color", offset_of!(MaterialData, specular_color)),
    ("attenuation_color", offset_of!(MaterialData, attenuation_color)),
    (
        "diffuse_transmission_color",
        offset_of!(MaterialData, diffuse_transmission_color),
    ),
    ("metallic", offset_of!(MaterialData, metallic)),
    ("roughness", offset_of!(MaterialData, roughness)),
    ("ior", offset_of!(MaterialData, ior)),
    ("normal_scale", offset_of!(MaterialData, normal_scale)),
    ("occlusion_strength", offset_of!(MaterialData, occlusion_strength)),
    ("clearcoat", offset_of!(MaterialData, clearcoat)),
    ("clearcoat_roughness", offset_of!(MaterialData, clearcoat_roughness)),
    ("transmission", offset_of!(MaterialData, transmission)),
    ("thickness", offset_of!(MaterialData, thickness)),
    ("alpha_cutoff", offset_of!(MaterialData, alpha_cutoff)),
    ("flags", offset_of!(MaterialData, flags)),
    ("base_color_texture", offset_of!(MaterialData, base_color_texture)),
    (
        "metallic_roughness_texture",
        offset_of!(MaterialData, metallic_roughness_texture),
    ),
    ("normal_texture", offset_of!(MaterialData, normal_texture)),
    ("occlusion_texture", offset_of!(MaterialData, occlusion_texture)),
    ("emissive_texture", offset_of!(MaterialData, emissive_texture)),
];

fn slot(texture: Option<TextureSlot>) -> u32 {
    texture.map_or(NO_TEXTURE, |slot| slot.0)
}

fn with_alpha(color: [f32; 3], alpha: f32) -> [f32; 4] {
    [color[0], color[1], color[2], alpha]
}

impl MaterialData {
    /// Validates `material` and packs it for the GPU.
    pub fn from_material(material: &StandardMaterial) -> Result<Self, MaterialError> {
        material.validate()?;
        let variant = MaterialVariant::of(material);
        let mut flags = match material.alpha_mode {
            AlphaMode::Opaque => 0,
            AlphaMode::Mask { .. } => flag_bits::ALPHA_MODE_MASKED,
            AlphaMode::Blend => flag_bits::ALPHA_MODE_BLEND,
        };
        for (set, bit) in [
            (material.double_sided, flag_bits::DOUBLE_SIDED),
            (variant.clearcoat, flag_bits::CLEARCOAT),
            (variant.transmission, flag_bits::TRANSMISSION),
            (variant.subsurface, flag_bits::SUBSURFACE),
            (
                material.diffuse_model == DiffuseModel::Burley,
                flag_bits::BURLEY_DIFFUSE,
            ),
        ] {
            if set {
                flags |= bit;
            }
        }
        let [emissive_r, emissive_g, emissive_b] =
            material.emissive.map(|channel| channel * material.emissive_strength);
        Ok(Self {
            base_color: material.base_color,
            emissive: [emissive_r, emissive_g, emissive_b, 0.0],
            specular_color: with_alpha(material.specular_color, material.specular_factor),
            attenuation_color: with_alpha(material.attenuation_color, material.attenuation_distance.unwrap_or(0.0)),
            diffuse_transmission_color: with_alpha(material.diffuse_transmission_color, material.diffuse_transmission),
            metallic: material.metallic,
            roughness: material.roughness,
            ior: material.ior,
            normal_scale: material.normal_scale,
            occlusion_strength: material.occlusion_strength,
            clearcoat: material.clearcoat,
            clearcoat_roughness: material.clearcoat_roughness,
            transmission: material.transmission,
            thickness: material.thickness,
            alpha_cutoff: match material.alpha_mode {
                AlphaMode::Mask { cutoff } => cutoff,
                _ => 0.5,
            },
            flags,
            base_color_texture: slot(material.textures.base_color),
            metallic_roughness_texture: slot(material.textures.metallic_roughness),
            normal_texture: slot(material.textures.normal),
            occlusion_texture: slot(material.textures.occlusion),
            emissive_texture: slot(material.textures.emissive),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_follow_the_documented_std430_layout() {
        let offsets: Vec<usize> = MATERIAL_DATA_FIELDS.iter().map(|(_, offset)| *offset).collect();
        assert_eq!(offsets[..5], [0, 16, 32, 48, 64]);
        assert_eq!(offsets[5], 80);
        assert_eq!(offsets.last(), Some(&140));
        assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn a_default_material_packs_to_glossy_white_without_textures() {
        let data = MaterialData::from_material(&StandardMaterial::default()).expect("valid");

        assert_eq!(data.base_color, [1.0; 4]);
        assert_eq!(data.flags, 0);
        assert_eq!(data.base_color_texture, NO_TEXTURE);
        assert_eq!(data.attenuation_color[3], 0.0);
    }

    #[test]
    fn emissive_strength_is_folded_into_the_colour() {
        let data = MaterialData::from_material(&StandardMaterial {
            emissive: [1.0, 0.5, 0.0],
            emissive_strength: 4.0,
            ..StandardMaterial::default()
        })
        .expect("valid");

        assert_eq!(data.emissive, [4.0, 2.0, 0.0, 0.0]);
    }

    #[test]
    fn flags_follow_the_features_a_material_uses() {
        let data = MaterialData::from_material(&StandardMaterial {
            alpha_mode: AlphaMode::Mask { cutoff: 0.3 },
            double_sided: true,
            clearcoat: 0.5,
            transmission: 0.2,
            diffuse_transmission: 0.1,
            diffuse_model: DiffuseModel::Burley,
            ..StandardMaterial::default()
        })
        .expect("valid");

        let expected = flag_bits::ALPHA_MODE_MASKED
            | flag_bits::DOUBLE_SIDED
            | flag_bits::CLEARCOAT
            | flag_bits::TRANSMISSION
            | flag_bits::SUBSURFACE
            | flag_bits::BURLEY_DIFFUSE;
        assert_eq!(data.flags, expected);
        assert_eq!(data.alpha_cutoff, 0.3);
    }

    #[test]
    fn an_invalid_material_does_not_pack() {
        let material = StandardMaterial {
            roughness: -1.0,
            ..StandardMaterial::default()
        };

        assert!(matches!(
            MaterialData::from_material(&material),
            Err(MaterialError::InvalidParameter { .. })
        ));
    }
}
