use std::num::NonZeroU32;

use pathon_rhi_null::NullDevice;
use pathon_rhi_types::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource, BindingType,
    Color, ColorAttachment, ColorTargetState, CommandList, ComputePassDesc, ComputePipelineDesc, ComputePipelineId,
    DepthAttachment, DeviceApi, Features, Format, PassTimestamps, PolygonMode, PrimitiveState, QuerySetDesc,
    QuerySetId, RenderPassDesc, RenderPipelineDesc, Resource, RhiError, ShaderModuleDesc, ShaderStage, ShaderStages,
    StorageAccess, SubresourceRange, SurfaceConfig, TextureDesc, TextureId, TextureUsage, TextureViewDesc,
    TextureViewDimension,
};

fn code(result: Result<(), RhiError>) -> &'static str {
    result.expect_err("validation should fail").code()
}

fn render_target(device: &NullDevice) -> TextureId {
    let desc = TextureDesc::new_2d(
        "target",
        4,
        4,
        Format::Rgba8Unorm,
        TextureUsage::RENDER_TARGET | TextureUsage::SAMPLED | TextureUsage::COPY_SRC,
    );
    device.create_texture(&desc).expect("texture")
}

fn clear_pass(device: &NullDevice, texture: TextureId, list: &mut CommandList) {
    let view = device
        .create_texture_view(texture, &TextureViewDesc::default())
        .expect("view");
    let _pass = list.render_pass(RenderPassDesc {
        label: "clear",
        color: vec![ColorAttachment::clear(view, Color::BLACK)],
        depth: None,
        timestamps: None,
    });
}

#[test]
fn attachment_without_barrier_is_rejected() {
    let device = NullDevice::new();
    let texture = render_target(&device);
    let mut list = CommandList::new();
    clear_pass(&device, texture, &mut list);

    assert_eq!(code(device.submit(list)), "R0101");
}

#[test]
fn attachment_after_barrier_is_accepted() {
    let device = NullDevice::new();
    let texture = render_target(&device);
    let mut list = CommandList::new();
    list.texture_access(
        texture,
        SubresourceRange::ALL,
        Access::empty(),
        Access::COLOR_ATTACHMENT,
    );
    clear_pass(&device, texture, &mut list);
    list.texture_access(
        texture,
        SubresourceRange::ALL,
        Access::COLOR_ATTACHMENT,
        Access::SAMPLED,
    );

    device.submit(list).expect("valid list");
}

#[test]
fn loading_a_never_written_attachment_is_rejected() {
    let device = NullDevice::new();
    let texture = render_target(&device);
    let view = device
        .create_texture_view(texture, &TextureViewDesc::default())
        .expect("view");
    let mut list = CommandList::new();
    list.texture_access(
        texture,
        SubresourceRange::ALL,
        Access::empty(),
        Access::COLOR_ATTACHMENT,
    );
    drop(list.render_pass(RenderPassDesc {
        label: "load",
        color: vec![ColorAttachment::load(view)],
        depth: None,
        timestamps: None,
    }));

    assert_eq!(code(device.submit(list)), "R0102");
}

#[test]
fn reading_hi_z_mip_before_it_is_written_is_rejected() {
    let device = NullDevice::new();
    let hi_z = device
        .create_texture(
            &TextureDesc::new_2d(
                "hi-z",
                8,
                8,
                Format::R32Float,
                TextureUsage::STORAGE | TextureUsage::SAMPLED,
            )
            .with_mips(3),
        )
        .expect("texture");
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "downsample",
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::texture_2d(),
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageTexture {
                        access: StorageAccess::WriteOnly,
                        format: Format::R32Float,
                        dimension: TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        })
        .expect("layout");
    let source = device
        .create_texture_view(hi_z, &TextureViewDesc::single_mip(1))
        .expect("view");
    let destination = device
        .create_texture_view(hi_z, &TextureViewDesc::single_mip(2))
        .expect("view");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "downsample mip 2",
            layout,
            entries: vec![
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(source),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(destination),
                },
            ],
        })
        .expect("bind group");
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "downsample",
            wgsl: "fn main() {}",
        })
        .expect("module");
    let pipeline = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "downsample",
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
    list.texture_access(hi_z, SubresourceRange::mip(1), Access::empty(), Access::SAMPLED);
    list.texture_access(hi_z, SubresourceRange::mip(2), Access::empty(), Access::STORAGE_WRITE);
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "downsample",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.dispatch(1, 1, 1);
    }

    assert_eq!(code(device.submit(list)), "R0102");
}

#[test]
fn a_texture_state_mixing_layouts_is_rejected() {
    let device = NullDevice::new();
    let texture = render_target(&device);
    let mut list = CommandList::new();
    list.texture_access(
        texture,
        SubresourceRange::ALL,
        Access::empty(),
        Access::SAMPLED | Access::COPY_SRC,
    );

    assert_eq!(code(device.submit(list)), "R0103");
}

#[test]
fn sampling_a_texture_in_a_storage_state_is_rejected() {
    let device = NullDevice::new();
    let texture = device
        .create_texture(&TextureDesc::new_2d(
            "storage",
            4,
            4,
            Format::R32Float,
            TextureUsage::STORAGE | TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        ))
        .expect("texture");
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "sampled",
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::texture_2d(),
                count: None,
            }],
        })
        .expect("layout");
    let view = device
        .create_texture_view(texture, &TextureViewDesc::default())
        .expect("view");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "sampled",
            layout,
            entries: vec![BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(view),
            }],
        })
        .expect("bind group");
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "sample",
            wgsl: "fn main() {}",
        })
        .expect("module");
    let pipeline = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "sample",
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
    list.texture_access(texture, SubresourceRange::ALL, Access::empty(), Access::STORAGE_READ);
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "sample",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.dispatch(1, 1, 1);
    }

    assert_eq!(code(device.submit(list)), "R0101");
}

