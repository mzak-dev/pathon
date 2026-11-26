// Conservative voxelization of one mesh instance: one thread per triangle marks every cell the triangle touches (the
// full triangle-box test of Akenine-Moeller 2001, 13 separating axes), so the surface has no holes a ray can pass.
// The albedo of a cell is the base colour factor times the base colour texture sampled at the triangle point nearest
// the cell centre, with a gradient the size of the cell, which picks the mip that averages the cell's footprint.
import package::gi::common::{GiVolume, pack_rgb9e5};
import package::material::texture_source::sample_material_texture;

// GI_BINDINGS
// GI_VOXELS

// 144 bytes, mirrored by `voxelize::VoxelJob`.
struct Job {
    row0: vec4<f32>,
    row1: vec4<f32>,
    row2: vec4<f32>,
    // First index, triangles, base vertex (an i32 in the bits), unused.
    mesh: vec4<u32>,
    // Base colour factor.
    albedo: vec4<f32>,
    // Emitted radiance (emissive times strength).
    emissive: vec4<f32>,
    // Base colour texture slot, emissive texture slot, unused, unused.
    textures: vec4<u32>,
}

@group(1) @binding(0) var<uniform> job: Job;
// Stream 0 of the pool (three floats per vertex) and stream 1 (nine floats: normal, tangent, uv0 at 7 and 8).
@group(1) @binding(1) var<storage, read> positions: array<f32>;
@group(1) @binding(2) var<storage, read> surface: array<f32>;
@group(1) @binding(3) var<storage, read> mesh_indices: array<u32>;

fn grid_position(vertex: u32) -> vec3<f32> {
    let local = vec4<f32>(positions[vertex * 3u], positions[vertex * 3u + 1u], positions[vertex * 3u + 2u], 1.0);
    let world = vec3<f32>(dot(job.row0, local), dot(job.row1, local), dot(job.row2, local));
    return (world - gi_grid.origin_size.xyz) / gi_grid.origin_size.w;
}

fn separated(axis: vec3<f32>, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, half_size: f32) -> bool {
    let pa = dot(axis, a);
    let pb = dot(axis, b);
    let pc = dot(axis, c);
    let radius = half_size * (abs(axis.x) + abs(axis.y) + abs(axis.z));
    return max(pa, max(pb, pc)) < -radius || min(pa, min(pb, pc)) > radius;
}

// `a`, `b` and `c` relative to the centre of a cell of edge `2 * half_size`.
fn triangle_touches_cell(a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, half_size: f32) -> bool {
    if (max(a.x, max(b.x, c.x)) < -half_size || min(a.x, min(b.x, c.x)) > half_size) { return false; }
    if (max(a.y, max(b.y, c.y)) < -half_size || min(a.y, min(b.y, c.y)) > half_size) { return false; }
    if (max(a.z, max(b.z, c.z)) < -half_size || min(a.z, min(b.z, c.z)) > half_size) { return false; }
    let ab = b - a;
    let bc = c - b;
    let ca = a - c;
    if (separated(cross(ab, bc), a, b, c, half_size)) { return false; }
    var edges = array<vec3<f32>, 3>(ab, bc, ca);
    var axes = array<vec3<f32>, 3>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    for (var e = 0u; e < 3u; e++) {
        for (var k = 0u; k < 3u; k++) {
            let axis = cross(edges[e], axes[k]);
            if (dot(axis, axis) > 1.0e-12 && separated(axis, a, b, c, half_size)) {
                return false;
            }
        }
    }
    return true;
}

fn barycentric(point: vec3<f32>, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>) -> vec3<f32> {
    let v0 = b - a;
    let v1 = c - a;
    let v2 = point - a;
    let d00 = dot(v0, v0);
    let d01 = dot(v0, v1);
    let d11 = dot(v1, v1);
    let d20 = dot(v2, v0);
    let d21 = dot(v2, v1);
    let denominator = d00 * d11 - d01 * d01;
    if (abs(denominator) < 1.0e-12) {
        return vec3<f32>(1.0 / 3.0);
    }
    let v = (d11 * d20 - d01 * d21) / denominator;
    let w = (d00 * d21 - d01 * d20) / denominator;
    let clamped = max(vec3<f32>(1.0 - v - w, v, w), vec3<f32>(0.0));
    return clamped / (clamped.x + clamped.y + clamped.z);
}

fn vertex_uv(vertex: u32) -> vec2<f32> {
    return vec2<f32>(surface[vertex * 9u + 7u], surface[vertex * 9u + 8u]);
}

@compute @workgroup_size(64)
fn voxelize(@builtin(global_invocation_id) id: vec3<u32>) {
    let triangle = id.x;
    if (triangle >= job.mesh.y) {
        return;
    }
    let base_vertex = bitcast<i32>(job.mesh.z);
    let first = job.mesh.x + triangle * 3u;
    let v0 = u32(i32(mesh_indices[first]) + base_vertex);
    let v1 = u32(i32(mesh_indices[first + 1u]) + base_vertex);
    let v2 = u32(i32(mesh_indices[first + 2u]) + base_vertex);
    let a = grid_position(v0);
    let b = grid_position(v1);
    let c = grid_position(v2);
    let area = length(cross(b - a, c - a));
    if (area < 1.0e-9) {
        return;
    }
    let uv0 = vertex_uv(v0);
    let uv1 = vertex_uv(v1);
    let uv2 = vertex_uv(v2);
    let uv_area = abs((uv1.x - uv0.x) * (uv2.y - uv0.y) - (uv1.y - uv0.y) * (uv2.x - uv0.x));
    // Texture coordinates per cell edge: the footprint of one cell in uv space (the grid positions are in cells).
    let footprint = sqrt(uv_area / area);
    let gradient_x = vec2<f32>(footprint, 0.0);
    let gradient_y = vec2<f32>(0.0, footprint);

    let low = vec3<i32>(max(floor(min(a, min(b, c))), vec3<f32>(0.0)));
    let high = min(vec3<i32>(floor(max(a, max(b, c)))), vec3<i32>(gi_grid.dims.xyz) - vec3<i32>(1));
    let emits = any(job.emissive.rgb > vec3<f32>(0.0));
    for (var z = low.z; z <= high.z; z++) {
        for (var y = low.y; y <= high.y; y++) {
            for (var x = low.x; x <= high.x; x++) {
                let center = vec3<f32>(f32(x), f32(y), f32(z)) + vec3<f32>(0.5);
                if (!triangle_touches_cell(a - center, b - center, c - center, 0.502)) {
                    continue;
                }
                let weights = barycentric(center, a, b, c);
                let uv = uv0 * weights.x + uv1 * weights.y + uv2 * weights.z;
                let base = sample_material_texture(job.textures.x, uv, gradient_x, gradient_y);
                let index = cell_index(vec3<i32>(x, y, z));
                voxel_albedo[index] = pack_albedo(job.albedo.rgb * base.rgb);
                if (emits) {
                    let glow = sample_material_texture(job.textures.y, uv, gradient_x, gradient_y);
                    voxel_emissive[index] = pack_rgb9e5(job.emissive.rgb * glow.rgb);
                }
            }
        }
    }
}
