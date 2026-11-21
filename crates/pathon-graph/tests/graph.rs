use pathon_graph::{
    FrameGraph, FrameSizes, GraphBufferDesc, GraphTextureDesc, PassContext, PassDesc, ResourcePool, Stage,
    TextureHandle,
};
use pathon_resources::DeferredRelease;
use pathon_rhi_null::NullDevice;
use pathon_rhi_types::{
    Access, BindGroupDesc, BindGroupLayoutDesc, BufferUsage, Color, ColorAttachment, Command, DeviceApi, Format,
    PhysicalSize, RenderPassDesc, RhiError, SubresourceRange, TextureCopy, TextureDesc, TextureId, TextureUsage,
};

const SIZE: PhysicalSize = PhysicalSize::new(8, 8);

fn output_texture(device: &NullDevice) -> (TextureId, TextureDesc) {
    let desc = TextureDesc::new_2d(
        "output",
        SIZE.width,
        SIZE.height,
        Format::Rgba8Unorm,
        TextureUsage::RENDER_TARGET | TextureUsage::SAMPLED | TextureUsage::COPY_DST | TextureUsage::COPY_SRC,
    );
    (device.create_texture(&desc).expect("output"), desc)
}

fn import_output(graph: &mut FrameGraph, device: &NullDevice) -> TextureHandle {
    let (texture, desc) = output_texture(device);
    graph.import_texture("output", texture, desc, Access::empty(), Access::SHADER_READ)
}

fn clear(target: TextureHandle) -> impl FnOnce(&mut PassContext<'_>) -> Result<(), RhiError> {
    move |context| {
        let view = context.view(target)?;
        drop(context.commands().render_pass(RenderPassDesc {
            label: "clear",
            color: vec![ColorAttachment::clear(view, Color::BLACK)],
            depth: None,
            timestamps: None,
        }));
        Ok(())
    }
}

fn copy(
    source: TextureHandle,
    destination: TextureHandle,
) -> impl FnOnce(&mut PassContext<'_>) -> Result<(), RhiError> {
    move |context| {
        let size = context.desc(source).size;
        let (source, destination) = (context.texture(source), context.texture(destination));
        context
            .commands()
            .copy_texture_to_texture(TextureCopy::whole(source), TextureCopy::whole(destination), size);
        Ok(())
    }
}

fn run(graph: FrameGraph, device: &NullDevice, pool: &mut ResourcePool, frame: u64) -> pathon_graph::Execution {
    let compiled = graph.compile(FrameSizes::same(SIZE)).expect("graph compiles");
    let mut release = DeferredRelease::new(2);
    let execution = graph
        .execute(&compiled, device, pool, &mut release, frame)
        .expect("graph executes");
    release.collect(frame + 2, device);
    execution
}

#[test]
fn every_write_creates_a_new_version() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let mut pass = graph.add_pass(PassDesc::new("write", Stage::Opaque));
    let written = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(clear(written));

    assert_ne!(output, written);
    assert!(graph.is_latest(written));
    assert!(!graph.is_latest(output));
}

#[test]
fn order_follows_stage_priority_and_constraints_not_registration() {
    let build = |names: [&'static str; 3]| {
        let device = NullDevice::new();
        let mut graph = FrameGraph::new();
        for name in names {
            let (texture, desc) = output_texture(&device);
            let target = graph.import_texture(name, texture, desc, Access::empty(), Access::SHADER_READ);
            let desc = match name {
                "a" => PassDesc::new("a", Stage::PostProcess).priority(10),
                "b" => PassDesc::new("b", Stage::PostProcess).after("c"),
                _ => PassDesc::new("c", Stage::PostProcess).priority(5),
            };
            let mut pass = graph.add_pass(desc);
            let written = pass.write(target, Access::COLOR_ATTACHMENT);
            pass.record(clear(written));
        }
        graph
            .compile(FrameSizes::same(SIZE))
            .expect("compiles")
            .order()
            .to_vec()
    };

    let expected = vec!["c", "b", "a"];
    assert_eq!(build(["a", "b", "c"]), expected);
    assert_eq!(build(["c", "a", "b"]), expected);
    assert_eq!(build(["b", "c", "a"]), expected);
}

