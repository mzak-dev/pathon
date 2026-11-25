// glTF material -> `StandardMaterial` of pathon-material plus the textures it names. Textures stay as image indices and
// UV transforms: how they reach the shader (bind group per material, binding array) is the renderer's business.

use pathon_material::{AlphaMode, StandardMaterial};

use super::{CpuMaterial, TextureRef, TextureRole, UvTransform};

fn uv_transform(transform: Option<gltf::texture::TextureTransform<'_>>) -> UvTransform {
    let Some(transform) = transform else {
        return UvTransform::IDENTITY;
    };
    let [scale_x, scale_y] = transform.scale();
    let (sin, cos) = transform.rotation().sin_cos();
    // KHR_texture_transform: uv' = T * R * S * uv, with the spec's rotation matrix.
    UvTransform {
        matrix: [cos * scale_x, sin * scale_y, -sin * scale_x, cos * scale_y],
        offset: transform.offset(),
    }
}

fn texture_ref(texture: &gltf::Texture<'_>, transform: Option<gltf::texture::TextureTransform<'_>>) -> TextureRef {
    TextureRef {
        image: texture.source().index(),
        transform: uv_transform(transform),
    }
}

fn clamp_unit(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

// Extensions the `gltf` crate has no typed API for are read from the raw JSON.
fn extension_factor(material: &gltf::Material<'_>, extension: &str, key: &str) -> Option<f32> {
    let value = material.extension_value(extension)?.get(key)?.as_f64()?;
    #[expect(clippy::cast_possible_truncation, reason = "material factors are f32 in the engine")]
    Some(value as f32)
}

pub(super) fn read_material(material: &gltf::Material<'_>) -> CpuMaterial {
    let pbr = material.pbr_metallic_roughness();
    let mut standard = StandardMaterial {
        base_color: pbr.base_color_factor().map(clamp_unit),
        metallic: clamp_unit(pbr.metallic_factor()),
        roughness: clamp_unit(pbr.roughness_factor()),
        emissive: material.emissive_factor().map(clamp_unit),
        double_sided: material.double_sided(),
        alpha_mode: match material.alpha_mode() {
            gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
            gltf::material::AlphaMode::Mask => AlphaMode::Mask {
                cutoff: clamp_unit(material.alpha_cutoff().unwrap_or(0.5)),
            },
            gltf::material::AlphaMode::Blend => AlphaMode::Blend,
        },
        ..StandardMaterial::default()
    };
    if let Some(strength) = material.emissive_strength() {
        standard.emissive_strength = strength.max(0.0);
    }
    if let Some(ior) = material.ior() {
        standard.ior = ior.max(1.0);
    }
    if let Some(specular) = material.specular() {
        standard.specular_factor = clamp_unit(specular.specular_factor());
        standard.specular_color = specular.specular_color_factor().map(clamp_unit);
    }
    if let Some(transmission) = material.transmission() {
        standard.transmission = clamp_unit(transmission.transmission_factor());
    }
    if let Some(volume) = material.volume() {
        standard.thickness = volume.thickness_factor().max(0.0);
        standard.attenuation_color = volume.attenuation_color().map(clamp_unit);
        standard.attenuation_distance = Some(volume.attenuation_distance()).filter(|distance| distance.is_finite());
    }
    if let Some(clearcoat) = extension_factor(material, "KHR_materials_clearcoat", "clearcoatFactor") {
        standard.clearcoat = clamp_unit(clearcoat);
        standard.clearcoat_roughness = clamp_unit(
            extension_factor(material, "KHR_materials_clearcoat", "clearcoatRoughnessFactor").unwrap_or(0.0),
        );
    }

    let mut textures: [Option<TextureRef>; TextureRole::COUNT] = Default::default();
    if let Some(info) = pbr.base_color_texture() {
        textures[TextureRole::BaseColor as usize] = Some(texture_ref(&info.texture(), info.texture_transform()));
    }
    if let Some(info) = pbr.metallic_roughness_texture() {
        textures[TextureRole::MetallicRoughness as usize] =
            Some(texture_ref(&info.texture(), info.texture_transform()));
    }
    if let Some(normal) = material.normal_texture() {
        standard.normal_scale = normal.scale();
        textures[TextureRole::Normal as usize] = Some(texture_ref(&normal.texture(), None));
    }
    if let Some(occlusion) = material.occlusion_texture() {
        standard.occlusion_strength = clamp_unit(occlusion.strength());
        textures[TextureRole::Occlusion as usize] = Some(texture_ref(&occlusion.texture(), None));
    }
    // The crate exposes `KHR_texture_transform` only on colour-like infos. Blender writes one mapping node per
    // material, so the normal and occlusion maps borrow the base colour's transform.
    if let Some(base) = textures[TextureRole::BaseColor as usize] {
        for role in [TextureRole::Normal, TextureRole::Occlusion] {
            if let Some(texture) = &mut textures[role as usize] {
                texture.transform = base.transform;
            }
        }
    }
    if let Some(info) = material.emissive_texture() {
        textures[TextureRole::Emissive as usize] = Some(texture_ref(&info.texture(), info.texture_transform()));
    }
    CpuMaterial {
        name: material.name().unwrap_or("material").to_owned(),
        standard,
        textures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_transform_is_the_default() {
        assert_eq!(uv_transform(None), UvTransform::IDENTITY);
    }
}
