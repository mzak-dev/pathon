#![cfg(feature = "gpu-tests")]
// The WGSL side (`package::ibl::sample` and `package::ibl::shading`) evaluated by a compute shader: energy in a white
// furnace, and the roughness to mip mapping against the CPU integral. `PATHON_TEST_BACKEND=vulkan|dx12|metal` picks
// the backend.

mod support;

use pathon_ibl::reference::prefiltered_radiance;
use pathon_ibl::{IblBindings, IblBuilder, IblEnvironment, IblSettings, SyntheticSky};
use pathon_rhi::{
    BindGroupDesc, BindGroupEntry, BindingResource, BufferDesc, BufferUsage, CommandList, ComputePassDesc,
    ComputePipelineDesc, Device, DeviceApi, ShaderModuleDesc, ShaderStage,
};
use pathon_shader::{ShaderFeatures, ShaderKey, ShaderLibrary, reflect_bind_groups};
use support::{gpu, sky_radiance};

const ROOT: &str = "
import package::ibl::sample::ibl_specular;
import package::ibl::shading::{IblInput, ibl_evaluate};
IBL_DECLARATIONS

struct Case {
    normal: vec4<f32>,
    view: vec4<f32>,
    diffuse_color: vec4<f32>,
    f0: vec4<f32>,
    f90: vec4<f32>,
    roughness: vec4<f32>,
}

@group(0) @binding(4) var<storage, read> cases: array<Case>;
// Per case: diffuse, specular and the raw prefiltered lookup along the view direction at the case's roughness.
@group(0) @binding(5) var<storage, read_write> results: array<vec4<f32>>;

@compute @workgroup_size(16)
fn cs(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= arrayLength(&cases)) {
        return;
    }
    let case_ = cases[id.x];
    let lighting = ibl_evaluate(
        ibl_irradiance,
        ibl_prefiltered,
        ibl_brdf_lut,
        ibl_sampler,
        IblInput(case_.normal.xyz, case_.view.xyz, case_.diffuse_color.xyz, case_.f0.xyz, case_.f90.xyz, case_.roughness.x),
    );
    results[id.x * 3u] = vec4<f32>(lighting.diffuse, 0.0);
    results[id.x * 3u + 1u] = vec4<f32>(lighting.specular, 0.0);
    results[id.x * 3u + 2u] = vec4<f32>(ibl_specular(ibl_prefiltered, ibl_sampler, case_.view.xyz, case_.roughness.x), 0.0);
}
";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Case {
    normal: [f32; 4],
    view: [f32; 4],
    diffuse_color: [f32; 4],
    f0: [f32; 4],
    f90: [f32; 4],
    roughness: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct Evaluation {
    diffuse: [f32; 3],
    specular: [f32; 3],
    lookup: [f32; 3],
}

fn case(view: [f32; 3], diffuse_color: f32, f0: f32, f90: f32, roughness: f32) -> Case {
    Case {
        normal: [0.0, 1.0, 0.0, 0.0],
        view: [view[0], view[1], view[2], 0.0],
        diffuse_color: [diffuse_color; 4],
        f0: [f0; 4],
        f90: [f90; 4],
        roughness: [roughness; 4],
    }
}

fn view_with_n_dot_v(n_dot_v: f32) -> [f32; 3] {
    [(1.0 - n_dot_v * n_dot_v).sqrt(), n_dot_v, 0.0]
}

fn evaluate(device: &Device, environment: &IblEnvironment, cases: &[Case]) -> Vec<Evaluation> {
    let ibl = IblBindings::starting_at(0);
    let mut library = ShaderLibrary::new();
    for (path, source) in pathon_ibl::modules() {
        library.add_module(path, source).expect("ibl module");
    }
    library.add_root("furnace", &ROOT.replace("IBL_DECLARATIONS", &ibl.wgsl_declarations(0)));
    let key = ShaderKey {
        root: "furnace".into(),
        features: ShaderFeatures::default(),
    };
    let wgsl = library.wgsl(&key).unwrap_or_else(|error| panic!("{error}"));
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "furnace",
            wgsl: &wgsl,
        })
        .expect("module");
    let layout = device
        .create_bind_group_layout(&reflect_bind_groups(&wgsl, &["cs"]).expect("reflection")[0])
        .expect("layout");
    let pipeline = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "furnace",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "cs",
                constants: vec![],
            },
            immediate_size: 0,
        })
        .expect("pipeline");

    let input_bytes: &[u8] = bytemuck::cast_slice(cases);
    let inputs = device
        .create_buffer(&BufferDesc {
            label: "cases",
            size: input_bytes.len() as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        })
        .expect("cases");
    device.write_buffer(inputs, 0, input_bytes).expect("upload");
    let output_size = (cases.len() * 3 * 16) as u64;
    let outputs = device
        .create_buffer(&BufferDesc {
            label: "results",
            size: output_size,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
        })
        .expect("results");
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size: output_size,
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");
    let mut entries = ibl.bind_group_entries(environment);
    entries.push(BindGroupEntry {
        binding: 4,
        resource: BindingResource::whole_buffer(inputs),
    });
    entries.push(BindGroupEntry {
        binding: 5,
        resource: BindingResource::whole_buffer(outputs),
    });
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "furnace",
            layout,
            entries,
        })
        .expect("bind group");

    let mut list = CommandList::new();
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "furnace",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.dispatch(u32::try_from(cases.len().div_ceil(16)).expect("few groups"), 1, 1);
    }
    list.copy_buffer_to_buffer(outputs, 0, readback, 0, output_size);
    device.submit(list).expect("submit");
    let texels: Vec<[f32; 4]> = bytemuck::pod_collect_to_vec(&device.read_buffer(readback).expect("read"));
    texels
        .as_chunks::<3>()
        .0
        .iter()
        .map(|chunk| Evaluation {
            diffuse: [chunk[0][0], chunk[0][1], chunk[0][2]],
            specular: [chunk[1][0], chunk[1][1], chunk[1][2]],
            lookup: [chunk[2][0], chunk[2][1], chunk[2][2]],
        })
        .collect()
}

