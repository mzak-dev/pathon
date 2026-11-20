use std::hash::{DefaultHasher, Hash, Hasher};

use pathon_diag::Diagnostic;

use crate::compile::CompiledGraph;
use crate::graph::{FrameGraph, FrameSizes, ResourceKind};

// Everything compilation depends on: passes, their accesses and ordering constraints, resource descriptions and
// frame sizes. Imported texture ids are left out on purpose — a window hands out a new id every frame.
fn structural_key(graph: &FrameGraph, sizes: FrameSizes) -> u64 {
    let mut hasher = DefaultHasher::new();
    sizes.hash(&mut hasher);
    graph.resources.len().hash(&mut hasher);
    for resource in &graph.resources {
        resource.name.hash(&mut hasher);
        resource.producers.hash(&mut hasher);
        match resource.kind {
            ResourceKind::Transient(desc) => (0_u8, desc).hash(&mut hasher),
            // Initial and final states only feed the barrier plan, which is rebuilt in `execute` every frame.
            ResourceKind::Imported { desc, .. } => (1_u8, desc).hash(&mut hasher),
        }
    }
    graph.passes.len().hash(&mut hasher);
    for pass in &graph.passes {
        pass.desc.hash(&mut hasher);
        pass.accesses.hash(&mut hasher);
    }
    hasher.finish()
}

// M2-07: a stable scene rebuilds the same graph every frame, so its compilation (validation, culling, ordering,
// lifetimes) is reused until the structure or the frame size changes. Ceiling: one cached graph per cache — the
// renderer keeps one cache per view.
#[derive(Debug, Default)]
pub struct GraphCache {
    cached: Option<(u64, CompiledGraph)>,
    compilations: u64,
}

impl GraphCache {
    pub fn new() -> Self {
        Self::default()
    }

    // Returns whether this call compiled, next to the compiled graph.
    pub fn compile(
        &mut self,
        graph: &FrameGraph,
        sizes: FrameSizes,
    ) -> Result<(&CompiledGraph, bool), Vec<Diagnostic>> {
        let key = structural_key(graph, sizes);
        let hit = matches!(&self.cached, Some((cached, _)) if *cached == key);
        if !hit {
            let compiled = graph.compile(sizes)?;
            self.compilations += 1;
            self.cached = Some((key, compiled));
        }
        let (_, compiled) = self.cached.as_ref().expect("filled above on a miss");
        Ok((compiled, !hit))
    }

    pub fn compilations(&self) -> u64 {
        self.compilations
    }

    pub fn clear(&mut self) {
        self.cached = None;
    }
}
