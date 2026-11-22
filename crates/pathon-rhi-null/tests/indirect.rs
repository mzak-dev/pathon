use pathon_rhi_null::NullDevice;
use pathon_rhi_types::{
    Access, BufferDesc, BufferId, BufferUsage, Color, ColorAttachment, ColorTargetState, Command, CommandList,
    ComputePassDesc, ComputePipelineDesc, DeviceApi, DispatchIndirectArgs, DrawIndexedIndirectArgs, DrawIndirectArgs,
    Features, Format, MultiDrawCount, PrimitiveState, RenderPassDesc, RenderPassEncoder, RenderPipelineDesc, RhiError,
    ShaderModuleDesc, ShaderStage, SubresourceRange, TextureDesc, TextureUsage, TextureViewDesc,
};

const ARGS_SIZE: u64 = 256;

fn code(result: Result<(), RhiError>) -> &'static str {
    result.expect_err("validation should fail").code()
}

fn buffer(device: &NullDevice, label: &'static str, size: u64, usage: BufferUsage) -> BufferId {
    device
        .create_buffer(&BufferDesc { label, size, usage })
        .expect("buffer")
}

fn arguments(device: &NullDevice) -> BufferId {
    buffer(device, "args", ARGS_SIZE, BufferUsage::INDIRECT | BufferUsage::STORAGE)
}

fn stage(device: &NullDevice, entry_point: &'static str) -> ShaderStage {
    let module = device
        .create_shader_module(&ShaderModuleDesc {
            label: "indirect",
            wgsl: "fn main() {}",
        })
        .expect("module");
    ShaderStage {
        module,
        entry_point,
        constants: vec![],
    }
}

