use std::fmt;

use pathon_api::resources::{DeferredRelease, GpuArray};
use pathon_api::rhi::{BufferId, DeviceApi, RhiError};

use crate::{MaterialData, MaterialError, MaterialVariant, StandardMaterial};

/// Logical index of a material: the element of the GPU array a shader reads, and what the bindless layer (M4-04)
/// maps to a `binding_array` slot or a per-material bind group.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MaterialId(u32);

impl MaterialId {
    pub fn index(self) -> u32 {
        self.0
    }
}

impl fmt::Display for MaterialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "material {}", self.0)
    }
}

#[derive(Debug)]
struct Entry {
    variant: MaterialVariant,
    live: bool,
}

/// The material array on the GPU: `array<MaterialData>` as a storage buffer, indexed by [`MaterialId`].
///
/// A removed index is reused only after `frames_in_flight` frames, because a recorded list may still read it
/// ([ADR-0022](../../../docs/adr/0022-zegar-klatki-paczek.md)).
#[derive(Debug)]
pub struct MaterialTable {
    materials: GpuArray<MaterialData>,
    entries: Vec<Entry>,
    free: Vec<u32>,
    pending: Vec<(u64, u32)>,
    frames_in_flight: u64,
}

impl MaterialTable {
    pub fn new(label: &'static str, frames_in_flight: u32) -> Self {
        Self {
            materials: GpuArray::new(label),
            entries: Vec::new(),
            free: Vec::new(),
            pending: Vec::new(),
            frames_in_flight: u64::from(frames_in_flight),
        }
    }

    pub fn add(&mut self, material: &StandardMaterial) -> Result<MaterialId, MaterialError> {
        let data = MaterialData::from_material(material)?;
        let entry = Entry {
            variant: MaterialVariant::of(material),
            live: true,
        };
        let index = if let Some(index) = self.free.pop() {
            self.materials.set(index as usize, data);
            self.entries[index as usize] = entry;
            index
        } else {
            let index = self.materials.push(data);
            self.entries.push(entry);
            u32::try_from(index).expect("a material table never holds 2^32 materials")
        };
        Ok(MaterialId(index))
    }

    /// Replaces the material under `id`; the shader permutation it needs may change with it.
    pub fn set(&mut self, id: MaterialId, material: &StandardMaterial) -> Result<(), MaterialError> {
        self.check_live(id)?;
        let data = MaterialData::from_material(material)?;
        self.materials.set(id.0 as usize, data);
        self.entries[id.0 as usize].variant = MaterialVariant::of(material);
        Ok(())
    }

    /// Frees the index for reuse once `frames_in_flight` frames have passed (see [`Self::begin_frame`]).
    pub fn remove(&mut self, id: MaterialId, frame: u64) -> Result<(), MaterialError> {
        self.check_live(id)?;
        self.entries[id.0 as usize].live = false;
        self.pending.push((frame, id.0));
        Ok(())
    }

    /// Call once per frame: indices removed `frames_in_flight` or more frames ago become reusable.
    pub fn begin_frame(&mut self, frame: u64) {
        let frames_in_flight = self.frames_in_flight;
        let free = &mut self.free;
        self.pending.retain(|&(removed_at, index)| {
            let reusable = removed_at + frames_in_flight <= frame;
            if reusable {
                free.push(index);
            }
            !reusable
        });
    }

    pub fn get(&self, id: MaterialId) -> Option<&MaterialData> {
        self.check_live(id).ok()?;
        self.materials.get(id.0 as usize)
    }

    /// The permutation `id` needs, to choose or merge the pipeline that draws it.
    pub fn variant(&self, id: MaterialId) -> Option<MaterialVariant> {
        self.check_live(id).ok()?;
        Some(self.entries[id.0 as usize].variant)
    }

    /// Slots including those waiting for reuse; the length of the array shaders see.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn live_count(&self) -> usize {
        self.entries.iter().filter(|entry| entry.live).count()
    }

    pub fn buffer(&self) -> Option<BufferId> {
        self.materials.buffer()
    }

    /// Uploads the changed range; the returned buffer may be a new one after growth.
    pub fn sync(
        &mut self,
        device: &dyn DeviceApi,
        release: &mut DeferredRelease,
        frame: u64,
    ) -> Result<BufferId, RhiError> {
        self.materials.sync(device, release, frame)
    }

    /// After device loss: the next `sync` creates the buffer again and uploads every material.
    pub fn forget_gpu_buffer(&mut self) {
        self.materials.forget_gpu_buffer();
    }

    fn check_live(&self, id: MaterialId) -> Result<(), MaterialError> {
        match self.entries.get(id.0 as usize) {
            Some(entry) if entry.live => Ok(()),
            _ => Err(MaterialError::UnknownMaterial { index: id.0 }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AlphaMode;

    fn material(roughness: f32) -> StandardMaterial {
        StandardMaterial {
            roughness,
            ..StandardMaterial::default()
        }
    }

    #[test]
    fn ids_are_consecutive_indices() {
        let mut table = MaterialTable::new("materials", 2);

        let first = table.add(&material(0.1)).expect("valid");
        let second = table.add(&material(0.2)).expect("valid");

        assert_eq!((first.index(), second.index()), (0, 1));
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn a_removed_index_is_reused_only_after_the_frames_in_flight() {
        let mut table = MaterialTable::new("materials", 2);
        let first = table.add(&material(0.1)).expect("valid");
        table.add(&material(0.2)).expect("valid");

        table.remove(first, 10).expect("live");
        assert!(table.get(first).is_none());
        table.begin_frame(11);
        assert_eq!(table.add(&material(0.3)).expect("valid").index(), 2, "still in flight");
        table.begin_frame(12);
        assert_eq!(table.add(&material(0.4)).expect("valid").index(), 0, "recycled");
        assert_eq!(table.live_count(), 3);
    }

    #[test]
    fn a_removed_or_foreign_id_is_y0002() {
        let mut table = MaterialTable::new("materials", 2);
        let id = table.add(&material(0.1)).expect("valid");
        table.remove(id, 0).expect("live");

        for result in [
            table.remove(id, 1),
            table.set(id, &material(0.5)),
            table.set(MaterialId(99), &material(0.5)),
        ] {
            let error = result.expect_err("unknown material");
            assert_eq!(error.code(), "Y0002");
        }
    }

    #[test]
    fn set_updates_the_variant_the_material_needs() {
        let mut table = MaterialTable::new("materials", 2);
        let id = table.add(&material(0.1)).expect("valid");
        assert_eq!(table.variant(id), Some(MaterialVariant::default()));

        table
            .set(
                id,
                &StandardMaterial {
                    alpha_mode: AlphaMode::Blend,
                    ..StandardMaterial::default()
                },
            )
            .expect("live");

        assert_eq!(
            table.variant(id).map(|variant| variant.alpha),
            Some(crate::AlphaKind::Transparent)
        );
    }

    #[test]
    fn an_invalid_material_does_not_take_a_slot() {
        let mut table = MaterialTable::new("materials", 2);

        assert!(table.add(&material(7.0)).is_err());
        assert!(table.is_empty());
    }
}
