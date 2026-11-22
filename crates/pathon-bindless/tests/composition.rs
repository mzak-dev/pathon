// The texture source of both paths composes into the standard material, validates in naga and reflects to exactly the
// layout the binder creates (rhi-null, no GPU).

mod common;

use common::{binder_on, material_library, null_device, reflected_layout};
use pathon_api::rhi::{BindGroupLayoutEntry, ShaderStages};
use pathon_bindless::{MAX_MATERIAL_TEXTURES, PathPreference, SamplerPreset, TexturePath};

fn without_visibility(entries: &[BindGroupLayoutEntry]) -> Vec<BindGroupLayoutEntry> {
    entries
        .iter()
        .map(|entry| BindGroupLayoutEntry {
            visibility: ShaderStages::empty(),
            ..*entry
        })
        .collect()
}

#[test]
fn the_bindless_source_reflects_to_the_binder_layout() {
    let device = null_device(true);
    let binder = binder_on(&device, PathPreference::Bindless, 256);
    assert_eq!(binder.path(), TexturePath::Bindless { capacity: 256 });
    let mut library = material_library(&binder, 1);

    let wgsl = library.wgsl(&common::fragment_key()).expect("composes and validates");
    let layouts = reflected_layout(&wgsl);

    assert_eq!(
        without_visibility(&layouts[1].entries),
        without_visibility(&binder.layout_desc().entries)
    );
    assert!(wgsl.contains("binding_array<texture_2d<f32>, 256>"), "{wgsl}");
    assert!(wgsl.contains("enable wgpu_binding_array"), "{wgsl}");
}

#[test]
fn the_per_material_source_reflects_to_the_binder_layout() {
    let device = null_device(false);
    let binder = binder_on(&device, PathPreference::Auto, 256);
    assert_eq!(binder.path(), TexturePath::PerMaterial);
    let mut library = material_library(&binder, 1);

    let wgsl = library.wgsl(&common::fragment_key()).expect("composes and validates");
    let layouts = reflected_layout(&wgsl);

    assert_eq!(
        without_visibility(&layouts[1].entries),
        without_visibility(&binder.layout_desc().entries)
    );
    assert_eq!(
        layouts[1].entries.len(),
        1 + MAX_MATERIAL_TEXTURES + SamplerPreset::COUNT
    );
}

#[test]
fn the_group_number_is_the_applications_choice() {
    let device = null_device(false);
    let binder = binder_on(&device, PathPreference::Auto, 256);
    let mut library = material_library(&binder, 3);

    let wgsl = library.wgsl(&common::fragment_key()).expect("composes");
    let layouts = reflected_layout(&wgsl);

    assert_eq!(layouts.len(), 4);
    assert!(layouts[1].entries.is_empty() && layouts[2].entries.is_empty());
    assert_eq!(layouts[3].entries.len(), binder.layout_desc().entries.len());
}

#[test]
fn the_default_texture_source_is_what_the_hook_replaces() {
    // Without the hook the material samples white and has no texture bindings: the material modules stay untouched.
    let mut library = common::default_library();

    let wgsl = library.wgsl(&common::fragment_key()).expect("composes");

    assert!(
        !wgsl.contains("binding_array") && !wgsl.contains("texture_2d"),
        "{wgsl}"
    );
}