#[test]
fn passes_without_consumers_are_culled_unless_they_have_side_effects() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);

    let unused = graph.create_texture("unused", GraphTextureDesc::render(Format::Rgba8Unorm));
    let mut pass = graph.add_pass(PassDesc::new("unused producer", Stage::Opaque));
    let unused = pass.write(unused, Access::COLOR_ATTACHMENT);
    pass.record(clear(unused));

    let mut pass = graph.add_pass(PassDesc::new("overwritten clear", Stage::Opaque));
    let first = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(clear(first));

    let mut pass = graph.add_pass(PassDesc::new("final clear", Stage::PostProcess));
    let last = pass.write(first, Access::COLOR_ATTACHMENT);
    pass.record(clear(last));

    graph.add_pass(PassDesc::new("readback", Stage::Overlay).side_effect());

    let compiled = graph.compile(FrameSizes::same(SIZE)).expect("compiles");
    assert_eq!(compiled.order(), ["final clear", "readback"]);
    assert_eq!(compiled.stats().passes_total, 4);
    assert_eq!(compiled.stats().passes_live, 2);
}

#[test]
fn transient_textures_with_disjoint_lifetimes_share_memory_and_pass_state_validation() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();
    for frame in 0..3 {
        let mut graph = FrameGraph::new();
        let output = import_output(&mut graph, &device);
        let desc = GraphTextureDesc::render(Format::Rgba8Unorm);

        let mut previous = None;
        for (index, name) in ["t1", "t2", "t3"].into_iter().enumerate() {
            let texture = graph.create_texture(name, desc);
            let mut pass =
                graph.add_pass(PassDesc::new(name, Stage::Opaque).priority(i32::try_from(index).expect("small")));
            let written = match previous {
                None => {
                    let written = pass.write(texture, Access::COLOR_ATTACHMENT);
                    pass.record(clear(written));
                    written
                }
                Some(source) => {
                    let source = pass.read(source, Access::COPY_SRC);
                    let written = pass.write(texture, Access::COPY_DST);
                    pass.record(copy(source, written));
                    written
                }
            };
            previous = Some(written);
        }
        let mut pass = graph.add_pass(PassDesc::new("to output", Stage::Tonemap));
        let source = pass.read(previous.expect("chain"), Access::COPY_SRC);
        let written = pass.write(output, Access::COPY_DST);
        pass.record(copy(source, written));

        let execution = run(graph, &device, &mut pool, frame);
        assert_eq!(execution.stats.logical_textures, 3);
        assert_eq!(execution.stats.physical_textures, 2);
        device
            .submit(execution.commands)
            .expect("barrier plan passes null validation");
    }
    assert_eq!(
        pool.created_total(),
        2,
        "a stable graph allocates nothing after the first frame"
    );
}

#[test]
fn hi_z_mip_chain_orders_partial_writes() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let pyramid = graph.create_texture("pyramid", GraphTextureDesc::render(Format::Rgba8Unorm).with_mips(3));

    let mut pass = graph.add_pass(PassDesc::new("mip 0", Stage::Prepass));
    let mut current = pass.write_range(pyramid, Access::COPY_DST, SubresourceRange::mip(0));
    for mip in 1..3 {
        let mut pass = graph.add_pass(PassDesc::new(["", "mip 1", "mip 2"][mip as usize], Stage::Prepass));
        let source = pass.read_range(current, Access::COPY_SRC, SubresourceRange::mip(mip - 1));
        current = pass.write_range(source, Access::COPY_DST, SubresourceRange::mip(mip));
    }
    let mut pass = graph.add_pass(PassDesc::new("consumer", Stage::Opaque));
    pass.read(current, Access::SHADER_READ);
    let written = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(clear(written));

    let compiled = graph.compile(FrameSizes::same(SIZE)).expect("compiles");
    assert_eq!(compiled.order(), ["mip 0", "mip 1", "mip 2", "consumer"]);
}

#[test]
fn reading_a_texture_nobody_writes_is_g0003() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let never_written = graph.create_texture("ao", GraphTextureDesc::render(Format::R8Unorm));
    let mut pass = graph.add_pass(PassDesc::new("lighting", Stage::Opaque));
    pass.read(never_written, Access::SHADER_READ);
    pass.write(output, Access::COLOR_ATTACHMENT);

    let errors = graph.compile(FrameSizes::same(SIZE)).expect_err("must fail");
    assert_eq!(errors[0].code, "G0003");
}

