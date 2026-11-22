struct Params {
    face_size: u32,
    padding0: u32,
    padding1: u32,
    padding2: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_2d_array<f32>;
@group(0) @binding(2) var destination: texture_storage_2d_array<rgba16float, write>;

// Cosine convolution by quadrature over every texel of a small mip of the source (no sampling noise, and a bright
// sun keeps all its energy because mips are box averages). Dividing by the weights' sum instead of by pi makes a
// constant environment map to exactly that constant, whatever the quadrature error: the result is E / pi.
@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.face_size || id.y >= params.face_size) {
        return;
    }
    let n = texel_direction(id.z, f32(id.x) + 0.5, f32(id.y) + 0.5, f32(params.face_size));
    let source_size = textureDimensions(source).x;
    let source_extent = f32(source_size);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var face = 0u; face < 6u; face++) {
        for (var y = 0u; y < source_size; y++) {
            for (var x = 0u; x < source_size; x++) {
                let s = 2.0 * (f32(x) + 0.5) / source_extent - 1.0;
                let t = 2.0 * (f32(y) + 0.5) / source_extent - 1.0;
                let l = face_direction(face, s, t);
                let cosine = dot(n, l);
                if (cosine > 0.0) {
                    // Solid angle of a cube texel: its area over (1 + s^2 + t^2)^1.5.
                    let texel_solid_angle = 4.0 / (source_extent * source_extent) / pow(1.0 + s * s + t * t, 1.5);
                    let w = cosine * texel_solid_angle;
                    sum += textureLoad(source, vec2<i32>(i32(x), i32(y)), i32(face), 0).rgb * w;
                    weight += w;
                }
            }
        }
    }
    textureStore(destination, vec2<i32>(id.xy), i32(id.z), vec4<f32>(sum / weight, 1.0));
}
