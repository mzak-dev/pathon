#![cfg(feature = "gpu-tests")]
// Textures come and go while the program runs: a slot freed by `unregister` is reused after the frames in flight, the
// array group is rebuilt, and the shader reads the new texture, on both paths. Pixels are read back from a GPU.
// `PATHON_TEST_BACKEND=vulkan|dx12|metal` picks the backend.

mod common;

use pathon_bindless::{
    BINDLESS_FEATURES, BinderDesc, PathPreference, SamplerPreset, TextureBinder, TextureHandle, TexturePath,
};
use pathon_golden::{announce_adapter, gpu_or_skip};
use pathon_material::TEXTURE_SOURCE_MODULE;
use pathon_rhi::{
    Access, Backend, BindGroupDesc, BindGroupEntry, BindingResource, BufferCopy, BufferDesc, BufferId, BufferUsage,
    Color, ColorAttachment, ColorTargetState, CommandList, Device, DeviceApi, DeviceRequest, Extent3d, Features,
    Format, PrimitiveState, RenderPassDesc, RenderPipelineDesc, RenderPipelineId, RhiError, ShaderStage, ShaderStages,
    SubresourceRange, TexelLayout, TextureCopy, TextureDesc, TextureUsage, TextureViewDesc, TextureViewId,
};
use pathon_shader::{ShaderLibrary, reflect_bind_groups};

const ROW_BYTES: u32 = 256;
const SIZE: u32 = ROW_BYTES / 4;

// The hook module alone: `sample_material_texture` is called with the slot value from a buffer.
const ROOT: &str = "
import package::material::texture_source::sample_material_texture;

@group(0) @binding(0) var<storage, read> slots: array<u32>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return sample_material_texture(slots[0], vec2<f32>(0.5, 0.5), vec2<f32>(0.0), vec2<f32>(0.0));
}
";

fn backend_from_environment() -> Option<Backend> {
    match std::env::var("PATHON_TEST_BACKEND").ok()?.to_ascii_lowercase().as_str() {
        "vulkan" => Some(Backend::Vulkan),
        "dx12" => Some(Backend::Dx12),
        "metal" => Some(Backend::Metal),
        other => panic!("PATHON_TEST_BACKEND `{other}` is not vulkan, dx12 or metal"),
    }
}

fn device(test: &str, required_features: Features) -> Option<Device> {
    let built = Device::build(&DeviceRequest {
        label: "dynamic textures",
        backend: backend_from_environment(),
        required_features,
        ..Default::default()
    });
    if let Err(RhiError::MissingFeatures(missing)) = &built {
        common::skip(test, *missing);
        return None;
    }
    let device = gpu_or_skip(test, built, |error| matches!(error, RhiError::NoAdapter(_)))?;
    announce_adapter(test, device.caps());
    Some(device)
}

fn solid_texture(device: &Device, color: [u8; 4]) -> TextureViewId {
    let texture = device
        .create_texture(&TextureDesc::new_2d(
            "solid",
            1,
            1,
            Format::Rgba8Unorm,
            TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        ))
        .expect("texture");
    device
        .write_texture(
            &TextureCopy::whole(texture),
            &color,
            TexelLayout {
                offset: 0,
                bytes_per_row: 4,
                rows_per_image: 1,
            },
            Extent3d::new_2d(1, 1),
        )
        .expect("upload");
    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);
    device.submit(list).expect("barrier");
    device
        .create_texture_view(texture, &TextureViewDesc::default())
        .expect("view")
}

struct Probe {
    pipeline: RenderPipelineId,
    group: pathon_rhi::BindGroupId,
    slots: BufferId,
}

