#![cfg(feature = "gpu-tests")]
// The standard material evaluated at fixed surface points by a compute shader and by a fragment shader of the same
// module (ADR-0030): both must give the same BSDF. `PATHON_TEST_BACKEND=vulkan|dx12|metal` picks the backend.

mod common;

use common::{OUTPUTS_PER_POINT, POINT_COUNT, ROOT_NAME, TestPoint, library, test_materials, test_points, wgsl_of};
use pathon_api::glam::Vec3;
use pathon_api::resources::DeferredRelease;
use pathon_api::rhi::{
    Access, Backend, BindGroupDesc, BindGroupEntry, BindingResource, BufferCopy, BufferDesc, BufferId, BufferUsage,
    Color, ColorAttachment, ColorTargetState, CommandList, ComputePassDesc, ComputePipelineDesc, DeviceApi,
    DeviceRequest, Extent3d, Format, PrimitiveState, RenderPassDesc, RenderPipelineDesc, RhiError, ShaderModuleDesc,
    ShaderStage, SubresourceRange, TexelLayout, TextureCopy, TextureDesc, TextureUsage, TextureViewDesc,
};
use pathon_golden::{announce_adapter, gpu_or_skip};
use pathon_material::{AlphaKind, MaterialHooks, MaterialTable, MaterialVariant, StandardMaterial};
use pathon_rhi::Device;
use pathon_shader::reflect_bind_groups;

type Texel = [f32; 4];

const OUTPUT_WIDTH: u32 = 160;
const _: () = assert!(OUTPUT_WIDTH as usize == POINT_COUNT * OUTPUTS_PER_POINT);

fn backend_from_environment() -> Option<Backend> {
    match std::env::var("PATHON_TEST_BACKEND").ok()?.to_ascii_lowercase().as_str() {
        "vulkan" => Some(Backend::Vulkan),
        "dx12" => Some(Backend::Dx12),
        "metal" => Some(Backend::Metal),
        other => panic!("PATHON_TEST_BACKEND `{other}` is not vulkan, dx12 or metal"),
    }
}

fn device(test: &str) -> Option<Device> {
    let built = Device::build(&DeviceRequest {
        label: "material evaluation",
        backend: backend_from_environment(),
        ..Default::default()
    });
    let device = gpu_or_skip(test, built, |error| matches!(error, RhiError::NoAdapter(_)))?;
    announce_adapter(test, device.caps());
    Some(device)
}

fn buffer_with(device: &Device, label: &'static str, bytes: &[u8], usage: BufferUsage) -> BufferId {
    let buffer = device
        .create_buffer(&BufferDesc {
            label,
            size: bytes.len() as u64,
            usage: usage | BufferUsage::COPY_DST,
        })
        .expect("buffer");
    device.write_buffer(buffer, 0, bytes).expect("upload");
    buffer
}

fn texels(bytes: &[u8]) -> Vec<Texel> {
    bytemuck::pod_collect_to_vec(bytes)
}

// The material array goes through `MaterialTable::sync`, the same path a renderer uses.
struct Scene<'a> {
    device: &'a Device,
    materials: BufferId,
    points: BufferId,
}

impl<'a> Scene<'a> {
    fn new(device: &'a Device, materials: &[StandardMaterial]) -> Self {
        let mut table = MaterialTable::new("materials", 2);
        for material in materials {
            table.add(material).expect("valid material");
        }
        let materials = table
            .sync(device, &mut DeferredRelease::new(2), 0)
            .expect("material upload");
        let points = buffer_with(
            device,
            "points",
            bytemuck::cast_slice(&test_points()),
            BufferUsage::STORAGE,
        );
        Self {
            device,
            materials,
            points,
        }
    }
}

fn read_back(device: &Device, source: BufferId, size: u64, list: &mut CommandList) -> BufferId {
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size,
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");
    list.copy_buffer_to_buffer(source, 0, readback, 0, size);
    readback
}

fn entries(buffers: &[BufferId]) -> Vec<BindGroupEntry> {
    buffers
        .iter()
        .enumerate()
        .map(|(binding, buffer)| BindGroupEntry {
            binding: u32::try_from(binding).expect("few bindings"),
            resource: BindingResource::whole_buffer(*buffer),
        })
        .collect()
}

