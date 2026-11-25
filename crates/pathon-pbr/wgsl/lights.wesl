// Derived from Filament (Apache-2.0), Copyright (c) Google LLC — shaders/src/surface_light_punctual.fs (distance
// falloff window, spot cone attenuation); changes: WGSL, one light array for directional, point and spot lights,
// radiance in physical units (lux for directional, candela for point and spot) instead of pre-exposed intensity.
import package::material::surface::LightSample;

const KIND_DIRECTIONAL: f32 = 0.0;
const KIND_POINT: f32 = 1.0;
const KIND_SPOT: f32 = 2.0;

// 64 bytes. Layout is mirrored by `light::GpuLight` in Rust.
struct GpuLight {
    // xyz: position, w: range in metres (0 for a directional light).
    position_range: vec4<f32>,
    // Directional: unit vector to the light. Spot: unit vector the light shines along. w: kind.
    direction_kind: vec4<f32>,
    // rgb: tint * photometric value (lux or candela).
    color_intensity: vec4<f32>,
    // x, y: spot angle scale and offset, z: 1 / range^2, w: unused.
    spot: vec4<f32>,
}

// Smooth window that brings the inverse-square law to exactly zero at the light's range, so a cluster can drop the
// light there without a visible edge.
fn square_falloff_attenuation(distance_square: f32, inverse_range_square: f32) -> f32 {
    let factor = distance_square * inverse_range_square;
    let smooth_factor = saturate(1.0 - factor * factor);
    return smooth_factor * smooth_factor;
}

fn angle_attenuation(to_light: vec3<f32>, axis: vec3<f32>, scale_offset: vec2<f32>) -> f32 {
    let cos_angle = dot(-axis, to_light);
    let attenuation = saturate(cos_angle * scale_offset.x + scale_offset.y);
    return attenuation * attenuation;
}

fn light_sample(light: GpuLight, position: vec3<f32>) -> LightSample {
    var result: LightSample;
    let kind = light.direction_kind.w;
    if (kind == KIND_DIRECTIONAL) {
        result.direction = light.direction_kind.xyz;
        result.radiance = light.color_intensity.rgb;
        return result;
    }
    let position_to_light = light.position_range.xyz - position;
    let distance_square = dot(position_to_light, position_to_light);
    result.direction = position_to_light * inverseSqrt(max(distance_square, 1.0e-8));
    var attenuation = square_falloff_attenuation(distance_square, light.spot.z) / max(distance_square, 1.0e-4);
    if (kind == KIND_SPOT) {
        attenuation = attenuation * angle_attenuation(result.direction, light.direction_kind.xyz, light.spot.xy);
    }
    result.radiance = light.color_intensity.rgb * attenuation;
    return result;
}
