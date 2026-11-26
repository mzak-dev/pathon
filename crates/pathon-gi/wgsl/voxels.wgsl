// Grid lookups and the ray march through the voxels (inserted after the bindings, `GiShaders::root`). The march is the
// 3D DDA of Amanatides and Woo (1987): it steps one cell face at a time, so a ray cannot slip between two cells that
// share an edge, and a wall of cells that touch by face or by edge stops every ray.

const MAX_MARCH_STEPS: u32 = 1024u;

struct VoxelHit {
    hit: bool,
    cell: vec3<i32>,
    // Face of the hit cell the ray entered through: 0 +x, 1 -x, 2 +y, 3 -y, 4 +z, 5 -z (its outward normal).
    face: u32,
    distance: f32,
}

fn face_normal(face: u32) -> vec3<f32> {
    let sign = select(1.0, -1.0, (face & 1u) == 1u);
    switch (face >> 1u) {
        case 0u: { return vec3<f32>(sign, 0.0, 0.0); }
        case 1u: { return vec3<f32>(0.0, sign, 0.0); }
        default: { return vec3<f32>(0.0, 0.0, sign); }
    }
}

fn cell_in_grid(cell: vec3<i32>) -> bool {
    return all(cell >= vec3<i32>(0)) && all(cell < vec3<i32>(gi_grid.dims.xyz));
}

fn cell_index(cell: vec3<i32>) -> u32 {
    let c = vec3<u32>(cell);
    return c.x + gi_grid.dims.x * (c.y + gi_grid.dims.y * c.z);
}

fn cell_solid(cell: vec3<i32>) -> bool {
    return cell_in_grid(cell) && (voxel_albedo[cell_index(cell)] >> 24u) != 0u;
}

fn cell_center(cell: vec3<i32>) -> vec3<f32> {
    return gi_grid.origin_size.xyz + (vec3<f32>(cell) + vec3<f32>(0.5)) * gi_grid.origin_size.w;
}

fn world_cell(position: vec3<f32>) -> vec3<i32> {
    return vec3<i32>(floor((position - gi_grid.origin_size.xyz) / gi_grid.origin_size.w));
}

fn albedo_of(packed: u32) -> vec3<f32> {
    let root = vec3<f32>(f32(packed & 255u), f32((packed >> 8u) & 255u), f32((packed >> 16u) & 255u)) / 255.0;
    return root * root;
}

fn pack_albedo(albedo: vec3<f32>) -> u32 {
    let root = vec3<u32>(floor(sqrt(clamp(albedo, vec3<f32>(0.0), vec3<f32>(1.0))) * 255.0 + vec3<f32>(0.5)));
    return root.x | (root.y << 8u) | (root.z << 16u) | (255u << 24u);
}

// First solid cell along the ray within `max_distance` metres. A ray that starts in a solid cell hits at distance 0.
fn trace_voxels(origin: vec3<f32>, direction: vec3<f32>, max_distance: f32) -> VoxelHit {
    var result: VoxelHit;
    result.hit = false;
    result.cell = vec3<i32>(0);
    result.face = 0u;
    result.distance = max_distance;

    let size = gi_grid.origin_size.w;
    let start = (origin - gi_grid.origin_size.xyz) / size;
    var cell = vec3<i32>(floor(start));
    if (cell_solid(cell)) {
        result.hit = true;
        result.cell = cell;
        result.distance = 0.0;
        return result;
    }
    let forward = direction >= vec3<f32>(0.0);
    let step = select(vec3<i32>(-1), vec3<i32>(1), forward);
    let inverse = vec3<f32>(1.0) / max(abs(direction), vec3<f32>(1.0e-7));
    let to_boundary = select(start - floor(start), floor(start) + vec3<f32>(1.0) - start, forward);
    var next = to_boundary * inverse;
    let advance = inverse;
    let limit = max_distance / size;
    for (var i = 0u; i < MAX_MARCH_STEPS; i++) {
        var axis = 0u;
        if (next.y < next.x && next.y <= next.z) {
            axis = 1u;
        } else if (next.z < next.x && next.z < next.y) {
            axis = 2u;
        }
        let travelled = next[axis];
        if (travelled > limit) {
            return result;
        }
        cell[axis] += step[axis];
        next[axis] += advance[axis];
        if (!cell_in_grid(cell)) {
            return result;
        }
        if ((voxel_albedo[cell_index(cell)] >> 24u) != 0u) {
            result.hit = true;
            result.cell = cell;
            result.face = axis * 2u + select(0u, 1u, step[axis] > 0);
            result.distance = travelled * size;
            return result;
        }
    }
    return result;
}
