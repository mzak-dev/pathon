use pathon_api::Camera;
use pathon_diag::Diagnostic;
use pathon_graph::{GraphCapture, GraphStats};

use crate::target::TargetId;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ViewInput {
    pub target: TargetId,
    pub camera: Camera,
}

// The host owns time: core never reads a clock (ADR-0005).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct FrameInput {
    pub time_seconds: f64,
    pub delta_seconds: f32,
    pub views: Vec<ViewInput>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct FrameReport {
    pub views: Vec<GraphStats>,
    pub skipped_views: usize,
    // Views whose frame graph had to be compiled this frame; 0 in a stable scene (M2-07).
    pub compiled_views: usize,
    // One per recorded view while `Renderer::set_capture(true)`.
    pub captures: Vec<GraphCapture>,
    // Every diagnostic raised while recording this frame (the log shows each only once).
    pub diagnostics: Vec<Diagnostic>,
}
