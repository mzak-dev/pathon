use pathon_rhi_null::NullDevice;
use pathon_rhi_types::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource, BindingType,
    Color, ColorAttachment, CommandList, ComputePassDesc, ComputePipelineDesc, DepthAttachment, DeviceApi, Format,
    RenderPassDesc, RhiError, ShaderModuleDesc, ShaderStage, ShaderStages, StorageAccess, SubresourceRange,
    SurfaceConfig, TextureDesc, TextureId, TextureUsage, TextureViewDesc, TextureViewDimension,
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
    list.texture_access(texture, SubresourceRange::ALL, Access::COLOR_ATTACHMENT);
    clear_pass(&device, texture, &mut list);
    list.texture_access(texture, SubresourceRange::ALL, Access::SAMPLED);

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
    list.texture_access(texture, SubresourceRange::ALL, Access::COLOR_ATTACHMENT);
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
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageTexture {
                        access: StorageAccess::WriteOnly,
                        format: Format::R32Float,
                        dimension: TextureViewDimension::D2,
                    },
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
            },
        })
        .expect("pipeline");

    let mut list = CommandList::new();
    list.texture_access(hi_z, SubresourceRange::mip(1), Access::SAMPLED);
    list.texture_access(hi_z, SubresourceRange::mip(2), Access::STORAGE_WRITE);
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
    list.texture_access(texture, SubresourceRange::ALL, Access::SAMPLED | Access::COPY_SRC);

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
            },
        })
        .expect("pipeline");

    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::STORAGE_READ);
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
            },
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
    list.texture_access(depth, SubresourceRange::ALL, Access::DEPTH_WRITE);
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
    list.texture_access(depth, SubresourceRange::ALL, Access::SAMPLED | Access::DEPTH_READ);
    sampled_compute_pass(&device, depth, &mut list);

    device.submit(list).expect("valid list");
}

#[test]
fn a_depth_texture_sampled_without_depth_read_is_rejected() {
    let device = NullDevice::new();
    let mut list = CommandList::new();
    let depth = cleared_depth_texture(&device, &mut list);
    list.texture_access(depth, SubresourceRange::ALL, Access::SAMPLED);
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
    list.texture_access(frame_again.texture, SubresourceRange::ALL, Access::COLOR_ATTACHMENT);
    clear_pass_on_view(frame_again.view, &mut list);
    list.texture_access(frame_again.texture, SubresourceRange::ALL, Access::PRESENT);
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
    list.texture_access(volume, SubresourceRange::ALL, Access::COPY_SRC);
    let copy = TextureDesc::new_2d("copy", 4, 4, Format::Rgba8Unorm, TextureUsage::COPY_DST);
    let destination = device.create_texture(&copy).expect("copy target");
    list.texture_access(destination, SubresourceRange::ALL, Access::COPY_DST);
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
    list.texture_access(texture, mip_three, Access::COPY_DST);

    assert_eq!(code(device.submit(list)), "R0112");
}