fn sampled_compute_pass(device: &NullDevice, texture: TextureId, list: &mut CommandList) {
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "sampled",
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::texture_2d(),
                count: None,
            }],
        })
        .expect("layout");
    let view = device
        .create_texture_view(texture, &TextureViewDesc::default())
        .expect("view");
    let group = device
        .create_bind_group(&BindGroupDesc {
            label: "sampled",
            layout,
            entries: vec![BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(view),
            }],
        })
        .expect("bind group");
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "sample",
            wgsl: "fn main() {}",
        })
        .expect("module");
    let pipeline = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "sample",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![],
            },
            immediate_size: 0,
        })
        .expect("pipeline");
    let mut pass = list.compute_pass(ComputePassDesc {
        label: "sample",
        timestamps: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group);
    pass.dispatch(1, 1, 1);
}

fn cleared_depth_texture(device: &NullDevice, list: &mut CommandList) -> TextureId {
    let depth = device
        .create_texture(&TextureDesc::new_2d(
            "depth",
            4,
            4,
            Format::Depth32Float,
            TextureUsage::RENDER_TARGET | TextureUsage::SAMPLED,
        ))
        .expect("depth texture");
    let view = device
        .create_texture_view(depth, &TextureViewDesc::default())
        .expect("view");
    list.texture_access(depth, SubresourceRange::ALL, Access::empty(), Access::DEPTH_WRITE);
    drop(list.render_pass(RenderPassDesc {
        label: "depth",
        color: vec![],
        depth: Some(DepthAttachment::clear_reverse_z(view)),
        timestamps: None,
    }));
    depth
}

#[test]
fn a_depth_texture_sampled_in_a_sampled_and_depth_read_state_is_accepted() {
    let device = NullDevice::new();
    let mut list = CommandList::new();
    let depth = cleared_depth_texture(&device, &mut list);
    list.texture_access(
        depth,
        SubresourceRange::ALL,
        Access::DEPTH_WRITE,
        Access::SAMPLED | Access::DEPTH_READ,
    );
    sampled_compute_pass(&device, depth, &mut list);

    device.submit(list).expect("valid list");
}

#[test]
fn a_depth_texture_sampled_without_depth_read_is_rejected() {
    let device = NullDevice::new();
    let mut list = CommandList::new();
    let depth = cleared_depth_texture(&device, &mut list);
    list.texture_access(depth, SubresourceRange::ALL, Access::DEPTH_WRITE, Access::SAMPLED);
    sampled_compute_pass(&device, depth, &mut list);

    assert_eq!(code(device.submit(list)), "R0101");
}

#[test]
fn a_read_only_storage_binding_on_a_texture_without_storage_usage_is_rejected() {
    let device = NullDevice::new();
    let texture = render_target(&device);
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "storage",
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::StorageTexture {
                    access: StorageAccess::ReadOnly,
                    format: Format::Rgba8Unorm,
                    dimension: TextureViewDimension::D2,
                },
                count: None,
            }],
        })
        .expect("layout");
    let view = device
        .create_texture_view(texture, &TextureViewDesc::default())
        .expect("view");
    let result = device.create_bind_group(&BindGroupDesc {
        label: "storage",
        layout,
        entries: vec![BindGroupEntry {
            binding: 0,
            resource: BindingResource::TextureView(view),
        }],
    });

    assert_eq!(result.expect_err("missing STORAGE usage").code(), "R0105");
}

#[test]
fn draw_outside_render_pass_is_rejected() {
    let device = NullDevice::new();
    let mut list = CommandList::new();
    drop(list.compute_pass(ComputePassDesc {
        label: "empty",
        timestamps: None,
    }));
    list.push(pathon_rhi_types::Command::Draw {
        vertices: 0..3,
        instances: 0..1,
    });

    assert_eq!(code(device.submit(list)), "R0104");
}

#[test]
fn two_writes_in_one_state_are_rejected() {
    let device = NullDevice::new();
    let texture = render_target(&device);
    let mut list = CommandList::new();
    list.texture_access(
        texture,
        SubresourceRange::ALL,
        Access::empty(),
        Access::COLOR_ATTACHMENT | Access::COPY_DST,
    );

    assert_eq!(code(device.submit(list)), "R0103");
}

#[test]
fn surface_frame_must_reach_present_state() {
    let device = NullDevice::new();
    let window = NoWindow;
    let target = pathon_rhi_types::SurfaceTarget::new(
        std::sync::Arc::new(window),
        pathon_rhi_types::PhysicalSize::new(4, 4),
        1.0,
    );
    let surface = device.create_surface(&target).expect("surface");
    device
        .configure_surface(
            surface,
            &SurfaceConfig {
                size: target.size,
                present_mode: pathon_rhi_types::PresentMode::default(),
                format: None,
                color_space: pathon_rhi_types::SurfaceColorSpace::Auto,
            },
        )
        .expect("configure");

    let frame = device.acquire_surface(surface).expect("frame");
    assert_eq!(
        device.present_surface(surface).expect_err("not written").code(),
        "R0106"
    );

    let frame_again = device.acquire_surface(surface).expect("frame");
    assert_ne!(frame.texture, frame_again.texture);
    let mut list = CommandList::new();
    list.texture_access(
        frame_again.texture,
        SubresourceRange::ALL,
        Access::empty(),
        Access::COLOR_ATTACHMENT,
    );
    clear_pass_on_view(frame_again.view, &mut list);
    list.texture_access(
        frame_again.texture,
        SubresourceRange::ALL,
        Access::COLOR_ATTACHMENT,
        Access::PRESENT,
    );
    device.submit(list).expect("valid list");
    device.present_surface(surface).expect("presentable");
}

#[test]
fn lost_device_rejects_submissions() {
    let device = NullDevice::new();
    device.simulate_device_loss();

    assert!(device.is_lost());
    assert_eq!(code(device.submit(CommandList::new())), "R0006");
}

fn clear_pass_on_view(view: pathon_rhi_types::TextureViewId, list: &mut CommandList) {
    drop(list.render_pass(RenderPassDesc {
        label: "clear",
        color: vec![ColorAttachment::clear(view, Color::BLACK)],
        depth: None,
        timestamps: None,
    }));
}

struct NoWindow;

impl raw_window_handle::HasWindowHandle for NoWindow {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::Unavailable)
    }
}

