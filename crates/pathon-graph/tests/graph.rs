use std::sync::{Arc, Mutex};

use pathon_graph::{
    CapturedKind, CapturedTarget, FrameGraph, FrameSizes, GraphBufferDesc, GraphTextureDesc, HistoryStore, PassContext,
    PassDesc, ResourcePool, Stage, TextureHandle, to_dot,
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
    graph.import_texture("output", texture, desc, Access::empty(), Access::SAMPLED)
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
        .execute(&compiled, device, pool, &mut HistoryStore::new(), &mut release, frame)
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
            let target = graph.import_texture(name, texture, desc, Access::empty(), Access::SAMPLED);
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
    pass.read(current, Access::SAMPLED);
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
    pass.read(never_written, Access::SAMPLED);
    pass.write(output, Access::COLOR_ATTACHMENT);

    let errors = graph.compile(FrameSizes::same(SIZE)).expect_err("must fail");
    assert_eq!(errors[0].code, "G0003");
}

#[test]
fn a_pass_reading_a_storage_texture_nobody_writes_gets_storage_usage() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let scratch = graph.create_texture("scratch", GraphTextureDesc::render(Format::Rgba8Unorm));
    let mut pass = graph.add_pass(PassDesc::new("fill", Stage::Prepass));
    let scratch = pass.write(scratch, Access::COLOR_ATTACHMENT);
    pass.record(clear(scratch));
    let usage = Arc::new(Mutex::new(TextureUsage::empty()));
    let seen = Arc::clone(&usage);
    let mut pass = graph.add_pass(PassDesc::new("consumer", Stage::Opaque));
    pass.read(scratch, Access::STORAGE_READ);
    let written = pass.write(output, Access::COLOR_ATTACHMENT);
    pass.record(move |context| {
        *seen.lock().expect("lock") = context.desc(scratch).usage;
        clear(written)(context)
    });

    run(graph, &device, &mut ResourcePool::new(), 0);
    assert!(usage.lock().expect("lock").contains(TextureUsage::STORAGE));
}

#[test]
fn one_pass_declaring_layout_mixing_accesses_on_a_texture_is_g0008() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, &device);
    let scratch = graph.create_texture("scratch", GraphTextureDesc::render(Format::Rgba8Unorm));
    let mut pass = graph.add_pass(PassDesc::new("fill", Stage::Prepass));
    let scratch = pass.write(scratch, Access::COLOR_ATTACHMENT);
    pass.record(clear(scratch));
    let mut pass = graph.add_pass(PassDesc::new("consumer", Stage::Opaque));
    pass.read(scratch, Access::SAMPLED);
    pass.read(scratch, Access::COPY_SRC);
    pass.write(output, Access::COLOR_ATTACHMENT);

    let errors = graph.compile(FrameSizes::same(SIZE)).expect_err("must fail");
    assert_eq!(errors[0].code, "G0008");
    assert!(errors[0].message.contains("consumer") && errors[0].message.contains("scratch"));
}

#[test]
fn an_import_ending_in_a_layout_mixing_state_is_g0008() {
    let device = NullDevice::new();
    let mut graph = FrameGraph::new();
    let (texture, desc) = output_texture(&device);
    graph.import_texture(
        "output",
        texture,
        desc,
        Access::empty(),
        Access::SAMPLED | Access::COPY_SRC,
    );

    let errors = graph.compile(FrameSizes::same(SIZE)).expect_err("must fail");
    assert_eq!(errors[0].code, "G0008");
    assert!(errors[0].message.contains("output"));
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
        let target = graph.import_texture(name, texture, desc, Access::empty(), Access::SAMPLED);
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
    let output = graph.import_texture("output", texture, desc, Access::empty(), Access::SAMPLED);
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
    assert!(to_output.barriers.iter().any(|barrier| barrier.target
        == CapturedTarget::Texture(physical("output").expect("physical"))
        && barrier.access == Access::COPY_DST));
    assert!(capture.final_barriers.iter().any(|barrier| barrier.target
        == CapturedTarget::Texture(physical("output").expect("physical"))
        && barrier.access == Access::SAMPLED));
}