fn furnace(device: &Device) -> (IblBuilder, IblEnvironment) {
    let mut builder = IblBuilder::new();
    let image = SyntheticSky::furnace(1.0).image(256, 128).expect("image");
    let environment = builder
        .build_environment(device, &image, &IblSettings::COMPACT)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    (builder, environment)
}

#[test]
fn a_white_furnace_returns_one_for_diffuse_and_for_a_white_metal() {
    let Some(device) = gpu("a_white_furnace_returns_one_for_diffuse_and_for_a_white_metal") else {
        return;
    };
    let (_builder, environment) = furnace(&device);

    let roughness = [0.05, 0.2, 0.4, 0.6, 0.8, 1.0];
    let n_dot_v = [0.2, 0.5, 0.8, 1.0];
    let mut cases = Vec::new();
    for &r in &roughness {
        for &nv in &n_dot_v {
            let view = view_with_n_dot_v(nv);
            cases.push(case(view, 1.0, 0.0, 0.0, r)); // Lambert only, albedo 1
            cases.push(case(view, 0.0, 1.0, 1.0, r)); // white metal
            cases.push(case(view, 1.0, 0.04, 1.0, r)); // dielectric: diffuse and specular share the energy
        }
    }
    let results = evaluate(&device, &environment, &cases);
    for (index, result) in results.iter().enumerate() {
        let (r, nv) = (roughness[index / 12], n_dot_v[(index / 3) % 4]);
        let sum = |values: [f32; 3]| values.iter().sum::<f32>() / 3.0;
        match index % 3 {
            0 => {
                assert!(
                    (sum(result.diffuse) - 1.0).abs() < 0.02,
                    "diffuse furnace, r {r} n.v {nv}: {result:?}"
                );
                assert!(
                    sum(result.specular) < 1e-3,
                    "no specular without f0 and f90: {result:?}"
                );
            }
            1 => {
                assert!(sum(result.diffuse) < 1e-3, "{result:?}");
                assert!(
                    (sum(result.specular) - 1.0).abs() < 0.03,
                    "white metal furnace, r {r} n.v {nv}: {result:?}"
                );
            }
            _ => {
                let total = sum(result.diffuse) + sum(result.specular);
                assert!(
                    (0.98..1.06).contains(&total),
                    "dielectric furnace, r {r} n.v {nv}: {total} {result:?}"
                );
            }
        }
    }
}

#[test]
fn the_specular_lookup_maps_roughness_to_the_prefiltered_mips() {
    let Some(device) = gpu("the_specular_lookup_maps_roughness_to_the_prefiltered_mips") else {
        return;
    };
    let sky = SyntheticSky::default();
    let mut builder = IblBuilder::new();
    let environment = builder
        .build_environment(&device, &sky.image(512, 256).expect("image"), &IblSettings::DESKTOP)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));

    // Directions away from the sun and from the face centres (where an 8 x 8 mip is coarsest), from the zenith to the
    // ground.
    let directions = [
        [0.2, 1.0, 0.1],
        [0.6, 0.64, 0.48],
        [-0.8, 0.0, 0.6],
        [0.3, -1.0, 0.2],
        [0.5, -0.5, std::f32::consts::FRAC_1_SQRT_2],
    ];
    let roughness = [0.0_f32, 0.25, 0.5, 0.75, 1.0];
    let mut cases = Vec::new();
    for direction in directions {
        let length = direction.iter().map(|axis| axis * axis).sum::<f32>().sqrt();
        for &r in &roughness {
            cases.push(case(direction.map(|axis| axis / length), 0.0, 0.0, 0.0, r));
        }
    }
    let results = evaluate(&device, &environment, &cases);
    for (index, result) in results.iter().enumerate() {
        let (direction, r) = (directions[index / 5], roughness[index % 5]);
        let length = direction.iter().map(|axis| axis * axis).sum::<f32>().sqrt();
        let unit = direction.map(|axis| f64::from(axis / length));
        let expected = prefiltered_radiance(sky_radiance(sky), unit, f64::from(r));
        for channel in 0..3 {
            let (actual, wanted) = (f64::from(result.lookup[channel]), expected[channel]);
            // Between two mips the lookup blends neighbours of the roughness asked for.
            assert!(
                (actual - wanted).abs() <= 0.1 * wanted + 0.03,
                "direction {direction:?} roughness {r} channel {channel}: gpu {actual} cpu {wanted}"
            );
        }
    }
    environment.release(&device);
    builder.release(&device);
}
