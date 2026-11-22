use std::num::NonZeroU32;

use pathon_rhi_types::{
    BindGroupLayoutDesc, BindGroupLayoutEntry, BindingType, Format, SamplerBindingType, ShaderStages, StorageAccess,
    TextureSampleType, TextureViewDimension,
};
use pathon_shader::{ShaderComposer, ShaderError, reflect_bind_groups};

fn entry(binding: u32, visibility: ShaderStages, ty: BindingType) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}

fn group(entries: Vec<BindGroupLayoutEntry>) -> BindGroupLayoutDesc {
    BindGroupLayoutDesc {
        label: "reflected",
        entries,
    }
}

fn sampled(sample_type: TextureSampleType, dimension: TextureViewDimension, multisampled: bool) -> BindingType {
    BindingType::SampledTexture {
        sample_type,
        dimension,
        multisampled,
    }
}

fn reflect_ok(wgsl: &str, entry_points: &[&str]) -> Vec<BindGroupLayoutDesc> {
    reflect_bind_groups(wgsl, entry_points).unwrap_or_else(|error| panic!("{error}\n{wgsl}"))
}

fn reflect_err(wgsl: &str, entry_points: &[&str]) -> ShaderError {
    reflect_bind_groups(wgsl, entry_points).expect_err("reflection must fail")
}

const RENDER: &str = "
enable wgpu_binding_array;
struct Camera { view_projection: mat4x4<f32> }
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var albedo: texture_2d<f32>;
@group(0) @binding(2) var albedo_sampler: sampler;
@group(0) @binding(3) var<storage, read> positions: array<vec4<f32>>;
@group(1) @binding(0) var target_image: texture_storage_2d<rgba8unorm, write>;
@group(1) @binding(1) var layers: binding_array<texture_2d<f32>, 4>;
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return camera.view_projection * positions[index];
}
@fragment fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    textureStore(target_image, vec2<i32>(0), vec4<f32>(1.0));
    let tint = camera.view_projection[0][0];
    return textureSample(albedo, albedo_sampler, position.xy) * tint + textureLoad(layers[1], vec2<i32>(0), 0);
}
";

#[test]
fn render_layout_equals_the_hand_written_one() {
    let layouts = reflect_ok(RENDER, &["vs", "fs"]);
    let expected = vec![
        group(vec![
            entry(0, ShaderStages::VERTEX_FRAGMENT, BindingType::UniformBuffer),
            entry(
                1,
                ShaderStages::FRAGMENT,
                sampled(TextureSampleType::Float, TextureViewDimension::D2, false),
            ),
            entry(
                2,
                ShaderStages::FRAGMENT,
                BindingType::Sampler(SamplerBindingType::Filtering),
            ),
            entry(
                3,
                ShaderStages::VERTEX,
                BindingType::StorageBuffer {
                    access: StorageAccess::ReadOnly,
                },
            ),
        ]),
        group(vec![
            entry(
                0,
                ShaderStages::FRAGMENT,
                BindingType::StorageTexture {
                    access: StorageAccess::WriteOnly,
                    format: Format::Rgba8Unorm,
                    dimension: TextureViewDimension::D2,
                },
            ),
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::FRAGMENT,
                ty: sampled(TextureSampleType::Float, TextureViewDimension::D2, false),
                count: NonZeroU32::new(4),
            },
        ]),
    ];
    assert_eq!(layouts, expected);
}

#[test]
fn visibility_and_omitted_bindings_follow_the_given_entry_points() {
    let vertex_only = reflect_ok(RENDER, &["vs"]);
    assert_eq!(
        vertex_only,
        vec![group(vec![
            entry(0, ShaderStages::VERTEX, BindingType::UniformBuffer),
            entry(
                3,
                ShaderStages::VERTEX,
                BindingType::StorageBuffer {
                    access: StorageAccess::ReadOnly
                }
            ),
        ])]
    );

    let fragment_only = reflect_ok(RENDER, &["fs"]);
    assert_eq!(fragment_only.len(), 2);
    let fragment_group_zero: Vec<u32> = fragment_only[0].entries.iter().map(|entry| entry.binding).collect();
    assert_eq!(fragment_group_zero, [0, 1, 2]);
    assert!(
        fragment_only
            .iter()
            .flat_map(|desc| &desc.entries)
            .all(|entry| entry.visibility == ShaderStages::FRAGMENT)
    );
}

