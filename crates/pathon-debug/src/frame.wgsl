struct Frame {
    view_projection: mat4x4<f32>,
    viewport: vec2<f32>,
    depth_scale: vec2<f32>,
    depth_bias: f32,
    padding0: f32,
    padding1: f32,
    padding2: f32,
}

@group(0) @binding(0) var<uniform> frame: Frame;

// Colours are given as they should look on screen; targets that the hardware or the viewer decodes as linear get the
// decoded value so that the result looks the same.
override LINEARIZE: bool = true;

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

fn premultiplied(display: vec3<f32>, alpha: f32) -> vec4<f32> {
    return vec4<f32>(for_target(display) * alpha, alpha);
}