impl raw_window_handle::HasDisplayHandle for NoWindow {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::Unavailable)
    }
}

#[test]
fn writing_a_deeper_slice_of_a_3d_texture_initializes_it() {
    let device = NullDevice::new();
    let desc = TextureDesc {
        size: pathon_rhi_types::Extent3d {
            width: 4,
            height: 4,
            depth_or_layers: 4,
        },
        dimension: pathon_rhi_types::TextureDimension::D3,
        ..TextureDesc::new_2d(
            "volume",
            4,
            4,
            Format::Rgba8Unorm,
            TextureUsage::COPY_DST | TextureUsage::COPY_SRC,
        )
    };
    let volume = device.create_texture(&desc).expect("volume");
    let slice = pathon_rhi_types::TextureCopy {
        origin: pathon_rhi_types::Origin3d { x: 0, y: 0, z: 2 },
        ..pathon_rhi_types::TextureCopy::whole(volume)
    };
    let layout = pathon_rhi_types::TexelLayout {
        offset: 0,
        bytes_per_row: 16,
        rows_per_image: 4,
    };
    device
        .write_texture(&slice, &[0; 64], layout, pathon_rhi_types::Extent3d::new_2d(4, 4))
        .expect("write");

    let mut list = CommandList::new();
    list.texture_access(volume, SubresourceRange::ALL, Access::COPY_DST, Access::COPY_SRC);
    let copy = TextureDesc::new_2d("copy", 4, 4, Format::Rgba8Unorm, TextureUsage::COPY_DST);
    let destination = device.create_texture(&copy).expect("copy target");
    list.texture_access(destination, SubresourceRange::ALL, Access::empty(), Access::COPY_DST);
    list.copy_texture_to_texture(
        pathon_rhi_types::TextureCopy::whole(volume),
        pathon_rhi_types::TextureCopy::whole(destination),
        pathon_rhi_types::Extent3d::new_2d(4, 4),
    );
    device
        .submit(list)
        .expect("the volume was written, so reading it is fine");
}

#[test]
fn a_barrier_outside_the_texture_mips_is_r0112() {
    let device = NullDevice::new();
    let texture = render_target(&device);
    let mut list = CommandList::new();
    let mip_three = SubresourceRange {
        base_mip: 3,
        mip_count: 1,
        base_layer: 0,
        layer_count: 1,
    };
    list.texture_access(texture, mip_three, Access::empty(), Access::COPY_DST);

    assert_eq!(code(device.submit(list)), "R0112");
}

fn array_layout_entry(count: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding: 0,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::texture_2d(),
        count: NonZeroU32::new(count),
    }
}

fn array_layout(device: &NullDevice, count: u32) -> pathon_rhi_types::BindGroupLayoutId {
    device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "array",
            entries: vec![array_layout_entry(count)],
        })
        .expect("array layout")
}

fn sampled_views(device: &NullDevice, count: usize) -> Vec<pathon_rhi_types::TextureViewId> {
    let texture = render_target(device);
    (0..count)
        .map(|_| {
            device
                .create_texture_view(texture, &TextureViewDesc::default())
                .expect("view")
        })
        .collect()
}

fn array_group(
    device: &NullDevice,
    layout: pathon_rhi_types::BindGroupLayoutId,
    resource: BindingResource,
) -> Result<pathon_rhi_types::BindGroupId, RhiError> {
    device.create_bind_group(&BindGroupDesc {
        label: "array",
        layout,
        entries: vec![BindGroupEntry { binding: 0, resource }],
    })
}

#[test]
fn a_binding_array_group_with_a_uniform_buffer_is_rejected() {
    let device = NullDevice::new();
    let result = device.create_bind_group_layout(&BindGroupLayoutDesc {
        label: "mixed",
        entries: vec![
            array_layout_entry(4),
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::UniformBuffer,
                count: None,
            },
        ],
    });

    assert_eq!(result.expect_err("uniform next to an array").code(), "R0107");
}

#[test]
fn a_binding_array_layout_needs_the_texture_binding_array_feature() {
    let device = NullDevice::with_features(Features::empty());
    let result = device.create_bind_group_layout(&BindGroupLayoutDesc {
        label: "array",
        entries: vec![array_layout_entry(4)],
    });

    assert_eq!(result.expect_err("feature is off").code(), "R0107");
}

#[test]
fn a_binding_array_layout_over_the_limit_is_rejected() {
    let device = NullDevice::new();
    let over = device.caps().limits.max_binding_array_elements_per_shader_stage + 1;
    let result = device.create_bind_group_layout(&BindGroupLayoutDesc {
        label: "array",
        entries: vec![array_layout_entry(over)],
    });

    assert_eq!(result.expect_err("over the limit").code(), "R0107");
}

#[test]
fn a_short_binding_array_needs_the_partially_bound_feature() {
    let strict = NullDevice::with_features(Features::TEXTURE_BINDING_ARRAY);
    let layout = array_layout(&strict, 4);
    let views = sampled_views(&strict, 3);
    let result = array_group(&strict, layout, BindingResource::TextureViewArray(views));
    assert_eq!(result.expect_err("3 of 4 elements").code(), "R0107");

    let partial = NullDevice::with_features(Features::TEXTURE_BINDING_ARRAY | Features::PARTIALLY_BOUND_BINDING_ARRAY);
    let layout = array_layout(&partial, 4);
    let views = sampled_views(&partial, 3);
    array_group(&partial, layout, BindingResource::TextureViewArray(views)).expect("partially bound group");
}

#[test]
fn a_full_binding_array_is_accepted() {
    let device = NullDevice::new();
    let layout = array_layout(&device, 4);
    let views = sampled_views(&device, 4);

    array_group(&device, layout, BindingResource::TextureViewArray(views)).expect("full array");
}

#[test]
fn an_array_resource_for_an_entry_without_count_is_rejected() {
    let device = NullDevice::new();
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "single",
            entries: vec![BindGroupLayoutEntry {
                count: None,
                ..array_layout_entry(1)
            }],
        })
        .expect("layout");
    let views = sampled_views(&device, 1);
    let result = array_group(&device, layout, BindingResource::TextureViewArray(views));

    assert_eq!(result.expect_err("array for a single binding").code(), "R0107");
}

