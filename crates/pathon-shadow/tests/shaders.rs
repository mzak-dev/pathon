// The shadow WESL module without a GPU: a lighting shader composes with it, naga accepts the result, and the bindings
// that `ShadowBindings` declares match its layout helper. Whether the numbers are right is the golden test's job.

use pathon_rhi::{Device, DeviceApi, ShaderModuleDesc, ShaderStages};
use pathon_shader::{ShaderFeatures, ShaderKey, ShaderLibrary, reflect_bind_groups};
use pathon_shadow::{SAMPLE_MODULE, ShadowBindings, modules};

const ROOT: &str = "
import package::shadow::sample::{ShadowCascades, shadow_cascade_index, shadow_factor_directional};

SHADOW_DECLARATIONS

@fragment
fn fs(
    @builtin(position) pixel: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) view_depth: f32,
) -> @location(0) vec4<f32> {
    let visibility = shadow_factor_directional(
        shadow_cascades,
        shadow_map,
        shadow_sampler,
        0u,
        world_position,
        normalize(normal),
        view_depth,
        pixel.xy,
    );
    return vec4<f32>(vec3<f32>(visibility), f32(shadow_cascade_index(shadow_cascades, view_depth)));
}
";

fn library(bindings: ShadowBindings) -> (ShaderLibrary, ShaderKey) {
    let mut library = ShaderLibrary::new();
    for (path, source) in modules() {
        library.add_module(path, source).expect("shadow module");
    }
    library.add_root(
        "lighting",
        &ROOT.replace("SHADOW_DECLARATIONS", &bindings.wgsl_declarations(0)),
    );
    let key = ShaderKey {
        root: "lighting".to_owned(),
        features: ShaderFeatures::default(),
    };
    (library, key)
}

#[test]
fn the_module_is_registered_under_the_documented_path() {
    assert_eq!(modules().map(|(path, _)| path), [SAMPLE_MODULE]);
    assert_eq!(SAMPLE_MODULE, "shadow::sample");
}

#[test]
fn a_lighting_shader_composes_with_the_sampling_module() {
    let (mut library, key) = library(ShadowBindings::starting_at(3));

    library
        .module(&Device::null(), &key)
        .unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn the_declared_bindings_match_the_layout_entries() {
    let (mut library, key) = library(ShadowBindings::starting_at(3));
    let wgsl = library.wgsl(&key).unwrap_or_else(|error| panic!("{error}"));

    let reflected = reflect_bind_groups(&wgsl, &["fs"]).expect("reflection").remove(0);

    let mut expected = ShadowBindings::starting_at(3).layout_entries(ShaderStages::FRAGMENT);
    let mut actual = reflected.entries;
    expected.sort_by_key(|entry| entry.binding);
    actual.sort_by_key(|entry| entry.binding);
    assert_eq!(actual, expected);
}

#[test]
fn the_null_device_accepts_the_composed_module() {
    let (mut library, key) = library(ShadowBindings::starting_at(0));
    let wgsl = library.wgsl(&key).unwrap_or_else(|error| panic!("{error}"));

    Device::null()
        .create_shader_module(&ShaderModuleDesc {
            label: "lighting",
            wgsl: &wgsl,
        })
        .expect("module");
}