#[test]
fn contradicting_constraints_are_g0001() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let mut pass = graph.add_pass(PassDesc::new("first", Stage::Opaque).after("second"));
    let first = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(clear(first));
    let mut pass = graph.add_pass(PassDesc::new("second", Stage::Opaque));
    pass.modify(first, Access::COLOR_ATTACHMENT);

    let errors = graph.compile(FrameSizes::same(SIZE)).expect_err("must fail");
    assert_eq!(errors[0].code, "G0001");
}

#[test]
fn mixing_render_and_output_size_attachments_is_g0004() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let scene = graph.create_texture("scene", GraphTextureDesc::render(Format::Rgba16Float));
    let mut pass = graph.add_pass(PassDesc::new("mixed", Stage::Opaque));
    pass.write(scene, Access::COLOR_ATTACHMENT);
    pass.write(output, Access::COLOR_ATTACHMENT);

    let upscaled = FrameSizes {
        render: PhysicalSize::new(SIZE.width / 2, SIZE.height / 2),
        output: SIZE,
    };
    let errors = graph.compile(upscaled).expect_err("must fail");
    assert_eq!(errors[0].code, "G0004");
}

#[test]
fn unordered_passes_in_one_stage_warn_g0006() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    for name in ["left", "right"] {
        let (texture, desc) = output_texture(&device);
        let target = graph.import_texture(name, texture, desc, Access::empty(), Access::SHADER_READ);
        let mut pass = graph.add_pass(PassDesc::new(name, Stage::Opaque));
        let written = pass.write(target, Access::COLOR_ATTACHMENT);
        pass.record(clear(written));
    }
    let compiled = graph.compile(FrameSizes::same(SIZE)).expect("compiles");
    assert!(compiled.warnings().iter().any(|warning| warning.code == "G0006"));
}

fn stable_graph(device: &NullDevice, texture: Option<(TextureId, TextureDesc)>) -> FrameGraph {
    let mut graph = FrameGraph::new();
    let (texture, desc) = texture.unwrap_or_else(|| output_texture(device));
    let output = graph.import_texture("output", texture, desc, Access::empty(), Access::SHADER_READ);
    let scene = graph.create_texture("scene", GraphTextureDesc::render(Format::Rgba8Unorm));
    let mut pass = graph.add_pass(PassDesc::new("scene", Stage::Opaque));
    let scene = pass.write(scene, Access::COPY_DST);
    pass.record(|_| Ok(()));
    let mut pass = graph.add_pass(PassDesc::new("present", Stage::Tonemap));
    let source = pass.read(scene, Access::COPY_SRC);
    let written = pass.write(output, Access::COPY_DST);
    pass.record(copy(source, written));
    graph
}

#[test]
fn stable_graph_compiles_once_and_resizes_recompile() {
    let device = NullDevice::new();
    let mut cache = pathon_graph::GraphCache::new();
    for _ in 0..5 {
        let graph = stable_graph(&device, None);
        cache.compile(&graph, FrameSizes::same(SIZE)).expect("compiles");
    }
    assert_eq!(
        cache.compilations(),
        1,
        "a new texture id every frame (like a window) must not invalidate"
    );

    let graph = stable_graph(&device, None);
    cache
        .compile(&graph, FrameSizes::same(PhysicalSize::new(16, 16)))
        .expect("compiles");
    assert_eq!(cache.compilations(), 2);
}

#[test]
fn structural_change_recompiles() {
    let device = NullDevice::new();
    let mut cache = pathon_graph::GraphCache::new();
    cache
        .compile(&stable_graph(&device, None), FrameSizes::same(SIZE))
        .expect("compiles");

    let mut graph = stable_graph(&device, None);
    graph.add_pass(PassDesc::new("readback", Stage::Overlay).side_effect());
    let (compiled, fresh) = cache.compile(&graph, FrameSizes::same(SIZE)).expect("compiles");
    assert!(fresh);
    assert_eq!(compiled.order().last(), Some(&"readback"));
}

