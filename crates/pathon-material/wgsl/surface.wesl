struct SurfaceInput {
    position: vec3<f32>,
    normal: vec3<f32>,
    tangent: vec4<f32>,
    uv: vec2<f32>,
    uv_dx: vec2<f32>,
    uv_dy: vec2<f32>,
    color: vec4<f32>,
    front_facing: bool,
}

struct Surface {
    position: vec3<f32>,
    geometric_normal: vec3<f32>,
    normal: vec3<f32>,
    base_color: vec3<f32>,
    alpha: f32,
    alpha_cutoff: f32,
    metallic: f32,
    roughness: f32,
    occlusion: f32,
    emissive: vec3<f32>,
    diffuse_color: vec3<f32>,
    f0: vec3<f32>,
    f90: vec3<f32>,
    clearcoat: f32,
    clearcoat_roughness: f32,
    transmission: f32,
    thickness: f32,
    attenuation_color: vec3<f32>,
    attenuation_distance: f32,
    diffuse_transmission: f32,
    diffuse_transmission_color: vec3<f32>,
}

struct LightSample {
    direction: vec3<f32>,
    radiance: vec3<f32>,
}

struct BsdfResult {
    diffuse: vec3<f32>,
    specular: vec3<f32>,
}
