use naga::front::wgsl;
use naga::valid::{Capabilities, ValidationFlags, Validator};
use pathon_shader::{ShaderComposer, ShaderError};

const COLORS: &str = "
fn pack_rgba8(r: u32, g: u32, b: u32, a: u32) -> u32 {
    return r | (g << 8u) | (b << 16u) | (a << 24u);
}
";

const CONSTANTS: &str = "
import package::colors::pack_rgba8;

const OPAQUE: u32 = 255u;

fn opaque_red() -> u32 {
    return pack_rgba8(255u, 0u, 0u, OPAQUE);
}
";

const SHADER: &str = "
import package::colors::pack_rgba8;
import package::constants::{OPAQUE, opaque_red};

override scale: f32 = 1.0;

@group(0) @binding(0) var<storage, read_write> out: array<u32>;

@compute @workgroup_size(1)
fn main() {
    out[0] = pack_rgba8(1u, 2u, 3u, OPAQUE);
    out[1] = opaque_red();
    out[2] = bitcast<u32>(scale * 2.0);
}
";

fn composer_with_modules() -> ShaderComposer {
    let mut composer = ShaderComposer::new();
    composer.add_module("colors", COLORS).expect("colors");
    composer.add_module("constants", CONSTANTS).expect("constants");
    composer
}

fn assert_valid(wgsl_source: &str) {
    let module = wgsl::parse_str(wgsl_source).unwrap_or_else(|error| panic!("{error}\n{wgsl_source}"));
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|error| panic!("{error:?}\n{wgsl_source}"));
}

fn compose_valid(composer: &ShaderComposer, source: &str) -> String {
    let composed = composer.compose(source, "shader.wgsl").expect("compose");
    assert_valid(&composed);
    composed
}

#[test]
fn composes_nested_imports_into_valid_wgsl() {
    let composed = compose_valid(&composer_with_modules(), SHADER);
    assert!(!composed.contains("import"), "{composed}");
}

#[test]
fn main_shader_keeps_entry_point_and_override_names_that_end_in_a_digit() {
    let source = "
override scale0: f32 = 1.0;
override my__scale: f32 = 2.0;
@compute @workgroup_size(1) fn pass1() { _ = scale0 + my__scale; }
@compute @workgroup_size(1) fn layer0() {}
";
    let composed = compose_valid(&ShaderComposer::new(), source);
    for name in ["fn pass1", "fn layer0", "override scale0", "override my__scale"] {
        assert!(composed.contains(name), "missing `{name}`:\n{composed}");
    }
}

#[test]
fn unused_override_of_the_main_shader_is_kept() {
    let composed = compose_valid(
        &ShaderComposer::new(),
        "override unused_scale: f32 = 1.0;\n@compute @workgroup_size(1) fn main() {}",
    );
    assert!(composed.contains("override unused_scale"), "{composed}");
}

#[test]
fn override_derived_from_another_override_composes() {
    let composed = compose_valid(
        &ShaderComposer::new(),
        "override a = 2.0;\noverride b = a * 2.0;\n@compute @workgroup_size(1) fn main() { _ = b; }",
    );
    assert!(composed.contains("override b"), "{composed}");
}

#[test]
fn immediates_compose_in_the_main_shader_and_in_a_module() {
    let main = "var<immediate> value: u32;\n@compute @workgroup_size(1) fn main() { _ = value; }";
    assert!(compose_valid(&ShaderComposer::new(), main).contains("var<immediate>"),);

    let mut composer = ShaderComposer::new();
    composer
        .add_module(
            "push",
            "var<immediate> value: u32;\nfn read_value() -> u32 { return value; }",
        )
        .expect("module");
    let composed = compose_valid(
        &composer,
        "import package::push::read_value;\n@compute @workgroup_size(1) fn main() { _ = read_value(); }",
    );
    assert!(composed.contains("var<immediate>"), "{composed}");
}