#[test]
fn capture_describes_order_culling_dependencies_aliasing_and_barriers() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();
    let mut graph = FrameGraph::new();
    graph.set_capture(true);
    let output = import_output(&mut graph, &device);
    let desc = GraphTextureDesc::render(Format::Rgba8Unorm);

    let first = graph.create_texture("t1", desc);
    let mut pass = graph.add_pass(PassDesc::new("t1", Stage::Opaque));
    let first = pass.write(first, Access::COLOR_ATTACHMENT);
    pass.record(clear(first));

    let mut previous = first;
    for (priority, name) in [(1, "t2"), (2, "t3")] {
        let texture = graph.create_texture(name, desc);
        let mut pass = graph.add_pass(PassDesc::new(name, Stage::Opaque).priority(priority));
        let source = pass.read(previous, Access::COPY_SRC);
        let written = pass.write(texture, Access::COPY_DST);
        pass.record(copy(source, written));
        previous = written;
    }
    let mut pass = graph.add_pass(PassDesc::new("to output", Stage::Tonemap));
    let source = pass.read(previous, Access::COPY_SRC);
    let written = pass.write(output, Access::COPY_DST);
    pass.record(copy(source, written));

    let unused = graph.create_texture("unused", desc);
    let mut pass = graph.add_pass(PassDesc::new("unused producer", Stage::Opaque));
    let unused = pass.write(unused, Access::COLOR_ATTACHMENT);
    pass.record(clear(unused));

    let capture = run(graph, &device, &mut pool, 0).capture.expect("capture was enabled");
    let orders: Vec<_> = capture.passes.iter().map(|pass| (pass.name, pass.order)).collect();
    assert_eq!(
        orders,
        [
            ("t1", Some(0)),
            ("t2", Some(1)),
            ("t3", Some(2)),
            ("to output", Some(3)),
            ("unused producer", None)
        ]
    );
    assert_eq!(capture.edges, [(0, 1), (1, 2), (2, 3)]);

    let physical = |name: &str| {
        let resource = capture.resources.iter().find(|resource| resource.name == name);
        resource.and_then(|resource| resource.physical)
    };
    assert_eq!(physical("t1"), physical("t3"), "disjoint lifetimes alias");
    assert_ne!(physical("t1"), physical("t2"));
    assert_eq!(physical("unused"), None);
    assert_eq!(capture.stats.physical_textures, 2);

    let to_output = &capture.passes[3];
    assert!(
        to_output
            .barriers
            .iter()
            .any(|barrier| Some(barrier.texture) == physical("output") && barrier.access == Access::COPY_DST)
    );
    assert!(
        capture
            .final_barriers
            .iter()
            .any(|barrier| Some(barrier.texture) == physical("output") && barrier.access == Access::SHADER_READ)
    );
}

#[test]
fn capture_is_off_by_default() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let mut pass = graph.add_pass(PassDesc::new("clear", Stage::Opaque));
    let written = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(clear(written));
    assert_eq!(run(graph, &device, &mut ResourcePool::new(), 0).capture, None);
}

#[test]
fn writes_stay_ordered_when_an_intermediate_writer_is_culled() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let shared = graph.create_texture("shared", GraphTextureDesc::render(Format::Rgba8Unorm));

    let mut pass = graph.add_pass(PassDesc::new("a", Stage::Opaque).priority(1));
    let first = pass.write(shared, Access::COLOR_ATTACHMENT);
    pass.record(clear(first));

    let mut pass = graph.add_pass(PassDesc::new("b reads a", Stage::Opaque).priority(2).side_effect());
    pass.read(first, Access::COPY_SRC);

    let mut pass = graph.add_pass(PassDesc::new("c overwritten", Stage::Opaque).priority(3));
    let second = pass.write(first, Access::COLOR_ATTACHMENT);
    pass.record(clear(second));

    let mut pass = graph.add_pass(PassDesc::new("d", Stage::Opaque));
    let third = pass.write(second, Access::COLOR_ATTACHMENT);
    pass.record(clear(third));

    let mut pass = graph.add_pass(PassDesc::new("e reads d", Stage::Tonemap));
    let source = pass.read(third, Access::COPY_SRC);
    let written = pass.write(output, Access::COPY_DST);
    pass.record(copy(source, written));

    let compiled = graph.compile(FrameSizes::same(SIZE)).expect("compiles");
    assert_eq!(compiled.order(), ["a", "b reads a", "d", "e reads d"]);
}

