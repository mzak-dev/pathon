struct Params {
    size: u32,
    sample_count: u32,
    padding0: u32,
    padding1: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var lut: texture_storage_2d<rgba16float, write>;

// Split-sum environment BRDF: x is n.v, y is perceptual roughness, texel centres land on the coordinates the sampling
// side uses. Red is the scale of f0, green the bias that f90 multiplies; the directional albedo is f0 * r + f90 * g.
@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.size || id.y >= params.size) {
        return;
    }
    let n_dot_v = (f32(id.x) + 0.5) / f32(params.size);
    let roughness = (f32(id.y) + 0.5) / f32(params.size);
    let alpha = roughness * roughness;
    let v = vec3<f32>(sqrt(1.0 - n_dot_v * n_dot_v), 0.0, n_dot_v);

    var scale = 0.0;
    var bias = 0.0;
    for (var i = 0u; i < params.sample_count; i++) {
        let h = importance_sample_ggx(hammersley(i, params.sample_count), alpha);
        let l = 2.0 * dot(v, h) * h - v;
        let n_dot_l = l.z;
        let n_dot_h = h.z;
        let v_dot_h = max(dot(v, h), 0.0);
        if (n_dot_l > 0.0 && n_dot_h > 0.0) {
            // Visibility already holds 1 / (4 n.l n.v); the estimator is G_vis = G v.h / (n.h n.v).
            let g_vis = 4.0 * visibility_smith_ggx(n_dot_l, n_dot_v, alpha) * n_dot_l * v_dot_h / n_dot_h;
            let fresnel = pow(1.0 - v_dot_h, 5.0);
            scale += (1.0 - fresnel) * g_vis;
            bias += fresnel * g_vis;
        }
    }
    let inverse_count = 1.0 / f32(params.sample_count);
    textureStore(lut, vec2<i32>(id.xy), vec4<f32>(scale * inverse_count, bias * inverse_count, 0.0, 1.0));
}