#[test]
fn dot_lists_passes_edges_and_culled_passes() {
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
    let mut pass = graph.add_pass(PassDesc::new("to \"output\"", Stage::Tonemap));
    let source = pass.read(first, Access::COPY_SRC);
    let written = pass.write(output, Access::COPY_DST);
    pass.record(copy(source, written));
    let unused = graph.create_texture("unused", desc);
    let mut pass = graph.add_pass(PassDesc::new("unused producer", Stage::Opaque));
    let unused = pass.write(unused, Access::COLOR_ATTACHMENT);
    pass.record(clear(unused));

    let capture = run(graph, &device, &mut pool, 0).capture.expect("capture was enabled");
    let dot = to_dot(&capture);

    assert!(dot.starts_with("digraph"), "{dot}");
    assert!(dot.contains("t1") && dot.contains("unused producer"), "{dot}");
    assert!(dot.contains("p0 -> p1"), "{dot}");
    assert!(dot.contains("Opaque #0"), "{dot}");
    assert_eq!(
        dot.matches("dashed").count(),
        1,
        "only the culled pass is dashed: {dot}"
    );
    assert!(dot.contains("to \\\"output\\\""), "quotes are escaped: {dot}");
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
    let result = graph.execute(&compiled, &device, &mut pool, &mut HistoryStore::new(), &mut release, 0);

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
    graph.import_texture("history", texture, desc, Access::SAMPLED, Access::SAMPLED)
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
    let written = pass.write_buffer(args, Access::STORAGE_WRITE);
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
        [Access::STORAGE_WRITE, Access::INDIRECT]
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
    pass.write_buffer(scratch, Access::STORAGE_WRITE);
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
    pass.modify_buffer(imported, Access::STORAGE_WRITE | Access::STORAGE_READ);
    pass.record(|_| Ok(()));

    let execution = run(graph, &device, &mut ResourcePool::new(), 0);
    assert_eq!(
        buffer_accesses(&execution.commands),
        [Access::STORAGE_WRITE | Access::STORAGE_READ, Access::VERTEX]
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
                pass.read_buffer(source, Access::STORAGE_READ);
            }
            previous = Some(pass.write_buffer(buffer, Access::STORAGE_WRITE));
            pass.record(|_| Ok(()));
        }
        let mut pass = graph.add_pass(PassDesc::new("to output", Stage::Tonemap));
        pass.read_buffer(previous.expect("chain"), Access::STORAGE_READ);
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
    let scratch = pass.write_buffer(scratch, Access::STORAGE_WRITE);
    pass.modify_buffer(imported, Access::STORAGE_WRITE | Access::STORAGE_READ);
    pass.record(|_| Ok(()));
    let mut pass = graph.add_pass(PassDesc::new("use", Stage::Tonemap));
    pass.read_buffer(scratch, Access::STORAGE_READ);
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

const HISTORY_DESC: GraphTextureDesc = GraphTextureDesc::render(Format::Rgba8Unorm);

struct HistoryFrame {
    previous_valid: bool,
    order: Vec<&'static str>,
}

// One view's frame: a pass reads `previous` and writes `current`, nobody else reads `current`.
fn history_frame(
    device: &NullDevice,
    pool: &mut ResourcePool,
    store: &mut HistoryStore,
    release: &mut DeferredRelease,
    sizes: FrameSizes,
    frame: u64,
) -> HistoryFrame {
    let mut graph = FrameGraph::for_view(store, sizes);
    let history = graph.history_texture("taa history", HISTORY_DESC);
    let mut pass = graph.add_pass(PassDesc::new("resolve", Stage::Resolve));
    pass.read(history.previous, Access::SAMPLED);
    let current = pass.write(history.current, Access::COLOR_ATTACHMENT);
    pass.record(clear(current));

    let compiled = graph.compile(sizes).expect("graph compiles");
    let order = compiled.order().to_vec();
    let execution = graph
        .execute(&compiled, device, pool, store, release, frame)
        .expect("graph executes");
    device
        .submit(execution.commands)
        .expect("history passes null validation");
    HistoryFrame {
        previous_valid: history.previous_valid,
        order,
    }
}

