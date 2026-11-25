// Derived from Filament (Apache-2.0), Copyright (c) Google LLC — shaders/src/surface_ambient_occlusion.fs
// (SpecularAO_Lagarde, gtaoMultiBounce); changes: WGSL, multi-bounce always on, no bent normals.

// Lagarde and de Rousiers 2014, "Moving Frostbite to PBR": how much of the reflected environment survives an ambient
// occlusion `visibility` for this view angle and roughness.
fn specular_ao_lagarde(n_dot_v: f32, visibility: f32, roughness: f32) -> f32 {
    return saturate(pow(n_dot_v + visibility, exp2(-16.0 * roughness - 1.0)) - 1.0 + visibility);
}

// Jimenez et al. 2016, "Practical Realtime Strategies for Accurate Indirect Occlusion": occlusion that lets the light
// bounce between the occluders, brighter for bright albedo. `albedo` is the diffuse colour, or f0 for the specular term.
fn gtao_multi_bounce(visibility: f32, albedo: vec3<f32>) -> vec3<f32> {
    let a = 2.0404 * albedo - vec3<f32>(0.3324);
    let b = -4.7951 * albedo + vec3<f32>(0.6417);
    let c = 2.7552 * albedo + vec3<f32>(0.6903);
    return max(vec3<f32>(visibility), ((visibility * a + b) * visibility + c) * visibility);
}
