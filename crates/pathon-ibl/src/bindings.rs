use std::num::NonZeroU32;

use pathon_api::rhi::{
    BindGroupEntry, BindGroupLayoutEntry, BindingResource, BindingType, SamplerBindingType, ShaderStages,
    TextureSampleType, TextureViewDimension,
};

use crate::IblEnvironment;

/// The four consecutive bindings of one group an environment needs; the ibl modules declare none, so the shader that
/// owns the entry point declares them and passes them to the `ibl_*` functions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IblBindings {
    pub first_binding: u32,
}

impl IblBindings {
    pub const COUNT: u32 = 4;

    pub const fn starting_at(first_binding: u32) -> Self {
        Self { first_binding }
    }

    pub fn wgsl_declarations(self, group: u32) -> String {
        let first = self.first_binding;
        format!(
            "@group({group}) @binding({first}) var ibl_irradiance: texture_cube<f32>;\n\
             @group({group}) @binding({}) var ibl_prefiltered: texture_cube<f32>;\n\
             @group({group}) @binding({}) var ibl_brdf_lut: texture_2d<f32>;\n\
             @group({group}) @binding({}) var ibl_sampler: sampler;\n",
            first + 1,
            first + 2,
            first + 3
        )
    }

    pub fn layout_entries(self, visibility: ShaderStages) -> Vec<BindGroupLayoutEntry> {
        let texture = |offset: u32, dimension| BindGroupLayoutEntry {
            binding: self.first_binding + offset,
            visibility,
            ty: BindingType::SampledTexture {
                sample_type: TextureSampleType::Float,
                dimension,
                multisampled: false,
            },
            count: None::<NonZeroU32>,
        };
        vec![
            texture(0, TextureViewDimension::Cube),
            texture(1, TextureViewDimension::Cube),
            texture(2, TextureViewDimension::D2),
            BindGroupLayoutEntry {
                binding: self.first_binding + 3,
                visibility,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None::<NonZeroU32>,
            },
        ]
    }

    pub fn bind_group_entries(self, environment: &IblEnvironment) -> Vec<BindGroupEntry> {
        let entry = |offset: u32, resource| BindGroupEntry {
            binding: self.first_binding + offset,
            resource,
        };
        vec![
            entry(0, BindingResource::TextureView(environment.irradiance.view)),
            entry(1, BindingResource::TextureView(environment.prefiltered.view)),
            entry(2, BindingResource::TextureView(environment.brdf_lut.view)),
            entry(3, BindingResource::Sampler(environment.sampler)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_follow_the_first_binding() {
        let text = IblBindings::starting_at(5).wgsl_declarations(2);
        assert!(text.contains("@group(2) @binding(5) var ibl_irradiance: texture_cube<f32>;"));
        assert!(text.contains("@group(2) @binding(8) var ibl_sampler: sampler;"));
    }

    #[test]
    fn layout_and_declarations_use_the_same_numbers() {
        let bindings = IblBindings::starting_at(3);
        let numbers: Vec<u32> = bindings
            .layout_entries(ShaderStages::FRAGMENT)
            .iter()
            .map(|entry| entry.binding)
            .collect();
        assert_eq!(numbers, [3, 4, 5, 6]);
    }
}