#[test]
fn a_single_resource_for_an_array_entry_is_rejected() {
    let device = NullDevice::new();
    let layout = array_layout(&device, 1);
    let view = sampled_views(&device, 1)[0];
    let result = array_group(&device, layout, BindingResource::TextureView(view));

    assert_eq!(result.expect_err("single for an array binding").code(), "R0107");
}

#[test]
fn binding_array_elements_are_checked_like_single_bindings() {
    let device = NullDevice::new();
    let layout = array_layout(&device, 2);
    let target = render_target(&device);
    let sampled_texture = device
        .create_texture(&TextureDesc::new_2d(
            "no-sampled",
            4,
            4,
            Format::Rgba8Unorm,
            TextureUsage::RENDER_TARGET,
        ))
        .expect("texture");
    let views = [target, sampled_texture]
        .map(|texture| {
            device
                .create_texture_view(texture, &TextureViewDesc::default())
                .expect("view")
        })
        .to_vec();
    let result = array_group(&device, layout, BindingResource::TextureViewArray(views));

    assert_eq!(result.expect_err("second element lacks SAMPLED").code(), "R0105");
}

fn sampler_array_layout_entry(count: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        ty: BindingType::Sampler(pathon_rhi_types::SamplerBindingType::Filtering),
        ..array_layout_entry(count)
    }
}

fn layout_error(device: &NullDevice, entries: Vec<BindGroupLayoutEntry>) -> RhiError {
    device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "rejected",
            entries,
        })
        .expect_err("layout should be rejected")
}

fn samplers(device: &NullDevice, count: usize) -> Vec<pathon_rhi_types::SamplerId> {
    (0..count)
        .map(|_| {
            device
                .create_sampler(&pathon_rhi_types::SamplerDesc::NEAREST_CLAMP)
                .expect("sampler")
        })
        .collect()
}

const PARTIAL: Features = Features::TEXTURE_BINDING_ARRAY.union(Features::PARTIALLY_BOUND_BINDING_ARRAY);

#[test]
fn a_binding_array_layout_without_the_feature_names_the_feature() {
    let device = NullDevice::with_features(Features::empty());
    let error = layout_error(&device, vec![array_layout_entry(4)]);

    assert!(error.to_string().contains("TEXTURE_BINDING_ARRAY"), "{error}");
}

#[test]
fn a_storage_texture_array_is_rejected() {
    let device = NullDevice::new();
    let entry = BindGroupLayoutEntry {
        ty: BindingType::StorageTexture {
            access: StorageAccess::ReadOnly,
            format: Format::Rgba8Unorm,
            dimension: TextureViewDimension::D2,
        },
        ..array_layout_entry(4)
    };

    assert_eq!(layout_error(&device, vec![entry]).code(), "R0107");
}

#[test]
fn a_buffer_array_is_rejected() {
    let device = NullDevice::new();
    let entry = BindGroupLayoutEntry {
        ty: BindingType::StorageBuffer {
            access: StorageAccess::ReadOnly,
        },
        ..array_layout_entry(4)
    };

    assert_eq!(layout_error(&device, vec![entry]).code(), "R0107");
}

#[test]
fn an_empty_binding_array_is_rejected_even_when_partially_bound() {
    let device = NullDevice::with_features(PARTIAL);
    let layout = array_layout(&device, 4);
    let result = array_group(&device, layout, BindingResource::TextureViewArray(Vec::new()));

    assert_eq!(result.expect_err("zero length").code(), "R0107");
}

#[test]
fn an_empty_array_of_the_wrong_kind_is_rejected() {
    let device = NullDevice::with_features(PARTIAL);
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "samplers",
            entries: vec![sampler_array_layout_entry(4)],
        })
        .expect("layout");
    let result = array_group(&device, layout, BindingResource::TextureViewArray(Vec::new()));

    assert_eq!(result.expect_err("views for samplers").code(), "R0107");
}

#[test]
fn a_binding_array_longer_than_count_is_rejected_even_when_partially_bound() {
    let device = NullDevice::with_features(PARTIAL);
    let layout = array_layout(&device, 2);
    let views = sampled_views(&device, 3);
    let result = array_group(&device, layout, BindingResource::TextureViewArray(views));

    assert_eq!(result.expect_err("3 of 2 elements").code(), "R0107");
}

#[test]
fn texture_arrays_are_limited_per_stage_across_entries() {
    let device = NullDevice::new();
    let half = device.caps().limits.max_binding_array_elements_per_shader_stage / 2 + 1;
    let second = BindGroupLayoutEntry {
        binding: 1,
        ..array_layout_entry(half)
    };

    assert_eq!(
        layout_error(&device, vec![array_layout_entry(half), second]).code(),
        "R0107"
    );
}

#[test]
fn arrays_in_different_stages_do_not_share_a_limit() {
    let device = NullDevice::new();
    let half = device.caps().limits.max_binding_array_elements_per_shader_stage / 2 + 1;
    let vertex_only = BindGroupLayoutEntry {
        binding: 1,
        visibility: ShaderStages::VERTEX,
        ..array_layout_entry(half)
    };

    device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "stages",
            entries: vec![array_layout_entry(half), vertex_only],
        })
        .expect("different stages");
}

#[test]
fn sampler_arrays_are_limited_per_stage_across_entries() {
    let device = NullDevice::new();
    let half = device.caps().limits.max_binding_array_sampler_elements_per_shader_stage / 2 + 1;
    let second = BindGroupLayoutEntry {
        binding: 1,
        ..sampler_array_layout_entry(half)
    };

    assert_eq!(
        layout_error(&device, vec![sampler_array_layout_entry(half), second]).code(),
        "R0107"
    );
}

#[test]
fn sampler_arrays_count_against_the_general_limit_too() {
    let device = NullDevice::new();
    let limits = &device.caps().limits;
    let sampler_count = limits.max_binding_array_sampler_elements_per_shader_stage;
    let textures = limits.max_binding_array_elements_per_shader_stage - sampler_count + 1;
    let sampler_entry = BindGroupLayoutEntry {
        binding: 1,
        ..sampler_array_layout_entry(sampler_count)
    };

    assert_eq!(
        layout_error(&device, vec![array_layout_entry(textures), sampler_entry]).code(),
        "R0107"
    );
}

