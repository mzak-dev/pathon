import package::ibl::sample::{ibl_brdf, ibl_diffuse, ibl_specular, ibl_split_sum};

const MIN_N_DOT_V: f32 = 0.0001;

struct IblInput {
    normal: vec3<f32>,
    view: vec3<f32>,
    diffuse_color: vec3<f32>,
    f0: vec3<f32>,
    f90: vec3<f32>,
    roughness: f32,
}

struct IblResult {
    diffuse: vec3<f32>,
    specular: vec3<f32>,
}

// Lagarde's dominant direction: a rough lobe leans from the mirror direction towards the normal.
fn ibl_dominant_direction(n: vec3<f32>, r: vec3<f32>, roughness: f32) -> vec3<f32> {
    let alpha = roughness * roughness;
    return normalize(mix(n, r, (1.0 - alpha) * (sqrt(1.0 - alpha) + alpha)));
}

fn ibl_evaluate(
    irradiance: texture_cube<f32>,
    prefiltered: texture_cube<f32>,
    brdf_lut: texture_2d<f32>,
    ibl_sampler: sampler,
    input: IblInput,
) -> IblResult {
    var result: IblResult;
    let n = input.normal;
    let n_dot_v = max(dot(n, input.view), MIN_N_DOT_V);
    let r = reflect(-input.view, n);

    let brdf = ibl_brdf(brdf_lut, ibl_sampler, n_dot_v, input.roughness);
    let albedo = ibl_split_sum(input.f0, input.f90, brdf);
    // Fdez-Aguera: light that scatters between the microfacets and comes out again, which the single-scattering LUT
    // lacks; it matters for rough metals, which would otherwise lose up to a third of their energy.
    let multiple_scattering = vec3<f32>(1.0) + input.f0 * (1.0 / max(brdf.x + brdf.y, 0.0001) - 1.0);

    let radiance = ibl_specular(prefiltered, ibl_sampler, ibl_dominant_direction(n, r, input.roughness), input.roughness);
    result.specular = radiance * albedo * multiple_scattering;
    result.diffuse = ibl_diffuse(irradiance, ibl_sampler, n) * input.diffuse_color
        * max(vec3<f32>(1.0) - albedo, vec3<f32>(0.0));
    return result;
}
