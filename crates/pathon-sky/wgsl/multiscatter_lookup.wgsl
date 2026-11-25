@group(0) @binding(3) var multiscatter_lut: texture_2d<f32>;

fn multiscatter_at(radius: f32, mu_sun: f32) -> vec3<f32> {
    let u = clamp(mu_sun * 0.5 + 0.5, 0.0, 1.0);
    let v = clamp((radius - bottom_radius()) / (top_radius() - bottom_radius()), 0.0, 1.0);
    let size = sky.sizes.z;
    return textureSampleLevel(multiscatter_lut, lut_sampler, vec2<f32>(unit_to_sub_uv(u, size), unit_to_sub_uv(v, size)), 0.0).rgb;
}
