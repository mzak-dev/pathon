struct Params {
    threshold: f32,
    knee: f32,
    scatter: f32,
    padding: f32,
}

override KARIS: bool = true;

// Colour that is already exposed never needs to exceed this: averages of it stay representable as half floats.
const BLOOM_CAP: f32 = 60000.0;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var hdr: texture_2d<f32>;
@group(0) @binding(2) var exposure_texture: texture_2d<f32>;
@group(0) @binding(3) var source: texture_2d<f32>;
@group(0) @binding(4) var base: texture_2d<f32>;
@group(0) @binding(5) var linear_sampler: sampler;

fn prefiltered_texel(coord: vec2<i32>, exposure: f32) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(hdr));
    let clamped = clamp(coord, vec2<i32>(0, 0), size - vec2<i32>(1, 1));
    let color = min(sanitize(textureLoad(hdr, clamped, 0).rgb) * exposure, vec3<f32>(BLOOM_CAP));
    let brightness = max(color.r, max(color.g, color.b));
    var soft = clamp(brightness - params.threshold + params.knee, 0.0, 2.0 * params.knee);
    soft = soft * soft / (4.0 * params.knee + 1.0e-5);
    let contribution = max(soft, brightness - params.threshold) / max(brightness, 1.0e-5);
    return color * contribution;
}

// Texels are read one by one, never through a filter: one NaN would poison its bilinear neighbours.
// A destination pixel covers the 2x2 source block at 2p; the 13 taps of the downsample need the 6x6 around it.
fn block_box(block: ptr<function, array<vec3<f32>, 36>>, x: u32, y: u32) -> vec3<f32> {
    let first = y * 6u + x;
    return ((*block)[first] + (*block)[first + 1u] + (*block)[first + 6u] + (*block)[first + 7u]) * 0.25;
}

fn karis_weight(color: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + luminance(color));
}

@fragment
fn fs_prefilter(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let exposure = clamp(sanitize_channel(textureLoad(exposure_texture, vec2<i32>(0, 0), 0).r), 0.0, 1.0e6);
    let origin = vec2<i32>(position.xy) * 2 - vec2<i32>(2, 2);
    var block: array<vec3<f32>, 36>;
    for (var y = 0; y < 6; y++) {
        for (var x = 0; x < 6; x++) {
            block[y * 6 + x] = prefiltered_texel(origin + vec2<i32>(x, y), exposure);
        }
    }
    // Four overlapping corner groups and the centre group of the 13-tap filter; weights 0.125 and 0.5.
    let center = (block_box(&block, 1u, 1u) + block_box(&block, 3u, 1u) + block_box(&block, 1u, 3u) + block_box(&block, 3u, 3u)) * 0.25;
    let top_left = (block_box(&block, 0u, 0u) + block_box(&block, 2u, 0u) + block_box(&block, 0u, 2u) + block_box(&block, 2u, 2u)) * 0.25;
    let top_right = (block_box(&block, 2u, 0u) + block_box(&block, 4u, 0u) + block_box(&block, 2u, 2u) + block_box(&block, 4u, 2u)) * 0.25;
    let bottom_left = (block_box(&block, 0u, 2u) + block_box(&block, 2u, 2u) + block_box(&block, 0u, 4u) + block_box(&block, 2u, 4u)) * 0.25;
    let bottom_right = (block_box(&block, 2u, 2u) + block_box(&block, 4u, 2u) + block_box(&block, 2u, 4u) + block_box(&block, 4u, 4u)) * 0.25;
    if (KARIS) {
        let w_center = 0.5 * karis_weight(center);
        let w_top_left = 0.125 * karis_weight(top_left);
        let w_top_right = 0.125 * karis_weight(top_right);
        let w_bottom_left = 0.125 * karis_weight(bottom_left);
        let w_bottom_right = 0.125 * karis_weight(bottom_right);
        let sum = center * w_center + top_left * w_top_left + top_right * w_top_right
            + bottom_left * w_bottom_left + bottom_right * w_bottom_right;
        let weights = w_center + w_top_left + w_top_right + w_bottom_left + w_bottom_right;
        return vec4<f32>(sum / weights, 1.0);
    }
    return vec4<f32>(center * 0.5 + (top_left + top_right + bottom_left + bottom_right) * 0.125, 1.0);
}

fn tap(uv: vec2<f32>, texel: vec2<f32>, offset: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(source, linear_sampler, uv + offset * texel, 0.0).rgb;
}

@fragment
fn fs_down(input: FullscreenOutput) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    let uv = input.uv;
    let a = tap(uv, texel, vec2<f32>(-2.0, -2.0));
    let b = tap(uv, texel, vec2<f32>(0.0, -2.0));
    let c = tap(uv, texel, vec2<f32>(2.0, -2.0));
    let d = tap(uv, texel, vec2<f32>(-1.0, -1.0));
    let e = tap(uv, texel, vec2<f32>(1.0, -1.0));
    let f = tap(uv, texel, vec2<f32>(-2.0, 0.0));
    let g = tap(uv, texel, vec2<f32>(0.0, 0.0));
    let h = tap(uv, texel, vec2<f32>(2.0, 0.0));
    let i = tap(uv, texel, vec2<f32>(-1.0, 1.0));
    let j = tap(uv, texel, vec2<f32>(1.0, 1.0));
    let k = tap(uv, texel, vec2<f32>(-2.0, 2.0));
    let l = tap(uv, texel, vec2<f32>(0.0, 2.0));
    let m = tap(uv, texel, vec2<f32>(2.0, 2.0));
    let color = (d + e + i + j) * 0.125
        + (a + b + f + g) * 0.03125
        + (b + c + g + h) * 0.03125
        + (f + g + k + l) * 0.03125
        + (g + h + l + m) * 0.03125;
    return vec4<f32>(color, 1.0);
}

// Tent filter of the narrower level, blended into the sharper one: every step is a convex combination, so the
// pyramid keeps the energy of its input.
@fragment
fn fs_up(input: FullscreenOutput) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    let uv = input.uv;
    let tent = (tap(uv, texel, vec2<f32>(-1.0, -1.0)) + tap(uv, texel, vec2<f32>(1.0, -1.0))
        + tap(uv, texel, vec2<f32>(-1.0, 1.0)) + tap(uv, texel, vec2<f32>(1.0, 1.0))
        + 2.0 * (tap(uv, texel, vec2<f32>(0.0, -1.0)) + tap(uv, texel, vec2<f32>(-1.0, 0.0))
            + tap(uv, texel, vec2<f32>(1.0, 0.0)) + tap(uv, texel, vec2<f32>(0.0, 1.0)))
        + 4.0 * tap(uv, texel, vec2<f32>(0.0, 0.0))) / 16.0;
    let sharper = textureSampleLevel(base, linear_sampler, uv, 0.0).rgb;
    return vec4<f32>(mix(sharper, tent, params.scatter), 1.0);
}
