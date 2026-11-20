use pathon_graph::{
    FrameGraph, FrameSizes, GraphTextureDesc, PassContext, PassDesc, Stage, TextureHandle, TexturePool,
};
use pathon_resources::DeferredRelease;
use pathon_rhi_null::NullDevice;
use pathon_rhi_types::{
    Access, Color, ColorAttachment, DeviceApi, Format, PhysicalSize, RenderPassDesc, RhiError, SubresourceRange,
    TextureCopy, TextureDesc, TextureId, TextureUsage,
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

fn run(graph: FrameGraph, device: &NullDevice, pool: &mut TexturePool, frame: u64) -> pathon_graph::Execution {
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
    let mut pool = TexturePool::new();
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

    let errors = graph.compile(FrameSizes::same(SIZE)).expect_err("must fail");
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
