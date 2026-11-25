struct Params {
    inverse_projection: mat4x4<f32>,
    mode: u32,
    level: u32,
    use_exposure_texture: u32,
    exposure: f32,
    depth_near: f32,
    depth_far: f32,
    motion_scale: f32,
    padding: f32,
}

@group(0) @binding(0) var<uniform> params: Params;

override LINEARIZE: bool = true;

const MODE_SCALAR: u32 = 0u;
const MODE_LINEAR_COLOR: u32 = 1u;
const MODE_EXPOSED_HDR: u32 = 2u;
const MODE_DEPTH: u32 = 3u;
const MODE_NORMALS: u32 = 4u;
const MODE_MOTION: u32 = 5u;
const MODE_MATERIAL_ID: u32 = 6u;
const TAU: f32 = 6.28318530718;

struct FullscreenOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> FullscreenOutput {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var output: FullscreenOutput;
    output.position = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    output.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return output;
}

fn srgb_eotf(display: vec3<f32>) -> vec3<f32> {
    let low = display / 12.92;
    let high = pow((display + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, display <= vec3<f32>(0.04045));
}

fn srgb_oetf(linear: vec3<f32>) -> vec3<f32> {
    let low = linear * 12.92;
    let high = 1.055 * pow(linear, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, linear <= vec3<f32>(0.0031308));
}

fn for_target(display: vec3<f32>) -> vec3<f32> {
    if (LINEARIZE) {
        return srgb_eotf(display);
    }
    return display;
}

// WGSL has no isnan: the bits decide, so the check survives the optimizers of every backend.
fn is_finite_value(value: f32) -> bool {
    return (bitcast<u32>(value) & 0x7f800000u) != 0x7f800000u;
}

fn all_finite(value: vec3<f32>) -> bool {
    return is_finite_value(value.x) && is_finite_value(value.y) && is_finite_value(value.z);
}

// Black, purple, red, orange, pale yellow: bright is high.
fn heat(value: f32) -> vec3<f32> {
    let x = clamp(value, 0.0, 1.0) * 4.0;
    let c0 = vec3<f32>(0.0, 0.0, 0.0);
    let c1 = vec3<f32>(0.35, 0.05, 0.55);
    let c2 = vec3<f32>(0.85, 0.15, 0.25);
    let c3 = vec3<f32>(0.98, 0.65, 0.05);
    let c4 = vec3<f32>(1.0, 1.0, 0.8);
    if (x < 1.0) {
        return mix(c0, c1, x);
    }
    if (x < 2.0) {
        return mix(c1, c2, x - 1.0);
    }
    if (x < 3.0) {
        return mix(c2, c3, x - 2.0);
    }
    return mix(c3, c4, x - 3.0);
}

fn hue_color(hue: f32) -> vec3<f32> {
    let h = fract(hue) * 6.0;
    return clamp(vec3<f32>(abs(h - 3.0) - 1.0, 2.0 - abs(h - 2.0), 2.0 - abs(h - 4.0)), vec3<f32>(0.0), vec3<f32>(1.0));
}

// PCG hash; the same ids always get the same colour and neighbouring ids look unrelated.
fn hash_color(id: u32) -> vec3<f32> {
    let state = id * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    let hashed = (word >> 22u) ^ word;
    let channels = vec3<u32>(hashed & 255u, (hashed >> 8u) & 255u, (hashed >> 16u) & 255u);
    return vec3<f32>(channels) / 255.0 * 0.75 + 0.25;
}

fn sign_not_zero(value: vec2<f32>) -> vec2<f32> {
    return select(vec2<f32>(-1.0), vec2<f32>(1.0), value >= vec2<f32>(0.0));
}

fn oct_decode(encoded: vec2<f32>) -> vec3<f32> {
    var normal = vec3<f32>(encoded.x, encoded.y, 1.0 - abs(encoded.x) - abs(encoded.y));
    if (normal.z < 0.0) {
        let folded = (vec2<f32>(1.0) - abs(normal.yx)) * sign_not_zero(normal.xy);
        normal = vec3<f32>(folded, normal.z);
    }
    return normalize(normal);
}

// Distance along the view axis of a reverse-Z depth value, 0 for "nothing there".
fn view_distance(depth: f32, uv: vec2<f32>) -> f32 {
    if (!(depth > 0.0)) {
        return 0.0;
    }
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let view = params.inverse_projection * vec4<f32>(ndc, depth, 1.0);
    return max(-view.z / view.w, 0.0);
}

fn depth_color(depth: f32, uv: vec2<f32>) -> vec3<f32> {
    let distance_metres = view_distance(depth, uv);
    if (distance_metres <= 0.0) {
        return vec3<f32>(0.0);
    }
    let t = clamp((distance_metres - params.depth_near) / (params.depth_far - params.depth_near), 0.0, 1.0);
    return heat(1.0 - t);
}

fn motion_color(motion: vec2<f32>) -> vec3<f32> {
    let length_pixels = length(motion);
    if (length_pixels < 1.0e-4) {
        return vec3<f32>(0.0);
    }
    let hue = atan2(motion.y, motion.x) / TAU;
    return hue_color(hue) * clamp(length_pixels / params.motion_scale, 0.0, 1.0);
}

@fragment
fn fs_missing(input: FullscreenOutput) -> @location(0) vec4<f32> {
    let cell = (u32(input.position.x) / 8u + u32(input.position.y) / 8u) & 1u;
    return vec4<f32>(for_target(select(vec3<f32>(0.12), vec3<f32>(0.9, 0.0, 0.9), cell == 0u)), 1.0);
}
