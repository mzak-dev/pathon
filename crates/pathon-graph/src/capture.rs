use pathon_diag::Diagnostic;
use pathon_rhi_types::{Access, BufferId, SubresourceRange, TextureDesc, TextureId};

use crate::compile::GraphStats;
use crate::graph::Stage;

// A picture of one executed frame graph for tools (pathon-devtools): what was declared, what survived culling, in
// which order, which memory each texture got and which barriers the plan inserted. Opt-in with
// `FrameGraph::set_capture`, because building it allocates.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GraphCapture {
    pub passes: Vec<PassCapture>,
    pub resources: Vec<ResourceCapture>,
    // Dependencies between live passes, as indices into `passes`.
    pub edges: Vec<(usize, usize)>,
    // Transitions after the last pass that hand imported textures back in their final state.
    pub final_barriers: Vec<BarrierCapture>,
    pub stats: GraphStats,
    pub warnings: Vec<Diagnostic>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PassCapture {
    pub name: &'static str,
    pub stage: Stage,
    pub priority: i32,
    // Position in the executed order; `None` when the pass was culled.
    pub order: Option<usize>,
    pub side_effect: bool,
    pub accesses: Vec<AccessCapture>,
    pub barriers: Vec<BarrierCapture>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AccessCapture {
    pub resource: usize,
    pub read_version: Option<u32>,
    pub write_version: Option<u32>,
    pub access: Access,
    pub range: SubresourceRange,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BarrierCapture {
    pub target: CapturedTarget,
    pub range: SubresourceRange,
    // The state the barrier assumed; buffer barriers do not carry one.
    pub from: Option<Access>,
    pub access: Access,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum CapturedTarget {
    Texture(TextureId),
    Buffer(BufferId),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum CapturedKind {
    Texture,
    Buffer,
    History,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResourceCapture {
    pub name: &'static str,
    pub kind: CapturedKind,
    pub imported: bool,
    // Resolved description and physical texture; `None` for buffers and for transients no live pass touched.
    pub desc: Option<TextureDesc>,
    pub physical: Option<TextureId>,
    pub buffer_size: Option<u64>,
    pub physical_buffer: Option<BufferId>,
    // First and last position in the executed order.
    pub lifetime: Option<(usize, usize)>,
    pub versions: usize,
}
