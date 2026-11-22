import package::material::hooks::modify_surface;
import package::material::surface::{BsdfResult, LightSample, Surface, SurfaceInput};
import package::material::texture_source::sample_material_texture;

const PI: f32 = 3.14159265358979;
const MIN_ROUGHNESS: f32 = 0.045;
const MIN_N_DOT_V: f32 = 0.0001;
const COAT_F0: f32 = 0.04;
const NO_TEXTURE: u32 = 0xffffffffu;

struct MaterialData {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    specular_color: vec4<f32>,
    attenuation_color: vec4<f32>,
    diffuse_transmission_color: vec4<f32>,
    metallic: f32,
    roughness: f32,
    ior: f32,
    normal_scale: f32,
    occlusion_strength: f32,
    clearcoat: f32,
    clearcoat_roughness: f32,
    transmission: f32,
    thickness: f32,
    alpha_cutoff: f32,
    flags: u32,
    base_color_texture: u32,
    metallic_roughness_texture: u32,
    normal_texture: u32,
    occlusion_texture: u32,
    emissive_texture: u32,
}

fn texel(slot: u32, input: SurfaceInput) -> vec4<f32> {
    if (slot == NO_TEXTURE) {
        return vec4<f32>(1.0);
    }
    return sample_material_texture(slot, input.uv, input.uv_dx, input.uv_dy);
}

// Normal in the frame of the interpolated normal, before the back-face flip.
fn shading_normal(material: MaterialData, input: SurfaceInput) -> vec3<f32> {
    let n = normalize(input.normal);
    if (material.normal_texture == NO_TEXTURE) {
        return n;
    }
    let t = normalize(input.tangent.xyz - n * dot(n, input.tangent.xyz));
    let b = cross(n, t) * input.tangent.w;
    let sampled = texel(material.normal_texture, input).xyz * 2.0 - 1.0;
    let tangent_space = vec3<f32>(sampled.xy * material.normal_scale, sampled.z);
    return normalize(mat3x3<f32>(t, b, n) * tangent_space);
}

fn load_surface(material: MaterialData, input: SurfaceInput) -> Surface {
    var surface: Surface;
    let facing = select(-1.0, 1.0, input.front_facing);

    let base = material.base_color * input.color * texel(material.base_color_texture, input);
    let metal_rough = texel(material.metallic_roughness_texture, input);
    let metallic = clamp(material.metallic * metal_rough.b, 0.0, 1.0);
    let ior_f0 = pow((material.ior - 1.0) / (material.ior + 1.0), 2.0);
    let specular_weight = material.specular_color.a;
    let dielectric_f0 = min(vec3<f32>(ior_f0) * material.specular_color.rgb, vec3<f32>(1.0)) * specular_weight;

    surface.position = input.position;
    surface.geometric_normal = normalize(input.normal) * facing;
    surface.normal = shading_normal(material, input) * facing;
    surface.base_color = base.rgb;
    surface.alpha = base.a;
    surface.alpha_cutoff = material.alpha_cutoff;
    surface.metallic = metallic;
    surface.roughness = clamp(material.roughness * metal_rough.g, MIN_ROUGHNESS, 1.0);
    surface.occlusion = 1.0 + material.occlusion_strength * (texel(material.occlusion_texture, input).r - 1.0);
    surface.emissive = material.emissive.rgb * texel(material.emissive_texture, input).rgb;
    surface.diffuse_color = base.rgb * (1.0 - metallic);
    surface.f0 = mix(dielectric_f0, base.rgb, metallic);
    surface.f90 = mix(vec3<f32>(specular_weight), vec3<f32>(1.0), metallic);

    @if(clearcoat) {
        surface.clearcoat = material.clearcoat;
        surface.clearcoat_roughness = clamp(material.clearcoat_roughness, MIN_ROUGHNESS, 1.0);
    }
    @if(transmission) {
        surface.transmission = material.transmission * (1.0 - metallic);
        surface.thickness = material.thickness;
        surface.attenuation_color = material.attenuation_color.rgb;
        surface.attenuation_distance = material.attenuation_color.a;
    }
    @if(subsurface) {
        surface.diffuse_transmission = material.diffuse_transmission_color.a * (1.0 - metallic);
        surface.diffuse_transmission_color = material.diffuse_transmission_color.rgb;
    }
    return modify_surface(input, surface);
}

@if(masked)
fn passes_alpha_test(surface: Surface) -> bool {
    return surface.alpha >= surface.alpha_cutoff;
}

@if(!masked)
fn passes_alpha_test(surface: Surface) -> bool {
    return true;
}

@if(transparent)
fn output_alpha(surface: Surface) -> f32 {
    return surface.alpha;
}

@if(!transparent)
fn output_alpha(surface: Surface) -> f32 {
    return 1.0;
}

fn fresnel_schlick(f0: vec3<f32>, f90: vec3<f32>, v_dot_h: f32) -> vec3<f32> {
    let x = clamp(1.0 - v_dot_h, 0.0, 1.0);
    let x2 = x * x;
    return f0 + (f90 - f0) * (x2 * x2 * x);
}

fn fresnel_schlick_scalar(f0: f32, f90: f32, v_dot_h: f32) -> f32 {
    return fresnel_schlick(vec3<f32>(f0), vec3<f32>(f90), v_dot_h).x;
}

fn distribution_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let f = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * f * f);
}

