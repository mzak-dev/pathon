use pathon_api::Slot;
use pathon_api::glam::Vec3;
use pathon_api::resources::UniformSlice;
use pathon_api::rhi::{
    Access, BindGroupEntry, BindGroupLayoutEntry, BindingResource, BindingType, Format, SamplerBindingType, SamplerId,
    ShaderStages, TextureSampleType, TextureViewDesc, TextureViewDimension, TextureViewId,
};

use crate::cascade::{Cascade, MAX_CASCADES};
use crate::input::DirectionalShadowLight;
use crate::settings::ShadowSettings;

/// How a pass that samples [`SHADOW_MAP`] declares its read: a depth texture is read as `SAMPLED | DEPTH_READ`.
pub const SHADOW_MAP_ACCESS: Access = Access::SAMPLED.union(Access::DEPTH_READ);

/// The depth maps of the cascades, one array layer each: `Depth32Float`, reverse-Z, written by the shadow pack in
/// `Stage::Shadows` and read by the lighting shader through a `D2Array` view and a comparison sampler.
pub const SHADOW_MAP: Slot = Slot::new("shadow.map", Format::Depth32Float);

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct CascadeUniform {
    view_projection: [[f32; 4]; 4],
    // x: metres per texel, y: normal offset in metres.
    params: [f32; 4],
}

/// The bytes of WGSL `shadow::sample::ShadowCascades`, 368 bytes in a uniform buffer.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShadowUniforms {
    cascades: [CascadeUniform; MAX_CASCADES],
    // xyz: the direction the light shines along.
    light_direction: [f32; 4],
    // x: light index, y: cascade count, z: filter, w: flags (bit 0: rotate the filter per pixel).
    counts: [u32; 4],
    // Far end of every cascade as a view distance in metres.
    splits: [f32; 4],
    // x: blend fraction, y: fade fraction, z: map resolution, w: 1 / resolution.
    tuning: [f32; 4],
}

impl ShadowUniforms {
    pub(crate) fn new(
        cascades: &[Cascade],
        direction: Vec3,
        light: DirectionalShadowLight,
        settings: &ShadowSettings,
    ) -> Self {
        let mut uniforms = Self::zeroed_with_defaults();
        let last_far = cascades.last().map_or(0.0, |cascade| cascade.far_depth);
        for (index, slot) in uniforms.cascades.iter_mut().enumerate() {
            if let Some(cascade) = cascades.get(index) {
                slot.view_projection = cascade.view_projection.to_cols_array_2d();
                slot.params = [
                    cascade.texel_world_size,
                    settings.normal_bias * cascade.texel_world_size,
                    0.0,
                    0.0,
                ];
                uniforms.splits[index] = cascade.far_depth;
            } else {
                uniforms.splits[index] = last_far;
            }
        }
        uniforms.light_direction = [direction.x, direction.y, direction.z, 0.0];
        let rotate = u32::from(settings.rotate_filter);
        uniforms.counts = [
            light.light_index,
            u32::try_from(cascades.len()).unwrap_or(0),
            settings.filter.shader_value(),
            rotate,
        ];
        let resolution = settings.resolution as f32;
        uniforms.tuning = [
            settings.blend_fraction,
            settings.fade_fraction,
            resolution,
            1.0 / resolution,
        ];
        uniforms
    }

    fn zeroed_with_defaults() -> Self {
        bytemuck::Zeroable::zeroed()
    }

    pub fn light_index(&self) -> u32 {
        self.counts[0]
    }

    pub fn cascade_count(&self) -> u32 {
        self.counts[1]
    }

    /// World to clip space of a cascade as the shader sees it.
    pub fn view_projection(&self, cascade: usize) -> [[f32; 4]; 4] {
        self.cascades[cascade].view_projection
    }

    /// Far end of a cascade as a view distance.
    pub fn split(&self, cascade: usize) -> f32 {
        self.splits[cascade]
    }

    pub fn texel_world_size(&self, cascade: usize) -> f32 {
        self.cascades[cascade].params[0]
    }
}