impl Probe {
    fn new(device: &Device, binder: &TextureBinder) -> Self {
        let mut library = ShaderLibrary::new();
        library
            .add_module(TEXTURE_SOURCE_MODULE, &binder.texture_source_wgsl(1))
            .expect("module path");
        library.add_root("probe", ROOT);
        let key = pathon_shader::ShaderKey {
            root: "probe".to_owned(),
            features: pathon_shader::ShaderFeatures::default(),
        };
        let wgsl = library.wgsl(&key).unwrap_or_else(|error| panic!("{error}"));
        let module = library.module(device, &key).expect("module");
        let layout = device
            .create_bind_group_layout(&reflect_bind_groups(&wgsl, &["vs", "fs"]).expect("reflection")[0])
            .expect("layout");
        let slots = device
            .create_buffer(&BufferDesc {
                label: "slots",
                size: 4,
                usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            })
            .expect("slots");
        let group = device
            .create_bind_group(&BindGroupDesc {
                label: "slots",
                layout,
                entries: vec![BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::whole_buffer(slots),
                }],
            })
            .expect("group");
        let stage = |entry_point| ShaderStage {
            module,
            entry_point,
            constants: vec![],
        };
        let pipeline = device
            .create_render_pipeline(&RenderPipelineDesc {
                label: "probe",
                bind_group_layouts: vec![layout, binder.layout()],
                vertex: stage("vs"),
                vertex_buffers: vec![],
                fragment: Some(stage("fs")),
                color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
                primitive: PrimitiveState::default(),
                depth: None,
                sample_count: 1,
                immediate_size: 0,
            })
            .expect("pipeline");
        Self { pipeline, group, slots }
    }

    // The colour a fragment reads for `slot`, with `textures` as the material's set.
    fn colour_of(&self, device: &Device, binder: &mut TextureBinder, handle: TextureHandle) -> [u8; 4] {
        let slot = binder.shader_value(handle).expect("live");
        device.write_buffer(self.slots, 0, &slot.to_le_bytes()).expect("slot");
        let textures = binder
            .bind_group(device, 0, &[handle])
            .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
        let target = device
            .create_texture(&TextureDesc::new_2d(
                "target",
                SIZE,
                1,
                Format::Rgba8Unorm,
                TextureUsage::RENDER_TARGET | TextureUsage::COPY_SRC,
            ))
            .expect("target");
        let view = device
            .create_texture_view(target, &TextureViewDesc::default())
            .expect("view");
        let readback = device
            .create_buffer(&BufferDesc {
                label: "readback",
                size: u64::from(ROW_BYTES),
                usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
            })
            .expect("readback");
        let mut list = CommandList::new();
        list.texture_access(target, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
        {
            let mut pass = list.render_pass(RenderPassDesc {
                label: "probe",
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
                timestamps: None,
            });
            pass.set_pipeline(self.pipeline);
            pass.set_bind_group(0, self.group);
            pass.set_bind_group(1, textures);
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
                    bytes_per_row: ROW_BYTES,
                    rows_per_image: 1,
                },
            },
            Extent3d::new_2d(SIZE, 1),
        );
        device.submit(list).expect("submit");
        let bytes = device.read_buffer(readback).expect("read");
        [bytes[0], bytes[1], bytes[2], bytes[3]]
    }
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

// The scenario of a texture streaming in and out, the same on every path.
fn textures_come_and_go(device: &Device, expected_path: impl Fn(TexturePath) -> bool) {
    let mut binder = TextureBinder::new(
        device,
        &BinderDesc {
            capacity: 128,
            visibility: ShaderStages::FRAGMENT,
            ..BinderDesc::default()
        },
    )
    .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    assert!(expected_path(binder.path()), "{:?}", binder.path());
    let probe = Probe::new(device, &binder);
    let register = |binder: &mut TextureBinder, color| {
        binder
            .register(solid_texture(device, color), SamplerPreset::LinearClamp)
            .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
    };

    binder.begin_frame(device, 0);
    let red = register(&mut binder, RED);
    let blue = register(&mut binder, BLUE);
    assert_eq!(probe.colour_of(device, &mut binder, red), RED);
    assert_eq!(probe.colour_of(device, &mut binder, blue), BLUE);

    binder.unregister(red).expect("live");
    binder.begin_frame(device, 1);
    let green = register(&mut binder, GREEN);
    assert_ne!(green.index(), red.index(), "the slot of `red` is still in flight");
    assert_eq!(probe.colour_of(device, &mut binder, green), GREEN);
    assert_eq!(
        probe.colour_of(device, &mut binder, blue),
        BLUE,
        "neighbours are untouched"
    );

    binder.begin_frame(device, 2);
    let yellow = register(&mut binder, YELLOW);
    assert_eq!(yellow.index(), red.index(), "the slot came back");
    assert_eq!(probe.colour_of(device, &mut binder, yellow), YELLOW);
    assert_eq!(probe.colour_of(device, &mut binder, green), GREEN);
    assert_eq!(probe.colour_of(device, &mut binder, blue), BLUE);
    assert!(
        binder.bind_group(device, 0, &[red]).is_err(),
        "the old handle stays dead"
    );

    device.wait_idle().expect("idle");
    binder.release(device);
}

#[test]
fn bindless_slots_are_reused_and_the_array_is_rebuilt() {
    let Some(device) = device("bindless_slots_are_reused_and_the_array_is_rebuilt", BINDLESS_FEATURES) else {
        return;
    };

    textures_come_and_go(&device, |path| matches!(path, TexturePath::Bindless { capacity: 128 }));
}

#[test]
fn per_material_groups_follow_the_textures_that_come_and_go() {
    let Some(device) = device(
        "per_material_groups_follow_the_textures_that_come_and_go",
        Features::empty(),
    ) else {
        return;
    };

    textures_come_and_go(&device, |path| path == TexturePath::PerMaterial);
}

#[test]
fn the_fallback_diagnostic_is_reported_for_a_device_without_arrays() {
    let Some(device) = device(
        "the_fallback_diagnostic_is_reported_for_a_device_without_arrays",
        Features::empty(),
    ) else {
        return;
    };

    let binder = TextureBinder::new(
        &device,
        &BinderDesc {
            preference: PathPreference::Auto,
            ..BinderDesc::default()
        },
    )
    .expect("auto never fails");

    assert_eq!(
        binder.fallback_diagnostic().map(|diagnostic| diagnostic.code),
        Some("B0001")
    );
}