// Height-correlated Smith visibility, already divided by 4 n.l n.v.
fn visibility_smith_ggx(n_dot_l: f32, n_dot_v: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let ggx_v = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let ggx_l = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    let sum = ggx_v + ggx_l;
    return select(0.0, 0.5 / sum, sum > 0.0);
}

fn diffuse_burley(n_dot_l: f32, n_dot_v: f32, l_dot_h: f32, roughness: f32) -> f32 {
    let grazing = 0.5 + 2.0 * roughness * l_dot_h * l_dot_h;
    let light_scatter = 1.0 + (grazing - 1.0) * pow(1.0 - n_dot_l, 5.0);
    let view_scatter = 1.0 + (grazing - 1.0) * pow(1.0 - n_dot_v, 5.0);
    return light_scatter * view_scatter / PI;
}

@if(burley)
fn front_diffuse(surface: Surface, fresnel: vec3<f32>, n_dot_l: f32, n_dot_v: f32, l_dot_h: f32) -> vec3<f32> {
    return surface.diffuse_color * diffuse_burley(n_dot_l, n_dot_v, l_dot_h, surface.roughness);
}

@if(!burley)
fn front_diffuse(surface: Surface, fresnel: vec3<f32>, n_dot_l: f32, n_dot_v: f32, l_dot_h: f32) -> vec3<f32> {
    return surface.diffuse_color * (vec3<f32>(1.0) - fresnel) / PI;
}

// Radiance one light sample adds to the view direction, split into the diffuse and the specular lobes.
// `view` and `light.direction` are unit vectors pointing away from the surface.
fn evaluate(surface: Surface, view: vec3<f32>, light: LightSample) -> BsdfResult {
    var result: BsdfResult;
    let l = light.direction;
    let n = surface.normal;
    let signed_n_dot_l = dot(n, l);
    let n_dot_l = max(signed_n_dot_l, 0.0);
    let n_dot_v = max(abs(dot(n, view)), MIN_N_DOT_V);

    var diffuse_weight = 1.0;
    @if(transmission) {
        diffuse_weight = 1.0 - surface.transmission;
    }
    @if(subsurface) {
        if (signed_n_dot_l < 0.0) {
            // The light is behind the surface: only the diffuse transmission lobe reaches the viewer.
            result.diffuse = surface.diffuse_transmission * surface.diffuse_transmission_color * (1.0 - surface.metallic)
                * (-signed_n_dot_l / PI) * light.radiance;
            return result;
        }
        diffuse_weight = diffuse_weight * (1.0 - surface.diffuse_transmission);
    }
    if (n_dot_l <= 0.0) {
        return result;
    }

    let half_sum = l + view;
    let half_length_squared = dot(half_sum, half_sum);
    let h = select(n, half_sum * inverseSqrt(half_length_squared), half_length_squared > 0.00000001);
    let n_dot_h = clamp(dot(n, h), 0.0, 1.0);
    let v_dot_h = clamp(dot(view, h), 0.0, 1.0);

    let fresnel = fresnel_schlick(surface.f0, surface.f90, v_dot_h);
    let alpha = surface.roughness * surface.roughness;
    let diffuse = diffuse_weight * front_diffuse(surface, fresnel, n_dot_l, n_dot_v, v_dot_h);
    let specular = fresnel * (distribution_ggx(n_dot_h, alpha) * visibility_smith_ggx(n_dot_l, n_dot_v, alpha));

    var base_attenuation = 1.0;
    var coat = 0.0;
    @if(clearcoat) {
        let coat_n = surface.geometric_normal;
        let coat_n_dot_l = max(dot(coat_n, l), 0.0);
        let coat_n_dot_v = max(abs(dot(coat_n, view)), MIN_N_DOT_V);
        let coat_n_dot_h = clamp(dot(coat_n, h), 0.0, 1.0);
        let coat_alpha = surface.clearcoat_roughness * surface.clearcoat_roughness;
        let coat_fresnel = surface.clearcoat * fresnel_schlick_scalar(COAT_F0, 1.0, v_dot_h);
        base_attenuation = 1.0 - coat_fresnel;
        coat = coat_fresnel * distribution_ggx(coat_n_dot_h, coat_alpha)
            * visibility_smith_ggx(coat_n_dot_l, coat_n_dot_v, coat_alpha) * coat_n_dot_l;
    }

    result.diffuse = diffuse * (base_attenuation * n_dot_l) * light.radiance;
    result.specular = (specular * (base_attenuation * n_dot_l) + vec3<f32>(coat)) * light.radiance;
    return result;
}

// Weight of the light refracted through the surface; the caller multiplies it by what lies behind.
fn transmission_weight(surface: Surface, view: vec3<f32>) -> vec3<f32> {
    let n_dot_v = max(abs(dot(surface.normal, view)), MIN_N_DOT_V);
    let fresnel = fresnel_schlick(surface.f0, surface.f90, n_dot_v);
    return surface.transmission * surface.diffuse_color * (vec3<f32>(1.0) - fresnel);
}

// Beer-Lambert absorption over `distance` inside the volume; no attenuation distance means a clear volume.
fn volume_transmittance(surface: Surface, distance: f32) -> vec3<f32> {
    if (surface.attenuation_distance <= 0.0) {
        return vec3<f32>(1.0);
    }
    return pow(max(surface.attenuation_color, vec3<f32>(0.0001)), vec3<f32>(distance / surface.attenuation_distance));
}
