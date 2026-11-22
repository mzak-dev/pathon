// Every combination of material flags composes, validates and creates a module on the null device, and the flags
// prune the code they promise to. No GPU needed; tests/evaluation.rs runs the same permutations on a real device.

mod common;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use common::{ROOT_NAME, library, wgsl_of};
use pathon_api::SerialTaskPool;
use pathon_api::rhi::DeviceApi;
use pathon_material::{AlphaKind, MaterialHooks, MaterialVariant};
use pathon_rhi::Device;
use pathon_shader::{BackgroundCompiler, BackgroundConfig, Lookup, reflect_bind_groups};

#[test]
fn all_forty_eight_permutations_compose_validate_and_create_modules() {
    let device = Device::null();
    let hooks = MaterialHooks::default();
    let mut library = library(&hooks);
    let mut texts = HashSet::new();

    for variant in MaterialVariant::all() {
        let key = variant.shader_key(ROOT_NAME);
        let wgsl = library
            .wgsl(&key)
            .unwrap_or_else(|error| panic!("{variant:?}: {error}"));
        for entry_points in [&["cs"][..], &["vs", "fs"][..]] {
            reflect_bind_groups(&wgsl, entry_points).unwrap_or_else(|error| panic!("{variant:?}: {error}"));
        }
        library
            .module(&device, &key)
            .unwrap_or_else(|error| panic!("{variant:?}: {error}"));
        texts.insert(wgsl.to_string());
    }

    assert_eq!(
        texts.len(),
        MaterialVariant::COUNT,
        "each flag set composes to its own text"
    );
}

#[test]
fn flags_prune_the_code_they_guard() {
    let hooks = MaterialHooks::default();
    let plain = wgsl_of(MaterialVariant::default(), &hooks);
    let coated = wgsl_of(
        MaterialVariant {
            clearcoat: true,
            ..MaterialVariant::default()
        },
        &hooks,
    );

    assert!(coated.contains("coat_fresnel") && !plain.contains("coat_fresnel"));
    let burley = wgsl_of(
        MaterialVariant {
            burley_diffuse: true,
            ..MaterialVariant::default()
        },
        &hooks,
    );
    assert!(burley.contains("diffuse_burley") && !plain.contains("diffuse_burley"));
    let subsurface = wgsl_of(
        MaterialVariant {
            subsurface: true,
            ..MaterialVariant::default()
        },
        &hooks,
    );
    assert!(subsurface.contains("diffuse_transmission_color") && !plain.contains("surface.diffuse_transmission"));
}

#[test]
fn alpha_kinds_choose_the_alpha_functions() {
    let hooks = MaterialHooks::default();
    let [opaque, masked, transparent] = [AlphaKind::Opaque, AlphaKind::Masked, AlphaKind::Transparent].map(|alpha| {
        wgsl_of(
            MaterialVariant {
                alpha,
                ..MaterialVariant::default()
            },
            &hooks,
        )
    });

    assert!(masked.contains("alpha_cutoff") && !opaque.contains("surface.alpha >= "));
    assert!(transparent.contains("return surface.alpha;") && !opaque.contains("return surface.alpha;"));
    assert_ne!(opaque, masked);
    assert_ne!(opaque, transparent);
    assert_ne!(masked, transparent);
}

#[test]
fn a_flag_nobody_declared_changes_nothing() {
    // The permutation key is what makes shaders differ; a stray flag the shader never reads must not break composing.
    let mut library = library(&MaterialHooks::default());
    let mut key = MaterialVariant::default().shader_key(ROOT_NAME);
    key.features.set("not_a_material_flag", true).expect("identifier");

    assert!(library.wgsl(&key).is_ok());
}

#[test]
fn the_background_compiler_builds_every_permutation() {
    let device: Arc<dyn DeviceApi> = Arc::new(Device::null());
    let mut compiler = BackgroundCompiler::new(device, Arc::new(SerialTaskPool), BackgroundConfig::new());
    for (path, source) in MaterialHooks::default().modules() {
        compiler.add_module(path, source).expect("module path");
    }
    compiler.add_root(ROOT_NAME, common::TEST_ROOT);
    let keys: Vec<_> = MaterialVariant::all()
        .map(|variant| variant.shader_key(ROOT_NAME))
        .collect();

    for key in &keys {
        assert!(matches!(
            compiler.shader_module(key),
            Lookup::Pending | Lookup::Ready(_)
        ));
    }
    assert!(compiler.finish_all(Duration::from_secs(30)));

    assert!(compiler.take_failures().is_empty());
    for key in &keys {
        assert!(matches!(compiler.shader_module(key), Lookup::Ready(_)), "{key:?}");
    }
}

#[test]
fn hooks_replace_the_modules_the_library_composes() {
    let mut hooks = MaterialHooks::default();
    hooks.set_surface_hook(
        "import package::material::surface::{Surface, SurfaceInput};
         fn modify_surface(input: SurfaceInput, surface: Surface) -> Surface {
             var changed = surface;
             changed.emissive = vec3<f32>(0.25, 0.5, 0.75);
             return changed;
         }",
    );
    assert_eq!(hooks.validate(), Ok(()));

    let plain = wgsl_of(MaterialVariant::default(), &MaterialHooks::default());
    let hooked = wgsl_of(MaterialVariant::default(), &hooks);

    assert!(hooked.contains("0.75") && !plain.contains("0.75"));
}
