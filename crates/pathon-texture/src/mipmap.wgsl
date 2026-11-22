// One dispatch writes mip N from mip N-1 of one layer. Mirrors `reference.rs`; keep the two in step.

// 0: linear, 1: sRGB-encoded colour averaged in linear light, 2: normals averaged as vectors and renormalized.
override MODE: u32 = 0u;

const NORMAL_EPSILON: f32 = 1.0e-5;

@group(0) @binding(0) var previous_mip: texture_2d<f32>;
@group(0) @binding(1) var next_mip: texture_storage_2d<STORAGE_FORMAT, write>;

fn srgb_to_linear(value: vec3<f32>) -> vec3<f32> {
    let low = value / 12.92;
    let high = pow((value + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, value <= vec3<f32>(0.04045));
}

fn linear_to_srgb(value: vec3<f32>) -> vec3<f32> {
    let low = value * 12.92;
    let high = 1.055 * pow(value, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(high, low, value <= vec3<f32>(0.0031308));
}

fn decode(texel: vec4<f32>) -> vec4<f32> {
    if MODE == 1u {
        return vec4<f32>(srgb_to_linear(texel.rgb), texel.a);
    }
    if MODE == 2u {
        return vec4<f32>(texel.rgb * 2.0 - vec3<f32>(1.0), texel.a);
    }
    return texel;
}

fn encode(mean: vec4<f32>) -> vec4<f32> {
    if MODE == 1u {
        return vec4<f32>(linear_to_srgb(mean.rgb), mean.a);
    }
    if MODE == 2u {
        let magnitude = length(mean.xyz);
        var direction = vec3<f32>(0.0, 0.0, 1.0);
        if magnitude >= NORMAL_EPSILON {
            direction = mean.xyz / magnitude;
        }
        return vec4<f32>(direction * 0.5 + vec3<f32>(0.5), mean.a);
    }
    return mean;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let source_size = textureDimensions(previous_mip);
    let target_size = textureDimensions(next_mip);
    if id.x >= target_size.x || id.y >= target_size.y {
        return;
    }
    // The source texels under this target texel: 2x2 for halving, wider when a size is odd.
    let first = (id.xy * source_size) / target_size;
    let end = ((id.xy + vec2<u32>(1u)) * source_size + target_size - vec2<u32>(1u)) / target_size;
    var sum = vec4<f32>(0.0);
    for (var y = first.y; y < end.y; y++) {
        for (var x = first.x; x < end.x; x++) {
            sum += decode(textureLoad(previous_mip, vec2<i32>(i32(x), i32(y)), 0));
        }
    }
    let count = f32((end.x - first.x) * (end.y - first.y));
    textureStore(next_mip, vec2<i32>(id.xy), encode(sum / count));
}
