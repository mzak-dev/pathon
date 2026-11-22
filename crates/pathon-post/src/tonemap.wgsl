struct Params {
    output_size: vec2<f32>,
    bloom_intensity: f32,
    padding: f32,
}

override OPERATOR: u32 = 0u;
override ENCODE_SRGB: bool = true;
override HAS_BLOOM: bool = false;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var hdr: texture_2d<f32>;
@group(0) @binding(2) var bloom: texture_2d<f32>;
@group(0) @binding(3) var exposure_texture: texture_2d<f32>;
@group(0) @binding(4) var linear_sampler: sampler;

fn hdr_texel(coord: vec2<i32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(hdr));
    return sanitize(textureLoad(hdr, clamp(coord, vec2<i32>(0, 0), size - vec2<i32>(1, 1)), 0).rgb);
}

// Bilinear by hand: texels are cleaned before they are mixed, so one NaN stays one black pixel.
fn sample_hdr(uv: vec2<f32>) -> vec3<f32> {
    let position = uv * vec2<f32>(textureDimensions(hdr)) - 0.5;
    let base = floor(position);
    let fraction = position - base;
    let corner = vec2<i32>(base);
    let top = mix(hdr_texel(corner), hdr_texel(corner + vec2<i32>(1, 0)), fraction.x);
    let bottom = mix(hdr_texel(corner + vec2<i32>(0, 1)), hdr_texel(corner + vec2<i32>(1, 1)), fraction.x);
    return mix(top, bottom, fraction.y);
}

// Hill's fit of the ACES RRT and ODT. The matrices are written by columns.
fn aces_fitted(color: vec3<f32>) -> vec3<f32> {
    let input_matrix = mat3x3<f32>(
        vec3<f32>(0.59719, 0.07600, 0.02840),
        vec3<f32>(0.35458, 0.90834, 0.13383),
        vec3<f32>(0.04823, 0.01566, 0.83777),
    );
    let output_matrix = mat3x3<f32>(
        vec3<f32>(1.60475, -0.10208, -0.00327),
        vec3<f32>(-0.53108, 1.10813, -0.07276),
        vec3<f32>(-0.07367, -0.00605, 1.07602),
    );
    let v = input_matrix * (color / 0.6);
    let a = v * (v + 0.0245786) - 0.000090537;
    let b = v * (0.983729 * v + 0.4329510) + 0.238081;
    return clamp(output_matrix * (a / b), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Khronos PBR Neutral.
fn pbr_neutral(color: vec3<f32>) -> vec3<f32> {
    let start_compression = 0.8 - 0.04;
    let desaturation = 0.15;
    let lowest = min(color.r, min(color.g, color.b));
    var offset = 0.04;
    if (lowest < 0.08) {
        offset = lowest - 6.25 * lowest * lowest;
    }
    let shifted = color - offset;
    let peak = max(shifted.r, max(shifted.g, shifted.b));
    if (peak < start_compression) {
        return shifted;
    }
    let d = 1.0 - start_compression;
    let new_peak = 1.0 - d * d / (peak + d - start_compression);
    let scaled = shifted * (new_peak / peak);
    let blend = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(scaled, vec3<f32>(new_peak), blend);
}

fn srgb_oetf(linear: vec3<f32>) -> vec3<f32> {
    let low = linear * 12.92;
    let high = 1.055 * pow(linear, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, linear <= vec3<f32>(0.0031308));
}

@fragment
fn fs_tonemap(input: FullscreenOutput) -> @location(0) vec4<f32> {
    let exposure = clamp(sanitize_channel(textureLoad(exposure_texture, vec2<i32>(0, 0), 0).r), 0.0, 1.0e6);
    var color = min(sample_hdr(input.uv) * exposure, vec3<f32>(MAX_FINITE_HALF));
    if (HAS_BLOOM) {
        color += textureSampleLevel(bloom, linear_sampler, input.uv, 0.0).rgb * params.bloom_intensity;
    }
    var display: vec3<f32>;
    if (OPERATOR == 1u) {
        display = pbr_neutral(color);
    } else {
        display = aces_fitted(color);
    }
    display = clamp(display, vec3<f32>(0.0), vec3<f32>(1.0));
    if (ENCODE_SRGB) {
        display = srgb_oetf(display);
    }
    return vec4<f32>(display, 1.0);
}
