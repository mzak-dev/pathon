// Assigns every point and spot light to the view-space clusters its range sphere touches (clustered shading: Olsson, Billeter and
// Assarsson 2012, "Clustered Deferred and Forward Shading"; exponential depth slices as in Persson 2015 and Avalanche Studios' practice).
// One thread per cluster, two passes over the lights (count, then write), one 32-bit atomic per cluster to reserve its slice of the index
// list: no 64-bit atomics, so the same shader runs on the Web profile. The per-cluster list is in light order.
import package::pbr::lights::GpuLight;

struct ClusterParams {
    inverse_projection: mat4x4<f32>,
    world_to_view: mat4x4<f32>,
    render_size: vec2<f32>,
    padding: vec2<f32>,
    // Tiles in x and y, depth slices, tile size in pixels.
    dims: vec4<u32>,
    // Near, far, slices / ln(far / near), distance at which the last slice ends.
    depth: vec4<f32>,
    // First clustered light, light count, capacity of the index list, unused.
    lights: vec4<u32>,
}

@group(0) @binding(0) var<uniform> params: ClusterParams;
@group(0) @binding(1) var<storage, read> lights: array<GpuLight>;
// Per cluster: offset into `indices`, number of lights.
@group(0) @binding(2) var<storage, read_write> ranges: array<vec2<u32>>;
@group(0) @binding(3) var<storage, read_write> indices: array<u32>;
// 0: entries reserved so far, 1: entries that did not fit.
@group(0) @binding(4) var<storage, read_write> counters: array<atomic<u32>, 2>;

fn view_point(ndc: vec2<f32>, z: f32) -> vec3<f32> {
    let p = params.inverse_projection * vec4<f32>(ndc, z, 1.0);
    return p.xyz / p.w;
}

// The point of the pixel ray at view-space depth `depth`; works for perspective and orthographic projections.
fn ray_at_depth(ndc: vec2<f32>, depth: f32) -> vec3<f32> {
    let near_point = view_point(ndc, 1.0);
    let middle_point = view_point(ndc, 0.5);
    let t = (-depth - near_point.z) / (middle_point.z - near_point.z);
    return near_point + (middle_point - near_point) * t;
}

fn slice_start(slice: u32) -> f32 {
    if (slice == 0u) {
        return 0.0;
    }
    return params.depth.x * exp(f32(slice) / params.depth.z);
}

fn slice_end(slice: u32) -> f32 {
    if (slice + 1u == params.dims.z) {
        return params.depth.w;
    }
    return slice_start(slice + 1u);
}

struct Bounds {
    low: vec3<f32>,
    high: vec3<f32>,
}

fn cluster_bounds(x: u32, y: u32, z: u32) -> Bounds {
    let tile = f32(params.dims.w);
    let left = f32(x) * tile;
    let right = min(f32(x + 1u) * tile, params.render_size.x);
    let top = f32(y) * tile;
    let bottom = min(f32(y + 1u) * tile, params.render_size.y);
    let ndc_left = left / params.render_size.x * 2.0 - 1.0;
    let ndc_right = right / params.render_size.x * 2.0 - 1.0;
    let ndc_top = 1.0 - top / params.render_size.y * 2.0;
    let ndc_bottom = 1.0 - bottom / params.render_size.y * 2.0;
    let near_depth = slice_start(z);
    let far_depth = slice_end(z);

    var bounds: Bounds;
    bounds.low = vec3<f32>(3.0e38);
    bounds.high = vec3<f32>(-3.0e38);
    for (var corner = 0u; corner < 8u; corner++) {
        let ndc = vec2<f32>(
            select(ndc_left, ndc_right, (corner & 1u) != 0u),
            select(ndc_top, ndc_bottom, (corner & 2u) != 0u),
        );
        let point = ray_at_depth(ndc, select(near_depth, far_depth, (corner & 4u) != 0u));
        bounds.low = min(bounds.low, point);
        bounds.high = max(bounds.high, point);
    }
    return bounds;
}

fn light_touches(light: GpuLight, bounds: Bounds) -> bool {
    let center = (params.world_to_view * vec4<f32>(light.position_range.xyz, 1.0)).xyz;
    let nearest = clamp(center, bounds.low, bounds.high);
    let offset = center - nearest;
    let range = light.position_range.w;
    return dot(offset, offset) <= range * range;
}

@compute @workgroup_size(1)
fn reset() {
    atomicStore(&counters[0], 0u);
    atomicStore(&counters[1], 0u);
}

@compute @workgroup_size(64)
fn assign(@builtin(global_invocation_id) id: vec3<u32>) {
    let cluster = id.x;
    let nx = params.dims.x;
    let ny = params.dims.y;
    if (cluster >= nx * ny * params.dims.z) {
        return;
    }
    let bounds = cluster_bounds(cluster % nx, (cluster / nx) % ny, cluster / (nx * ny));
    let first = params.lights.x;
    let end = params.lights.x + params.lights.y;

    var count = 0u;
    for (var light = first; light < end; light++) {
        if (light_touches(lights[light], bounds)) {
            count++;
        }
    }
    if (count == 0u) {
        ranges[cluster] = vec2<u32>(0u, 0u);
        return;
    }
    let offset = atomicAdd(&counters[0], count);
    let capacity = params.lights.z;
    var kept = 0u;
    if (offset < capacity) {
        kept = min(count, capacity - offset);
    }
    if (kept < count) {
        atomicAdd(&counters[1], count - kept);
    }
    ranges[cluster] = vec2<u32>(offset, kept);
    var written = 0u;
    for (var light = first; light < end && written < kept; light++) {
        if (light_touches(lights[light], bounds)) {
            indices[offset + written] = light;
            written++;
        }
    }
}
