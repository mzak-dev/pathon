@group(0) @binding(4) var lut_out: texture_storage_2d<rgba16float, write>;

override STEPS: u32 = 32u;

// Luminance scattered towards the camera per unit sun illuminance, for one direction of the sky-view table.
@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = sky.view_sizes.xy;
    if (f32(id.x) >= size.x || f32(id.y) >= size.y) {
        return;
    }
    let radius = sky.radii.z;
    let params = sky_view_uv_to_params((vec2<f32>(id.xy) + 0.5) / size, radius);
    let mu_sun = sky.sun_direction.y;
    let sun = vec3<f32>(sqrt(max(1.0 - mu_sun * mu_sun, 0.0)), mu_sun, 0.0);
    let sine = sqrt(max(1.0 - params.x * params.x, 0.0));
    let direction = vec3<f32>(sine * params.y, params.x, sine * sqrt(max(1.0 - params.y * params.y, 0.0)));

    let ground = ground_distance(radius, direction.y);
    let has_ground = ground >= 0.0;
    let t_max = select(top_distance(radius, direction.y), ground, has_ground);
    let view_sun = dot(direction, sun);
    let rayleigh_weight = rayleigh_phase(view_sun);
    let mie_weight = mie_phase(view_sun);

    var luminance = vec3<f32>(0.0);
    var throughput = vec3<f32>(1.0);
    for (var index = 0u; index < STEPS; index++) {
        let begin = f32(index) / f32(STEPS);
        let finish = f32(index + 1u) / f32(STEPS);
        let start = t_max * begin * begin;
        let end = t_max * finish * finish;
        let step = end - start;
        let point = point_along(radius, direction, 0.5 * (start + end));
        let medium = medium_at(point.altitude);
        let step_transmittance = exp(-medium.extinction * step);
        let mu_here = dot(point.up, sun);
        let source = sun_transmittance(point.radius, mu_here) * (medium.rayleigh * rayleigh_weight + medium.mie * mie_weight)
            + multiscatter_at(point.radius, mu_here) * (medium.rayleigh + medium.mie);
        luminance += throughput * integrate_step(source, medium.extinction, step_transmittance, step);
        throughput *= step_transmittance;
    }
    if (has_ground) {
        luminance += throughput * ground_light(radius, direction, ground, sun);
    }
    textureStore(lut_out, vec2<i32>(id.xy), vec4<f32>(luminance, 1.0));
}