#[test]
fn groups_without_bindings_below_the_highest_used_one_are_empty() {
    let wgsl = "
@group(2) @binding(0) var<uniform> value: vec4<f32>;
@compute @workgroup_size(1) fn main() { _ = value; }
";
    let layouts = reflect_ok(wgsl, &["main"]);
    assert_eq!(
        layouts,
        vec![
            group(vec![]),
            group(vec![]),
            group(vec![entry(0, ShaderStages::COMPUTE, BindingType::UniformBuffer)]),
        ]
    );
}

#[test]
fn no_used_bindings_gives_no_groups() {
    let wgsl = "
@group(0) @binding(0) var<uniform> unused: vec4<f32>;
@compute @workgroup_size(1) fn main() {}
";
    assert_eq!(reflect_ok(wgsl, &["main"]), vec![]);
}

#[test]
fn entries_are_sorted_by_binding() {
    let wgsl = "
@group(0) @binding(5) var<uniform> late: vec4<f32>;
@group(0) @binding(1) var<uniform> early: vec4<f32>;
@compute @workgroup_size(1) fn main() { _ = late; _ = early; }
";
    let bindings: Vec<u32> = reflect_ok(wgsl, &["main"])[0]
        .entries
        .iter()
        .map(|entry| entry.binding)
        .collect();
    assert_eq!(bindings, [1, 5]);
}

#[test]
fn compute_storage_buffers_keep_their_access() {
    let wgsl = "
@group(0) @binding(0) var<storage, read> source: array<u32>;
@group(0) @binding(1) var<storage, read_write> sink: array<u32>;
@compute @workgroup_size(1) fn main() { sink[0] = source[0]; }
";
    let layouts = reflect_ok(wgsl, &["main"]);
    assert_eq!(
        layouts,
        vec![group(vec![
            entry(
                0,
                ShaderStages::COMPUTE,
                BindingType::StorageBuffer {
                    access: StorageAccess::ReadOnly
                }
            ),
            entry(
                1,
                ShaderStages::COMPUTE,
                BindingType::StorageBuffer {
                    access: StorageAccess::ReadWrite
                }
            ),
        ])]
    );
}

#[test]
fn depth_texture_with_comparison_sampler() {
    let wgsl = "
@group(0) @binding(0) var shadow: texture_depth_2d;
@group(0) @binding(1) var shadow_sampler: sampler_comparison;
@fragment fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(textureSampleCompare(shadow, shadow_sampler, vec2<f32>(0.5), 0.5));
}
";
    assert_eq!(
        reflect_ok(wgsl, &["fs"]),
        vec![group(vec![
            entry(
                0,
                ShaderStages::FRAGMENT,
                sampled(TextureSampleType::Depth, TextureViewDimension::D2, false)
            ),
            entry(
                1,
                ShaderStages::FRAGMENT,
                BindingType::Sampler(SamplerBindingType::Comparison)
            ),
        ])]
    );
}