#[test]
fn a_failing_pass_still_hands_its_frame_resources_to_deferred_release() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();
    let mut release = DeferredRelease::new(2);
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let mut pass = graph.add_pass(PassDesc::new("fails after allocating", Stage::Opaque));
    let written = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(move |context| {
        context.view(written)?;
        let layout = context.device().create_bind_group_layout(&BindGroupLayoutDesc {
            label: "empty",
            entries: Vec::new(),
        })?;
        context.create_bind_group(&BindGroupDesc {
            label: "per frame",
            layout,
            entries: Vec::new(),
        })?;
        Err(RhiError::InvalidHandle("pass gave up"))
    });

    let compiled = graph.compile(FrameSizes::same(SIZE)).expect("compiles");
    let result = graph.execute(&compiled, &device, &mut pool, &mut release, 0);

    assert!(matches!(result, Err(RhiError::InvalidHandle("pass gave up"))));
    assert_eq!(release.len(), 2, "the output's view and the frame's bind group");
}

fn render_sized_history(graph: &mut FrameGraph, device: &NullDevice, size: PhysicalSize) -> TextureHandle {
    let desc = TextureDesc::new_2d(
        "history",
        size.width,
        size.height,
        Format::Rgba16Float,
        TextureUsage::RENDER_TARGET | TextureUsage::SAMPLED,
    );
    let texture = device.create_texture(&desc).expect("history");
    graph.import_texture("history", texture, desc, Access::SHADER_READ, Access::SHADER_READ)
}

#[test]
fn a_render_sized_import_next_to_a_render_sized_transient_is_not_g0004() {
    let half = PhysicalSize::new(SIZE.width / 2, SIZE.height / 2);
    for sizes in [
        FrameSizes::same(SIZE),
        FrameSizes {
            render: half,
            output: SIZE,
        },
    ] {
        let device = NullDevice::new();
        let mut graph = FrameGraph::new();
        let history = render_sized_history(&mut graph, &device, sizes.render);
        let scene = graph.create_texture("scene", GraphTextureDesc::render(Format::Rgba16Float));
        let mut pass = graph.add_pass(PassDesc::new("taa resolve", Stage::Resolve).side_effect());
        pass.write(scene, Access::COLOR_ATTACHMENT);
        pass.write(history, Access::COLOR_ATTACHMENT);

        let compiled = graph.compile(sizes);
        assert!(compiled.is_ok(), "{sizes:?}: {:?}", compiled.err());
    }
}

fn buffer_accesses(commands: &pathon_rhi_types::CommandList) -> Vec<Access> {
    commands
        .commands()
        .iter()
        .filter_map(|command| match command {
            Command::BufferAccess { access, .. } => Some(*access),
            _ => None,
        })
        .collect()
}

#[test]
fn a_compute_pass_feeding_an_indirect_draw_is_kept_and_ordered_first() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let args = graph.create_buffer("args", GraphBufferDesc::new(64, BufferUsage::empty()));

    let mut pass = graph.add_pass(PassDesc::new("a", Stage::Opaque));
    let written = pass.write_buffer(args, Access::SHADER_WRITE);
    pass.record(|_| Ok(()));
    let mut pass = graph.add_pass(PassDesc::new("b", Stage::Opaque));
    pass.read_buffer(written, Access::INDIRECT);
    pass.write(output, Access::COPY_DST);
    pass.record(|_| Ok(()));

    let compiled = graph.compile(FrameSizes::same(SIZE)).expect("graph compiles");
    assert_eq!(compiled.order(), ["a", "b"]);
    let execution = run(graph, &device, &mut ResourcePool::new(), 0);
    assert_eq!(
        buffer_accesses(&execution.commands),
        [Access::SHADER_WRITE, Access::INDIRECT]
    );
    device
        .submit(execution.commands)
        .expect("buffer barriers pass null validation");
}

#[test]
fn a_buffer_nobody_reads_does_not_keep_its_writer() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let scratch = graph.create_buffer("scratch", GraphBufferDesc::new(64, BufferUsage::empty()));
    let mut pass = graph.add_pass(PassDesc::new("writer", Stage::Opaque));
    pass.write_buffer(scratch, Access::SHADER_WRITE);
    pass.record(|_| Ok(()));

    let execution = run(graph, &device, &mut ResourcePool::new(), 0);
    assert_eq!(execution.stats.passes_live, 0);
    assert_eq!(execution.stats.logical_buffers, 0);
}

