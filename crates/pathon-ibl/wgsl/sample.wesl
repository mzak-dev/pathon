fn ibl_diffuse(irradiance: texture_cube<f32>, ibl_sampler: sampler, n: vec3<f32>) -> vec3<f32> {
    return textureSampleLevel(irradiance, ibl_sampler, n, 0.0).rgb;
}

fn ibl_specular(prefiltered: texture_cube<f32>, ibl_sampler: sampler, r: vec3<f32>, roughness: f32) -> vec3<f32> {
    let top_level = f32(textureNumLevels(prefiltered) - 1u);
    return textureSampleLevel(prefiltered, ibl_sampler, r, clamp(roughness, 0.0, 1.0) * top_level).rgb;
}

fn ibl_brdf(brdf_lut: texture_2d<f32>, ibl_sampler: sampler, n_dot_v: f32, roughness: f32) -> vec2<f32> {
    let uv = vec2<f32>(clamp(n_dot_v, 0.0, 1.0), clamp(roughness, 0.0, 1.0));
    return textureSampleLevel(brdf_lut, ibl_sampler, uv, 0.0).rg;
}

fn ibl_split_sum(f0: vec3<f32>, f90: vec3<f32>, brdf: vec2<f32>) -> vec3<f32> {
    return f0 * brdf.x + f90 * brdf.y;
}
