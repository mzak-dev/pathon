use std::collections::HashMap;

use pathon_core::{FeatureStatus, FrameReport, Renderer};
use pathon_diag::Severity;
use pathon_graph::{BarrierCapture, GraphCapture, Stage};
use pathon_rhi::{Access, SubresourceRange, TextureId, TextureUsage};

use crate::protocol::{
    AccessSnapshot, BarrierSnapshot, DiagnosticSnapshot, FeatureSnapshot, PassSnapshot, ResourceSnapshot, Snapshot,
    StatsSnapshot, ViewSnapshot,
};

fn access_names(access: Access) -> Vec<String> {
    access.iter_names().map(|(name, _)| name.to_owned()).collect()
}

fn usage_names(usage: TextureUsage) -> Vec<String> {
    usage.iter_names().map(|(name, _)| name.to_owned()).collect()
}

fn range_text(range: SubresourceRange) -> String {
    if range == SubresourceRange::ALL {
        return "wszystko".to_owned();
    }
    let mips = match range.mip_count {
        1 => format!("mip {}", range.base_mip),
        u32::MAX => format!("mipy {}..", range.base_mip),
        count => format!("mipy {}..{}", range.base_mip, range.base_mip + count),
    };
    match range.layer_count {
        u32::MAX if range.base_layer == 0 => mips,
        1 => format!("{mips}, warstwa {}", range.base_layer),
        count => format!(
            "{mips}, warstwy {}..{}",
            range.base_layer,
            range.base_layer.saturating_add(count)
        ),
    }
}

fn stage_rank(stage: Stage) -> u8 {
    let all = [
        Stage::Setup,
        Stage::Shadows,
        Stage::Prepass,
        Stage::Opaque,
        Stage::Sky,
        Stage::Transparent,
        Stage::PostProcess,
        Stage::Resolve,
        Stage::Tonemap,
        Stage::Overlay,
    ];
    let index = all.iter().position(|candidate| *candidate == stage).unwrap_or(0);
    u8::try_from(index).unwrap_or(u8::MAX)
}

fn view_snapshot(index: usize, capture: &GraphCapture) -> ViewSnapshot {
    let mut aliases: HashMap<TextureId, Vec<usize>> = HashMap::new();
    for (resource, entry) in capture.resources.iter().enumerate() {
        if let Some(texture) = entry.physical {
            aliases.entry(texture).or_default().push(resource);
        }
    }
    let mut groups: HashMap<TextureId, usize> = HashMap::new();
    let barrier = |barrier: &BarrierCapture| BarrierSnapshot {
        resources: aliases.get(&barrier.texture).cloned().unwrap_or_default(),
        access: access_names(barrier.access),
        range: range_text(barrier.range),
    };
    let passes = capture
        .passes
        .iter()
        .map(|pass| PassSnapshot {
            name: pass.name.to_owned(),
            stage: format!("{:?}", pass.stage),
            stage_rank: stage_rank(pass.stage),
            priority: pass.priority,
            order: pass.order,
            side_effect: pass.side_effect,
            accesses: pass
                .accesses
                .iter()
                .map(|access| AccessSnapshot {
                    resource: access.resource,
                    read_version: access.read_version,
                    write_version: access.write_version,
                    access: access_names(access.access),
                    range: range_text(access.range),
                })
                .collect(),
            barriers: pass.barriers.iter().map(barrier).collect(),
        })
        .collect();
    let resources = capture
        .resources
        .iter()
        .map(|resource| {
            let alias_group = match (resource.imported, resource.physical) {
                (false, Some(texture)) => {
                    let next = groups.len();
                    Some(*groups.entry(texture).or_insert(next))
                }
                _ => None,
            };
            ResourceSnapshot {
                name: resource.name.to_owned(),
                imported: resource.imported,
                format: resource.desc.map(|desc| format!("{:?}", desc.format)),
                size: resource.desc.map(|desc| (desc.size.width, desc.size.height)),
                mips: resource.desc.map_or(0, |desc| desc.mip_level_count),
                layers: resource.desc.map_or(0, |desc| desc.array_layers()),
                usage: resource.desc.map(|desc| usage_names(desc.usage)).unwrap_or_default(),
                alias_group,
                lifetime: resource.lifetime,
                versions: resource.versions,
            }
        })
        .collect();
    ViewSnapshot {
        index,
        passes,
        resources,
        edges: capture.edges.clone(),
        final_barriers: capture.final_barriers.iter().map(barrier).collect(),
        stats: StatsSnapshot {
            passes_total: capture.stats.passes_total,
            passes_live: capture.stats.passes_live,
            logical_textures: capture.stats.logical_textures,
            physical_textures: capture.stats.physical_textures,
        },
    }
}

// Converts what the renderer reported for one frame into what the devtools show. Needs `set_capture(true)` on the
// renderer, otherwise the views are empty.
pub fn snapshot(frame: u64, frame_ms: f32, renderer: &Renderer, report: &FrameReport) -> Snapshot {
    let features = renderer
        .feature_details()
        .into_iter()
        .map(|info| {
            let (status, code, reason) = match info.status {
                FeatureStatus::Active => ("aktywna".to_owned(), None, None),
                FeatureStatus::Fallback { reason } => ("fallback".to_owned(), Some("F0103".to_owned()), Some(reason)),
                FeatureStatus::Disabled { code, reason } => {
                    ("wyłączona".to_owned(), Some(code.to_owned()), Some(reason))
                }
            };
            FeatureSnapshot {
                name: info.name.to_owned(),
                status,
                code,
                reason,
                stage: format!("{:?}", info.stage),
                priority: info.priority,
                role: info.role.map(|role| role.name().to_owned()),
                order: info.order,
            }
        })
        .collect();
    let diagnostics = report
        .diagnostics
        .iter()
        .map(|diagnostic| DiagnosticSnapshot {
            frame,
            code: diagnostic.code.to_owned(),
            severity: match diagnostic.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Note => "note",
            }
            .to_owned(),
            message: diagnostic.message.clone(),
            hint: diagnostic.hint.clone(),
        })
        .collect();
    Snapshot {
        frame,
        frame_ms,
        views: report
            .captures
            .iter()
            .enumerate()
            .map(|(index, capture)| view_snapshot(index, capture))
            .collect(),
        features,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_read_like_prose() {
        assert_eq!(range_text(SubresourceRange::ALL), "wszystko");
        assert_eq!(range_text(SubresourceRange::mip(2)), "mip 2");
    }
}