#[test]
fn a_sampler_array_over_the_sampler_limit_is_rejected() {
    let device = NullDevice::new();
    let over = device.caps().limits.max_binding_array_sampler_elements_per_shader_stage + 1;

    assert_eq!(
        layout_error(&device, vec![sampler_array_layout_entry(over)]).code(),
        "R0107"
    );
}

#[test]
fn a_sampler_array_group_matches_only_sampler_arrays() {
    let device = NullDevice::new();
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "samplers",
            entries: vec![sampler_array_layout_entry(2)],
        })
        .expect("layout");
    let two_samplers = samplers(&device, 2);
    array_group(&device, layout, BindingResource::SamplerArray(two_samplers)).expect("sampler array");

    let views = sampled_views(&device, 2);
    let result = array_group(&device, layout, BindingResource::TextureViewArray(views));
    assert_eq!(result.expect_err("views for samplers").code(), "R0107");

    let texture_layout = array_layout(&device, 2);
    let two_samplers = samplers(&device, 2);
    let result = array_group(&device, texture_layout, BindingResource::SamplerArray(two_samplers));
    assert_eq!(result.expect_err("samplers for textures").code(), "R0107");
}

#[test]
fn an_array_element_in_the_wrong_state_at_dispatch_is_rejected() {
    let device = NullDevice::new();
    let mut list = CommandList::new();
    let ready = render_target(&device);
    let stale = render_target(&device);
    for texture in [ready, stale] {
        list.texture_access(
            texture,
            SubresourceRange::ALL,
            Access::empty(),
            Access::COLOR_ATTACHMENT,
        );
        clear_pass(&device, texture, &mut list);
    }
    list.texture_access(ready, SubresourceRange::ALL, Access::COLOR_ATTACHMENT, Access::SAMPLED);
    let views: Vec<_> = [ready, stale]
        .map(|texture| {
            device
                .create_texture_view(texture, &TextureViewDesc::default())
                .expect("view")
        })
        .to_vec();
    let layout = array_layout(&device, 2);
    let group = array_group(&device, layout, BindingResource::TextureViewArray(views)).expect("group");
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "sample",
            wgsl: "fn main() {}",
        })
        .expect("module");
    let pipeline = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "sample",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![],
            },
            immediate_size: 0,
        })
        .expect("pipeline");
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "sample",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group);
        pass.dispatch(1, 1, 1);
    }

    assert_eq!(code(device.submit(list)), "R0101");
}

fn immediate_pipeline(device: &NullDevice, immediate_size: u32) -> Result<ComputePipelineId, RhiError> {
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "immediates",
            wgsl: "fn main() {}",
        })
        .expect("module");
    device.create_compute_pipeline(&ComputePipelineDesc {
        label: "immediates",
        bind_group_layouts: vec![],
        shader: ShaderStage {
            module,
            entry_point: "main",
            constants: vec![],
        },
        immediate_size,
    })
}

fn submit_immediates(
    device: &NullDevice,
    pipeline: Option<ComputePipelineId>,
    offset: u32,
    len: usize,
) -> Result<(), RhiError> {
    let mut list = CommandList::new();
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "immediates",
            timestamps: None,
        });
        if let Some(pipeline) = pipeline {
            pass.set_pipeline(pipeline);
        }
        pass.set_immediates(offset, &vec![0; len]);
    }
    device.submit(list)
}

#[test]
fn a_pipeline_with_immediates_needs_the_immediates_feature() {
    let device = NullDevice::with_features(Features::empty());
    let error = immediate_pipeline(&device, 16).expect_err("feature is off");
    assert_eq!(error.code(), "R0002");
}

#[test]
fn a_pipeline_with_immediates_over_the_limit_is_rejected() {
    let device = NullDevice::with_features(Features::IMMEDIATES);
    let over = device.caps().limits.max_immediate_size + 4;
    let error = immediate_pipeline(&device, over).expect_err("over the limit");
    assert_eq!(error.code(), "R0107");
}

#[test]
fn immediates_inside_the_pipeline_range_are_accepted() {
    let device = NullDevice::with_features(Features::IMMEDIATES);
    let pipeline = immediate_pipeline(&device, 16).expect("pipeline");
    submit_immediates(&device, Some(pipeline), 4, 8).expect("immediates");
}

#[test]
fn immediates_past_the_pipeline_range_are_rejected() {
    let device = NullDevice::with_features(Features::IMMEDIATES);
    let pipeline = immediate_pipeline(&device, 16).expect("pipeline");
    assert_eq!(code(submit_immediates(&device, Some(pipeline), 12, 8)), "R0104");
}

#[test]
fn immediates_without_a_pipeline_are_rejected() {
    let device = NullDevice::with_features(Features::IMMEDIATES);
    assert_eq!(code(submit_immediates(&device, None, 0, 4)), "R0104");
}

#[test]
fn unaligned_immediates_are_rejected() {
    let device = NullDevice::with_features(Features::IMMEDIATES);
    let pipeline = immediate_pipeline(&device, 16).expect("pipeline");
    assert_eq!(code(submit_immediates(&device, Some(pipeline), 2, 4)), "R0104");
    assert_eq!(code(submit_immediates(&device, Some(pipeline), 0, 6)), "R0104");
}

fn immediate_dispatch(device: &NullDevice, steps: &[ImmediateStep]) -> Result<(), RhiError> {
    let mut list = CommandList::new();
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "immediates",
            timestamps: None,
        });
        for step in steps {
            match step {
                ImmediateStep::Pipeline(pipeline) => pass.set_pipeline(*pipeline),
                ImmediateStep::Write(offset, len) => pass.set_immediates(*offset, &vec![0; *len]),
                ImmediateStep::Dispatch => pass.dispatch(1, 1, 1),
            }
        }
    }
    device.submit(list)
}

use ImmediateStep::{Dispatch, Pipeline, Write};

