const MAX_FINITE_HALF: f32 = 65504.0;

// WGSL has no isnan: the bits decide, so the check survives the optimizers of every backend.
fn sanitize_channel(value: f32) -> f32 {
    let bits = bitcast<u32>(value);
    if ((bits & 0x7f800000u) == 0x7f800000u) {
        if ((bits & 0x007fffffu) != 0u || (bits & 0x80000000u) != 0u) {
            return 0.0;
        }
        return MAX_FINITE_HALF;
    }
    return clamp(value, 0.0, MAX_FINITE_HALF);
}

fn sanitize(color: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(sanitize_channel(color.x), sanitize_channel(color.y), sanitize_channel(color.z));
}

fn luminance(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
}
