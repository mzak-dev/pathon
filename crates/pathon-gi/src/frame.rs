use std::num::NonZeroU32;

use pathon_api::graph::TextureHandle;
use pathon_api::resources::UniformSlice;
use pathon_api::rhi::{
    Access, BindGroupEntry, BindGroupLayoutEntry, BindingResource, BindingType, SamplerBindingType, SamplerId,
    ShaderStages, TextureSampleType, TextureViewDimension, TextureViewId,
};

use crate::volume::GiVolumeUniforms;

/// How a pass that samples the atlases declares its read.
pub const GI_TEXTURE_ACCESS: Access = Access::SAMPLED;

/// What the GI feature publishes in `Shared` for the lighting pack, every frame the volume is up to date: the volume
/// description and its place in the frame's uniform ring, the sampler for the atlases and the two atlases as graph
/// handles of this frame's graph. Absent when there is no volume (GI off, no geometry); do not keep it across frames.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GiFrame {
    pub uniforms: GiVolumeUniforms,
    pub uniform_buffer: UniformSlice,
    pub sampler: SamplerId,
    /// Mean incoming radiance per probe direction (rgb, scaled so that a uniform radiance `L` reads `L`), with the
    /// fraction of the sky that direction sees in alpha: `Rgba16Float`.
    pub irradiance: TextureHandle,
    /// Mean and mean square of the distance to the nearest surface per probe direction, divided by the longest ray.
    pub visibility: TextureHandle,
    pub probe_count: u32,
}

/// The four consecutive bindings of one group a lighting shader needs: the volume uniform, the two atlases and a
/// filtering clamp sampler. The `gi::sample` module declares none of them (like `ibl::sample`): the shader that owns
/// the entry point declares them with [`GiBindings::wgsl_declarations`] and passes them to `gi_sample`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GiBindings {
    pub first_binding: u32,
}

impl GiBindings {
    pub const COUNT: u32 = 4;

    pub const fn starting_at(first_binding: u32) -> Self {
        Self { first_binding }
    }

    /// Declares `gi_volume`, `gi_irradiance_atlas`, `gi_visibility_atlas` and `gi_atlas_sampler`; the shader imports
    /// `package::gi::common::GiVolume` for the first one's type.
    pub fn wgsl_declarations(self, group: u32) -> String {
        let first = self.first_binding;
        format!(
            "@group({group}) @binding({first}) var<uniform> gi_volume: GiVolume;\n\
             @group({group}) @binding({}) var gi_irradiance_atlas: texture_2d<f32>;\n\
             @group({group}) @binding({}) var gi_visibility_atlas: texture_2d<f32>;\n\
             @group({group}) @binding({}) var gi_atlas_sampler: sampler;\n",
            first + 1,
            first + 2,
            first + 3
        )
    }

    pub fn layout_entries(self, visibility: ShaderStages) -> Vec<BindGroupLayoutEntry> {
        let texture = |offset: u32| BindGroupLayoutEntry {
            binding: self.first_binding + offset,
            visibility,
            ty: BindingType::SampledTexture {
                sample_type: TextureSampleType::Float,
                dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: None::<NonZeroU32>,
        };
        vec![
            BindGroupLayoutEntry {
                binding: self.first_binding,
                visibility,
                ty: BindingType::UniformBuffer,
                count: None::<NonZeroU32>,
            },
            texture(1),
            texture(2),
            BindGroupLayoutEntry {
                binding: self.first_binding + 3,
                visibility,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None::<NonZeroU32>,
            },
        ]
    }

    /// `irradiance` and `visibility` are the views `PassContext::view` returns for the two handles of the frame.
    pub fn bind_group_entries(
        self,
        frame: &GiFrame,
        irradiance: TextureViewId,
        visibility: TextureViewId,
    ) -> Vec<BindGroupEntry> {
        let entry = |offset: u32, resource| BindGroupEntry {
            binding: self.first_binding + offset,
            resource,
        };
        vec![
            entry(0, frame.uniform_buffer.binding()),
            entry(1, BindingResource::TextureView(irradiance)),
            entry(2, BindingResource::TextureView(visibility)),
            entry(3, BindingResource::Sampler(frame.sampler)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_and_layout_use_the_same_bindings() {
        let bindings = GiBindings::starting_at(13);
        let text = bindings.wgsl_declarations(0);

        assert!(text.contains("@group(0) @binding(13) var<uniform> gi_volume: GiVolume;"));
        assert!(text.contains("@group(0) @binding(16) var gi_atlas_sampler: sampler;"));
        let numbers: Vec<u32> = bindings
            .layout_entries(ShaderStages::FRAGMENT)
            .iter()
            .map(|entry| entry.binding)
            .collect();
        assert_eq!(numbers, [13, 14, 15, 16]);
    }
}