enum ImmediateStep {
    Pipeline(ComputePipelineId),
    Write(u32, usize),
    Dispatch,
}

#[test]
fn a_pipeline_immediate_size_must_be_a_multiple_of_four() {
    let device = NullDevice::with_features(Features::IMMEDIATES);
    let error = immediate_pipeline(&device, 6).expect_err("misaligned size");
    assert_eq!(error.code(), "R0107");
}

#[test]
fn a_dispatch_needs_every_immediate_byte_written() {
    let device = NullDevice::with_features(Features::IMMEDIATES);
    let pipeline = immediate_pipeline(&device, 8).expect("pipeline");

    assert_eq!(
        code(immediate_dispatch(&device, &[Pipeline(pipeline), Dispatch])),
        "R0104"
    );
    assert_eq!(
        code(immediate_dispatch(
            &device,
            &[Pipeline(pipeline), Write(0, 4), Dispatch]
        )),
        "R0104"
    );
    immediate_dispatch(
        &device,
        &[Pipeline(pipeline), Write(4, 4), Write(0, 4), Dispatch, Dispatch],
    )
    .expect("two halves cover the block");
}

#[test]
fn switching_pipeline_forgets_written_immediates() {
    let device = NullDevice::with_features(Features::IMMEDIATES);
    let first = immediate_pipeline(&device, 4).expect("first");
    let second = immediate_pipeline(&device, 4).expect("second");

    let steps = [Pipeline(first), Write(0, 4), Pipeline(second), Dispatch];
    assert_eq!(code(immediate_dispatch(&device, &steps)), "R0104");
}

#[test]
fn a_pipeline_without_immediates_dispatches_without_writes() {
    let device = NullDevice::with_features(Features::IMMEDIATES);
    let pipeline = immediate_pipeline(&device, 0).expect("pipeline");
    immediate_dispatch(&device, &[ImmediateStep::Pipeline(pipeline), ImmediateStep::Dispatch]).expect("dispatch");
}

#[test]
fn immediates_outside_a_pass_are_rejected_and_empty_writes_accepted() {
    let device = NullDevice::with_features(Features::IMMEDIATES);
    let pipeline = immediate_pipeline(&device, 4).expect("pipeline");
    submit_immediates(&device, Some(pipeline), 0, 0).expect("zero-length write");

    let mut list = CommandList::new();
    list.push(pathon_rhi_types::Command::SetImmediates {
        offset: 0,
        data: Box::new([0; 4]),
    });
    assert_eq!(code(device.submit(list)), "R0104");
}

#[test]
fn render_passes_validate_immediates_too() {
    let device = NullDevice::with_features(Features::IMMEDIATES);
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "render",
            wgsl: "fn main() {}",
        })
        .expect("module");
    let stage = |entry_point| ShaderStage {
        module,
        entry_point,
        constants: vec![],
    };
    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "render",
            bind_group_layouts: vec![],
            vertex: stage("vs_main"),
            vertex_buffers: vec![],
            fragment: Some(stage("fs_main")),
            color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
            immediate_size: 8,
        })
        .expect("pipeline");
    let texture = render_target(&device);
    let view = device
        .create_texture_view(texture, &TextureViewDesc::default())
        .expect("view");
    let record = |from: Access, writes: &[(u32, usize)]| {
        let mut list = CommandList::new();
        list.texture_access(texture, SubresourceRange::ALL, from, Access::COLOR_ATTACHMENT);
        {
            let mut pass = list.render_pass(RenderPassDesc {
                label: "render",
                color: vec![ColorAttachment::clear(view, Color::BLACK)],
                depth: None,
                timestamps: None,
            });
            pass.set_pipeline(pipeline);
            for (offset, len) in writes {
                pass.set_immediates(*offset, &vec![0; *len]);
            }
            pass.draw(0..3, 0..1);
        }
        list
    };

    device.submit(record(Access::empty(), &[(0, 8)])).expect("full block");
    assert_eq!(
        code(device.submit(record(Access::COLOR_ATTACHMENT, &[(0, 4)]))),
        "R0104"
    );
    assert_eq!(
        code(device.submit(record(Access::COLOR_ATTACHMENT, &[(4, 8)]))),
        "R0104"
    );
}

#[test]
fn a_duplicate_override_name_is_rejected() {
    let device = NullDevice::new();
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "overrides",
            wgsl: "fn main() {}",
        })
        .expect("module");
    let error = device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "overrides",
            bind_group_layouts: vec![],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![("scale", 1.0.into()), ("scale", 2.0.into())],
            },
            immediate_size: 0,
        })
        .expect_err("duplicate");
    assert_eq!(error.code(), "R0107");
}

#[test]
fn immediate_limit_is_zero_without_the_feature() {
    assert_eq!(
        NullDevice::with_features(Features::empty())
            .caps()
            .limits
            .max_immediate_size,
        0
    );
    assert!(
        NullDevice::with_features(Features::IMMEDIATES)
            .caps()
            .limits
            .max_immediate_size
            > 0
    );
}

fn upload_target(device: &NullDevice, width: u32, height: u32, layers: u32) -> TextureId {
    let desc = TextureDesc {
        size: pathon_rhi_types::Extent3d {
            width,
            height,
            depth_or_layers: layers,
        },
        ..TextureDesc::new_2d(
            "upload",
            width,
            height,
            Format::Rgba8Unorm,
            TextureUsage::COPY_DST | TextureUsage::COPY_SRC | TextureUsage::SAMPLED,
        )
    };
    device.create_texture(&desc).expect("texture")
}

fn texel_layout(bytes_per_row: u32, rows_per_image: u32) -> pathon_rhi_types::TexelLayout {
    pathon_rhi_types::TexelLayout {
        offset: 0,
        bytes_per_row,
        rows_per_image,
    }
}

#[test]
fn a_barrier_starting_from_the_wrong_state_is_rejected() {
    let device = NullDevice::new();
    let texture = render_target(&device);
    let mut list = CommandList::new();
    list.texture_access(
        texture,
        SubresourceRange::ALL,
        Access::SAMPLED,
        Access::COLOR_ATTACHMENT,
    );

    assert_eq!(code(device.submit(list)), "R0103");
}

