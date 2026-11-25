use pathon_api::glam::Mat4;
use pathon_api::graph::BufferHandle;
use pathon_api::rhi::BufferId;
use pathon_mesh::{DeformedRange, GeometryPool, MeshId, VertexLayout};

use crate::deformed::SkinTargets;
use crate::error::SkinError;
use crate::morph::MorphTargets;

/// Where the rest pose of one mesh lives: the buffers of its pool chunk and its `base_vertex`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct SkinSource {
    pub positions: BufferId,
    pub surface: BufferId,
    /// `None` for a mesh with no skinning stream (morphs only).
    pub skin: Option<BufferId>,
    pub base_vertex: u32,
    pub vertex_count: u32,
}

impl SkinSource {
    /// Reads the location of `mesh` from a pool with the `standard()` or `skinned()` layout.
    pub fn from_pool(pool: &GeometryPool, mesh: MeshId) -> Result<Self, SkinError> {
        let invalid = |reason: &str| SkinError::InvalidInstance {
            reason: reason.to_owned(),
        };
        let skinned = if pool.layout() == &VertexLayout::skinned() {
            true
        } else if pool.layout() == &VertexLayout::standard() {
            false
        } else {
            return Err(invalid("the pool layout is neither standard() nor skinned()"));
        };
        let location = pool
            .mesh(mesh)
            .ok_or_else(|| invalid("unknown or removed mesh"))?
            .location;
        let buffers = pool
            .vertex_buffers(location.vertex_chunk)
            .ok_or_else(|| invalid("the mesh's chunk has no buffers"))?;
        Ok(Self {
            positions: buffers[0],
            surface: buffers[1],
            skin: skinned.then(|| buffers[2]),
            base_vertex: location.base_vertex,
            vertex_count: location.vertex_count,
        })
    }
}

/// Matrices `first..first + count` of [`SkinFrame::bone_matrices`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BoneSlice {
    pub first: u32,
    pub count: u32,
}

/// Weights `first_weight..first_weight + targets.target_count()` of [`SkinFrame::morph_weights`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MorphBinding {
    pub targets: MorphTargets,
    pub first_weight: u32,
}

/// Whether the other bank of the deformed buffers holds this instance's pose from the previous frame.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum MotionHistory {
    /// First frame, after a skipped frame or a cut: both banks get the current pose, so motion is zero.
    #[default]
    Fresh,
    /// The instance was skinned in the previous frame: the other bank is its previous pose.
    Continuing,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct SkinInstance {
    pub source: SkinSource,
    pub range: DeformedRange,
    pub bones: Option<BoneSlice>,
    pub morph: Option<MorphBinding>,
    pub history: MotionHistory,
}

impl SkinInstance {
    pub fn new(source: SkinSource, range: DeformedRange) -> Self {
        Self {
            source,
            range,
            bones: None,
            morph: None,
            history: MotionHistory::Fresh,
        }
    }

    #[must_use]
    pub fn with_bones(self, first: u32, count: u32) -> Self {
        Self {
            bones: Some(BoneSlice { first, count }),
            ..self
        }
    }

    #[must_use]
    pub fn with_morph(self, targets: MorphTargets, first_weight: u32) -> Self {
        Self {
            morph: Some(MorphBinding { targets, first_weight }),
            ..self
        }
    }

    #[must_use]
    pub fn continuing(self) -> Self {
        Self {
            history: MotionHistory::Continuing,
            ..self
        }
    }

    pub(crate) fn validate(&self, frame: &SkinFrame) -> Result<(), SkinError> {
        let invalid = |reason: String| Err(SkinError::InvalidInstance { reason });
        let count = self.source.vertex_count;
        if count == 0 || self.range.vertex_count != count {
            return invalid(format!(
                "the deformed range has {} vertices, the mesh {count}",
                self.range.vertex_count
            ));
        }
        if u64::from(self.range.first_vertex) + u64::from(count) > u64::from(frame.targets.bank_capacity) {
            return invalid(format!(
                "the deformed range ends past the bank capacity of {}",
                frame.targets.bank_capacity
            ));
        }
        match (self.source.skin, self.bones) {
            (Some(_), None) => {
                return invalid("the mesh has a skinning stream but the instance has no bones".to_owned());
            }
            (None, Some(_)) => return invalid("the instance has bones but the mesh has no skinning stream".to_owned()),
            (Some(_), Some(bones))
                if bones.count == 0
                    || u64::from(bones.first) + u64::from(bones.count) > frame.bone_matrices.len() as u64 =>
            {
                return invalid(format!(
                    "bones {}..+{} are outside the {} matrices of the frame",
                    bones.first,
                    bones.count,
                    frame.bone_matrices.len()
                ));
            }
            _ => {}
        }
        if let Some(morph) = self.morph {
            if morph.targets.vertex_count() != count {
                return invalid(format!(
                    "the morph targets have {} vertices, the mesh {count}",
                    morph.targets.vertex_count()
                ));
            }
            if u64::from(morph.first_weight) + u64::from(morph.targets.target_count())
                > frame.morph_weights.len() as u64
            {
                return invalid(format!(
                    "weights {}..+{} are outside the {} weights of the frame",
                    morph.first_weight,
                    morph.targets.target_count(),
                    frame.morph_weights.len()
                ));
            }
        }
        Ok(())
    }
}

/// Everything skinned in one frame; the application puts it in `Shared` before `render_frame` and replaces it every
/// frame an instance should be deformed.
///
/// `bone_matrices` are final skinning matrices in object space (joint pose times inverse bind), column-major like
/// `Mat4`; instances index slices of them, so shared skeletons and instance palettes live in one vector.
#[derive(Clone, PartialEq, Debug)]
#[non_exhaustive]
pub struct SkinFrame {
    pub targets: SkinTargets,
    pub instances: Vec<SkinInstance>,
    pub bone_matrices: Vec<Mat4>,
    pub morph_weights: Vec<f32>,
}

impl SkinFrame {
    pub fn new(targets: SkinTargets) -> Self {
        Self {
            targets,
            instances: Vec::new(),
            bone_matrices: Vec::new(),
            morph_weights: Vec::new(),
        }
    }
}

/// Published in `Shared` by `SkinPack` in every view of a frame that skinned something: graph handles of the deformed
/// buffers, so a later pass declares `read_buffer(.., Access::VERTEX)` and the graph orders it after the compute pass.
///
/// In frame `frame` the current pose is bank `frame & 1` of `positions`, the previous pose the other one
/// (`DeformedPositions::current_offset` / `previous_offset`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct SkinOutput {
    pub positions: BufferHandle,
    pub surface: BufferHandle,
    pub bank_capacity: u32,
    pub frame: u64,
}
