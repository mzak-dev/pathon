#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

use pathon_material::{
    AlphaMode, DiffuseModel, MaterialHooks, MaterialTextures, MaterialVariant, StandardMaterial, TextureSlot,
};
use pathon_shader::ShaderLibrary;

pub const OUTPUTS_PER_POINT: usize = 5;
pub const POINT_COUNT: usize = 32;
pub const MATERIAL_COUNT: usize = 8;

// One shader that evaluates a test point two ways: a compute thread per point (`cs`) and a fragment per channel
// (`fs`). Both call `shade`, so any difference is the stage, not the material.
pub const TEST_ROOT: &str = "
import package::material::standard::{
    MaterialData, evaluate, load_surface, output_alpha, passes_alpha_test, transmission_weight, volume_transmittance,
};
import package::material::surface::{LightSample, SurfaceInput};

struct TestPoint {
    position: vec4<f32>,
    normal: vec4<f32>,
    tangent: vec4<f32>,
    uv: vec4<f32>,
    uv_gradients: vec4<f32>,
    color: vec4<f32>,
    view: vec4<f32>,
    light_direction: vec4<f32>,
    light_radiance: vec4<f32>,
    material: vec4<u32>,
}

@group(0) @binding(0) var<storage, read> materials: array<MaterialData>;
@group(0) @binding(1) var<storage, read> points: array<TestPoint>;
@group(0) @binding(2) var<storage, read_write> results: array<vec4<f32>>;

fn shade(index: u32, channel: u32) -> vec4<f32> {
    let point = points[index];
    var input: SurfaceInput;
    input.position = point.position.xyz;
    input.normal = point.normal.xyz;
    input.front_facing = point.normal.w > 0.5;
    input.tangent = point.tangent;
    input.uv = point.uv.xy;
    input.uv_dx = point.uv_gradients.xy;
    input.uv_dy = point.uv_gradients.zw;
    input.color = point.color;
    let surface = load_surface(materials[point.material.x], input);
    let view = point.view.xyz;
    let bsdf = evaluate(surface, view, LightSample(point.light_direction.xyz, point.light_radiance.xyz));
    switch channel {
        case 0u: { return vec4<f32>(bsdf.diffuse, output_alpha(surface)); }
        case 1u: { return vec4<f32>(bsdf.specular, select(0.0, 1.0, passes_alpha_test(surface))); }
        case 2u: { return vec4<f32>(transmission_weight(surface, view), surface.occlusion); }
        case 3u: { return vec4<f32>(surface.emissive, surface.thickness); }
        default: { return vec4<f32>(volume_transmittance(surface, 1.5), surface.metallic); }
    }
}

@compute @workgroup_size(16)
fn cs(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= arrayLength(&points)) {
        return;
    }
    for (var channel = 0u; channel < 5u; channel++) {
        results[id.x * 5u + channel] = shade(id.x, channel);
    }
}

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = u32(position.x);
    return shade(pixel / 5u, pixel % 5u);
}
";

pub const ROOT_NAME: &str = "test";

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TestPoint {
    pub position: [f32; 4],
    pub normal: [f32; 4],
    pub tangent: [f32; 4],
    pub uv: [f32; 4],
    pub uv_gradients: [f32; 4],
    pub color: [f32; 4],
    pub view: [f32; 4],
    pub light_direction: [f32; 4],
    pub light_radiance: [f32; 4],
    pub material: [u32; 4],
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / length, v[1] / length, v[2] / length]
}

fn direction(polar: f32, azimuth: f32) -> [f32; 3] {
    [polar.sin() * azimuth.cos(), polar.sin() * azimuth.sin(), polar.cos()]
}

// Normals tilt a little, views stay above the horizon, lights sweep from overhead to behind the surface, and a few
// points are seen from the back, so every branch of the BSDF is reached by some point.
pub fn test_points() -> Vec<TestPoint> {
    (0..POINT_COUNT)
        .map(|index| {
            let t = index as f32;
            let normal = normalized([0.35 * (1.7 * t).sin(), 0.35 * (2.3 * t).cos(), 1.0]);
            let view = direction(0.05 + (t * 0.37) % 1.45, 0.8 * t);
            // The later points of the metal and the clearcoat material look straight into the light's reflection.
            let light = if index >= 16 && matches!(index % 8, 1 | 3) {
                let n_dot_v = normal[0] * view[0] + normal[1] * view[1] + normal[2] * view[2];
                [0, 1, 2].map(|axis| 2.0 * n_dot_v * normal[axis] - view[axis])
            } else {
                direction(0.1 + (t * 0.21) % 2.9, 1.9 * t + 0.5)
            };
            let front_facing = index % 7 != 3;
            let tinted = index % 5 == 2;
            TestPoint {
                position: [t, 0.0, 0.0, 1.0],
                normal: [normal[0], normal[1], normal[2], if front_facing { 1.0 } else { 0.0 }],
                tangent: [1.0, 0.0, 0.0, if index % 4 == 3 { -1.0 } else { 1.0 }],
                uv: [0.1 * (index % 8) as f32, 0.1 * (index / 8) as f32, 0.0, 0.0],
                uv_gradients: [0.01, 0.0, 0.0, 0.01],
                color: if tinted { [0.9, 0.8, 0.7, 0.9] } else { [1.0; 4] },
                view: [view[0], view[1], view[2], 0.0],
                light_direction: [light[0], light[1], light[2], 0.0],
                light_radiance: [1.0 + 0.1 * (index % 5) as f32, 0.9, 0.8, 0.0],
                material: [u32::try_from(index % MATERIAL_COUNT).expect("a few materials"), 0, 0, 0],
            }
        })
        .collect()
}