#[test]
fn a_barrier_after_a_host_write_must_start_from_copy_dst() {
    let device = NullDevice::new();
    let texture = upload_target(&device, 4, 4, 1);
    device
        .write_texture(
            &pathon_rhi_types::TextureCopy::whole(texture),
            &[0; 64],
            texel_layout(16, 4),
            pathon_rhi_types::Extent3d::new_2d(4, 4),
        )
        .expect("write");

    let mut discarding = CommandList::new();
    discarding.texture_access(texture, SubresourceRange::ALL, Access::empty(), Access::SAMPLED);
    assert_eq!(code(device.submit(discarding)), "R0103");

    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);
    device.submit(list).expect("the upload keeps its data and its state");
}

#[test]
fn one_from_for_a_range_with_mixed_states_is_rejected() {
    let device = NullDevice::new();
    let desc = TextureDesc {
        mip_level_count: 2,
        ..TextureDesc::new_2d("mips", 4, 4, Format::Rgba8Unorm, TextureUsage::COPY_DST)
    };
    let texture = device.create_texture(&desc).expect("texture");
    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::mip(1), Access::empty(), Access::COPY_DST);
    list.texture_access(texture, SubresourceRange::ALL, Access::empty(), Access::COPY_DST);

    assert_eq!(code(device.submit(list)), "R0103");
}

fn buffer_copy(device: &NullDevice, bytes_per_row: u32) -> pathon_rhi_types::BufferCopy {
    let buffer = device
        .create_buffer(&pathon_rhi_types::BufferDesc {
            label: "staging",
            size: 4096,
            usage: pathon_rhi_types::BufferUsage::COPY_SRC | pathon_rhi_types::BufferUsage::COPY_DST,
        })
        .expect("buffer");
    pathon_rhi_types::BufferCopy {
        buffer,
        layout: texel_layout(bytes_per_row, 4),
    }
}

#[test]
fn buffer_texture_copies_need_a_256_aligned_bytes_per_row() {
    let device = NullDevice::new();
    let texture = upload_target(&device, 25, 4, 1);
    let size = pathon_rhi_types::Extent3d::new_2d(25, 4);
    let whole = pathon_rhi_types::TextureCopy::whole(texture);

    let mut upload = CommandList::new();
    upload.texture_access(texture, SubresourceRange::ALL, Access::empty(), Access::COPY_DST);
    upload.copy_buffer_to_texture(buffer_copy(&device, 100), whole, size);
    assert_eq!(code(device.submit(upload)), "R0113");

    let mut download = CommandList::new();
    download.copy_texture_to_buffer(whole, buffer_copy(&device, 100), size);
    assert_eq!(code(device.submit(download)), "R0113");
}

#[test]
fn write_texture_only_needs_bytes_per_row_that_fits_a_row() {
    let device = NullDevice::new();
    let texture = upload_target(&device, 25, 4, 1);
    let copy = pathon_rhi_types::TextureCopy::whole(texture);
    let size = pathon_rhi_types::Extent3d::new_2d(25, 4);

    device
        .write_texture(&copy, &[0; 400], texel_layout(100, 4), size)
        .expect("a row of 25 texels fits 100 bytes");
    let too_short = device.write_texture(&copy, &[0; 400], texel_layout(96, 4), size);
    assert_eq!(too_short.expect_err("row is cut off").code(), "R0113");
}

#[test]
fn rows_per_image_must_cover_the_height_of_every_copy() {
    let device = NullDevice::new();
    let texture = upload_target(&device, 4, 4, 2);
    let copy = pathon_rhi_types::TextureCopy::whole(texture);
    let layers = pathon_rhi_types::Extent3d {
        width: 4,
        height: 4,
        depth_or_layers: 2,
    };

    let overlapping = device.write_texture(&copy, &[0; 128], texel_layout(16, 3), layers);
    assert_eq!(overlapping.expect_err("images overlap").code(), "R0113");
    device
        .write_texture(&copy, &[0; 128], texel_layout(16, 4), layers)
        .expect("images do not overlap");
    let one_image = pathon_rhi_types::Extent3d::new_2d(4, 4);
    let too_few_rows = device.write_texture(&copy, &[0; 64], texel_layout(16, 0), one_image);
    assert_eq!(
        too_few_rows.expect_err("rows_per_image below the height").code(),
        "R0113"
    );
}

#[test]
fn creating_a_pipeline_from_another_thread_during_submit_succeeds() {
    let device = NullDevice::new();
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "shader",
            wgsl: "fn main() {}",
        })
        .expect("module");
    let create = || {
        device.create_compute_pipeline(&ComputePipelineDesc {
            label: "background",
            bind_group_layouts: vec![],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![],
            },
            immediate_size: 0,
        })
    };

    std::thread::scope(|scope| {
        let background = scope.spawn(|| {
            for _ in 0..50 {
                create().expect("pipeline created while another thread submits");
            }
        });
        for _ in 0..50 {
            device.submit(CommandList::new()).expect("submit");
        }
        background.join().expect("the background thread finishes");
    });
}

#[test]
fn a_rejected_list_changes_no_texture_state() {
    let device = NullDevice::new();
    let texture = render_target(&device);
    let mut rejected = CommandList::new();
    rejected.texture_access(
        texture,
        SubresourceRange::ALL,
        Access::empty(),
        Access::COLOR_ATTACHMENT,
    );
    rejected.texture_access(texture, SubresourceRange::ALL, Access::SAMPLED, Access::COPY_SRC);
    assert_eq!(code(device.submit(rejected)), "R0103");

    let mut retry = CommandList::new();
    retry.texture_access(
        texture,
        SubresourceRange::ALL,
        Access::empty(),
        Access::COLOR_ATTACHMENT,
    );
    device
        .submit(retry)
        .expect("the first barrier of the rejected list was not applied");
}