#[test]
fn history_is_invalid_on_the_first_frame_and_valid_on_the_next() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();
    let mut store = HistoryStore::new();
    let mut release = DeferredRelease::new(2);
    let sizes = FrameSizes::same(SIZE);

    let first = history_frame(&device, &mut pool, &mut store, &mut release, sizes, 0);
    let created = store.created_total();
    let second = history_frame(&device, &mut pool, &mut store, &mut release, sizes, 1);
    let third = history_frame(&device, &mut pool, &mut store, &mut release, sizes, 2);

    assert!(!first.previous_valid);
    assert!(second.previous_valid);
    assert!(third.previous_valid);
    assert_eq!(created, 2);
    assert_eq!(
        store.created_total(),
        2,
        "a stable history allocates nothing after the first frame"
    );
    assert_eq!(pool.created_total(), 0, "history textures do not live in the pool");
    assert_eq!(release.len(), 0);
}

#[test]
fn resizing_invalidates_history() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();
    let mut store = HistoryStore::new();
    let mut release = DeferredRelease::new(2);

    history_frame(&device, &mut pool, &mut store, &mut release, FrameSizes::same(SIZE), 0);
    let before = release.len();
    let resized = FrameSizes::same(PhysicalSize::new(16, 16));
    let frame = history_frame(&device, &mut pool, &mut store, &mut release, resized, 1);

    assert!(!frame.previous_valid);
    // Both textures go, each with the one view the frame created on it.
    assert_eq!(release.len(), before + 4);
    let next = history_frame(&device, &mut pool, &mut store, &mut release, resized, 2);
    assert!(next.previous_valid);
}

#[test]
fn changing_only_the_render_size_invalidates_history() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();
    let mut store = HistoryStore::new();
    let mut release = DeferredRelease::new(2);
    let output = PhysicalSize::new(16, 16);
    let sizes = |render| FrameSizes { render, output };

    history_frame(
        &device,
        &mut pool,
        &mut store,
        &mut release,
        sizes(PhysicalSize::new(8, 8)),
        0,
    );
    let same = history_frame(
        &device,
        &mut pool,
        &mut store,
        &mut release,
        sizes(PhysicalSize::new(8, 8)),
        1,
    );
    let scaled = history_frame(
        &device,
        &mut pool,
        &mut store,
        &mut release,
        sizes(PhysicalSize::new(4, 4)),
        2,
    );
    let after = history_frame(
        &device,
        &mut pool,
        &mut store,
        &mut release,
        sizes(PhysicalSize::new(4, 4)),
        3,
    );

    assert!(same.previous_valid);
    assert!(!scaled.previous_valid, "a new render size is a new history");
    assert!(after.previous_valid);
}

#[test]
fn the_pass_writing_history_is_not_culled() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();
    let mut store = HistoryStore::new();
    let mut release = DeferredRelease::new(2);

    let frame = history_frame(&device, &mut pool, &mut store, &mut release, FrameSizes::same(SIZE), 0);

    assert_eq!(frame.order, ["resolve"]);
}

#[test]
fn a_graph_without_history_never_reports_valid_history() {
    let mut graph = FrameGraph::new();
    assert!(!graph.history_texture("taa history", HISTORY_DESC).previous_valid);
}

#[test]
fn capture_lists_buffers_and_history() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();
    let mut store = HistoryStore::new();
    let mut release = DeferredRelease::new(2);
    let sizes = FrameSizes::same(SIZE);

    let mut graph = FrameGraph::for_view(&store, sizes);
    graph.set_capture(true);
    let history = graph.history_texture("taa history", HISTORY_DESC);
    let scratch = graph.create_buffer("scratch", GraphBufferDesc::new(256, BufferUsage::empty()));
    let mut pass = graph.add_pass(PassDesc::new("fill", Stage::Opaque));
    let scratch = pass.write_buffer(scratch, Access::STORAGE_WRITE);
    pass.record(|_| Ok(()));
    let mut pass = graph.add_pass(PassDesc::new("resolve", Stage::Resolve));
    pass.read_buffer(scratch, Access::STORAGE_READ);
    pass.read(history.previous, Access::SAMPLED);
    let current = pass.write(history.current, Access::COLOR_ATTACHMENT);
    pass.record(clear(current));

    let compiled = graph.compile(sizes).expect("graph compiles");
    let execution = graph
        .execute(&compiled, &device, &mut pool, &mut store, &mut release, 0)
        .expect("graph executes");
    let capture = execution.capture.expect("capture was enabled");

    let resource = |name: &str| {
        capture
            .resources
            .iter()
            .find(|resource| resource.name == name)
            .expect("resource")
    };
    assert_eq!(resource("scratch").kind, CapturedKind::Buffer);
    assert_eq!(resource("scratch").buffer_size, Some(256));
    assert_eq!(resource("taa history").kind, CapturedKind::History);
    assert!(
        !resource("taa history").imported,
        "history belongs to the store, not to the host"
    );
    assert_eq!(resource("taa history").buffer_size, None);
    assert_eq!(capture.stats.physical_buffers, 1);
    let buffer_barrier = capture
        .passes
        .iter()
        .flat_map(|pass| &pass.barriers)
        .find(|barrier| matches!(barrier.target, CapturedTarget::Buffer(_)))
        .expect("a buffer barrier");
    assert_eq!(buffer_barrier.access, Access::STORAGE_WRITE);
}

