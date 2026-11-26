// What the shadow pack publishes about local lights for the lighting shader: the uniform block that maps a light to its
// tiles in the atlas, the slot of the atlas and the bindings that read them.

use pathon_api::Slot;
use pathon_api::resources::UniformSlice;
use pathon_api::rhi::{
    BindGroupEntry, BindGroupLayoutEntry, BindingResource, BindingType, Format, SamplerBindingType, SamplerId,
    ShaderStages, TextureSampleType, TextureViewDesc, TextureViewDimension, TextureViewId,
};

use crate::local::{LocalPlan, LocalShadowShape};
use crate::settings::{ShadowFilter, ShadowSettings};

/// The depth maps of every local light, side by side: one `Depth32Float` texture, reverse-Z (1 at the light, in
/// `near / distance` units), read through a plain `D2` view and a comparison sampler. It persists between frames: the
/// map of a light that did not change is not rendered again.
pub const SHADOW_LOCAL_MAP: Slot = Slot::new("shadow.local", Format::Depth32Float);

const MAX_LIGHTS: usize = ShadowSettings::MAX_LOCAL_LIGHTS as usize;
const MAX_TILES: usize = ShadowSettings::MAX_LOCAL_TILES as usize;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct LightUniform {
    // xyz: position, w: near plane of the maps.
    position_near: [f32; 4],
    // x: light index, y: first tile, z: 1 for a point light (six tiles), 0 for a spot light, w: unused.
    ids: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct TileUniform {
    view_projection: [[f32; 4]; 4],
    // xy: offset of the tile in the atlas as a fraction, z: size as a fraction, w: metres per texel per metre of
    // distance along the axis (2 tan(half fov) / size).
    rect: [f32; 4],
}

/// The bytes of WGSL `shadow::sample::LocalShadows`, 5680 bytes in a uniform buffer.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LocalShadowUniforms {
    lights: [LightUniform; MAX_LIGHTS],
    tiles: [TileUniform; MAX_TILES],
    // x: number of lights, y: filter, z: flags (bit 0: rotate the filter per pixel), w: unused.
    counts: [u32; 4],
    // x: atlas side in texels, y: 1 / atlas side, z: constant bias in metres, w: relative bias.
    tuning: [f32; 4],
    // x: normal offset in texels of the map at the receiver's distance.
    normal: [f32; 4],
}

impl LocalShadowUniforms {
    pub(crate) fn new(plan: &LocalPlan, atlas_size: u32, settings: &ShadowSettings) -> Self {
        let mut uniforms: Self = bytemuck::Zeroable::zeroed();
        let atlas = atlas_size as f32;
        for (planned, slot) in plan.lights.iter().zip(&mut uniforms.lights) {
            let light = &planned.light;
            *slot = LightUniform {
                position_near: [light.position.x, light.position.y, light.position.z, planned.near],
                ids: [
                    light.light_index,
                    u32::try_from(planned.first_tile).unwrap_or(0),
                    u32::from(matches!(light.shape, LocalShadowShape::Point)),
                    0,
                ],
            };
        }
        for (planned, slot) in plan.tiles.iter().zip(&mut uniforms.tiles) {
            let size = planned.tile.size as f32;
            *slot = TileUniform {
                view_projection: planned.view_projection.to_cols_array_2d(),
                rect: [
                    planned.tile.x as f32 / atlas,
                    planned.tile.y as f32 / atlas,
                    size / atlas,
                    2.0 * planned.tan_half / size,
                ],
            };
        }
        uniforms.counts = [
            u32::try_from(plan.lights.len().min(MAX_LIGHTS)).unwrap_or(0),
            settings.filter.shader_value(),
            u32::from(settings.rotate_filter),
            0,
        ];
        uniforms.tuning = [
            atlas,
            1.0 / atlas,
            settings.local_constant_bias,
            settings.local_relative_bias,
        ];
        uniforms.normal = [settings.local_normal_bias, 0.0, 0.0, 0.0];
        uniforms
    }

    pub fn light_count(&self) -> u32 {
        self.counts[0]
    }

    pub fn filter(&self) -> ShadowFilter {
        match self.counts[1] {
            0 => ShadowFilter::Hard,
            1 => ShadowFilter::Pcf3,
            _ => ShadowFilter::Pcf5,
        }
    }
}

/// What the shadow pack publishes in `Shared` for the lighting pack, every frame some local light has a map: where the
/// uniforms sit in the frame's ring and the comparison sampler. The atlas is the slot [`SHADOW_LOCAL_MAP`]. A new value
/// every frame; do not keep it across frames.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LocalShadowFrame {
    pub uniform_buffer: UniformSlice,
    pub sampler: SamplerId,
    pub light_count: u32,
    pub atlas_size: u32,
}

/// One light that got a map this frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LocalLightReport {
    pub key: u64,
    pub light_index: u32,
    /// Side of the map (of each face, for a point light) in texels.
    pub tile_size: u32,
}

