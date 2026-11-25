// The backdrop of the pbr viewer: the sharpest level of the environment, drawn where nothing else was (depth 0).
// The depth is read in the shader instead of tested by the pipeline: an `Equal` test on a read-only depth buffer
// let the sky through over geometry on the RX 9070 XT.

struct Sky {
    inverse_view_projection: mat4x4<f32>,
    camera: vec4<f32>,
    // x: intensity of the environment as a backdrop.
    params: vec4<f32>,
}

@group(0) @binding(0) var<uniform> sky: Sky;
@group(0) @binding(1) var environment: texture_cube<f32>;
@group(0) @binding(2) var environment_sampler: sampler;
@group(0) @binding(3) var scene_depth: texture_depth_2d;

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs(@builtin(vertex_index) index: u32) -> Varying {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    let ndc = corner * 2.0 - 1.0;
    var out: Varying;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.ndc = ndc;
    return out;
}

@fragment
fn fs(in: Varying) -> @location(0) vec4<f32> {
    // Reverse-Z: the buffer is cleared to 0, the far plane; anything drawn has a larger value.
    if (textureLoad(scene_depth, vec2<i32>(in.clip.xy), 0) > 0.0) {
        discard;
    }
    // Any depth strictly between the planes gives the same ray; the middle keeps the unprojection well conditioned.
    let point = sky.inverse_view_projection * vec4<f32>(in.ndc, 0.5, 1.0);
    let direction = normalize(point.xyz / point.w - sky.camera.xyz);
    let radiance = textureSampleLevel(environment, environment_sampler, direction, 0.0).rgb;
    return vec4<f32>(radiance * sky.params.x, 1.0);
}