/// What the shadow pack publishes in `Shared` for the lighting pack, every frame there is a light: the cascade data,
/// where it sits in the frame's uniform ring, and the comparison sampler for the map. The map itself is the slot
/// [`SHADOW_MAP`]. A new value every frame; do not keep it across frames.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ShadowFrame {
    pub uniforms: ShadowUniforms,
    pub uniform_buffer: UniformSlice,
    pub sampler: SamplerId,
    pub cascade_count: u32,
    pub resolution: u32,
}

/// The three consecutive bindings of one group a lighting shader needs: the cascades (uniform), the depth array and a
/// comparison sampler. The `shadow::sample` module declares none of them (like `ibl::sample`): the shader that owns the
/// entry point declares them with [`ShadowBindings::wgsl_declarations`] and passes them to
/// `shadow_factor_directional`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ShadowBindings {
    pub first_binding: u32,
}

impl ShadowBindings {
    pub const COUNT: u32 = 3;

    pub const fn starting_at(first_binding: u32) -> Self {
        Self { first_binding }
    }

    /// Declares `shadow_cascades`, `shadow_map` and `shadow_sampler`; the shader imports
    /// `package::shadow::sample::ShadowCascades` for the first one's type.
    pub fn wgsl_declarations(self, group: u32) -> String {
        let first = self.first_binding;
        format!(
            "@group({group}) @binding({first}) var<uniform> shadow_cascades: ShadowCascades;\n\
             @group({group}) @binding({}) var shadow_map: texture_depth_2d_array;\n\
             @group({group}) @binding({}) var shadow_sampler: sampler_comparison;\n",
            first + 1,
            first + 2
        )
    }

    pub fn layout_entries(self, visibility: ShaderStages) -> Vec<BindGroupLayoutEntry> {
        vec![
            BindGroupLayoutEntry {
                binding: self.first_binding,
                visibility,
                ty: BindingType::UniformBuffer,
                count: None,
            },
            BindGroupLayoutEntry {
                binding: self.first_binding + 1,
                visibility,
                ty: BindingType::SampledTexture {
                    sample_type: TextureSampleType::Depth,
                    dimension: TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: self.first_binding + 2,
                visibility,
                ty: BindingType::Sampler(SamplerBindingType::Comparison),
                count: None,
            },
        ]
    }

    /// The view of the [`SHADOW_MAP`] slot texture the shader reads: all layers as an array.
    pub fn map_view_desc() -> TextureViewDesc {
        TextureViewDesc {
            dimension: Some(TextureViewDimension::D2Array),
            ..TextureViewDesc::default()
        }
    }

    /// `map_view` is the view `PassContext::view_with(map, &ShadowBindings::map_view_desc())` returns.
    pub fn bind_group_entries(self, frame: &ShadowFrame, map_view: TextureViewId) -> Vec<BindGroupEntry> {
        vec![
            BindGroupEntry {
                binding: self.first_binding,
                resource: frame.uniform_buffer.binding(),
            },
            BindGroupEntry {
                binding: self.first_binding + 1,
                resource: BindingResource::TextureView(map_view),
            },
            BindGroupEntry {
                binding: self.first_binding + 2,
                resource: BindingResource::Sampler(frame.sampler),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniform_block_has_the_size_the_shader_expects() {
        assert_eq!(size_of::<ShadowUniforms>(), 4 * 80 + 4 * 16);
    }

    #[test]
    fn declarations_and_layout_use_the_same_bindings() {
        let bindings = ShadowBindings::starting_at(4);
        let text = bindings.wgsl_declarations(1);

        assert!(text.contains("@group(1) @binding(4) var<uniform> shadow_cascades: ShadowCascades;"));
        assert!(text.contains("@group(1) @binding(6) var shadow_sampler: sampler_comparison;"));
        let numbers: Vec<u32> = bindings
            .layout_entries(ShaderStages::FRAGMENT)
            .iter()
            .map(|entry| entry.binding)
            .collect();
        assert_eq!(numbers, [4, 5, 6]);
    }
}
