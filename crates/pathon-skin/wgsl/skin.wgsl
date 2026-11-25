// One dispatch deforms one skinned instance: morph targets on the rest pose, then the blended bone matrix.
// Order and arithmetic mirror `pathon_skin::reference`.
const FLAG_SKINNED: u32 = 1u;
const FLAG_WRITE_PREVIOUS: u32 = 2u;
const MIN_LENGTH_SQUARED: f32 = 1e-20;

struct Params {
    vertex_count: u32,
    source_base_vertex: u32,
    position_current: u32,
    position_previous: u32,
    surface_first: u32,
    bone_base: u32,
    bone_count: u32,
    weight_base: u32,
    morph_count: u32,
    morph_vertices: u32,
    flags: u32,
    row_threads: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> pose: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> source_positions: array<f32>;
@group(0) @binding(3) var<storage, read> source_surface: array<f32>;
@group(0) @binding(4) var<storage, read> source_skin: array<u32>;
@group(0) @binding(5) var<storage, read> morph_deltas: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> deformed_positions: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> deformed_surface: array<f32>;

fn bone_matrix(bone: u32) -> mat4x4<f32> {
    let first = params.bone_base + bone * 4u;
    return mat4x4<f32>(pose[first], pose[first + 1u], pose[first + 2u], pose[first + 3u]);
}

fn morph_weight(target_index: u32) -> f32 {
    let at = params.weight_base + target_index;
    return pose[at >> 2u][at & 3u];
}

// Weights are renormalized over the influences that point at a real bone, so a rounded Unorm8 sum or a stray index
// cannot shrink or blow up the mesh; a vertex left with no influence stays where the morphs put it.
fn blended_bone_matrix(source_vertex: u32) -> mat4x4<f32> {
    let at = source_vertex * 3u;
    let low = source_skin[at];
    let high = source_skin[at + 1u];
    let joints = vec4<u32>(low & 0xffffu, low >> 16u, high & 0xffffu, high >> 16u);
    let weights = unpack4x8unorm(source_skin[at + 2u]);
    var sum = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    var total = 0.0;
    for (var influence = 0u; influence < 4u; influence += 1u) {
        let weight = weights[influence];
        if weight > 0.0 && joints[influence] < params.bone_count {
            sum += bone_matrix(joints[influence]) * weight;
            total += weight;
        }
    }
    if total <= 0.0 {
        return mat4x4<f32>(
            vec4<f32>(1.0, 0.0, 0.0, 0.0),
            vec4<f32>(0.0, 1.0, 0.0, 0.0),
            vec4<f32>(0.0, 0.0, 1.0, 0.0),
            vec4<f32>(0.0, 0.0, 0.0, 1.0),
        );
    }
    return sum * (1.0 / total);
}

fn safe_normalize(direction: vec3<f32>) -> vec3<f32> {
    let length_squared = dot(direction, direction);
    if length_squared > MIN_LENGTH_SQUARED {
        return direction * inverseSqrt(length_squared);
    }
    return direction;
}

fn position_at(vertex: u32) -> vec3<f32> {
    let at = vertex * 3u;
    return vec3<f32>(source_positions[at], source_positions[at + 1u], source_positions[at + 2u]);
}

fn surface_vec3(at: u32) -> vec3<f32> {
    return vec3<f32>(source_surface[at], source_surface[at + 1u], source_surface[at + 2u]);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let vertex = id.y * params.row_threads + id.x;
    if vertex >= params.vertex_count {
        return;
    }
    let source_vertex = params.source_base_vertex + vertex;
    let surface_at = source_vertex * 9u;

    var position = position_at(source_vertex);
    var normal = surface_vec3(surface_at);
    var tangent = surface_vec3(surface_at + 3u);
    let handedness = source_surface[surface_at + 6u];
    let uv = vec2<f32>(source_surface[surface_at + 7u], source_surface[surface_at + 8u]);

    for (var target_index = 0u; target_index < params.morph_count; target_index += 1u) {
        let weight = morph_weight(target_index);
        if weight != 0.0 {
            let delta_at = (target_index * params.morph_vertices + vertex) * 3u;
            position += morph_deltas[delta_at].xyz * weight;
            normal += morph_deltas[delta_at + 1u].xyz * weight;
            tangent += morph_deltas[delta_at + 2u].xyz * weight;
        }
    }

    if (params.flags & FLAG_SKINNED) != 0u {
        let skin = blended_bone_matrix(source_vertex);
        position = (skin * vec4<f32>(position, 1.0)).xyz;
        normal = (skin * vec4<f32>(normal, 0.0)).xyz;
        tangent = (skin * vec4<f32>(tangent, 0.0)).xyz;
    }
    normal = safe_normalize(normal);
    tangent = safe_normalize(tangent);

    let deformed = vec4<f32>(position, 1.0);
    deformed_positions[params.position_current + vertex] = deformed;
    if (params.flags & FLAG_WRITE_PREVIOUS) != 0u {
        deformed_positions[params.position_previous + vertex] = deformed;
    }
    let out = (params.surface_first + vertex) * 9u;
    deformed_surface[out] = normal.x;
    deformed_surface[out + 1u] = normal.y;
    deformed_surface[out + 2u] = normal.z;
    deformed_surface[out + 3u] = tangent.x;
    deformed_surface[out + 4u] = tangent.y;
    deformed_surface[out + 5u] = tangent.z;
    deformed_surface[out + 6u] = handedness;
    deformed_surface[out + 7u] = uv.x;
    deformed_surface[out + 8u] = uv.y;
}
