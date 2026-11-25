// The viewer's forward shader (M4-10): the standard material of pathon-material lit by every light of the scene plus a
// hemisphere ambient term and one shadow map for the sun. Image-based lighting, cascaded shadows and the post chain
// replace these stand-ins in M4-05..M4-09.

import package::material::standard::{MaterialData, evaluate, load_surface, output_alpha, passes_alpha_test};
import package::material::surface::{LightSample, SurfaceInput};

const PI: f32 = 3.14159265358979;
const KIND_DIRECTIONAL: f32 = 0.0;
const KIND_SPOT: f32 = 2.0;
const SHADOW_NORMAL_OFFSET: f32 = 0.02;
const SHADOW_DEPTH_BIAS: f32 = 0.00002;

struct Frame {
    view_projection: mat4x4<f32>,
    sun_view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    // x exposure, y emissive scale, z ambient sky lux, w ambient ground lux.
    params: vec4<f32>,
    // x light count, y 1 when the sun has a shadow map, z index of the sun in `lights`.
    counts: vec4<u32>,
}

struct Instance {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    // x material, y 1 when the model matrix mirrors (negative determinant).
    material: vec4<u32>,
}

struct Light {
    position_range: vec4<f32>,
    direction_kind: vec4<f32>,
    // Colour times lux (directional) or candela (point, spot).
    color_intensity: vec4<f32>,
    // x cosine of the outer cone, y 1 / (cosine of the inner cone - cosine of the outer cone).
    spot: vec4<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> materials: array<MaterialData>;
@group(0) @binding(2) var<storage, read> instances: array<Instance>;
@group(0) @binding(3) var<storage, read> lights: array<Light>;
@group(0) @binding(4) var sun_shadow: texture_depth_2d;
@group(0) @binding(5) var sun_shadow_sampler: sampler_comparison;

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) @interpolate(flat) material: u32,
    @location(5) @interpolate(flat) mirrored: u32,
}

@vertex
fn vs(
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec2<f32>,
) -> Varying {
    let instance = instances[instance_index];
    let world = instance.model * vec4<f32>(position, 1.0);
    var out: Varying;
    out.clip = frame.view_projection * world;
    out.world_position = world.xyz;
    out.normal = normalize((instance.normal_matrix * vec4<f32>(normal, 0.0)).xyz);
    out.tangent = vec4<f32>((instance.model * vec4<f32>(tangent.xyz, 0.0)).xyz, tangent.w);
    out.uv = uv;
    out.material = instance.material.x;
    out.mirrored = instance.material.y;
    return out;
}

// The depth-only pass of the sun's shadow map reads positions alone.
@vertex
fn vs_shadow(
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
) -> @builtin(position) vec4<f32> {
    return frame.sun_view_projection * instances[instance_index].model * vec4<f32>(position, 1.0);
}

fn spot_factor(light: Light, to_light: vec3<f32>) -> f32 {
    let cosine = dot(light.direction_kind.xyz, -to_light);
    let t = clamp((cosine - light.spot.x) * light.spot.y, 0.0, 1.0);
    return t * t;
}

// glTF punctual lights: the range window is a smooth cutoff, zero range means unlimited.
fn range_window(distance_squared: f32, range: f32) -> f32 {
    if (range <= 0.0) {
        return 1.0;
    }
    let ratio = distance_squared / (range * range);
    let window = clamp(1.0 - ratio * ratio, 0.0, 1.0);
    return window * window;
}

fn sample_light(light: Light, position: vec3<f32>) -> LightSample {
    let color = light.color_intensity.rgb;
    if (light.direction_kind.w == KIND_DIRECTIONAL) {
        return LightSample(-light.direction_kind.xyz, color);
    }
    let offset = light.position_range.xyz - position;
    let distance_squared = max(dot(offset, offset), 0.0001);
    let to_light = offset * inverseSqrt(distance_squared);
    var attenuation = range_window(distance_squared, light.position_range.w) / distance_squared;
    if (light.direction_kind.w == KIND_SPOT) {
        attenuation = attenuation * spot_factor(light, to_light);
    }
    return LightSample(to_light, color * attenuation);
}

fn sun_visibility(position: vec3<f32>, normal: vec3<f32>) -> f32 {
    let clip = frame.sun_view_projection * vec4<f32>(position + normal * SHADOW_NORMAL_OFFSET, 1.0);
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    let texel = 1.0 / vec2<f32>(textureDimensions(sun_shadow));
    var lit = 0.0;
    // A 3x3 percentage-closer filter. The map is reverse-Z, so a lit point is at least as near as the occluder.
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel;
            lit += textureSampleCompareLevel(sun_shadow, sun_shadow_sampler, uv + offset, ndc.z + SHADOW_DEPTH_BIAS);
        }
    }
    return lit / 9.0;
}

@fragment
fn fs(in: Varying, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    var input: SurfaceInput;
    input.position = in.world_position;
    input.normal = in.normal;
    input.tangent = in.tangent;
    input.uv = in.uv;
    // The material never takes screen-space derivatives itself: the entry point hands them over.
    input.uv_dx = dpdx(in.uv);
    input.uv_dy = dpdy(in.uv);
    input.color = vec4<f32>(1.0);
    // A mirrored model flips the winding, and with it the hardware's idea of which side faces the camera.
    input.front_facing = front_facing != (in.mirrored == 1u);

    let surface = load_surface(materials[in.material], input);
    if (!passes_alpha_test(surface)) {
        discard;
    }

    let view = normalize(frame.camera_position.xyz - in.world_position);
    var diffuse = vec3<f32>(0.0);
    var specular = vec3<f32>(0.0);
    for (var index = 0u; index < frame.counts.x; index++) {
        var sample = sample_light(lights[index], in.world_position);
        if (frame.counts.y == 1u && index == frame.counts.z) {
            sample.radiance = sample.radiance * sun_visibility(in.world_position, surface.geometric_normal);
        }
        let result = evaluate(surface, view, sample);
        diffuse += result.diffuse;
        specular += result.specular;
    }

    let up = surface.normal.y * 0.5 + 0.5;
    let ambient_lux = mix(frame.params.w, frame.params.z, up);
    diffuse += surface.diffuse_color * ambient_lux / PI * surface.occlusion;
    specular += surface.f0 * ambient_lux / PI * 0.25 * (1.0 - surface.roughness) * surface.occlusion;

    var alpha = output_alpha(surface);
    // No refraction: a transmissive surface keeps only a trace of its own colour and lets the scene through.
    @if(transmission) {
        alpha = alpha * (1.0 - 0.9 * surface.transmission);
    }
    let radiance = diffuse * alpha + specular + surface.emissive * frame.params.y;
    return vec4<f32>(radiance * frame.params.x, alpha);
}
