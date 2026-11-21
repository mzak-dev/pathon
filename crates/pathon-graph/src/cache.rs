use pathon_diag::Diagnostic;

use crate::compile::CompiledGraph;
use crate::graph::{FrameGraph, FrameSizes, PassAccess, PassDesc, ResourceKind, ResourceNode};

// Everything compilation depends on: passes, their accesses and ordering constraints, resource descriptions and
// frame sizes. Imported texture ids and access states are left out on purpose — a window hands out a new texture id
// every frame, and states only feed the barrier plan, which `execute` rebuilds anyway.
#[derive(Clone, Debug)]
struct Structure {
    sizes: FrameSizes,
    resources: Vec<ResourceNode>,
    passes: Vec<(PassDesc, Vec<PassAccess>)>,
}

fn same_resource(cached: &ResourceNode, current: &ResourceNode) -> bool {
    let same_kind = match (cached.kind, current.kind) {
        (ResourceKind::Transient(a), ResourceKind::Transient(b)) => a == b,
        (ResourceKind::Imported { desc: a, .. }, ResourceKind::Imported { desc: b, .. }) => a == b,
        (ResourceKind::TransientBuffer(a), ResourceKind::TransientBuffer(b)) => a == b,
        (ResourceKind::ImportedBuffer { size: a, .. }, ResourceKind::ImportedBuffer { size: b, .. }) => a == b,
        _ => false,
    };
    same_kind && cached.name == current.name && cached.producers == current.producers
}

impl Structure {
    fn of(graph: &FrameGraph, sizes: FrameSizes) -> Self {
        Self {
            sizes,
            resources: graph.resources.clone(),
            passes: graph
                .passes
                .iter()
                .map(|pass| (pass.desc.clone(), pass.accesses.clone()))
                .collect(),
        }
    }

    // Exact comparison rather than a hash: hashing every pass costs about as much as compiling (measured), and a
    // collision would silently reuse the wrong plan.
    fn matches(&self, graph: &FrameGraph, sizes: FrameSizes) -> bool {
        self.sizes == sizes
            && self.resources.len() == graph.resources.len()
            && self.passes.len() == graph.passes.len()
            && self
                .passes
                .iter()
                .zip(&graph.passes)
                .all(|((desc, accesses), pass)| *accesses == pass.accesses && *desc == pass.desc)
            && self
                .resources
                .iter()
                .zip(&graph.resources)
                .all(|(cached, current)| same_resource(cached, current))
    }
}

// M2-07: a stable scene rebuilds the same graph every frame, so its compilation (validation, culling, ordering,
// lifetimes) is reused until the structure or the frame size changes. Ceiling: one cached graph per cache — the
// renderer keeps one cache per view.
#[derive(Debug, Default)]
pub struct GraphCache {
    cached: Option<(Structure, CompiledGraph)>,
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
        let hit = self
            .cached
            .as_ref()
            .is_some_and(|(structure, _)| structure.matches(graph, sizes));
        if !hit {
            let compiled = graph.compile(sizes)?;
            self.compilations += 1;
            self.cached = Some((Structure::of(graph, sizes), compiled));
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
