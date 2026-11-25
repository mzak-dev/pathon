@group(0) @binding(1) var transmittance_lut: texture_2d<f32>;
@group(0) @binding(2) var lut_sampler: sampler;

// Transmittance to the top of the atmosphere; the ray must not meet the ground.
fn transmittance_to_top(radius: f32, mu: f32) -> vec3<f32> {
    return textureSampleLevel(transmittance_lut, lut_sampler, transmittance_uv(radius, mu), 0.0).rgb;
}

// Transmittance towards the sun, zero where the planet hides it.
fn sun_transmittance(radius: f32, mu_sun: f32) -> vec3<f32> {
    if (mu_sun < horizon_cosine(radius)) {
        return vec3<f32>(0.0);
    }
    return transmittance_to_top(radius, mu_sun);
}

// Light the sun leaves on the ground at `distance` along `direction`, reflected by a Lambertian surface.
fn ground_light(camera_radius: f32, direction: vec3<f32>, distance: f32, sun: vec3<f32>) -> vec3<f32> {
    let point = point_along(camera_radius, direction, distance);
    let cosine = dot(point.up, sun);
    return sun_transmittance(point.radius, cosine) * sky.ground_albedo.xyz * max(cosine, 0.0) / PI;
}
