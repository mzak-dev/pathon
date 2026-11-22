// The CPU `MaterialData` and the WGSL `MaterialData` must agree on every offset and on the array stride; the shader
// side is read back from the composed WGSL by naga, so no GPU is involved.

mod common;

use common::{TestPoint, wgsl_of};
use pathon_api::rhi::{BindingType, ShaderStages, StorageAccess};
use pathon_material::{MATERIAL_DATA_FIELDS, MaterialData, MaterialHooks, MaterialVariant};
use pathon_shader::{reflect_bind_groups, reflect_buffer_layout};

fn default_wgsl() -> String {
    wgsl_of(MaterialVariant::default(), &MaterialHooks::default())
}

#[test]
fn material_data_has_the_layout_the_shader_declares() {
    let layout = reflect_buffer_layout(&default_wgsl(), 0, 0).expect("materials binding");

    let cpu: Vec<(String, u32)> = MATERIAL_DATA_FIELDS
        .iter()
        .map(|(name, offset)| ((*name).to_owned(), u32::try_from(*offset).expect("small offset")))
        .collect();
    let shader: Vec<(String, u32)> = layout
        .members
        .iter()
        .map(|member| (member.name.clone(), member.offset))
        .collect();
    assert_eq!(shader, cpu, "field names and offsets");
    let size = u32::try_from(size_of::<MaterialData>()).expect("small");
    assert_eq!(layout.struct_size, Some(size));
    assert_eq!(
        layout.element_stride,
        Some(size),
        "array<MaterialData> has no gaps between elements"
    );
    assert_eq!(size % 16, 0, "valid as a uniform buffer element too");
}

#[test]
fn every_field_is_as_wide_as_its_rust_counterpart() {
    let layout = reflect_buffer_layout(&default_wgsl(), 0, 0).expect("materials binding");

    let sizes: Vec<u32> = layout.members.iter().map(|member| member.size).collect();
    let mut expected = vec![16; 5];
    expected.extend([4; 16]);
    assert_eq!(sizes, expected);
}

#[test]
fn the_test_point_layout_matches_its_shader_struct() {
    let layout = reflect_buffer_layout(&default_wgsl(), 0, 1).expect("points binding");

    assert_eq!(
        layout.element_stride,
        Some(u32::try_from(size_of::<TestPoint>()).expect("small"))
    );
    assert!(layout.members.iter().all(|member| member.size == 16));
}

#[test]
fn an_undeclared_binding_has_no_layout() {
    assert!(reflect_buffer_layout(&default_wgsl(), 3, 0).is_err());
}

#[test]
fn the_compute_entry_reads_two_buffers_and_writes_one() {
    let layouts = reflect_bind_groups(&default_wgsl(), &["cs"]).expect("reflection");

    let binding_types: Vec<_> = layouts[0]
        .entries
        .iter()
        .map(|entry| (entry.binding, entry.ty))
        .collect();
    assert_eq!(
        binding_types,
        [
            (
                0,
                BindingType::StorageBuffer {
                    access: StorageAccess::ReadOnly
                }
            ),
            (
                1,
                BindingType::StorageBuffer {
                    access: StorageAccess::ReadOnly
                }
            ),
            (
                2,
                BindingType::StorageBuffer {
                    access: StorageAccess::ReadWrite
                }
            ),
        ]
    );
    assert!(
        layouts[0]
            .entries
            .iter()
            .all(|entry| entry.visibility == ShaderStages::COMPUTE)
    );
}

#[test]
fn the_fragment_entry_needs_the_material_and_point_buffers_only() {
    let layouts = reflect_bind_groups(&default_wgsl(), &["vs", "fs"]).expect("reflection");

    let bindings: Vec<_> = layouts[0]
        .entries
        .iter()
        .map(|entry| (entry.binding, entry.visibility))
        .collect();
    assert_eq!(bindings, [(0, ShaderStages::FRAGMENT), (1, ShaderStages::FRAGMENT)]);
}