#[test]
fn integer_and_multisampled_textures() {
    let wgsl = "
@group(0) @binding(0) var ints: texture_2d<i32>;
@group(0) @binding(1) var uints: texture_3d<u32>;
@group(0) @binding(2) var samples: texture_multisampled_2d<f32>;
@group(0) @binding(3) var layered: texture_2d_array<f32>;
@group(0) @binding(4) var cube: texture_cube<f32>;
@group(0) @binding(5) var line: texture_1d<f32>;
@group(0) @binding(6) var cubes: texture_cube_array<f32>;
@compute @workgroup_size(1) fn main() {
    _ = textureDimensions(ints);
    _ = textureDimensions(uints);
    _ = textureDimensions(samples);
    _ = textureDimensions(layered);
    _ = textureDimensions(cube);
    _ = textureDimensions(line);
    _ = textureDimensions(cubes);
}
";
    let expected = [
        sampled(TextureSampleType::Sint, TextureViewDimension::D2, false),
        sampled(TextureSampleType::Uint, TextureViewDimension::D3, false),
        sampled(TextureSampleType::Float, TextureViewDimension::D2, true),
        sampled(TextureSampleType::Float, TextureViewDimension::D2Array, false),
        sampled(TextureSampleType::Float, TextureViewDimension::Cube, false),
        sampled(TextureSampleType::Float, TextureViewDimension::D1, false),
        sampled(TextureSampleType::Float, TextureViewDimension::CubeArray, false),
    ];
    let layouts = reflect_ok(wgsl, &["main"]);
    let reflected: Vec<BindingType> = layouts[0].entries.iter().map(|entry| entry.ty).collect();
    assert_eq!(reflected, expected);
}

#[test]
fn storage_texture_access_and_format() {
    let wgsl = "
@group(0) @binding(0) var written: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(1) var read_back: texture_storage_2d<r32uint, read>;
@group(0) @binding(2) var both: texture_storage_2d<r32float, read_write>;
@compute @workgroup_size(1) fn main() {
    textureStore(written, vec2<i32>(0), vec4<f32>(1.0));
    _ = textureLoad(read_back, vec2<i32>(0));
    textureStore(both, vec2<i32>(0), vec4<f32>(textureLoad(both, vec2<i32>(0)).x));
}
";
    let storage = |access, format| BindingType::StorageTexture {
        access,
        format,
        dimension: TextureViewDimension::D2,
    };
    let layouts = reflect_ok(wgsl, &["main"]);
    let reflected: Vec<BindingType> = layouts[0].entries.iter().map(|entry| entry.ty).collect();
    assert_eq!(
        reflected,
        [
            storage(StorageAccess::WriteOnly, Format::Rgba8Unorm),
            storage(StorageAccess::ReadOnly, Format::R32Uint),
            storage(StorageAccess::ReadWrite, Format::R32Float),
        ]
    );
}

#[test]
fn storage_texture_format_without_an_equivalent_is_an_error() {
    let wgsl = "
@group(0) @binding(0) var image: texture_storage_2d<rgba8snorm, write>;
@compute @workgroup_size(1) fn main() { textureStore(image, vec2<i32>(0), vec4<f32>(1.0)); }
";
    assert!(matches!(
        reflect_err(wgsl, &["main"]),
        ShaderError::UnsupportedBinding {
            group: 0,
            binding: 0,
            ..
        }
    ));
}

#[test]
fn binding_arrays_carry_their_count_and_element_type() {
    let wgsl = "
enable wgpu_binding_array;
@group(0) @binding(0) var samplers: binding_array<sampler, 3>;
@group(0) @binding(1) var views: binding_array<texture_cube_array<f32>, 8>;
@compute @workgroup_size(1) fn main() { _ = textureDimensions(views[0]); _ = samplers[1]; }
";
    let layouts = reflect_ok(wgsl, &["main"]);
    assert_eq!(
        layouts[0].entries[0].ty,
        BindingType::Sampler(SamplerBindingType::Filtering)
    );
    assert_eq!(layouts[0].entries[0].count, NonZeroU32::new(3));
    assert_eq!(
        layouts[0].entries[1].ty,
        sampled(TextureSampleType::Float, TextureViewDimension::CubeArray, false)
    );
    assert_eq!(layouts[0].entries[1].count, NonZeroU32::new(8));
}

