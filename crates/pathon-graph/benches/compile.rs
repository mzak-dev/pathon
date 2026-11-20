// Frame graph compilation, cold versus served from `GraphCache` (M2-07). Results: docs/bench/README.md.

use divan::Bencher;
use pathon_graph::{FrameGraph, FrameSizes, GraphCache, GraphTextureDesc, PassDesc, Stage};
use pathon_rhi_types::{Access, Format, Id, PhysicalSize, RawHandle, TextureDesc, TextureId, TextureUsage};

const SIZE: PhysicalSize = PhysicalSize::new(1920, 1080);

fn main() {
    divan::main();
}

// A chain of render-size passes, each reading the previous one, ending in the imported output — the shape of a
// post-processing stack.
fn chain(passes: usize) -> FrameGraph {
    let mut graph = FrameGraph::new();
    let output_desc = TextureDesc::new_2d(
        "output",
        SIZE.width,
        SIZE.height,
        Format::Rgba8Unorm,
        TextureUsage::RENDER_TARGET,
    );
    let output = graph.import_texture(
        "output",
        TextureId::from_raw(RawHandle::new(0, 0)),
        output_desc,
        Access::empty(),
        Access::PRESENT,
    );
    let mut previous = None;
    for _ in 0..passes {
        let texture = graph.create_texture("intermediate", GraphTextureDesc::output(Format::Rgba16Float));
        let mut pass = graph.add_pass(PassDesc::new("post", Stage::PostProcess));
        if let Some(previous) = previous {
            pass.read(previous, Access::SHADER_READ);
        }
        previous = Some(pass.write(texture, Access::COLOR_ATTACHMENT));
    }
    let mut pass = graph.add_pass(PassDesc::new("present", Stage::Tonemap));
    if let Some(previous) = previous {
        pass.read(previous, Access::SHADER_READ);
    }
    pass.write(output, Access::COLOR_ATTACHMENT);
    graph
}

#[divan::bench(args = [16, 64, 256])]
fn compile_cold(bencher: Bencher<'_, '_>, passes: usize) {
    let graph = chain(passes);
    bencher.bench_local(|| graph.compile(FrameSizes::same(SIZE)).map(|compiled| compiled.stats()));
}

#[divan::bench(args = [16, 64, 256])]
fn compile_cached(bencher: Bencher<'_, '_>, passes: usize) {
    let graph = chain(passes);
    let mut cache = GraphCache::new();
    let _ = cache.compile(&graph, FrameSizes::same(SIZE));
    bencher.bench_local(|| {
        cache
            .compile(&graph, FrameSizes::same(SIZE))
            .map(|(compiled, _)| compiled.stats())
    });
}
