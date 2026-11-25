// The backdrop of the pbr viewer: the sharpest level of the environment, drawn where nothing else was (depth 0).

struct Sky {
    inverse_view_projection: mat4x4<f32>,
    camera: vec4<f32>,
    // x: intensity of the environment as a backdrop.
    params: vec4<f32>,
}

@group(0) @binding(0) var<uniform> sky: Sky;
@group(0) @binding(1) var environment: texture_cube<f32>;
@group(0) @binding(2) var environment_sampler: sampler;

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs(@builtin(vertex_index) index: u32) -> Varying {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    let ndc = corner * 2.0 - 1.0;
    var out: Varying;
    // Depth 0 is the far plane of the reverse-Z buffer, the one value the test passes for.
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.ndc = ndc;
    return out;
}

@fragment
fn fs(in: Varying) -> @location(0) vec4<f32> {
    // Any depth strictly between the planes gives the same ray; the middle keeps the unprojection well conditioned.
    let point = sky.inverse_view_projection * vec4<f32>(in.ndc, 0.5, 1.0);
    let direction = normalize(point.xyz / point.w - sky.camera.xyz);
    let radiance = textureSampleLevel(environment, environment_sampler, direction, 0.0).rgb;
    return vec4<f32>(radiance * sky.params.x, 1.0);
}
