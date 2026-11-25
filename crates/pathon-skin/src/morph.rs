use pathon_api::rhi::{BufferDesc, BufferId, BufferUsage, DeviceApi, Resource};

use crate::error::SkinError;

/// Position, normal and tangent offsets of one blend shape from the rest pose. An empty stream means zero deltas.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct MorphTarget {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
}

impl MorphTarget {
    pub fn positions(positions: Vec<[f32; 3]>) -> Self {
        Self {
            positions,
            ..Self::default()
        }
    }

    /// The three deltas of one vertex.
    pub fn delta(&self, vertex: usize) -> [[f32; 3]; 3] {
        let at = |stream: &[[f32; 3]]| stream.get(vertex).copied().unwrap_or([0.0; 3]);
        [at(&self.positions), at(&self.normals), at(&self.tangents)]
    }
}

/// Deltas of every target of one mesh in a storage buffer, dense: `[target][vertex][position, normal, tangent]`, each a
/// `vec4<f32>` (w unused). Sparse storage would save memory for localized shapes (faces) but needs a lookup per
/// vertex; it can replace this buffer without touching the instance API.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MorphTargets {
    pub(crate) buffer: BufferId,
    vertex_count: u32,
    target_count: u32,
}

const VECTORS_PER_DELTA: u64 = 3;
const VECTOR_BYTES: u64 = 16;

impl MorphTargets {
    pub fn upload(
        device: &dyn DeviceApi,
        label: &'static str,
        vertex_count: u32,
        targets: &[MorphTarget],
    ) -> Result<Self, SkinError> {
        let invalid = |reason: String| SkinError::InvalidMorph { label, reason };
        if vertex_count == 0 || targets.is_empty() {
            return Err(invalid("a morph set needs vertices and at least one target".to_owned()));
        }
        let limits = &device.caps().limits;
        let largest = limits.max_buffer_size.min(limits.max_storage_buffer_binding_size);
        let size = targets.len() as u64 * u64::from(vertex_count) * VECTORS_PER_DELTA * VECTOR_BYTES;
        if size > largest {
            return Err(invalid(format!(
                "{} targets of {vertex_count} vertices need {size} bytes, at most {largest} fit",
                targets.len()
            )));
        }
        let mut packed = Vec::with_capacity(usize::try_from(size).unwrap_or_default() / 4);
        for (index, target) in targets.iter().enumerate() {
            for (name, stream) in [
                ("positions", &target.positions),
                ("normals", &target.normals),
                ("tangents", &target.tangents),
            ] {
                if !stream.is_empty() && stream.len() != vertex_count as usize {
                    return Err(invalid(format!(
                        "target {index} has {} {name}, expected 0 or {vertex_count}",
                        stream.len()
                    )));
                }
                if stream.iter().flatten().any(|value| !value.is_finite()) {
                    return Err(invalid(format!("target {index} has a non-finite delta in {name}")));
                }
            }
            for vertex in 0..vertex_count as usize {
                for delta in target.delta(vertex) {
                    packed.extend_from_slice(&[delta[0], delta[1], delta[2], 0.0]);
                }
            }
        }
        let buffer = device.create_buffer(&BufferDesc {
            label,
            size,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        })?;
        device.write_buffer(buffer, 0, bytemuck::cast_slice(&packed))?;
        Ok(Self {
            buffer,
            vertex_count,
            target_count: u32::try_from(targets.len()).map_err(|_| invalid("too many targets".to_owned()))?,
        })
    }

    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    pub fn vertex_count(&self) -> u32 {
        self.vertex_count
    }

    pub fn target_count(&self) -> u32 {
        self.target_count
    }

    /// Hand the buffer to `RecordCtx::release_later` instead when a frame in flight may still read it.
    pub fn release(self, device: &dyn DeviceApi) {
        device.destroy(Resource::Buffer(self.buffer));
    }
}

#[cfg(test)]
mod tests {
    use pathon_rhi::NullDevice;

    use super::*;

    #[test]
    fn deltas_are_packed_densely_and_bad_data_is_rejected() {
        let device = NullDevice::new();
        let target = MorphTarget::positions(vec![[1.0, 0.0, 0.0]; 3]);
        let targets = MorphTargets::upload(&device, "morph", 3, &[target.clone(), target]).expect("upload");
        assert_eq!((targets.vertex_count(), targets.target_count()), (3, 2));

        let short = MorphTarget::positions(vec![[0.0; 3]; 2]);
        assert!(matches!(
            MorphTargets::upload(&device, "short", 3, &[short]),
            Err(SkinError::InvalidMorph { .. })
        ));
        let infinite = MorphTarget::positions(vec![[f32::INFINITY, 0.0, 0.0]; 3]);
        assert!(MorphTargets::upload(&device, "infinite", 3, &[infinite]).is_err());
        assert!(MorphTargets::upload(&device, "none", 3, &[]).is_err());
    }
}
