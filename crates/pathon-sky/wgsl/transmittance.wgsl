@group(0) @binding(4) var lut_out: texture_storage_2d<rgba16float, write>;

override STEPS: u32 = 40u;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = sky.sizes.xy;
    if (f32(id.x) >= size.x || f32(id.y) >= size.y) {
        return;
    }
    let params = transmittance_params((vec2<f32>(id.xy) + 0.5) / size);
    let step = top_distance(params.radius, params.mu) / f32(STEPS);
    let direction = vec3<f32>(sqrt(max(1.0 - params.mu * params.mu, 0.0)), params.mu, 0.0);
    var depth = vec3<f32>(0.0);
    for (var index = 0u; index < STEPS; index++) {
        let point = point_along(params.radius, direction, (f32(index) + 0.5) * step);
        depth += medium_at(point.altitude).extinction * step;
    }
    textureStore(lut_out, vec2<i32>(id.xy), vec4<f32>(exp(-depth), 1.0));
}
