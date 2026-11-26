// Group 0 of every compute pass of the pack (inserted into each root, `GiShaders::root`). A pass uses the subset it
// needs; the layout is the union (`pipelines::compute_layout`).

// 32 bytes, mirrored by `voxel::VoxelGridUniforms`.
struct VoxelGrid {
    // xyz: world position of the corner of cell (0, 0, 0), w: edge of a cell in metres.
    origin_size: vec4<f32>,
    // Cells along x, y and z, cells in all.
    dims: vec4<u32>,
}

// 112 bytes, mirrored by `feature::PassUniforms`.
struct GiPass {
    // Frame number, first probe of this frame's batch, probes in the batch, rays per probe.
    frame: vec4<u32>,
    // Voxel relight: index of this frame's slice, number of slices, lights, unused.
    relight: vec4<u32>,
    // Rows of the rotation applied to the ray directions of this frame.
    rotation_x: vec4<f32>,
    rotation_y: vec4<f32>,
    rotation_z: vec4<f32>,
    // Hysteresis, scale of the environment, scale of the bounce, depth sharpness.
    blend: vec4<f32>,
    // Sun-like distance of directional shadow rays, unused, unused, unused.
    reach: vec4<f32>,
}

// 64 bytes, the layout of `pbr::lights::GpuLight`.
struct GpuLight {
    // xyz: position, w: range in metres (0 for a directional light).
    position_range: vec4<f32>,
    // Directional: unit vector to the light. Spot: unit vector the light shines along. w: kind (0, 1, 2).
    direction_kind: vec4<f32>,
    // rgb: tint times lux or candela.
    color_intensity: vec4<f32>,
    // x, y: spot angle scale and offset, z: 1 / range^2.
    spot: vec4<f32>,
}

@group(0) @binding(0) var<uniform> gi_volume: GiVolume;
@group(0) @binding(1) var<uniform> gi_grid: VoxelGrid;
@group(0) @binding(2) var<uniform> gi_pass: GiPass;
// Per cell: square root of the albedo in bytes 0..2, 255 in byte 3 when the cell is solid.
@group(0) @binding(3) var<storage, read_write> voxel_albedo: array<u32>;
// Per cell: emitted radiance, shared-exponent RGB.
@group(0) @binding(4) var<storage, read_write> voxel_emissive: array<u32>;
// Per cell and face (+x, -x, +y, -y, +z, -z): the radiance leaving the face, shared-exponent RGB.
@group(0) @binding(5) var<storage, read_write> voxel_radiance: array<u32>;
@group(0) @binding(6) var<storage, read> gi_lights: array<GpuLight>;
// Per ray, two vec4: radiance and distance; direction and kind (0 hit, 1 sky, 2 probe inside a solid cell).
@group(0) @binding(7) var<storage, read_write> gi_rays: array<vec4<f32>>;
@group(0) @binding(8) var previous_irradiance: texture_2d<f32>;
@group(0) @binding(9) var previous_visibility: texture_2d<f32>;
@group(0) @binding(10) var gi_sampler: sampler;
@group(0) @binding(11) var next_irradiance: texture_storage_2d<rgba16float, write>;
@group(0) @binding(12) var next_visibility: texture_storage_2d<rgba16float, write>;
@group(0) @binding(13) var environment: texture_cube<f32>;
@group(0) @binding(14) var environment_sampler: sampler;
// Element 0: the number of solid cells; then their indices (built by `compact_solid`, read by `light_voxels`).
@group(0) @binding(15) var<storage, read_write> solid_cells: array<atomic<u32>>;
