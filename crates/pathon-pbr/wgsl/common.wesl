// Frame constants, instance data, octahedral normals and the cluster grid lookup shared by every PBR shader.

struct Frame {
    view_projection: mat4x4<f32>,
    unjittered_view_projection: mat4x4<f32>,
    previous_view_projection: mat4x4<f32>,
    world_to_view: mat4x4<f32>,
    camera_position: vec4<f32>,
    render_size: vec2<f32>,
    environment_intensity: f32,
    padding: f32,
    // Tiles in x and y, depth slices, tile size in pixels.
    cluster_dims: vec4<u32>,
    // Near, far, slices / ln(far / near), unused.
    cluster_depth: vec4<f32>,
    // Directional lights (first in the light array), all lights, unused, 1 = test every light instead of the cluster.
    light_counts: vec4<u32>,
}

// `model` and `previous_model` hold the rows of the affine transform, as `pathon_mesh::ObjectTransforms` does:
// the point `p` becomes `vec4(p, 1.0) * model`.
struct Instance {
    model: mat3x4<f32>,
    previous_model: mat3x4<f32>,
    normal_matrix: mat3x3<f32>,
    material: vec4<u32>,
}

fn world_position(instance: Instance, p: vec3<f32>) -> vec3<f32> {
    return vec4<f32>(p, 1.0) * instance.model;
}

fn previous_world_position(instance: Instance, p: vec3<f32>) -> vec3<f32> {
    return vec4<f32>(p, 1.0) * instance.previous_model;
}

fn world_normal(instance: Instance, n: vec3<f32>) -> vec3<f32> {
    return normalize(instance.normal_matrix * n);
}

fn world_tangent(instance: Instance, t: vec4<f32>) -> vec4<f32> {
    let m = instance.model;
    let axis = vec3<f32>(dot(m[0].xyz, t.xyz), dot(m[1].xyz, t.xyz), dot(m[2].xyz, t.xyz));
    return vec4<f32>(normalize(axis), t.w);
}

fn sign_not_zero(v: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(select(-1.0, 1.0, v.x >= 0.0), select(-1.0, 1.0, v.y >= 0.0));
}

// Octahedral mapping of a unit vector to [-1, 1]^2 (Cigolle et al. 2014, "A Survey of Efficient Representations for
// Independent Unit Vectors").
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if (n.z < 0.0) {
        return (vec2<f32>(1.0) - abs(p.yx)) * sign_not_zero(p);
    }
    return p;
}

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) {
        let folded = (vec2<f32>(1.0) - abs(n.yx)) * sign_not_zero(n.xy);
        n = vec3<f32>(folded.x, folded.y, n.z);
    }
    return normalize(n);
}

// Pixels the point moved from the previous frame to this one, seen from this frame: current pixel + motion = the
// pixel the point was at. Both clip positions are unjittered.
fn motion_vector(current_clip: vec4<f32>, previous_clip: vec4<f32>, render_size: vec2<f32>) -> vec2<f32> {
    let current = current_clip.xy / current_clip.w;
    let previous = previous_clip.xy / previous_clip.w;
    let delta = previous - current;
    return vec2<f32>(delta.x, -delta.y) * 0.5 * render_size;
}

// Exponential depth slices: slice k starts at near * (far / near)^(k / slices); slice 0 also takes everything closer
// than its end and the last slice everything beyond `far`.
fn cluster_slice(depth_params: vec4<f32>, slices: u32, view_depth: f32) -> u32 {
    if (view_depth <= depth_params.x) {
        return 0u;
    }
    let slice = log(view_depth / depth_params.x) * depth_params.z;
    return u32(clamp(slice, 0.0, f32(slices - 1u)));
}

fn cluster_index(frame: Frame, frag_xy: vec2<f32>, view_depth: f32) -> u32 {
    let dims = frame.cluster_dims;
    let tile = min(vec2<u32>(frag_xy) / dims.w, dims.xy - vec2<u32>(1u));
    let slice = cluster_slice(frame.cluster_depth, dims.z, view_depth);
    return (slice * dims.y + tile.y) * dims.x + tile.x;
}