// Records `draw` inside a render pass with a pipeline set, so only the indirect command itself can fail.
fn submit_draw(device: &NullDevice, draw: impl FnOnce(&mut RenderPassEncoder<'_>)) -> Result<(), RhiError> {
    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "indirect",
            bind_group_layouts: vec![],
            vertex: stage(device, "vs"),
            vertex_buffers: vec![],
            fragment: Some(stage(device, "fs")),
            color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        })
        .expect("pipeline");
    let target = device
        .create_texture(&TextureDesc::new_2d(
            "target",
            4,
            4,
            Format::Rgba8Unorm,
            TextureUsage::RENDER_TARGET,
        ))
        .expect("target");
    let view = device
        .create_texture_view(target, &TextureViewDesc::default())
        .expect("view");
    let mut list = CommandList::new();
    list.texture_access(target, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
    {
        let mut pass = list.render_pass(RenderPassDesc {
            label: "indirect",
            color: vec![ColorAttachment::clear(view, Color::BLACK)],
            depth: None,
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        draw(&mut pass);
    }
    device.submit(list)
}

fn compute_pipeline(device: &NullDevice) -> pathon_rhi_types::ComputePipelineId {
    device
        .create_compute_pipeline(&ComputePipelineDesc {
            label: "indirect",
            bind_group_layouts: vec![],
            shader: stage(device, "main"),
            immediate_size: 0,
        })
        .expect("pipeline")
}

fn submit_dispatch(device: &NullDevice, buffer: BufferId, offset: u64) -> Result<(), RhiError> {
    let pipeline = compute_pipeline(device);
    let mut list = CommandList::new();
    {
        let mut pass = list.compute_pass(ComputePassDesc {
            label: "indirect",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
        pass.dispatch_indirect(buffer, offset);
    }
    device.submit(list)
}

#[test]
fn every_indirect_draw_inside_the_buffer_is_accepted() {
    let device = NullDevice::with_features(Features::MULTI_DRAW_INDIRECT_COUNT);
    let args = arguments(&device);
    let count = buffer(&device, "count", 16, BufferUsage::INDIRECT);
    submit_draw(&device, |pass| {
        pass.draw_indirect(args, 0);
        pass.draw_indexed_indirect(args, 20);
        pass.multi_draw_indirect(args, 4, 15);
        pass.multi_draw_indexed_indirect(args, 0, 12);
        pass.multi_draw_indirect_count(MultiDrawCount {
            args,
            args_offset: 0,
            count_buffer: count,
            count_offset: 12,
            max_count: 16,
        });
        pass.multi_draw_indexed_indirect_count(MultiDrawCount {
            args,
            args_offset: 16,
            count_buffer: args,
            count_offset: 0,
            max_count: 12,
        });
    })
    .expect("valid indirect draws");
}

#[test]
fn an_indirect_dispatch_inside_the_buffer_is_accepted() {
    let device = NullDevice::new();
    let args = arguments(&device);
    submit_dispatch(&device, args, ARGS_SIZE - DispatchIndirectArgs::SIZE).expect("last dispatch element");
}

#[test]
fn arguments_in_a_buffer_without_indirect_usage_are_r0105() {
    let device = NullDevice::new();
    let plain = buffer(&device, "plain", ARGS_SIZE, BufferUsage::STORAGE);
    assert_eq!(code(submit_draw(&device, |pass| pass.draw_indirect(plain, 0))), "R0105");
    assert_eq!(
        code(submit_draw(&device, |pass| pass.draw_indexed_indirect(plain, 0))),
        "R0105"
    );
    assert_eq!(
        code(submit_draw(&device, |pass| pass.multi_draw_indirect(plain, 0, 2))),
        "R0105"
    );
    assert_eq!(
        code(submit_draw(&device, |pass| pass.multi_draw_indexed_indirect(plain, 0, 2))),
        "R0105"
    );
    assert_eq!(code(submit_dispatch(&device, plain, 0)), "R0105");
}

#[test]
fn a_count_buffer_needs_indirect_usage_too() {
    let device = NullDevice::with_features(Features::MULTI_DRAW_INDIRECT_COUNT);
    let args = arguments(&device);
    let plain = buffer(&device, "count", 16, BufferUsage::STORAGE);
    let result = submit_draw(&device, |pass| {
        pass.multi_draw_indirect_count(MultiDrawCount {
            args,
            args_offset: 0,
            count_buffer: plain,
            count_offset: 0,
            max_count: 4,
        });
    });
    assert_eq!(code(result), "R0105");
}

#[test]
fn an_offset_that_is_not_a_multiple_of_four_is_r0114() {
    let device = NullDevice::with_features(Features::MULTI_DRAW_INDIRECT_COUNT);
    let args = arguments(&device);
    assert_eq!(code(submit_draw(&device, |pass| pass.draw_indirect(args, 2))), "R0114");
    assert_eq!(
        code(submit_draw(&device, |pass| pass.multi_draw_indexed_indirect(args, 6, 2))),
        "R0114"
    );
    assert_eq!(code(submit_dispatch(&device, args, 1)), "R0114");
    let misaligned_count = submit_draw(&device, |pass| {
        pass.multi_draw_indirect_count(MultiDrawCount {
            args,
            args_offset: 0,
            count_buffer: args,
            count_offset: 2,
            max_count: 1,
        });
    });
    assert_eq!(code(misaligned_count), "R0114");
}

#[test]
fn arguments_reaching_past_the_buffer_are_r0108() {
    let device = NullDevice::new();
    let args = arguments(&device);
    let last_draw = ARGS_SIZE - DrawIndirectArgs::SIZE;
    submit_draw(&device, |pass| pass.draw_indirect(args, last_draw)).expect("the last element fits");
    assert_eq!(
        code(submit_draw(&device, |pass| pass.draw_indirect(args, last_draw + 4))),
        "R0108"
    );
    assert_eq!(
        code(submit_draw(&device, |pass| {
            pass.draw_indexed_indirect(args, ARGS_SIZE - DrawIndexedIndirectArgs::SIZE + 4);
        })),
        "R0108"
    );
    assert_eq!(
        code(submit_draw(&device, |pass| pass.multi_draw_indirect(args, 0, 17))),
        "R0108",
        "17 draws of 16 bytes do not fit 256 bytes"
    );
    assert_eq!(code(submit_dispatch(&device, args, ARGS_SIZE - 8)), "R0108");
    assert_eq!(
        code(submit_draw(&device, |pass| pass.multi_draw_indirect(args, 0, u32::MAX))),
        "R0108"
    );
    assert_eq!(
        code(submit_draw(&device, |pass| pass.draw_indirect(args, u64::MAX - 3))),
        "R0108",
        "offset arithmetic must not wrap"
    );
}

#[test]
fn the_count_variants_reserve_room_for_max_count_and_the_counter() {
    let device = NullDevice::with_features(Features::MULTI_DRAW_INDIRECT_COUNT);
    let args = arguments(&device);
    let count = buffer(&device, "count", 8, BufferUsage::INDIRECT);
    let draws = |max_count, count_offset| MultiDrawCount {
        args,
        args_offset: 0,
        count_buffer: count,
        count_offset,
        max_count,
    };
    submit_draw(&device, |pass| pass.multi_draw_indirect_count(draws(16, 4))).expect("counter in the last 4 bytes");
    assert_eq!(
        code(submit_draw(&device, |pass| pass.multi_draw_indirect_count(draws(17, 0)))),
        "R0108",
        "max_count draws do not fit the argument buffer"
    );
    assert_eq!(
        code(submit_draw(&device, |pass| pass.multi_draw_indexed_indirect_count(draws(2, 8)))),
        "R0108",
        "the counter lies outside the count buffer"
    );
}

#[test]
fn the_count_variants_need_the_feature() {
    let device = NullDevice::with_features(Features::empty());
    let args = arguments(&device);
    let draws = MultiDrawCount {
        args,
        args_offset: 0,
        count_buffer: args,
        count_offset: 0,
        max_count: 1,
    };
    let error = submit_draw(&device, |pass| pass.multi_draw_indirect_count(draws)).expect_err("no feature");
    assert!(matches!(
        error,
        RhiError::MissingFeatures(Features::MULTI_DRAW_INDIRECT_COUNT)
    ));
    let error = submit_draw(&device, |pass| pass.multi_draw_indexed_indirect_count(draws)).expect_err("no feature");
    assert_eq!(error.code(), "R0002");
    submit_draw(&device, |pass| pass.multi_draw_indirect(args, 0, 4)).expect("plain multi-draw needs no feature");
}

#[test]
fn indirect_commands_in_the_wrong_kind_of_pass_are_r0104() {
    let device = NullDevice::new();
    let args = arguments(&device);

    let mut outside = CommandList::new();
    outside.push(Command::DrawIndirect {
        buffer: args,
        offset: 0,
    });
    assert_eq!(code(device.submit(outside)), "R0104", "a draw outside any pass");

    let mut dispatch_outside = CommandList::new();
    dispatch_outside.push(Command::DispatchIndirect {
        buffer: args,
        offset: 0,
    });
    assert_eq!(
        code(device.submit(dispatch_outside)),
        "R0104",
        "a dispatch outside any pass"
    );

    let mut draw_in_compute = CommandList::new();
    let pipeline = compute_pipeline(&device);
    {
        let mut pass = draw_in_compute.compute_pass(ComputePassDesc {
            label: "wrong",
            timestamps: None,
        });
        pass.set_pipeline(pipeline);
    }
    let mut commands = draw_in_compute.into_commands();
    let end = commands.pop().expect("end of pass");
    commands.push(Command::MultiDrawIndirect {
        buffer: args,
        offset: 0,
        count: 1,
    });
    commands.push(end);
    let mut list = CommandList::new();
    for command in commands {
        list.push(command);
    }
    assert_eq!(code(device.submit(list)), "R0104", "a draw in a compute pass");
}

#[test]
fn a_rejected_list_with_indirect_draws_leaves_the_device_usable() {
    let device = NullDevice::new();
    let args = arguments(&device);
    assert_eq!(code(submit_draw(&device, |pass| pass.draw_indirect(args, 2))), "R0114");
    submit_draw(&device, |pass| pass.draw_indirect(args, 0)).expect("the next valid list is accepted");
}
