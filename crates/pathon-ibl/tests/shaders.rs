// The ibl WESL modules without a GPU: they compose next to the standard material in every permutation, naga accepts
// the result, and the bindings the declarations text produces match the layout helper.

use pathon_ibl::{IblBindings, SAMPLE_MODULE, SHADING_MODULE, modules};
use pathon_material::{MaterialHooks, MaterialVariant};
use pathon_rhi::{Device, DeviceApi, ShaderModuleDesc, ShaderStages};
use pathon_shader::{ShaderLibrary, reflect_bind_groups};

// The root a lighting pack writes: the material resolves the surface, IBL lights it.
const ROOT: &str = "
import package::ibl::shading::{IblInput, ibl_evaluate};
import package::material::standard::{MaterialData, load_surface};
import package::material::surface::SurfaceInput;

@group(0) @binding(0) var<storage, read> materials: array<MaterialData>;
IBL_DECLARATIONS

@fragment
fn fs(@location(0) normal: vec3<f32>, @location(1) view: vec3<f32>) -> @location(0) vec4<f32> {
    var input: SurfaceInput;
    input.normal = normalize(normal);
    input.tangent = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    input.color = vec4<f32>(1.0);
    input.front_facing = true;
    let surface = load_surface(materials[0], input);
    let lighting = ibl_evaluate(
        ibl_irradiance,
        ibl_prefiltered,
        ibl_brdf_lut,
        ibl_sampler,
        IblInput(surface.normal, normalize(view), surface.diffuse_color, surface.f0, surface.f90, surface.roughness),
    );
    return vec4<f32>(surface.emissive + lighting.diffuse * surface.occlusion + lighting.specular, 1.0);
}
";

fn library() -> ShaderLibrary {
    let mut library = ShaderLibrary::new();
    for (path, source) in MaterialHooks::default().modules() {
        library.add_module(path, source).expect("material module");
    }
    for (path, source) in modules() {
        library.add_module(path, source).expect("ibl module");
    }
    library.add_root(
        "lighting",
        &ROOT.replace("IBL_DECLARATIONS", &IblBindings::starting_at(1).wgsl_declarations(0)),
    );
    library
}

#[test]
fn the_modules_are_registered_under_the_documented_paths() {
    assert_eq!(modules().map(|(path, _)| path), [SAMPLE_MODULE, SHADING_MODULE]);
    assert_eq!(SAMPLE_MODULE, "ibl::sample");
    assert_eq!(SHADING_MODULE, "ibl::shading");
}

#[test]
fn lighting_composes_with_every_material_permutation() {
    let mut library = library();
    let device = Device::null();
    for variant in MaterialVariant::all() {
        let key = variant.shader_key("lighting");
        library
            .module(&device, &key)
            .unwrap_or_else(|error| panic!("{variant:?}: {error}"));
    }
}

#[test]
fn the_declared_bindings_match_the_layout_entries() {
    let mut library = library();
    let wgsl = library
        .wgsl(&MaterialVariant::default().shader_key("lighting"))
        .unwrap_or_else(|error| panic!("{error}"));
    let reflected = reflect_bind_groups(&wgsl, &["fs"]).expect("reflection").remove(0);

    let bindings = IblBindings::starting_at(1);
    let mut expected = bindings.layout_entries(ShaderStages::FRAGMENT);
    let mut actual: Vec<_> = reflected
        .entries
        .into_iter()
        .filter(|entry| entry.binding >= 1)
        .collect();
    expected.sort_by_key(|entry| entry.binding);
    actual.sort_by_key(|entry| entry.binding);
    assert_eq!(actual, expected);
}

#[test]
fn the_null_device_accepts_the_composed_module() {
    let mut library = library();
    let wgsl = library
        .wgsl(&MaterialVariant::default().shader_key("lighting"))
        .unwrap_or_else(|error| panic!("{error}"));
    Device::null()
        .create_shader_module(&ShaderModuleDesc {
            label: "lighting",
            wgsl: &wgsl,
        })
        .expect("module");
}