// Point `i` uses material `i % 8`: plain dielectric, metal, smooth dielectric, clearcoat, transmission, diffuse
// transmission, alpha mask, and one that turns every extension on.
pub fn test_materials() -> Vec<StandardMaterial> {
    vec![
        StandardMaterial {
            base_color: [0.8, 0.2, 0.1, 1.0],
            metallic: 0.0,
            roughness: 0.5,
            ..StandardMaterial::default()
        },
        StandardMaterial {
            base_color: [1.0, 0.77, 0.34, 1.0],
            metallic: 1.0,
            roughness: 0.3,
            ..StandardMaterial::default()
        },
        StandardMaterial {
            base_color: [0.1, 0.4, 0.9, 1.0],
            metallic: 0.0,
            roughness: 0.1,
            ..StandardMaterial::default()
        },
        StandardMaterial {
            base_color: [0.7, 0.05, 0.05, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            clearcoat: 1.0,
            clearcoat_roughness: 0.1,
            ..StandardMaterial::default()
        },
        StandardMaterial {
            base_color: [0.9, 0.9, 1.0, 1.0],
            metallic: 0.0,
            roughness: 0.2,
            transmission: 0.8,
            thickness: 0.5,
            attenuation_color: [0.5, 0.8, 0.5],
            attenuation_distance: Some(2.0),
            ..StandardMaterial::default()
        },
        StandardMaterial {
            base_color: [0.3, 0.6, 0.2, 1.0],
            metallic: 0.0,
            roughness: 0.7,
            diffuse_transmission: 0.6,
            diffuse_transmission_color: [0.2, 0.8, 0.3],
            ..StandardMaterial::default()
        },
        StandardMaterial {
            base_color: [0.6, 0.6, 0.6, 0.55],
            metallic: 0.0,
            roughness: 0.4,
            alpha_mode: AlphaMode::Mask { cutoff: 0.5 },
            ..StandardMaterial::default()
        },
        StandardMaterial {
            base_color: [0.5, 0.5, 0.9, 0.8],
            metallic: 0.3,
            roughness: 0.35,
            emissive: [0.2, 0.1, 0.0],
            emissive_strength: 3.0,
            occlusion_strength: 0.7,
            alpha_mode: AlphaMode::Blend,
            diffuse_model: DiffuseModel::Burley,
            ior: 1.8,
            specular_factor: 0.5,
            specular_color: [1.0, 0.9, 0.8],
            clearcoat: 0.6,
            clearcoat_roughness: 0.3,
            transmission: 0.3,
            thickness: 1.0,
            attenuation_color: [0.9, 0.6, 0.3],
            attenuation_distance: Some(1.0),
            diffuse_transmission: 0.25,
            diffuse_transmission_color: [0.9, 0.5, 0.2],
            textures: MaterialTextures {
                base_color: Some(TextureSlot(3)),
                metallic_roughness: Some(TextureSlot(4)),
                normal: Some(TextureSlot(1)),
                occlusion: Some(TextureSlot(5)),
                emissive: Some(TextureSlot(6)),
            },
            ..StandardMaterial::default()
        },
    ]
}

/// A library with every material module and the test root registered, hooks applied.
pub fn library(hooks: &MaterialHooks) -> ShaderLibrary {
    let mut library = ShaderLibrary::new();
    for (path, source) in hooks.modules() {
        library.add_module(path, source).expect("material module path");
    }
    library.add_root(ROOT_NAME, TEST_ROOT);
    library
}

pub fn wgsl_of(variant: MaterialVariant, hooks: &MaterialHooks) -> String {
    library(hooks)
        .wgsl(&variant.shader_key(ROOT_NAME))
        .unwrap_or_else(|error| panic!("{variant:?}: {error}"))
        .to_string()
}