impl Scene<'_> {
    fn module(&self, wgsl: &str) -> pathon_api::rhi::ShaderModuleId {
        self.device
            .create_shader_module(&ShaderModuleDesc {
                label: "permutation",
                wgsl,
            })
            .unwrap_or_else(|error| panic!("{error}\n{wgsl}"))
    }

    fn compute(&self, wgsl: &str) -> Vec<Texel> {
        let layout_desc = reflect_bind_groups(wgsl, &["cs"]).expect("reflection").remove(0);
        let layout = self.device.create_bind_group_layout(&layout_desc).expect("layout");
        let module = self.module(wgsl);
        let pipeline = self
            .device
            .create_compute_pipeline(&ComputePipelineDesc {
                label: "material compute",
                bind_group_layouts: vec![layout],
                shader: ShaderStage {
                    module,
                    entry_point: "cs",
                    constants: vec![],
                },
                immediate_size: 0,
            })
            .unwrap_or_else(|error| panic!("{error}\n{wgsl}"));
        let size = u64::from(OUTPUT_WIDTH) * 16;
        let results = self
            .device
            .create_buffer(&BufferDesc {
                label: "results",
                size,
                usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
            })
            .expect("results");
        let group = self
            .device
            .create_bind_group(&BindGroupDesc {
                label: "material compute",
                layout,
                entries: entries(&[self.materials, self.points, results]),
            })
            .expect("bind group");
        let mut list = CommandList::new();
        {
            let mut pass = list.compute_pass(ComputePassDesc {
                label: "material compute",
                timestamps: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group);
            pass.dispatch(u32::try_from(POINT_COUNT.div_ceil(16)).expect("few groups"), 1, 1);
        }
        let readback = read_back(self.device, results, size, &mut list);
        self.device.submit(list).expect("submit");
        texels(&self.device.read_buffer(readback).expect("read"))
    }

    fn fragment(&self, wgsl: &str) -> Vec<Texel> {
        let layout_desc = reflect_bind_groups(wgsl, &["vs", "fs"]).expect("reflection").remove(0);
        let layout = self.device.create_bind_group_layout(&layout_desc).expect("layout");
        let module = self.module(wgsl);
        let pipeline = self
            .device
            .create_render_pipeline(&RenderPipelineDesc {
                label: "material fragment",
                bind_group_layouts: vec![layout],
                vertex: ShaderStage {
                    module,
                    entry_point: "vs",
                    constants: vec![],
                },
                vertex_buffers: vec![],
                fragment: Some(ShaderStage {
                    module,
                    entry_point: "fs",
                    constants: vec![],
                }),
                color_targets: vec![ColorTargetState::opaque(Format::Rgba32Float)],
                primitive: PrimitiveState::default(),
                depth: None,
                sample_count: 1,
                immediate_size: 0,
            })
            .unwrap_or_else(|error| panic!("{error}\n{wgsl}"));
        let group = self
            .device
            .create_bind_group(&BindGroupDesc {
                label: "material fragment",
                layout,
                entries: entries(&[self.materials, self.points]),
            })
            .expect("bind group");
        let target = self
            .device
            .create_texture(&TextureDesc::new_2d(
                "results",
                OUTPUT_WIDTH,
                1,
                Format::Rgba32Float,
                TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
            ))
            .expect("target");
        let view = self
            .device
            .create_texture_view(target, &TextureViewDesc::default())
            .expect("view");
        let row_bytes = OUTPUT_WIDTH * 16;
        let readback = self
            .device
            .create_buffer(&BufferDesc {
                label: "readback",
                size: u64::from(row_bytes),
                usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
            })
            .expect("readback");
        let mut list = CommandList::new();
        list.texture_access(target, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
        {
            let mut pass = list.render_pass(RenderPassDesc {
                label: "material fragment",
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
                timestamps: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group);
            pass.draw(0..3, 0..1);
        }
        list.texture_access(
            target,
            SubresourceRange::ALL,
            Access::COLOR_ATTACHMENT,
            Access::COPY_SRC,
        );
        list.copy_texture_to_buffer(
            TextureCopy::whole(target),
            BufferCopy {
                buffer: readback,
                layout: TexelLayout {
                    offset: 0,
                    bytes_per_row: row_bytes,
                    rows_per_image: 1,
                },
            },
            Extent3d::new_2d(OUTPUT_WIDTH, 1),
        );
        self.device.submit(list).expect("submit");
        texels(&self.device.read_buffer(readback).expect("read"))
    }
}

fn at(results: &[Texel], point: usize, channel: usize) -> Texel {
    results[point * OUTPUTS_PER_POINT + channel]
}

fn assert_close(actual: f32, expected: f32, tolerance: f32, context: &str) {
    let allowed = tolerance * (1.0 + expected.abs());
    assert!(
        (actual - expected).abs() <= allowed,
        "{context}: {actual} differs from {expected} by more than {allowed}"
    );
}

fn assert_same_results(compute: &[Texel], fragment: &[Texel], context: &str) {
    assert_eq!(compute.len(), fragment.len(), "{context}");
    for (index, (from_compute, from_fragment)) in compute.iter().zip(fragment).enumerate() {
        for lane in 0..4 {
            assert!(
                from_compute[lane].is_finite(),
                "{context}: point {index} lane {lane} is not finite"
            );
            assert_close(
                from_fragment[lane],
                from_compute[lane],
                1e-4,
                &format!(
                    "{context}: point {} channel {} lane {lane}",
                    index / OUTPUTS_PER_POINT,
                    index % OUTPUTS_PER_POINT
                ),
            );
        }
    }
}

#[test]
fn compute_and_fragment_agree_for_every_permutation() {
    let Some(device) = device("compute_and_fragment_agree_for_every_permutation") else {
        return;
    };
    let scene = Scene::new(&device, &test_materials());
    let mut library = library(&MaterialHooks::default());
    let mut lit_points = 0;

    for variant in MaterialVariant::all() {
        let wgsl = library.wgsl(&variant.shader_key(ROOT_NAME)).expect("composes");
        let compute = scene.compute(&wgsl);
        let fragment = scene.fragment(&wgsl);

        assert_same_results(&compute, &fragment, &format!("{variant:?}"));
        lit_points += (0..POINT_COUNT)
            .filter(|point| {
                let [r, g, b, _] = at(&compute, *point, 0);
                r + g + b > 0.0
            })
            .count();
    }

    assert!(lit_points > 0, "the test points never light a surface");
}

// The same evaluation in plain Rust (glTF 2.0 appendix B, Lambert diffuse, no extensions): the shader is checked
// against the equations and not only against itself.
#[expect(clippy::many_single_char_names, reason = "the glTF equations use these letters")]
fn reference(material: &StandardMaterial, point: &TestPoint) -> (Vec3, Vec3) {
    let vec3 = |value: [f32; 4]| Vec3::new(value[0], value[1], value[2]);
    let facing = if point.normal[3] > 0.5 { 1.0 } else { -1.0 };
    let n = vec3(point.normal).normalize() * facing;
    let (v, l) = (vec3(point.view), vec3(point.light_direction));
    let base = vec3(material.base_color) * vec3(point.color);
    let roughness = material.roughness.clamp(0.045, 1.0);
    let alpha = roughness * roughness;
    let f0 = Vec3::splat(0.04).lerp(base, material.metallic);
    let diffuse_color = base * (1.0 - material.metallic);

    let n_dot_l = n.dot(l).max(0.0);
    if n_dot_l <= 0.0 {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let n_dot_v = n.dot(v).abs().max(1e-4);
    let h = (l + v).normalize();
    let (n_dot_h, v_dot_h) = (n.dot(h).clamp(0.0, 1.0), v.dot(h).clamp(0.0, 1.0));
    let fresnel = f0 + (Vec3::ONE - f0) * (1.0 - v_dot_h).powi(5);
    let a2 = alpha * alpha;
    let d = a2 / (std::f32::consts::PI * (n_dot_h * n_dot_h * (a2 - 1.0) + 1.0).powi(2));
    let ggx_v = n_dot_l * (n_dot_v * n_dot_v * (1.0 - a2) + a2).sqrt();
    let ggx_l = n_dot_v * (n_dot_l * n_dot_l * (1.0 - a2) + a2).sqrt();
    let visibility = 0.5 / (ggx_v + ggx_l);
    let radiance = vec3(point.light_radiance);
    let diffuse = diffuse_color * (Vec3::ONE - fresnel) / std::f32::consts::PI * n_dot_l * radiance;
    let specular = fresnel * (d * visibility) * n_dot_l * radiance;
    (diffuse, specular)
}

#[test]
fn the_shader_matches_the_gltf_equations() {
    let Some(device) = device("the_shader_matches_the_gltf_equations") else {
        return;
    };
    let materials = test_materials();
    let scene = Scene::new(&device, &materials);
    let compute = scene.compute(&wgsl_of(MaterialVariant::default(), &MaterialHooks::default()));
    let points = test_points();

    let mut checked = 0;
    // Materials 0, 1, 2 and 6 use no extension, so the permutation without flags is exact for them.
    for (index, point) in points.iter().enumerate() {
        let material = point.material[0] as usize;
        if ![0, 1, 2, 6].contains(&material) {
            continue;
        }
        let (diffuse, specular) = reference(&materials[material], point);
        let [dr, dg, db, _] = at(&compute, index, 0);
        let [sr, sg, sb, _] = at(&compute, index, 1);
        for (actual, expected, lane) in [
            (dr, diffuse.x, "diffuse r"),
            (dg, diffuse.y, "diffuse g"),
            (db, diffuse.z, "diffuse b"),
            (sr, specular.x, "specular r"),
            (sg, specular.y, "specular g"),
            (sb, specular.z, "specular b"),
        ] {
            assert_close(
                actual,
                expected,
                2e-3,
                &format!("point {index} material {material} {lane}"),
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 16);
}

fn sum(results: &[Texel], material: usize, channel: usize, lane: usize) -> f32 {
    (0..POINT_COUNT)
        .filter(|point| point % 8 == material)
        .map(|point| at(results, point, channel)[lane])
        .sum()
}

fn variant_results(scene: &Scene<'_>, variant: MaterialVariant) -> Vec<Texel> {
    scene.compute(&wgsl_of(variant, &MaterialHooks::default()))
}

#[test]
fn each_flag_turns_on_exactly_its_lobe() {
    let Some(device) = device("each_flag_turns_on_exactly_its_lobe") else {
        return;
    };
    let scene = Scene::new(&device, &test_materials());
    let plain = variant_results(&scene, MaterialVariant::default());

    let coated = variant_results(
        &scene,
        MaterialVariant {
            clearcoat: true,
            ..MaterialVariant::default()
        },
    );
    assert!(
        sum(&coated, 3, 1, 0) > 1.2 * sum(&plain, 3, 1, 0),
        "the clearcoat adds a sharp highlight"
    );
    assert_close(
        sum(&coated, 0, 1, 0),
        sum(&plain, 0, 1, 0),
        1e-5,
        "a material without coat is unchanged",
    );

    let transmissive = variant_results(
        &scene,
        MaterialVariant {
            transmission: true,
            ..MaterialVariant::default()
        },
    );
    assert!(
        sum(&transmissive, 4, 0, 0) < 0.4 * sum(&plain, 4, 0, 0),
        "transmission takes 80% of the diffuse"
    );
    assert_eq!(sum(&plain, 4, 2, 0), 0.0, "no transmission weight without the flag");
    assert!(sum(&transmissive, 4, 2, 0) > 0.0);
    assert_eq!(sum(&plain, 4, 3, 3), 0.0, "thickness is dropped without the flag");
    assert!(sum(&transmissive, 4, 3, 3) > 0.0);
    assert!(
        sum(&transmissive, 4, 4, 0) < 4.0 * 1.0,
        "attenuation darkens the volume"
    );
    assert_eq!(sum(&plain, 4, 4, 0), 4.0, "a clear volume without the flag");

    let subsurface = variant_results(
        &scene,
        MaterialVariant {
            subsurface: true,
            ..MaterialVariant::default()
        },
    );
    assert!(
        sum(&subsurface, 5, 0, 1) > sum(&plain, 5, 0, 1),
        "backlit points gain the transmitted lobe"
    );
}

#[test]
fn alpha_kinds_decide_the_alpha_and_the_test() {
    let Some(device) = device("alpha_kinds_decide_the_alpha_and_the_test") else {
        return;
    };
    let scene = Scene::new(&device, &test_materials());
    let variant = |alpha| MaterialVariant {
        alpha,
        ..MaterialVariant::default()
    };

    let opaque = variant_results(&scene, variant(AlphaKind::Opaque));
    let masked = variant_results(&scene, variant(AlphaKind::Masked));
    let transparent = variant_results(&scene, variant(AlphaKind::Transparent));

    // Material 6 has alpha 0.55 and cutoff 0.5; point 22 is tinted with alpha 0.9, so 0.495 falls below it.
    for point in [6, 14, 22, 30] {
        let expected_test = if point == 22 { 0.0 } else { 1.0 };
        assert_eq!(at(&masked, point, 1)[3], expected_test, "masked, point {point}");
        assert_eq!(at(&opaque, point, 1)[3], 1.0, "opaque never rejects, point {point}");
        assert_eq!(at(&opaque, point, 0)[3], 1.0, "opaque writes full alpha, point {point}");
        let alpha = at(&transparent, point, 0)[3];
        assert_close(alpha, if point == 22 { 0.495 } else { 0.55 }, 1e-5, "transparent alpha");
    }
}

fn procedural_texture_source() -> MaterialHooks {
    let mut hooks = MaterialHooks::default();
    hooks.set_texture_source(
        "fn sample_material_texture(slot: u32, uv: vec2<f32>, uv_dx: vec2<f32>, uv_dy: vec2<f32>) -> vec4<f32> {
             return vec4<f32>(fract(uv.x * 4.0 + f32(slot) * 0.1), uv_dx.x * 10.0, uv_dy.y * 10.0, 1.0);
         }",
    );
    hooks
}

#[test]
fn a_texture_source_hook_receives_the_slot_and_the_explicit_gradients() {
    let Some(device) = device("a_texture_source_hook_receives_the_slot_and_the_explicit_gradients") else {
        return;
    };
    let scene = Scene::new(&device, &test_materials());
    let hooks = procedural_texture_source();
    let wgsl = wgsl_of(MaterialVariant::default(), &hooks);

    let compute = scene.compute(&wgsl);
    assert_same_results(&compute, &scene.fragment(&wgsl), "procedural textures");

    // Point 7 uses material 7 (emissive 0.2, 0.1, 0 times strength 3) with the emissive texture in slot 6 at uv.x = 0.7:
    // the texture reads (fract(0.7 * 4 + 0.6), 0.01 * 10, 0.01 * 10).
    let [red, green, blue, _] = at(&compute, 7, 3);
    assert_close(
        red,
        0.6 * (2.8_f32 + 0.6).fract(),
        1e-4,
        "emissive red from the slot and uv",
    );
    assert_close(green, 0.3 * 0.1, 1e-4, "emissive green from the uv gradient");
    assert_close(blue, 0.0, 1e-4, "emissive blue has no emissive factor");
}

#[test]
fn a_surface_hook_rewrites_the_surface_in_both_stages() {
    let Some(device) = device("a_surface_hook_rewrites_the_surface_in_both_stages") else {
        return;
    };
    let scene = Scene::new(&device, &test_materials());
    let mut hooks = MaterialHooks::default();
    hooks.set_surface_hook(
        "import package::material::surface::{Surface, SurfaceInput};
         fn modify_surface(input: SurfaceInput, surface: Surface) -> Surface {
             var changed = surface;
             changed.emissive = vec3<f32>(input.position.x * 0.5, 0.25, 0.75);
             return changed;
         }",
    );
    assert_eq!(hooks.validate(), Ok(()));
    let wgsl = wgsl_of(MaterialVariant::default(), &hooks);

    let compute = scene.compute(&wgsl);

    assert_same_results(&compute, &scene.fragment(&wgsl), "surface hook");
    for point in 0..POINT_COUNT {
        let [red, green, blue, _] = at(&compute, point, 3);
        assert_close(red, point as f32 * 0.5, 1e-5, "hook reads the surface input");
        assert_eq!((green, blue), (0.25, 0.75));
    }
}
