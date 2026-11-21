use serde::{Deserialize, Serialize};

// One JSON line per published frame. Plain owned data, so the TUI needs nothing from the renderer's crates.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub frame: u64,
    pub frame_ms: f32,
    pub views: Vec<ViewSnapshot>,
    pub features: Vec<FeatureSnapshot>,
    pub diagnostics: Vec<DiagnosticSnapshot>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ViewSnapshot {
    pub index: usize,
    pub passes: Vec<PassSnapshot>,
    pub resources: Vec<ResourceSnapshot>,
    // Dependencies between live passes, as indices into `passes`.
    pub edges: Vec<(usize, usize)>,
    pub final_barriers: Vec<BarrierSnapshot>,
    pub stats: StatsSnapshot,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct PassSnapshot {
    pub name: String,
    pub stage: String,
    pub stage_rank: u8,
    pub priority: i32,
    // Position in the executed order; `None` when culled.
    pub order: Option<usize>,
    pub side_effect: bool,
    pub accesses: Vec<AccessSnapshot>,
    pub barriers: Vec<BarrierSnapshot>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct AccessSnapshot {
    pub resource: usize,
    pub read_version: Option<u32>,
    pub write_version: Option<u32>,
    pub access: Vec<String>,
    pub range: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct BarrierSnapshot {
    // Resources living in the transitioned physical texture or buffer (aliases share one).
    pub resources: Vec<usize>,
    pub access: Vec<String>,
    pub range: String,
}

fn texture_kind() -> String {
    "tekstura".to_owned()
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub name: String,
    // "tekstura", "bufor" or "historia".
    #[serde(default = "texture_kind")]
    pub kind: String,
    pub imported: bool,
    pub format: Option<String>,
    pub size: Option<(u32, u32)>,
    // Buffers only.
    #[serde(default)]
    pub bytes: Option<u64>,
    pub mips: u32,
    pub layers: u32,
    pub usage: Vec<String>,
    // Transients sharing a group share one physical texture or buffer.
    pub alias_group: Option<usize>,
    pub lifetime: Option<(usize, usize)>,
    pub versions: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct StatsSnapshot {
    pub passes_total: usize,
    pub passes_live: usize,
    pub logical_textures: usize,
    pub physical_textures: usize,
    #[serde(default)]
    pub logical_buffers: usize,
    #[serde(default)]
    pub physical_buffers: usize,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct FeatureSnapshot {
    pub name: String,
    pub status: String,
    pub code: Option<String>,
    pub reason: Option<String>,
    pub stage: String,
    pub priority: i32,
    pub role: Option<String>,
    pub order: Option<usize>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct DiagnosticSnapshot {
    pub frame: u64,
    pub code: String,
    pub severity: String,
    pub message: String,
    pub hint: Option<String>,
}

impl Snapshot {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("snapshots contain only serialisable data")
    }

    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_survives_the_wire() {
        let snapshot = Snapshot {
            frame: 7,
            frame_ms: 16.5,
            views: vec![ViewSnapshot {
                index: 0,
                edges: vec![(0, 1)],
                ..ViewSnapshot::default()
            }],
            ..Snapshot::default()
        };
        assert_eq!(Snapshot::from_line(&snapshot.to_line()).expect("parses"), snapshot);
    }

    #[test]
    fn a_snapshot_from_before_buffers_and_history_still_loads() {
        let older = r#"{"frame":1,"frame_ms":1.0,"views":[{"index":0,"passes":[],"resources":[{"name":"a","imported":false,"format":null,"size":null,"mips":1,"layers":1,"usage":[],"alias_group":null,"lifetime":null,"versions":1}],"edges":[],"final_barriers":[],"stats":{"passes_total":1,"passes_live":1,"logical_textures":1,"physical_textures":1}}],"features":[],"diagnostics":[]}"#;

        let snapshot = Snapshot::from_line(older).expect("an older snapshot parses");

        let resource = &snapshot.views[0].resources[0];
        assert_eq!(resource.kind, "tekstura");
        assert_eq!(resource.bytes, None);
        assert_eq!(snapshot.views[0].stats.logical_buffers, 0);
    }
}