/// What the pack decided in the last frame it planned, for tests, a debug overlay and the budget tuning of a host.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct LocalShadowReport {
    /// The lights that have a map, strongest first.
    pub lights: Vec<LocalLightReport>,
    /// Lights offered by the host.
    pub candidates: usize,
    /// Of those, the ones outside the view (their maps were neither rendered nor sampled).
    pub out_of_view: usize,
    /// Wanted a map and found no room in the atlas.
    pub dropped_for_space: usize,
    /// Tiles drawn this frame, and tiles that were still valid from before.
    pub rendered_tiles: usize,
    pub cached_tiles: usize,
    /// Casters drawn into the maps this frame, summed over the tiles.
    pub drawn_casters: usize,
    pub atlas_size: u32,
}

/// The three consecutive bindings of one group a lighting shader needs for local lights: the uniforms, the atlas and a
/// comparison sampler. Like `ShadowBindings`, the `shadow::sample` module declares none of them: the shader declares them
/// with [`LocalShadowBindings::wgsl_declarations`] and passes them to `shadow_factor_local`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LocalShadowBindings {
    pub first_binding: u32,
}

impl LocalShadowBindings {
    pub const COUNT: u32 = 3;

    pub const fn starting_at(first_binding: u32) -> Self {
        Self { first_binding }
    }

    /// Declares `local_shadow_data`, `local_shadow_atlas` and `local_shadow_sampler`; the shader imports
    /// `package::shadow::sample::LocalShadows` for the first one's type.
    pub fn wgsl_bindings(self, group: u32) -> String {
        let first = self.first_binding;
        format!(
            "@group({group}) @binding({first}) var<uniform> local_shadow_data: LocalShadows;\n\
             @group({group}) @binding({}) var local_shadow_atlas: texture_depth_2d;\n\
             @group({group}) @binding({}) var local_shadow_sampler: sampler_comparison;\n",
            first + 1,
            first + 2
        )
    }

    /// `local_shadow_factor(light_index, world_position, normal, pixel) -> f32`: 0 in shadow, 1 lit, and 1 for a light
    /// that has no map. A function (it begins with `fn`) over the three bindings, which the module cannot name; the
    /// shader imports `shadow_local_tile_index` and `shadow_local_visibility` from `package::shadow::sample` for it.
    /// `normal` is the unit geometric normal and `pixel` the fragment's `@builtin(position).xy`.
    pub fn wgsl_helper() -> &'static str {
        "fn local_shadow_factor(light_index: u32, world_position: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> f32 {
    for (var entry = 0u; entry < local_shadow_data.counts.x; entry += 1u) {
        if (local_shadow_data.lights[entry].ids.x == light_index) {
            let light = local_shadow_data.lights[entry];
            let tile = local_shadow_data.tiles[shadow_local_tile_index(light, world_position)];
            return shadow_local_visibility(
                light,
                tile,
                local_shadow_data.counts,
                local_shadow_data.tuning,
                local_shadow_data.normal.x,
                local_shadow_atlas,
                local_shadow_sampler,
                world_position,
                normal,
                pixel,
            );
        }
    }
    return 1.0;
}
"
    }

    /// The bindings and the helper, for a shader that has no permutation to put them behind.
    pub fn wgsl_declarations(self, group: u32) -> String {
        [self.wgsl_bindings(group).as_str(), Self::wgsl_helper()].concat()
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
                    dimension: TextureViewDimension::D2,
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

    /// The view of the [`SHADOW_LOCAL_MAP`] slot texture the shader reads: the whole atlas as one 2D image.
    pub fn atlas_view_desc() -> TextureViewDesc {
        TextureViewDesc {
            dimension: Some(TextureViewDimension::D2),
            ..TextureViewDesc::default()
        }
    }

    /// `atlas_view` is the view `PassContext::view_with(atlas, &LocalShadowBindings::atlas_view_desc())` returns.
    pub fn bind_group_entries(self, frame: &LocalShadowFrame, atlas_view: TextureViewId) -> Vec<BindGroupEntry> {
        vec![
            BindGroupEntry {
                binding: self.first_binding,
                resource: frame.uniform_buffer.binding(),
            },
            BindGroupEntry {
                binding: self.first_binding + 1,
                resource: BindingResource::TextureView(atlas_view),
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
        assert_eq!(size_of::<LocalShadowUniforms>(), 16 * 32 + 64 * 80 + 3 * 16);
    }

    #[test]
    fn declarations_and_layout_use_the_same_bindings() {
        let bindings = LocalShadowBindings::starting_at(12);
        let text = bindings.wgsl_declarations(0);

        assert!(text.contains("@group(0) @binding(12) var<uniform> local_shadow_data: LocalShadows;"));
        assert!(text.contains("@group(0) @binding(14) var local_shadow_sampler: sampler_comparison;"));
        let numbers: Vec<u32> = bindings
            .layout_entries(ShaderStages::FRAGMENT)
            .iter()
            .map(|entry| entry.binding)
            .collect();
        assert_eq!(numbers, [12, 13, 14]);
    }
}
