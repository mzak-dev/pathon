struct Params {
    face_size: u32,
    sample_count: u32,
    roughness: f32,
    // Solid angle of one texel of the source cube's mip 0.
    texel_solid_angle: f32,
    top_lod: f32,
    padding0: u32,
    padding1: u32,
    padding2: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_cube<f32>;
@group(0) @binding(2) var source_sampler: sampler;
@group(0) @binding(3) var destination: texture_storage_2d_array<rgba16float, write>;

// Split-sum prefilter: the normal, the view and the reflection direction are one vector, so every sample is a
// GGX-distributed light direction around it, weighted by n.l. Samples read a mip of the source whose texel is about
// as large as the sample's solid angle, which keeps a small bright sun from turning into fireflies.
@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.face_size || id.y >= params.face_size) {
        return;
    }
    let n = texel_direction(id.z, f32(id.x) + 0.5, f32(id.y) + 0.5, f32(params.face_size));
    var color = textureSampleLevel(source, source_sampler, n, 0.0).rgb;
    if (params.roughness > 0.0) {
        let alpha = params.roughness * params.roughness;
        let frame = tangent_frame(n);
        var sum = vec3<f32>(0.0);
        var weight = 0.0;
        for (var i = 0u; i < params.sample_count; i++) {
            let h = frame * importance_sample_ggx(hammersley(i, params.sample_count), alpha);
            let l = 2.0 * dot(n, h) * h - n;
            let n_dot_l = dot(n, l);
            if (n_dot_l > 0.0) {
                let n_dot_h = max(dot(n, h), 0.0001);
                // pdf of l is D n.h / (4 v.h), and v.h = n.h here.
                let pdf = distribution_ggx(n_dot_h, alpha) * 0.25;
                let sample_solid_angle = 1.0 / (f32(params.sample_count) * pdf + 0.0001);
                let lod = clamp(0.5 * log2(sample_solid_angle / params.texel_solid_angle) + 1.0, 0.0, params.top_lod);
                sum += textureSampleLevel(source, source_sampler, l, lod).rgb * n_dot_l;
                weight += n_dot_l;
            }
        }
        color = sum / max(weight, 0.0001);
    }
    textureStore(destination, vec2<i32>(id.xy), i32(id.z), vec4<f32>(min(color, vec3<f32>(MAX_RADIANCE)), 1.0));
}