#[test]
fn an_imported_buffer_ends_in_its_final_state() {
    let device = NullDevice::new();
    let buffer = device
        .create_buffer(&pathon_rhi_types::BufferDesc {
            label: "imported",
            size: 64,
            usage: BufferUsage::COPY_DST | BufferUsage::VERTEX | BufferUsage::STORAGE,
        })
        .expect("buffer");
    let mut graph = FrameGraph::new();
    let imported = graph.import_buffer("imported", buffer, 64, Access::COPY_DST, Access::VERTEX);
    let mut pass = graph.add_pass(PassDesc::new("fill", Stage::Opaque));
    pass.modify_buffer(imported, Access::SHADER_WRITE | Access::SHADER_READ);
    pass.record(|_| Ok(()));

    let execution = run(graph, &device, &mut ResourcePool::new(), 0);
    assert_eq!(
        buffer_accesses(&execution.commands),
        [Access::SHADER_WRITE | Access::SHADER_READ, Access::VERTEX]
    );
    device
        .submit(execution.commands)
        .expect("buffer barriers pass null validation");
}

#[test]
fn buffers_with_disjoint_lifetimes_share_memory() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();
    for frame in 0..3 {
        let mut graph = FrameGraph::new();
        let output = import_output(&mut graph, &device);
        let desc = GraphBufferDesc::new(256, BufferUsage::empty());

        let mut previous = None;
        for (index, name) in ["b1", "b2", "b3"].into_iter().enumerate() {
            let buffer = graph.create_buffer(name, desc);
            let mut pass =
                graph.add_pass(PassDesc::new(name, Stage::Opaque).priority(i32::try_from(index).expect("small")));
            if let Some(source) = previous {
                pass.read_buffer(source, Access::SHADER_READ);
            }
            previous = Some(pass.write_buffer(buffer, Access::SHADER_WRITE));
            pass.record(|_| Ok(()));
        }
        let mut pass = graph.add_pass(PassDesc::new("to output", Stage::Tonemap));
        pass.read_buffer(previous.expect("chain"), Access::SHADER_READ);
        pass.write(output, Access::COPY_DST);
        pass.record(|_| Ok(()));

        let execution = run(graph, &device, &mut pool, frame);
        assert_eq!(execution.stats.logical_buffers, 3);
        assert_eq!(execution.stats.physical_buffers, 2);
        device
            .submit(execution.commands)
            .expect("barrier plan passes null validation");
    }
    assert_eq!(pool.created_total(), 2);
}

fn buffer_graph(device: &NullDevice) -> FrameGraph {
    let buffer = device
        .create_buffer(&pathon_rhi_types::BufferDesc {
            label: "imported",
            size: 64,
            usage: BufferUsage::COPY_DST | BufferUsage::VERTEX | BufferUsage::STORAGE,
        })
        .expect("buffer");
    let mut graph = FrameGraph::new();
    let imported = graph.import_buffer("imported", buffer, 64, Access::COPY_DST, Access::VERTEX);
    let scratch = graph.create_buffer("scratch", GraphBufferDesc::new(64, BufferUsage::empty()));
    let mut pass = graph.add_pass(PassDesc::new("fill", Stage::Opaque));
    let scratch = pass.write_buffer(scratch, Access::SHADER_WRITE);
    pass.modify_buffer(imported, Access::SHADER_WRITE | Access::SHADER_READ);
    pass.record(|_| Ok(()));
    let mut pass = graph.add_pass(PassDesc::new("use", Stage::Tonemap));
    pass.read_buffer(scratch, Access::SHADER_READ);
    pass.record(|_| Ok(()));
    graph
}

#[test]
fn a_stable_graph_with_buffers_compiles_once() {
    let device = NullDevice::new();
    let mut cache = pathon_graph::GraphCache::new();
    for _ in 0..5 {
        cache
            .compile(&buffer_graph(&device), FrameSizes::same(SIZE))
            .expect("compiles");
    }
    assert_eq!(
        cache.compilations(),
        1,
        "a new buffer id every frame must not invalidate"
    );
}

#[test]
fn an_untouched_imported_buffer_still_ends_in_its_final_state() {
    let device = NullDevice::new();
    let buffer = device
        .create_buffer(&pathon_rhi_types::BufferDesc {
            label: "idle",
            size: 64,
            usage: BufferUsage::COPY_DST | BufferUsage::VERTEX,
        })
        .expect("buffer");
    let mut graph = FrameGraph::new();
    graph.import_buffer("idle", buffer, 64, Access::COPY_DST, Access::VERTEX);

    let execution = run(graph, &device, &mut ResourcePool::new(), 0);
    assert_eq!(buffer_accesses(&execution.commands), [Access::VERTEX]);
    device
        .submit(execution.commands)
        .expect("buffer barriers pass null validation");
}
