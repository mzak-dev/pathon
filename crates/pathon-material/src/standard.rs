use crate::MaterialError;

/// glTF `alphaMode`.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum AlphaMode {
    #[default]
    Opaque,
    Mask {
        cutoff: f32,
    },
    Blend,
}

/// Logical index of a texture; the bindless layer (M4-04) decides how it reaches the shader.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TextureSlot(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct MaterialTextures {
    /// sRGB, multiplied with `base_color`.
    pub base_color: Option<TextureSlot>,
    /// glTF packing: green is roughness, blue is metallic.
    pub metallic_roughness: Option<TextureSlot>,
    pub normal: Option<TextureSlot>,
    /// Red channel.
    pub occlusion: Option<TextureSlot>,
    /// sRGB, multiplied with `emissive`.
    pub emissive: Option<TextureSlot>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum DiffuseModel {
    #[default]
    Lambert,
    Burley,
}

/// glTF metallic-roughness with the KHR extensions listed in ADR-0030; the defaults are the glTF defaults.
#[derive(Clone, PartialEq, Debug)]
pub struct StandardMaterial {
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub emissive: [f32; 3],
    /// `KHR_materials_emissive_strength`.
    pub emissive_strength: f32,
    pub alpha_mode: AlphaMode,
    pub double_sided: bool,
    pub diffuse_model: DiffuseModel,
    /// `KHR_materials_ior`.
    pub ior: f32,
    /// `KHR_materials_specular`.
    pub specular_factor: f32,
    pub specular_color: [f32; 3],
    /// `KHR_materials_clearcoat`, without a separate normal map.
    pub clearcoat: f32,
    pub clearcoat_roughness: f32,
    /// `KHR_materials_transmission`.
    pub transmission: f32,
    /// `KHR_materials_volume`; `None` is an infinite attenuation distance.
    pub thickness: f32,
    pub attenuation_color: [f32; 3],
    pub attenuation_distance: Option<f32>,
    /// `KHR_materials_diffuse_transmission`, the `subsurface` shader flag.
    pub diffuse_transmission: f32,
    pub diffuse_transmission_color: [f32; 3],
    pub textures: MaterialTextures,
}

impl Default for StandardMaterial {
    fn default() -> Self {
        Self {
            base_color: [1.0; 4],
            metallic: 1.0,
            roughness: 1.0,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            emissive_strength: 1.0,
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
            diffuse_model: DiffuseModel::Lambert,
            ior: 1.5,
            specular_factor: 1.0,
            specular_color: [1.0; 3],
            clearcoat: 0.0,
            clearcoat_roughness: 0.0,
            transmission: 0.0,
            thickness: 0.0,
            attenuation_color: [1.0; 3],
            attenuation_distance: None,
            diffuse_transmission: 0.0,
            diffuse_transmission_color: [1.0; 3],
            textures: MaterialTextures::default(),
        }
    }
}

fn invalid(parameter: &'static str, reason: &str) -> MaterialError {
    MaterialError::InvalidParameter {
        parameter,
        reason: reason.to_owned(),
    }
}

fn unit(parameter: &'static str, value: f32) -> Result<(), MaterialError> {
    if (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(invalid(parameter, &format!("{value} is outside 0..=1")))
    }
}

fn unit_color(parameter: &'static str, color: &[f32]) -> Result<(), MaterialError> {
    color.iter().try_for_each(|channel| unit(parameter, *channel))
}

fn non_negative(parameter: &'static str, value: f32) -> Result<(), MaterialError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(invalid(
            parameter,
            &format!("{value} is not a finite number of 0 or more"),
        ))
    }
}

impl StandardMaterial {
    /// Checks the ranges glTF defines; NaN fails every check.
    pub fn validate(&self) -> Result<(), MaterialError> {
        unit_color("base_color", &self.base_color)?;
        unit("metallic", self.metallic)?;
        unit("roughness", self.roughness)?;
        unit("occlusion_strength", self.occlusion_strength)?;
        unit("specular_factor", self.specular_factor)?;
        unit_color("specular_color", &self.specular_color)?;
        unit("clearcoat", self.clearcoat)?;
        unit("clearcoat_roughness", self.clearcoat_roughness)?;
        unit("transmission", self.transmission)?;
        unit_color("attenuation_color", &self.attenuation_color)?;
        unit("diffuse_transmission", self.diffuse_transmission)?;
        unit_color("diffuse_transmission_color", &self.diffuse_transmission_color)?;
        non_negative("emissive_strength", self.emissive_strength)?;
        self.emissive
            .iter()
            .try_for_each(|channel| non_negative("emissive", *channel))?;
        non_negative("thickness", self.thickness)?;
        if !self.normal_scale.is_finite() {
            return Err(invalid("normal_scale", "not a finite number"));
        }
        if !(self.ior.is_finite() && self.ior >= 1.0) {
            return Err(invalid("ior", &format!("{} is below 1 or not finite", self.ior)));
        }
        if let AlphaMode::Mask { cutoff } = self.alpha_mode {
            unit("alpha_cutoff", cutoff)?;
        }
        if let Some(distance) = self.attenuation_distance
            && !(distance.is_finite() && distance > 0.0)
        {
            return Err(invalid("attenuation_distance", "must be positive and finite, or None"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_material_is_valid() {
        assert_eq!(StandardMaterial::default().validate(), Ok(()));
    }

    #[test]
    fn out_of_range_and_nan_values_name_their_parameter() {
        for (material, expected) in [
            (
                StandardMaterial {
                    roughness: 1.5,
                    ..StandardMaterial::default()
                },
                "roughness",
            ),
            (
                StandardMaterial {
                    metallic: f32::NAN,
                    ..StandardMaterial::default()
                },
                "metallic",
            ),
            (
                StandardMaterial {
                    ior: 0.5,
                    ..StandardMaterial::default()
                },
                "ior",
            ),
            (
                StandardMaterial {
                    attenuation_distance: Some(0.0),
                    ..StandardMaterial::default()
                },
                "attenuation_distance",
            ),
            (
                StandardMaterial {
                    alpha_mode: AlphaMode::Mask { cutoff: 2.0 },
                    ..StandardMaterial::default()
                },
                "alpha_cutoff",
            ),
        ] {
            let Err(MaterialError::InvalidParameter { parameter, .. }) = material.validate() else {
                panic!("{expected} must be rejected");
            };
            assert_eq!(parameter, expected);
        }
    }
}
