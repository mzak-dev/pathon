// Sky-view parametrisation of Hillaire 2020: the horizon is the middle row and rows are dense near it; columns are the
// angle to the azimuth of the sun and are dense towards the sun.

// (view zenith cosine, cosine of the azimuth between the view and the sun) of a texel uv.
fn sky_view_uv_to_params(texel_uv: vec2<f32>, radius: f32) -> vec2<f32> {
    let uv = vec2<f32>(sub_uv_to_unit(texel_uv.x, sky.view_sizes.x), sub_uv_to_unit(texel_uv.y, sky.view_sizes.y));
    let beta = acos(-horizon_cosine(radius));
    let horizon_angle = PI - beta;
    var mu: f32;
    if (uv.y < 0.5) {
        var coord = 1.0 - 2.0 * uv.y;
        coord = 1.0 - coord * coord;
        mu = cos(horizon_angle * coord);
    } else {
        let coord = uv.y * 2.0 - 1.0;
        mu = cos(horizon_angle + beta * coord * coord);
    }
    return vec2<f32>(mu, -(uv.x * uv.x * 2.0 - 1.0));
}

fn sky_view_params_to_uv(on_ground: bool, mu: f32, azimuth: f32, radius: f32) -> vec2<f32> {
    let beta = acos(-horizon_cosine(radius));
    let horizon_angle = PI - beta;
    let view_angle = acos(clamp(mu, -1.0, 1.0));
    var uv: vec2<f32>;
    if (!on_ground) {
        let coord = 1.0 - sqrt(max(1.0 - view_angle / horizon_angle, 0.0));
        uv.y = coord * 0.5;
    } else {
        uv.y = sqrt(max((view_angle - horizon_angle) / beta, 0.0)) * 0.5 + 0.5;
    }
    uv.x = sqrt(clamp(-azimuth * 0.5 + 0.5, 0.0, 1.0));
    return vec2<f32>(unit_to_sub_uv(uv.x, sky.view_sizes.x), unit_to_sub_uv(uv.y, sky.view_sizes.y));
}

fn azimuth_cosine(sun: vec3<f32>, view: vec3<f32>) -> f32 {
    let sun_horizontal = sun.xz;
    let view_horizontal = view.xz;
    let lengths = length(sun_horizontal) * length(view_horizontal);
    if (lengths < 1e-8) {
        return 1.0;
    }
    return clamp(dot(sun_horizontal, view_horizontal) / lengths, -1.0, 1.0);
}
