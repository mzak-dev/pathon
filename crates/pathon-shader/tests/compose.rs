use naga::front::wgsl;
use pathon_shader::{ShaderComposer, ShaderError};

const COLORS: &str = "
#define_import_path pathon::colors

fn pack_rgba8(r: u32, g: u32, b: u32, a: u32) -> u32 {
    return r | (g << 8u) | (b << 16u) | (a << 24u);
}
";

const CONSTANTS: &str = "
#define_import_path pathon::constants
#import pathon::colors::{pack_rgba8}

const OPAQUE: u32 = 255u;

fn opaque_red() -> u32 {
    return pack_rgba8(255u, 0u, 0u, OPAQUE);
}
";

const SHADER: &str = "
#import pathon::colors::{pack_rgba8}
#import pathon::constants::{OPAQUE, opaque_red}

@group(0) @binding(0) var<storage, read_write> out: array<u32>;

@compute @workgroup_size(1)
fn main() {
    out[0] = pack_rgba8(1u, 2u, 3u, OPAQUE);
    out[1] = opaque_red();
}
";

fn composer_with_modules() -> ShaderComposer {
    let mut composer = ShaderComposer::new();
    composer.add_module(COLORS, "colors.wgsl").expect("colors");
    composer.add_module(CONSTANTS, "constants.wgsl").expect("constants");
    composer
}

fn message_of(error: &ShaderError) -> String {
    assert!(matches!(error, ShaderError::Compose { .. }), "{error:?}");
    error.to_string()
}

#[test]
fn composes_a_shader_from_nested_imports_into_parsable_wgsl() {
    let wgsl = composer_with_modules().compose(SHADER, "shader.wgsl").expect("compose");
    assert!(!wgsl.contains("#import"), "{wgsl}");
    wgsl::parse_str(&wgsl).expect("composed WGSL parses");
}

#[test]
fn composed_shader_validates() {
    let wgsl = composer_with_modules().compose(SHADER, "shader.wgsl").expect("compose");
    let module = wgsl::parse_str(&wgsl).expect("parse");
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .expect("valid");
}

#[test]
fn unknown_import_names_the_importing_file() {
    let mut composer = ShaderComposer::new();
    // naga_oil ignores an import that nothing uses, so the shader has to call it.
    let source = "#import pathon::missing::{gone}\nfn main() { _ = gone(); }";
    let message = message_of(
        &composer
            .compose(source, "needs_missing.wgsl")
            .expect_err("missing module"),
    );
    assert!(message.contains("needs_missing.wgsl"), "{message}");
    assert!(message.contains("pathon::missing"), "{message}");
}

#[test]
fn syntax_error_in_a_module_names_that_module_file() {
    let mut composer = ShaderComposer::new();
    let broken = "#define_import_path pathon::broken\nfn oops( -> u32 { return 1u; }";
    composer
        .add_module(broken, "broken.wgsl")
        .expect("registration does not parse");
    let source = "#import pathon::broken::{oops}\nfn main() { _ = oops(); }";
    let message = message_of(&composer.compose(source, "shader.wgsl").expect_err("syntax error"));
    assert!(message.contains("broken.wgsl"), "{message}");
}

#[test]
fn syntax_error_in_a_shader_names_the_shader_file() {
    let mut composer = ShaderComposer::new();
    let message = message_of(&composer.compose("fn main( {", "shader.wgsl").expect_err("syntax error"));
    assert!(message.contains("shader.wgsl"), "{message}");
}

#[test]
fn import_cycle_is_rejected_with_a_file_name() {
    let mut composer = ShaderComposer::new();
    let a = "#define_import_path pathon::a\n#import pathon::b::{b}\nfn a() { b(); }";
    // A module is accepted only once what it uses is added, so neither side of a cycle can be registered first.
    let message = message_of(&composer.add_module(a, "a.wgsl").expect_err("b is not added yet"));
    assert!(message.contains("a.wgsl"), "{message}");
    assert!(message.contains("pathon::b"), "{message}");
}

#[test]
fn module_without_import_path_is_rejected() {
    let mut composer = ShaderComposer::new();
    let message = message_of(&composer.add_module("fn f() {}", "anonymous.wgsl").expect_err("no path"));
    assert!(message.contains("anonymous.wgsl"), "{message}");
}

#[test]
fn adding_a_module_under_the_same_path_replaces_it() {
    let mut composer = ShaderComposer::new();
    composer
        .add_module(
            "#define_import_path pathon::answer\nfn answer() -> u32 { return 1u; }",
            "v1.wgsl",
        )
        .expect("v1");
    composer
        .add_module(
            "#define_import_path pathon::answer\nfn answer() -> u32 { return 2u; }",
            "v2.wgsl",
        )
        .expect("v2");
    let wgsl = composer
        .compose(
            "#import pathon::answer::{answer}\n@compute @workgroup_size(1) fn main() { _ = answer(); }",
            "shader.wgsl",
        )
        .expect("compose");
    assert!(wgsl.contains("return 2u"), "{wgsl}");
    assert!(!wgsl.contains("return 1u"), "{wgsl}");
}

mod rhi {
    use pathon_rhi::{ComputePipelineDesc, Device, DeviceApi, ShaderModuleDesc, ShaderStage};

    use super::{SHADER, composer_with_modules};

    #[test]
    fn composed_source_creates_a_module_and_a_compute_pipeline_on_the_null_device() {
        let wgsl = composer_with_modules().compose(SHADER, "shader.wgsl").expect("compose");
        let device = Device::null();
        let module = device
            .create_shader_module(&ShaderModuleDesc {
                label: "composed",
                wgsl: &wgsl,
            })
            .expect("module");
        device
            .create_compute_pipeline(&ComputePipelineDesc {
                label: "composed",
                bind_group_layouts: vec![],
                shader: ShaderStage {
                    module,
                    entry_point: "main",
                    constants: vec![],
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
    fn compute_with_imported_function_and_constant_writes_the_expected_values() {
        let wgsl = composer_with_modules().compose(SHADER, "shader.wgsl").expect("compose");
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
                size: 8,
                usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
            })
            .expect("output");
        let readback = device
            .create_buffer(&BufferDesc {
                label: "readback",
                size: 8,
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
                wgsl: &wgsl,
            })
            .expect("module");
        let pipeline = device
            .create_compute_pipeline(&ComputePipelineDesc {
                label: "imports",
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
                label: "imports",
                timestamps: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group);
            pass.dispatch(1, 1, 1);
        }
        list.copy_buffer_to_buffer(output, 0, readback, 0, 8);
        device.submit(list).expect("submit");

        let bytes = device.read_buffer(readback).expect("read");
        let values: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| u32::from_le_bytes(*chunk))
            .collect();
        assert_eq!(values, [0xff03_0201, 0xff00_00ff]);
    }
}