#[test]
fn immediate_variables_are_not_bindings() {
    let wgsl = "
var<immediate> constants: vec4<f32>;
@group(0) @binding(0) var<storage, read_write> out: array<vec4<f32>>;
@compute @workgroup_size(1) fn main() { out[0] = constants; }
";
    let layouts = reflect_ok(wgsl, &["main"]);
    assert_eq!(layouts.len(), 1);
    assert_eq!(layouts[0].entries.len(), 1);
}

#[test]
fn unknown_entry_point_is_an_error() {
    let error = reflect_err(RENDER, &["vs", "missing"]);
    assert!(
        matches!(&error, ShaderError::UnknownEntryPoint { name } if name == "missing"),
        "{error}"
    );
}

#[test]
fn two_different_bindings_at_one_slot_across_entry_points_conflict() {
    let wgsl = "
@group(0) @binding(0) var<uniform> first: vec4<f32>;
@group(0) @binding(0) var second: texture_2d<f32>;
@vertex fn vs() -> @builtin(position) vec4<f32> { return first; }
@fragment fn fs() -> @location(0) vec4<f32> { return textureLoad(second, vec2<i32>(0), 0); }
";
    assert!(matches!(
        reflect_err(wgsl, &["vs", "fs"]),
        ShaderError::BindingConflict { group: 0, binding: 0 }
    ));
    assert!(reflect_bind_groups(wgsl, &["vs"]).is_ok());
}

#[test]
fn two_bindings_at_one_slot_in_one_entry_point_is_an_error() {
    let wgsl = "
@group(0) @binding(0) var<uniform> first: vec4<f32>;
@group(0) @binding(0) var<uniform> second: vec4<f32>;
@compute @workgroup_size(1) fn main() { _ = first; _ = second; }
";
    assert!(reflect_bind_groups(wgsl, &["main"]).is_err());
}

#[test]
fn identical_bindings_at_one_slot_merge_their_visibility() {
    let wgsl = "
@group(0) @binding(0) var<uniform> first: vec4<f32>;
@group(0) @binding(0) var<uniform> second: vec4<f32>;
@vertex fn vs() -> @builtin(position) vec4<f32> { return first; }
@fragment fn fs() -> @location(0) vec4<f32> { return second; }
";
    assert_eq!(
        reflect_ok(wgsl, &["vs", "fs"]),
        vec![group(vec![entry(
            0,
            ShaderStages::VERTEX_FRAGMENT,
            BindingType::UniformBuffer
        )])]
    );
}

#[test]
fn acceleration_structure_is_an_error() {
    let wgsl = "
enable wgpu_ray_query;
@group(0) @binding(0) var scene: acceleration_structure;
@compute @workgroup_size(1) fn main() {
    var query: ray_query;
    rayQueryInitialize(&query, scene, RayDesc(0u, 0xFFu, 0.0, 1.0, vec3<f32>(0.0), vec3<f32>(0.0, 0.0, 1.0)));
}
";
    assert!(matches!(
        reflect_err(wgsl, &["main"]),
        ShaderError::UnsupportedBinding {
            group: 0,
            binding: 0,
            ..
        }
    ));
}

#[test]
fn runtime_sized_binding_array_is_an_error_not_a_panic() {
    let wgsl = "
enable wgpu_binding_array;
@group(0) @binding(0) var views: binding_array<texture_2d<f32>>;
@compute @workgroup_size(1) fn main() { _ = textureDimensions(views[0]); }
";
    assert!(reflect_bind_groups(wgsl, &["main"]).is_err());
}

#[test]
fn invalid_wgsl_is_an_error_with_a_plain_message() {
    let syntax = reflect_err("fn main( {", &["main"]);
    assert!(matches!(syntax, ShaderError::InvalidWgsl { .. }), "{syntax}");
    let types = reflect_err("@compute @workgroup_size(1) fn main() { var x: u32 = 1.5; }", &["main"]);
    assert!(matches!(types, ShaderError::InvalidWgsl { .. }), "{types}");
    assert!(!types.to_string().contains('\u{1b}'));
}