#[test]
fn a_rejected_list_leaves_buffer_bytes_untouched() {
    use pathon_rhi_types::{BufferDesc, BufferUsage};
    let device = NullDevice::new();
    let source = device
        .create_buffer(&BufferDesc {
            label: "source",
            size: 16,
            usage: BufferUsage::COPY_SRC | BufferUsage::COPY_DST,
        })
        .expect("source");
    let target = device
        .create_buffer(&BufferDesc {
            label: "target",
            size: 16,
            usage: BufferUsage::COPY_DST | BufferUsage::MAP_READ,
        })
        .expect("target");
    device.write_buffer(source, 0, &[7; 16]).expect("write");

    let mut rejected = CommandList::new();
    rejected.copy_buffer_to_buffer(source, 0, target, 0, 16);
    rejected.texture_access(
        render_target(&device),
        SubresourceRange::ALL,
        Access::SAMPLED,
        Access::COPY_SRC,
    );
    assert_eq!(code(device.submit(rejected)), "R0103");

    assert_eq!(device.read_buffer(target).expect("read"), [0; 16]);
}

fn timestamp_pass(query_set: QuerySetId, begin_index: u32, end_index: u32) -> ComputePassDesc {
    ComputePassDesc {
        label: "timed",
        timestamps: Some(PassTimestamps {
            query_set,
            begin_index,
            end_index,
        }),
    }
}

fn run_compute_pass(device: &NullDevice, desc: ComputePassDesc) -> Result<(), RhiError> {
    let mut list = CommandList::new();
    drop(list.compute_pass(desc));
    device.submit(list)
}

fn query_set_of(device: &NullDevice, count: u32) -> QuerySetId {
    device
        .create_query_set(&QuerySetDesc {
            label: "timestamps",
            count,
        })
        .expect("query set")
}

#[test]
fn pass_timestamps_inside_the_query_set_are_accepted() {
    let device = NullDevice::new();
    let query_set = query_set_of(&device, 4);

    run_compute_pass(&device, timestamp_pass(query_set, 2, 3)).expect("valid timestamps");
}

#[test]
fn pass_timestamps_outside_the_query_set_are_rejected() {
    let device = NullDevice::new();
    let query_set = query_set_of(&device, 2);

    assert_eq!(
        code(run_compute_pass(&device, timestamp_pass(query_set, 0, 2))),
        "R0104"
    );
}

#[test]
fn pass_timestamps_with_equal_begin_and_end_are_rejected() {
    let device = NullDevice::new();
    let query_set = query_set_of(&device, 2);

    assert_eq!(
        code(run_compute_pass(&device, timestamp_pass(query_set, 1, 1))),
        "R0104"
    );
}

#[test]
fn pass_timestamps_of_a_destroyed_query_set_are_rejected() {
    let device = NullDevice::new();
    let query_set = query_set_of(&device, 2);
    device.destroy(Resource::QuerySet(query_set));

    assert_eq!(
        code(run_compute_pass(&device, timestamp_pass(query_set, 0, 1))),
        "R0003"
    );
}

#[test]
fn query_sets_need_the_timestamp_feature() {
    let device = NullDevice::with_features(Features::empty());

    let error = device
        .create_query_set(&QuerySetDesc {
            label: "timestamps",
            count: 2,
        })
        .expect_err("no timestamp feature");

    assert!(matches!(error, RhiError::MissingFeatures(Features::TIMESTAMP_QUERY)));
}

fn pipeline_with_polygon_mode(device: &NullDevice, polygon_mode: PolygonMode) -> Result<(), RhiError> {
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "render",
            wgsl: "fn main() {}",
        })
        .expect("module");
    let stage = |entry_point| ShaderStage {
        module,
        entry_point,
        constants: vec![],
    };
    device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "wireframe",
            bind_group_layouts: vec![],
            vertex: stage("vs_main"),
            vertex_buffers: vec![],
            fragment: Some(stage("fs_main")),
            color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
            primitive: PrimitiveState {
                polygon_mode,
                ..PrimitiveState::default()
            },
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        })
        .map(drop)
}

#[test]
fn line_polygon_mode_needs_its_feature() {
    let without = NullDevice::with_features(Features::empty());
    let error = pipeline_with_polygon_mode(&without, PolygonMode::Line).expect_err("no line feature");
    assert!(matches!(error, RhiError::MissingFeatures(Features::POLYGON_MODE_LINE)));
    pipeline_with_polygon_mode(&without, PolygonMode::Fill).expect("fill needs no feature");

    let with = NullDevice::with_features(Features::POLYGON_MODE_LINE);
    pipeline_with_polygon_mode(&with, PolygonMode::Line).expect("line with the feature");
}

fn depth_pipeline(device: &NullDevice, depth: pathon_rhi_types::DepthState) -> Result<(), RhiError> {
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "depth",
            wgsl: "@vertex fn vs() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }",
        })
        .expect("module");
    device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "depth",
            bind_group_layouts: vec![],
            vertex: ShaderStage {
                module,
                entry_point: "vs",
                constants: vec![],
            },
            vertex_buffers: vec![],
            fragment: None,
            color_targets: vec![],
            primitive: PrimitiveState::default(),
            depth: Some(depth),
            sample_count: 1,
            immediate_size: 0,
        })
        .map(|_| ())
}

#[test]
fn a_depth_only_pipeline_with_a_finite_bias_is_accepted() {
    let device = NullDevice::new();
    let state = pathon_rhi_types::DepthState::reverse_z(Format::Depth32Float)
        .with_bias(pathon_rhi_types::DepthBias::new(-2, -2.0, 0.0));
    depth_pipeline(&device, state).expect("valid depth state");
}

#[test]
fn a_non_finite_depth_bias_is_r0109() {
    let device = NullDevice::new();
    let state = pathon_rhi_types::DepthState::reverse_z(Format::Depth32Float)
        .with_bias(pathon_rhi_types::DepthBias::new(0, f32::INFINITY, 0.0));
    assert_eq!(code(depth_pipeline(&device, state)), "R0109");
}

#[test]
fn a_colour_format_as_depth_state_is_r0109() {
    let device = NullDevice::new();
    let state = pathon_rhi_types::DepthState::reverse_z(Format::Rgba8Unorm);
    assert_eq!(code(depth_pipeline(&device, state)), "R0109");
}