type Seen = Arc<Mutex<Vec<TextureId>>>;

fn zero_history_passes(commands: &pathon_rhi_types::CommandList) -> usize {
    commands
        .commands()
        .iter()
        .filter(|command| matches!(command, Command::BeginRenderPass(pass) if pass.label == "zero history"))
        .count()
}

struct Probe {
    device: NullDevice,
    pool: ResourcePool,
    store: HistoryStore,
    release: DeferredRelease,
    frame: u64,
}

struct ProbeFrame {
    valid: Vec<bool>,
    previous: Vec<TextureId>,
    current: Vec<TextureId>,
    zero_passes: usize,
}

impl Probe {
    fn new() -> Self {
        Self {
            device: NullDevice::new(),
            pool: ResourcePool::new(),
            store: HistoryStore::new(),
            release: DeferredRelease::new(2),
            frame: 0,
        }
    }

    // Each name gets a pass that writes `current` and reads `previous`; a conforming pass reads it only while valid.
    fn frame(&mut self, names: &[&'static str], conforming: bool) -> ProbeFrame {
        let sizes = FrameSizes::same(SIZE);
        let previous: Seen = Seen::default();
        let current: Seen = Seen::default();
        let mut graph = FrameGraph::for_view(&self.store, sizes);
        let mut valid = Vec::new();
        for &name in names {
            let history = graph.history_texture(name, HISTORY_DESC);
            valid.push(history.previous_valid);
            let mut pass = graph.add_pass(PassDesc::new(name, Stage::Resolve));
            let reads = history.previous_valid || !conforming;
            if reads {
                pass.read(history.previous, Access::SAMPLED);
            }
            let written = pass.write(history.current, Access::COLOR_ATTACHMENT);
            let (previous, current) = (Arc::clone(&previous), Arc::clone(&current));
            pass.record(move |context| {
                if reads {
                    previous.lock().expect("mutex").push(context.texture(history.previous));
                }
                current.lock().expect("mutex").push(context.texture(written));
                clear(written)(context)
            });
        }
        let compiled = graph.compile(sizes).expect("graph compiles");
        let execution = graph
            .execute(
                &compiled,
                &self.device,
                &mut self.pool,
                &mut self.store,
                &mut self.release,
                self.frame,
            )
            .expect("graph executes");
        let zero_passes = zero_history_passes(&execution.commands);
        self.device
            .submit(execution.commands)
            .expect("history passes null validation");
        self.frame += 1;
        let previous = previous.lock().expect("mutex").clone();
        let current = current.lock().expect("mutex").clone();
        ProbeFrame {
            valid,
            previous,
            current,
            zero_passes,
        }
    }
}

#[test]
fn a_conforming_pass_reads_what_the_last_frame_wrote_without_recreating_textures() {
    let mut probe = Probe::new();

    let first = probe.frame(&["taa"], true);
    let second = probe.frame(&["taa"], true);
    let third = probe.frame(&["taa"], true);

    assert_eq!(first.valid, [false]);
    assert_eq!(second.valid, [true]);
    assert_eq!(
        second.previous, first.current,
        "frame 1 reads the texture frame 0 wrote"
    );
    assert_eq!(
        third.previous, second.current,
        "frame 2 reads the texture frame 1 wrote"
    );
    assert_eq!(
        probe.store.created_total(),
        2,
        "growing usage must not recreate the pair"
    );
    assert_eq!(probe.release.len(), 0);
}

#[test]
fn an_invalid_previous_that_is_read_is_zeroed_and_a_valid_one_is_not() {
    let mut probe = Probe::new();

    let first = probe.frame(&["taa"], false);
    let second = probe.frame(&["taa"], false);

    assert_eq!(first.zero_passes, 1);
    assert_eq!(second.zero_passes, 0);
    assert_eq!(second.previous, first.current);
}

#[test]
fn history_nobody_wrote_is_not_valid_even_though_the_pair_exists() {
    let mut probe = Probe::new();
    let sizes = FrameSizes::same(SIZE);
    for _ in 0..2 {
        let mut graph = FrameGraph::for_view(&probe.store, sizes);
        let history = graph.history_texture("taa", HISTORY_DESC);
        let mut pass = graph.add_pass(PassDesc::new("reader", Stage::Resolve).side_effect());
        pass.read(history.previous, Access::SAMPLED);
        pass.record(|_| Ok(()));
        let compiled = graph.compile(sizes).expect("graph compiles");
        let valid = history.previous_valid;
        let execution = graph
            .execute(
                &compiled,
                &probe.device,
                &mut probe.pool,
                &mut probe.store,
                &mut probe.release,
                probe.frame,
            )
            .expect("graph executes");
        assert!(!valid, "no frame has written this history");
        assert_eq!(zero_history_passes(&execution.commands), 1);
        probe.device.submit(execution.commands).expect("valid commands");
        probe.frame += 1;
    }
}

#[test]
fn a_name_declared_twice_is_one_history_and_still_swaps() {
    let mut probe = Probe::new();
    let sizes = FrameSizes::same(SIZE);
    let mut written = Vec::new();
    let mut read = Vec::new();
    for _ in 0..3 {
        let mut graph = FrameGraph::for_view(&probe.store, sizes);
        let first = graph.history_texture("taa", HISTORY_DESC);
        let second = graph.history_texture("taa", HISTORY_DESC);
        assert_eq!(first, second, "the same declaration returns the same pair");
        let seen = Seen::default();
        let mut pass = graph.add_pass(PassDesc::new("resolve", Stage::Resolve));
        if first.previous_valid {
            pass.read(second.previous, Access::SAMPLED);
        }
        let current = pass.write(first.current, Access::COLOR_ATTACHMENT);
        let (seen_in_pass, valid, previous) = (Arc::clone(&seen), first.previous_valid, second.previous);
        pass.record(move |context| {
            if valid {
                seen_in_pass.lock().expect("mutex").push(context.texture(previous));
            }
            seen_in_pass.lock().expect("mutex").push(context.texture(current));
            clear(current)(context)
        });
        let compiled = graph.compile(sizes).expect("graph compiles");
        let execution = graph
            .execute(
                &compiled,
                &probe.device,
                &mut probe.pool,
                &mut probe.store,
                &mut probe.release,
                probe.frame,
            )
            .expect("graph executes");
        probe.device.submit(execution.commands).expect("valid commands");
        probe.frame += 1;
        let seen = seen.lock().expect("mutex").clone();
        if let [previous, current] = seen[..] {
            read.push(previous);
            written.push(current);
        } else {
            written.push(seen[0]);
        }
    }
    assert_eq!(read, [written[0], written[1]]);
}

#[test]
fn one_name_with_two_descriptions_is_a_graph_error() {
    let mut graph = FrameGraph::for_view(&HistoryStore::new(), FrameSizes::same(SIZE));
    graph.history_texture("taa", HISTORY_DESC);
    graph.history_texture("taa", GraphTextureDesc::render(Format::Rgba16Float));

    let errors = graph
        .compile(FrameSizes::same(SIZE))
        .expect_err("conflicting descriptions");

    assert_eq!(errors[0].code, "G0007");
}

#[test]
fn history_that_a_successful_frame_did_not_declare_is_released() {
    let mut probe = Probe::new();
    probe.frame(&["taa"], true);
    probe.frame(&["taa"], true);

    probe.frame(&[], true);

    assert_eq!(probe.store.textures().count(), 0);
    let back = probe.frame(&["taa"], true);
    assert_eq!(back.valid, [false], "stale contents never come back as valid");
}

#[test]
fn a_failing_pass_discards_history() {
    let mut probe = Probe::new();
    let sizes = FrameSizes::same(SIZE);
    probe.frame(&["taa"], true);
    let mut graph = FrameGraph::for_view(&probe.store, sizes);
    let history = graph.history_texture("taa", HISTORY_DESC);
    let mut pass = graph.add_pass(PassDesc::new("resolve", Stage::Resolve));
    pass.write(history.current, Access::COLOR_ATTACHMENT);
    pass.record(|_| Err(RhiError::InvalidHandle("boom")));
    let compiled = graph.compile(sizes).expect("graph compiles");

    let result = graph.execute(
        &compiled,
        &probe.device,
        &mut probe.pool,
        &mut probe.store,
        &mut probe.release,
        1,
    );

    assert!(result.is_err());
    assert_eq!(probe.store.textures().count(), 0);
    assert_eq!(probe.frame(&["taa"], true).valid, [false]);
}

#[test]
fn release_all_releases_the_views_with_the_textures() {
    let mut probe = Probe::new();
    probe.frame(&["taa"], true);
    let before = probe.release.len();

    probe.store.release_all(&mut probe.pool, &mut probe.release, 1);

    assert_eq!(probe.store.textures().count(), 0);
    assert_eq!(
        probe.release.len(),
        before + 3,
        "two textures and the view the pass created"
    );
}

fn mixed_state_frame(device: &NullDevice, pool: &mut ResourcePool, frame: u64) -> pathon_graph::Execution {
    let mut graph = FrameGraph::new();
    let output = import_output(&mut graph, device);
    let pyramid = graph.create_texture("pyramid", GraphTextureDesc::render(Format::Rgba8Unorm).with_mips(3));
    let mut pass = graph.add_pass(PassDesc::new("mip 0", Stage::Prepass));
    let written = pass.write_range(pyramid, Access::COPY_DST, SubresourceRange::mip(0));
    pass.record(|_| Ok(()));
    let mut pass = graph.add_pass(PassDesc::new("consumer", Stage::Opaque));
    pass.read(written, Access::SAMPLED);
    pass.write(output, Access::COPY_DST);
    pass.record(|_| Ok(()));
    run(graph, device, pool, frame)
}

fn per_mip_barriers(commands: &pathon_rhi_types::CommandList) -> Vec<(u32, Access, Access)> {
    commands
        .commands()
        .iter()
        .filter_map(|command| match command {
            Command::TextureAccess { range, from, to, .. } if range.mip_count == 1 => {
                Some((range.base_mip, *from, *to))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn barriers_over_a_texture_with_mixed_states_start_from_each_subresources_state() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();

    let first = mixed_state_frame(&device, &mut pool, 0);
    assert_eq!(
        per_mip_barriers(&first.commands),
        [
            (0, Access::empty(), Access::COPY_DST),
            (0, Access::COPY_DST, Access::SAMPLED),
            (1, Access::empty(), Access::SAMPLED),
            (2, Access::empty(), Access::SAMPLED),
        ]
    );
    device.submit(first.commands).expect("barriers pass null validation");
    pool.commit_states();

    let second = mixed_state_frame(&device, &mut pool, 1);
    assert_eq!(
        per_mip_barriers(&second.commands),
        [
            (0, Access::SAMPLED, Access::COPY_DST),
            (0, Access::COPY_DST, Access::SAMPLED),
        ]
    );
    device
        .submit(second.commands)
        .expect("the pool remembered the first frame");
}

#[test]
fn a_dropped_frame_leaves_the_pool_remembering_the_last_committed_states() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();
    let committed = mixed_state_frame(&device, &mut pool, 0);
    device.submit(committed.commands).expect("first frame");
    pool.commit_states();

    let dropped = mixed_state_frame(&device, &mut pool, 1);
    drop(dropped);
    pool.abandon_states();

    let next = mixed_state_frame(&device, &mut pool, 2);
    device
        .submit(next.commands)
        .expect("barriers start from the committed states");
}

#[test]
fn views_of_one_frame_chain_through_the_uncommitted_pool_states() {
    let device = NullDevice::new();
    let mut pool = ResourcePool::new();
    let first = mixed_state_frame(&device, &mut pool, 0);
    let second = mixed_state_frame(&device, &mut pool, 0);
    let mut frame = first.commands;
    frame.append(second.commands);

    device
        .submit(frame)
        .expect("the second list starts where the first one ends");
}