#[test]
fn shader_composed_from_modules_reflects_module_bindings() {
    let mut composer = ShaderComposer::new();
    composer
        .add_module(
            "camera",
            "struct Camera { view_projection: mat4x4<f32> }\n\
             @group(0) @binding(0) var<uniform> camera: Camera;\n\
             @group(0) @binding(1) var<uniform> unused: Camera;\n\
             fn project(p: vec3<f32>) -> vec4<f32> { return camera.view_projection * vec4<f32>(p, 1.0); }",
        )
        .expect("module");
    let composed = composer
        .compose(
            "import package::camera::project;\n\
             @group(1) @binding(0) var<storage, read_write> out: array<vec4<f32>>;\n\
             @compute @workgroup_size(1) fn main() { out[0] = project(vec3<f32>(1.0)); }",
            "shader.wgsl",
        )
        .expect("compose");
    assert_eq!(
        reflect_ok(&composed, &["main"]),
        vec![
            group(vec![entry(0, ShaderStages::COMPUTE, BindingType::UniformBuffer)]),
            group(vec![entry(
                0,
                ShaderStages::COMPUTE,
                BindingType::StorageBuffer {
                    access: StorageAccess::ReadWrite
                }
            )]),
        ]
    );
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use pathon_rhi::{
        BindGroupDesc, BindGroupEntry, BindingResource, BufferDesc, BufferUsage, CommandList, ComputePassDesc,
        ComputePipelineDesc, Device, DeviceApi, DeviceRequest, ShaderModuleDesc, ShaderStage,
    };
    use pathon_shader::reflect_bind_groups;

    const SHADER: &str = "
struct Params { scale: u32 }
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> out: array<u32>;
@compute @workgroup_size(1) fn main() { out[0] = params.scale * 7u; }
";

    #[test]
    fn reflected_layout_creates_a_working_compute_pipeline() {
        let layouts = reflect_bind_groups(SHADER, &["main"]).expect("reflect");
        assert_eq!(layouts.len(), 1);
        let device = Device::build(&DeviceRequest {
            label: "reflection",
            ..Default::default()
        })
        .expect("a GPU adapter is required for gpu-tests");

        let layout = device.create_bind_group_layout(&layouts[0]).expect("layout");
        let params = device
            .create_buffer(&BufferDesc {
                label: "params",
                size: 16,
                usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
            })
            .expect("params");
        device.write_buffer(params, 0, &6u32.to_le_bytes()).expect("write");
        let output = device
            .create_buffer(&BufferDesc {
                label: "out",
                size: 4,
                usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
            })
            .expect("output");
        let readback = device
            .create_buffer(&BufferDesc {
                label: "readback",
                size: 4,
                usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
            })
            .expect("readback");
        let group = device
            .create_bind_group(&BindGroupDesc {
                label: "reflection",
                layout,
                entries: vec![
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::whole_buffer(params),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::whole_buffer(output),
                    },
                ],
            })
            .expect("bind group");
        let module = device
            .create_shader_module(&ShaderModuleDesc {
                label: "reflection",
                wgsl: SHADER,
            })
            .expect("module");
        let pipeline = device
            .create_compute_pipeline(&ComputePipelineDesc {
                label: "reflection",
                bind_group_layouts: vec![layout],
                shader: ShaderStage {
                    module,
                    entry_point: "main",
                    constants: vec![],
                },
                immediate_size: 0,
            })
            .expect("pipeline");

        let mut list = CommandList::new();
        {
            let mut pass = list.compute_pass(ComputePassDesc {
                label: "reflection",
                timestamps: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group);
            pass.dispatch(1, 1, 1);
        }
        list.copy_buffer_to_buffer(output, 0, readback, 0, 4);
        device.submit(list).expect("submit");

        let bytes = device.read_buffer(readback).expect("read");
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().expect("four bytes")), 42);
    }
}
