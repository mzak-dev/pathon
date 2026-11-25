// Known patterns over uv in [0, 1]^2, one per base slot. The values are simple on purpose: a test or a reader can
// tell from a picture whether a visualisation is right.

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

const NEAR: f32 = 0.05;
const KIND_DEPTH_PYRAMID: u32 = 0u;
const KIND_NORMALS: u32 = 1u;
const KIND_MOTION: u32 = 2u;
const KIND_ROUGHNESS: u32 = 3u;
const KIND_ALBEDO: u32 = 4u;
const KIND_REACTIVE: u32 = 5u;
const KIND_DIFFUSE: u32 = 6u;
const KIND_SPECULAR: u32 = 7u;
const KIND_HDR: u32 = 8u;
const KIND_AO: u32 = 9u;
const KIND_EXPOSURE: u32 = 10u;

override KIND: u32 = 0u;

// View distance in metres: 2 m at the left edge, 12 m at the right one; the bottom strip is empty (0).
fn scene_distance(uv: vec2<f32>) -> f32 {
    if (uv.y > 0.85) {
        return 0.0;
    }
    return mix(2.0, 12.0, uv.x);
}

// Reverse-Z of the default camera (near plane 0.05, infinite far plane).
fn scene_depth(uv: vec2<f32>) -> f32 {
    let distance_metres = scene_distance(uv);
    if (distance_metres <= 0.0) {
        return 0.0;
    }
    return NEAR / distance_metres;
}

fn sign_not_zero(value: vec2<f32>) -> vec2<f32> {
    return select(vec2<f32>(-1.0), vec2<f32>(1.0), value >= vec2<f32>(0.0));
}

fn oct_encode(normal: vec3<f32>) -> vec2<f32> {
    let projected = normal.xy / (abs(normal.x) + abs(normal.y) + abs(normal.z));
    if (normal.z < 0.0) {
        return (vec2<f32>(1.0) - abs(projected.yx)) * sign_not_zero(projected);
    }
    return projected;
}

// A sphere facing the camera in the middle of the picture, a flat wall (0, 0, 1) around it.
fn sphere_normal(uv: vec2<f32>) -> vec3<f32> {
    let p = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0) * 1.25;
    let r2 = dot(p, p);
    if (r2 < 1.0) {
        return vec3<f32>(p, sqrt(1.0 - r2));
    }
    return vec3<f32>(0.0, 0.0, 1.0);
}

fn color_bar(index: u32) -> vec3<f32> {
    switch index {
        case 0u: { return vec3<f32>(1.0, 1.0, 1.0); }
        case 1u: { return vec3<f32>(1.0, 1.0, 0.0); }
        case 2u: { return vec3<f32>(0.0, 1.0, 1.0); }
        case 3u: { return vec3<f32>(0.0, 1.0, 0.0); }
        case 4u: { return vec3<f32>(1.0, 0.0, 1.0); }
        case 5u: { return vec3<f32>(1.0, 0.0, 0.0); }
        case 6u: { return vec3<f32>(0.0, 0.0, 1.0); }
        default: { return vec3<f32>(0.05, 0.05, 0.05); }
    }
}

fn infinity() -> f32 {
    return bitcast<f32>(0x7f800000u);
}

@fragment
fn fs_float(input: FullscreenOutput) -> @location(0) vec4<f32> {
    let uv = input.uv;
    switch KIND {
        case KIND_DEPTH_PYRAMID: {
            return vec4<f32>(scene_depth(uv), 0.0, 0.0, 1.0);
        }
        case KIND_NORMALS: {
            return vec4<f32>(oct_encode(sphere_normal(uv)), 0.0, 1.0);
        }
        case KIND_MOTION: {
            // Radial: pointing away from the centre, 32 px at the edge of the picture.
            return vec4<f32>((uv.x - 0.5) * 32.0, (uv.y - 0.5) * 32.0, 0.0, 1.0);
        }
        case KIND_ROUGHNESS: {
            return vec4<f32>(uv.x, 0.0, 0.0, 1.0);
        }
        case KIND_ALBEDO: {
            if (uv.y < 0.5) {
                return vec4<f32>(color_bar(u32(uv.x * 8.0)) * 0.8, 1.0);
            }
            return vec4<f32>(vec3<f32>(uv.x), 1.0);
        }
        case KIND_REACTIVE: {
            let cell = (u32(uv.x * 8.0) + u32(uv.y * 8.0)) & 1u;
            return vec4<f32>(f32(cell), 0.0, 0.0, 1.0);
        }
        case KIND_DIFFUSE: {
            return vec4<f32>(uv.x * 4.0, uv.y, 0.1, 1.0);
        }
        case KIND_SPECULAR: {
            return vec4<f32>(0.1, uv.y, uv.x * 4.0, 1.0);
        }
        case KIND_HDR: {
            // 0.18 * 2^x over eight stops left to right, with a patch of infinity in the corner.
            if (uv.x < 0.06 && uv.y < 0.06) {
                return vec4<f32>(infinity(), 0.0, 0.0, 1.0);
            }
            let warmth = vec3<f32>(1.0, 1.0 - 0.5 * uv.y, 1.0 - uv.y);
            return vec4<f32>(vec3<f32>(0.18 * exp2(uv.x * 8.0 - 4.0)) * warmth, 1.0);
        }
        case KIND_AO: {
            return vec4<f32>(1.0 - clamp(length(uv - vec2<f32>(0.5)) * 1.6, 0.0, 1.0), 0.0, 0.0, 1.0);
        }
        default: {
            return vec4<f32>(2.0, 0.0, 0.0, 1.0);
        }
    }
}

@fragment
fn fs_depth(input: FullscreenOutput) -> @builtin(frag_depth) f32 {
    return scene_depth(input.uv);
}

// Sixteen material ids in a 4 x 4 grid.
@fragment
fn fs_material_id(input: FullscreenOutput) -> @location(0) vec4<u32> {
    let cell = vec2<u32>(min(input.uv * 4.0, vec2<f32>(3.0)));
    return vec4<u32>(cell.x + 4u * cell.y, 0u, 0u, 1u);
}