#[test]
fn binding_array_composes_with_enable_after_import() {
    let source = "
import package::sizes::first_size;
enable wgpu_binding_array;
@group(0) @binding(0) var textures: binding_array<texture_2d<f32>, 4>;
@compute @workgroup_size(1) fn main() { _ = textureDimensions(textures[0]); _ = first_size(); }
";
    let mut composer = ShaderComposer::new();
    composer
        .add_module("sizes", "fn first_size() -> u32 { return 1u; }")
        .expect("module");
    let composed = compose_valid(&composer, source);
    assert!(composed.contains("enable wgpu_binding_array"), "{composed}");
    assert!(composed.contains("binding_array<texture_2d<f32>, 4>"), "{composed}");
}

#[test]
fn f16_enable_in_a_module_composes() {
    let mut composer = ShaderComposer::new();
    composer
        .add_module("half", "enable f16;\nfn twice(x: f16) -> f16 { return x * 2.0h; }")
        .expect("module");
    let composed = compose_valid(
        &composer,
        "import package::half::twice;\n@compute @workgroup_size(1) fn main() { _ = twice(1.0h); }",
    );
    assert!(composed.contains("enable f16"), "{composed}");
}

#[test]
fn render_shader_keeps_groups_from_two_modules_and_module_override_id() {
    let mut composer = ShaderComposer::new();
    composer
        .add_module(
            "camera",
            "struct Camera { view_projection: mat4x4<f32> }\n\
             @group(0) @binding(0) var<uniform> camera: Camera;\n\
             @id(3) override exposure: f32 = 1.0;\n\
             fn project(p: vec3<f32>) -> vec4<f32> { return camera.view_projection * vec4<f32>(p, 1.0) * exposure; }",
        )
        .expect("camera");
    composer
        .add_module(
            "material",
            "@group(1) @binding(0) var albedo: texture_2d<f32>;\n\
             @group(1) @binding(1) var albedo_sampler: sampler;\n\
             fn sample_albedo(uv: vec2<f32>) -> vec4<f32> { return textureSampleLevel(albedo, albedo_sampler, uv, 0.0); }",
        )
        .expect("material");
    let source = "
import package::camera::project;
import package::material::sample_albedo;
struct VertexOut { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    var out: VertexOut;
    out.position = project(vec3<f32>(f32(index), 0.0, 0.0));
    out.uv = vec2<f32>(0.0);
    return out;
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> { return sample_albedo(in.uv); }
";
    let composed = compose_valid(&composer, source);
    for expected in ["@group(0)", "@group(1)", "@id(3)", "fn vs_main", "fn fs_main"] {
        assert!(composed.contains(expected), "missing `{expected}`:\n{composed}");
    }
}

fn compose_error(composer: &ShaderComposer, source: &str) -> ShaderError {
    composer.compose(source, "shader.wgsl").expect_err("compose must fail")
}

#[test]
fn unknown_import_is_an_error_naming_the_shader_file() {
    let error = compose_error(
        &ShaderComposer::new(),
        "import package::missing::gone;\n@compute @workgroup_size(1) fn main() { gone(); }",
    );
    assert!(matches!(error, ShaderError::Compose { .. }), "{error:?}");
    let message = error.to_string();
    assert!(message.contains("shader.wgsl"), "{message}");
    assert!(message.contains("missing"), "{message}");
}

#[test]
fn unknown_item_of_a_known_module_is_an_error() {
    let error = compose_error(
        &composer_with_modules(),
        "import package::colors::nope;\n@compute @workgroup_size(1) fn main() { nope(); }",
    );
    assert!(error.to_string().contains("shader.wgsl"), "{error}");
}

#[test]
fn import_cycle_is_an_error_not_a_hang() {
    let mut composer = ShaderComposer::new();
    composer
        .add_module("a", "import package::b::fb;\nfn fa() { fb(); }")
        .expect("a");
    composer
        .add_module("b", "import package::a::fa;\nfn fb() { fa(); }")
        .expect("b");
    let error = compose_error(
        &composer,
        "import package::a::fa;\n@compute @workgroup_size(1) fn main() { fa(); }",
    );
    assert!(error.to_string().contains("shader.wgsl"), "{error}");
}

#[test]
fn syntax_error_in_a_module_is_an_error_naming_the_shader_file() {
    let mut composer = ShaderComposer::new();
    composer
        .add_module("broken", "fn oops( { }")
        .expect("registration does not parse");
    let error = compose_error(
        &composer,
        "import package::broken::oops;\n@compute @workgroup_size(1) fn main() { oops(); }",
    );
    assert!(error.to_string().contains("shader.wgsl"), "{error}");
}

#[test]
fn syntax_error_in_the_shader_is_an_error() {
    let error = compose_error(&ShaderComposer::new(), "fn main( {");
    assert!(error.to_string().contains("shader.wgsl"), "{error}");
}

#[test]
fn invalid_module_path_is_rejected() {
    for path in ["", "a b", "::a", "a::", "a-b"] {
        let error = ShaderComposer::new()
            .add_module(path, "fn f() {}")
            .expect_err("invalid path");
        assert!(
            matches!(error, ShaderError::InvalidModulePath { .. }),
            "{path:?}: {error:?}"
        );
        assert!(error.to_string().contains(path), "{error}");
    }
}

#[test]
fn adding_a_module_under_the_same_path_replaces_it() {
    let mut composer = ShaderComposer::new();
    composer
        .add_module("answer", "fn answer() -> u32 { return 1u; }")
        .expect("first");
    composer
        .add_module(
            "dependant",
            "import package::answer::answer;\nfn doubled() -> u32 { return answer() * 2u; }",
        )
        .expect("dependant");
    composer
        .add_module("answer", "fn answer() -> u32 { return 2u; }")
        .expect("second");
    let composed = compose_valid(
        &composer,
        "import package::dependant::doubled;\n@compute @workgroup_size(1) fn main() { _ = doubled(); }",
    );
    assert!(composed.contains("return 2u"), "{composed}");
    assert!(!composed.contains("return 1u"), "{composed}");
}

mod rhi {
    use pathon_rhi::{ComputePipelineDesc, Device, DeviceApi, ShaderModuleDesc, ShaderStage};

    use super::{SHADER, composer_with_modules};

    #[test]
    fn composed_source_creates_a_module_and_a_compute_pipeline_on_the_null_device() {
        let composed = composer_with_modules().compose(SHADER, "shader.wgsl").expect("compose");
        let device = Device::null();
        let module = device
            .create_shader_module(&ShaderModuleDesc {
                label: "composed",
                wgsl: &composed,
            })
            .expect("module");
        device
            .create_compute_pipeline(&ComputePipelineDesc {
                label: "composed",
                bind_group_layouts: vec![],
                shader: ShaderStage {
                    module,
                    entry_point: "main",
                    constants: vec![("scale", 3.0.into())],
                },
                immediate_size: 0,
            })
            .expect("pipeline");
    }
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use pathon_rhi::{
        BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource, BindingType,
        BufferDesc, BufferUsage, CommandList, ComputePassDesc, ComputePipelineDesc, Device, DeviceApi, DeviceRequest,
        ShaderModuleDesc, ShaderStage, ShaderStages, StorageAccess,
    };

    use super::{SHADER, composer_with_modules};

    #[test]
    fn compute_with_imported_function_and_main_shader_override_writes_the_expected_values() {
        let composed = composer_with_modules().compose(SHADER, "shader.wgsl").expect("compose");
        let device = Device::build(&DeviceRequest {
            label: "shader imports",
            ..Default::default()
        })
        .expect("a GPU adapter is required for gpu-tests");

        let layout = device
            .create_bind_group_layout(&BindGroupLayoutDesc {
                label: "imports",
                entries: vec![BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageBuffer {
                        access: StorageAccess::ReadWrite,
                    },
                    count: None,
                }],
            })
            .expect("layout");
        let output = device
            .create_buffer(&BufferDesc {
                label: "out",
                size: 12,
                usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
            })
            .expect("output");
        let readback = device
            .create_buffer(&BufferDesc {
                label: "readback",
                size: 12,
                usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
            })
            .expect("readback");
        let group = device
            .create_bind_group(&BindGroupDesc {
                label: "imports",
                layout,
                entries: vec![BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::whole_buffer(output),
                }],
            })
            .expect("bind group");
        let module = device
            .create_shader_module(&ShaderModuleDesc {
                label: "imports",
                wgsl: &composed,
            })
            .expect("module");
        let pipeline = device
            .create_compute_pipeline(&ComputePipelineDesc {
                label: "imports",
                bind_group_layouts: vec![layout],
                shader: ShaderStage {
                    module,
                    entry_point: "main",
                    constants: vec![("scale", 3.0.into())],
                },
                immediate_size: 0,
            })
            .expect("pipeline");

        let mut list = CommandList::new();
        {
            let mut pass = list.compute_pass(ComputePassDesc {
                label: "imports",
                timestamps: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group);
            pass.dispatch(1, 1, 1);
        }
        list.copy_buffer_to_buffer(output, 0, readback, 0, 12);
        device.submit(list).expect("submit");

        let bytes = device.read_buffer(readback).expect("read");
        let values: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| u32::from_le_bytes(*chunk))
            .collect();
        assert_eq!(values, [0xff03_0201, 0xff00_00ff, 6.0f32.to_bits()]);
    }
}

