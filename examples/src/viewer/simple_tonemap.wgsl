// Placeholder for the post chain (M4-09): Narkowicz's fit of the ACES curve on the pre-exposed HDR image.

@group(0) @binding(0) var hdr: texture_2d<f32>;

struct Fullscreen {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs(@builtin(vertex_index) index: u32) -> Fullscreen {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return Fullscreen(vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0));
}

fn aces(color: vec3<f32>) -> vec3<f32> {
    let mapped = (color * (2.51 * color + 0.03)) / (color * (2.43 * color + 0.59) + 0.14);
    return clamp(mapped, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn srgb_encode(linear: vec3<f32>) -> vec3<f32> {
    let low = linear * 12.92;
    let high = 1.055 * pow(linear, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, linear <= vec3<f32>(0.0031308));
}

fn tonemapped(position: vec4<f32>) -> vec3<f32> {
    return aces(max(textureLoad(hdr, vec2<i32>(position.xy), 0).rgb, vec3<f32>(0.0)));
}

// The target is an sRGB format: the hardware encodes.
@fragment
fn fs_srgb_target(in: Fullscreen) -> @location(0) vec4<f32> {
    return vec4<f32>(tonemapped(in.clip), 1.0);
}

@fragment
fn fs_linear_target(in: Fullscreen) -> @location(0) vec4<f32> {
    return vec4<f32>(srgb_encode(tonemapped(in.clip)), 1.0);
}
