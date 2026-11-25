@group(0) @binding(4) var lut_out: texture_storage_2d<rgba16float, write>;

override STEPS: u32 = 20u;
const SIDE: u32 = 8u;

// Psi_ms of Hillaire 2020: second-order scattering with an isotropic phase, summed over all higher orders.
@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = sky.sizes.z;
    if (f32(id.x) >= size || f32(id.y) >= size) {
        return;
    }
    let unit = vec2<f32>(id.xy) / (size - 1.0);
    let mu_sun = unit.x * 2.0 - 1.0;
    let radius = bottom_radius() + unit.y * (top_radius() - bottom_radius());
    let sun = vec3<f32>(sqrt(max(1.0 - mu_sun * mu_sun, 0.0)), mu_sun, 0.0);

    var luminance = vec3<f32>(0.0);
    var transfer = vec3<f32>(0.0);
    for (var i = 0u; i < SIDE; i++) {
        for (var j = 0u; j < SIDE; j++) {
            let theta = 2.0 * PI * (f32(i) + 0.5) / f32(SIDE);
            let cosine = 1.0 - 2.0 * (f32(j) + 0.5) / f32(SIDE);
            let sine = sqrt(max(1.0 - cosine * cosine, 0.0));
            let direction = vec3<f32>(cos(theta) * sine, cosine, sin(theta) * sine);
            let ground = ground_distance(radius, direction.y);
            let has_ground = ground >= 0.0;
            let t_max = select(top_distance(radius, direction.y), ground, has_ground);
            let step = t_max / f32(STEPS);
            var throughput = vec3<f32>(1.0);
            for (var index = 0u; index < STEPS; index++) {
                let point = point_along(radius, direction, (f32(index) + 0.5) * step);
                let medium = medium_at(point.altitude);
                let step_transmittance = exp(-medium.extinction * step);
                let scattering = medium.rayleigh + medium.mie;
                let source = sun_transmittance(point.radius, dot(point.up, sun)) * scattering / (4.0 * PI);
                luminance += throughput * integrate_step(source, medium.extinction, step_transmittance, step);
                transfer += throughput * integrate_step(scattering, medium.extinction, step_transmittance, step);
                throughput *= step_transmittance;
            }
            if (has_ground) {
                luminance += throughput * ground_light(radius, direction, ground, sun);
            }
        }
    }
    let samples = f32(SIDE * SIDE);
    luminance /= samples;
    transfer /= samples;
    textureStore(lut_out, vec2<i32>(id.xy), vec4<f32>(luminance / (vec3<f32>(1.0) - transfer), 1.0));
}