#[test]
fn unused_module_declarations_are_stripped_but_unused_main_overrides_stay() {
    let mut composer = ShaderComposer::new();
    composer
        .add_module(
            "cam",
            "@group(0) @binding(0) var<uniform> unused_uniform: vec4<f32>;\n\
             @id(7) override unused_module_override: f32 = 1.0;\n\
             fn unused_helper() {}\n\
             @compute @workgroup_size(1) fn module_entry() {}\n\
             fn used() -> u32 { return 1u; }",
        )
        .expect("module");
    let composed = compose_valid(
        &composer,
        "import package::cam::used;\n\
         override unused_main_override: f32 = 1.0;\n\
         @compute @workgroup_size(1) fn main() { _ = used(); }",
    );
    assert!(composed.contains("override unused_main_override"), "{composed}");
    for stripped in [
        "unused_uniform",
        "unused_module_override",
        "unused_helper",
        "module_entry",
    ] {
        assert!(
            !composed.contains(stripped),
            "`{stripped}` should be stripped:\n{composed}"
        );
    }
}

#[test]
fn module_override_reached_through_a_function_keeps_its_id() {
    let mut composer = ShaderComposer::new();
    composer
        .add_module(
            "cfg",
            "@id(7) override bias: f32 = 1.0;\nfn biased() -> f32 { return bias; }",
        )
        .expect("module");
    let composed = compose_valid(
        &composer,
        "import package::cfg::biased;\n@compute @workgroup_size(1) fn main() { _ = biased(); }",
    );
    assert!(composed.contains("@id(7)"), "{composed}");
}

#[test]
fn unknown_item_through_an_inline_path_is_an_error() {
    let error = compose_error(
        &composer_with_modules(),
        "@compute @workgroup_size(1) fn main() { package::colors::nope(); }",
    );
    assert!(error.to_string().contains("shader.wgsl"), "{error}");
}

#[test]
fn sources_with_a_byte_order_mark_compose() {
    let mut composer = ShaderComposer::new();
    composer
        .add_module("bom", "\u{feff}fn from_bom() -> u32 { return 1u; }")
        .expect("module");
    compose_valid(
        &composer,
        "\u{feff}import package::bom::from_bom;\n@compute @workgroup_size(1) fn main() { _ = from_bom(); }",
    );
}

#[test]
fn keywords_are_not_module_path_segments() {
    for path in ["fn", "colors::import", "_", "package", "super"] {
        let error = ShaderComposer::new()
            .add_module(path, "fn f() {}")
            .expect_err("keyword");
        assert!(
            matches!(error, ShaderError::InvalidModulePath { .. }),
            "{path:?}: {error:?}"
        );
    }
}
