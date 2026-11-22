use bitflags::bitflags;
use pathon_api::glam::Affine3A;
use pathon_api::rhi::{Id, RawHandle};

use crate::bounds::Aabb;
use crate::error::SceneError;
use crate::handle::{InstanceId, MaterialHandle, MeshHandle};
use crate::layer::LayerMask;

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    pub struct InstanceFlags: u8 {
        const VISIBLE = 1;
        const CASTS_SHADOWS = 1 << 1;
    }
}

impl Default for InstanceFlags {
    fn default() -> Self {
        Self::all()
    }
}

bitflags! {
    // What changed in a slot during the extract that produced the snapshot.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct Dirty: u8 {
        const SPAWNED = 1;
        const DESPAWNED = 1 << 1;
        // The model matrix or the previous one (the latter also changes when an object stops moving).
        const TRANSFORM = 1 << 2;
        const MESH = 1 << 3;
        const MATERIAL = 1 << 4;
        const LAYERS = 1 << 5;
        const FLAGS = 1 << 6;
        const BOUNDS = 1 << 7;
    }
}

// What the host writes for a new instance.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct InstanceDesc {
    pub mesh: MeshHandle,
    pub material: MaterialHandle,
    // Object to world.
    pub transform: Affine3A,
    pub layers: LayerMask,
    pub flags: InstanceFlags,
    // Mesh-space bounds; `Aabb::UNBOUNDED` when the host does not know them.
    pub local_bounds: Aabb,
}

impl InstanceDesc {
    pub fn new(mesh: MeshHandle, material: MaterialHandle, transform: Affine3A) -> Self {
        Self {
            mesh,
            material,
            transform,
            layers: LayerMask::DEFAULT,
            flags: InstanceFlags::default(),
            local_bounds: Aabb::UNBOUNDED,
        }
    }

    #[must_use]
    pub fn with_layers(self, layers: LayerMask) -> Self {
        Self { layers, ..self }
    }

    #[must_use]
    pub fn with_flags(self, flags: InstanceFlags) -> Self {
        Self { flags, ..self }
    }

    #[must_use]
    pub fn with_local_bounds(self, local_bounds: Aabb) -> Self {
        Self { local_bounds, ..self }
    }
}

// Structure of arrays: one vector per property, indexed by slot, so a consumer reads or uploads exactly the columns it
// needs and a transform update touches one column. Slots are reused; `generation` tells a live handle from a stale one.
#[derive(Clone, Debug, Default)]
pub struct Instances {
    generation: Vec<u32>,
    alive: Vec<bool>,
    transform: Vec<Affine3A>,
    // Last extract's model matrix: the motion vector input (ADR-0027 `ObjectTransforms::previous_model`).
    previous_transform: Vec<Affine3A>,
    mesh: Vec<MeshHandle>,
    material: Vec<MaterialHandle>,
    layers: Vec<LayerMask>,
    flags: Vec<InstanceFlags>,
    local_bounds: Vec<Aabb>,
    dirty: Vec<Dirty>,
    changes: Vec<u32>,
    live: usize,
}

impl Instances {
    // Number of slots, live or free: the length of every column.
    pub fn slot_count(&self) -> usize {
        self.generation.len()
    }

    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    pub fn slot_of(&self, id: InstanceId) -> Option<usize> {
        let raw = id.raw();
        let slot = raw.index() as usize;
        let alive = *self.alive.get(slot)?;
        (alive && self.generation[slot] == raw.generation()).then_some(slot)
    }

    pub fn contains(&self, id: InstanceId) -> bool {
        self.slot_of(id).is_some()
    }

    pub fn id_at(&self, slot: usize) -> Option<InstanceId> {
        if !*self.alive.get(slot)? {
            return None;
        }
        Some(InstanceId::from_raw(RawHandle::new(
            slot_index(slot),
            self.generation[slot],
        )))
    }

    pub fn is_alive(&self, slot: usize) -> bool {
        self.alive.get(slot).copied().unwrap_or(false)
    }

    pub fn transforms(&self) -> &[Affine3A] {
        &self.transform
    }

    pub fn previous_transforms(&self) -> &[Affine3A] {
        &self.previous_transform
    }

    pub fn meshes(&self) -> &[MeshHandle] {
        &self.mesh
    }

    pub fn materials(&self) -> &[MaterialHandle] {
        &self.material
    }

    pub fn layers(&self) -> &[LayerMask] {
        &self.layers
    }

    pub fn flags(&self) -> &[InstanceFlags] {
        &self.flags
    }

    pub fn local_bounds(&self) -> &[Aabb] {
        &self.local_bounds
    }

    pub fn world_bounds(&self, slot: usize) -> Option<Aabb> {
        Some(self.local_bounds.get(slot)?.transformed(&self.transform[slot]))
    }

    // Slots written by the extract that produced this world (spawned, changed, despawned or settled), once each.
    pub fn changes(&self) -> &[u32] {
        &self.changes
    }

    pub fn dirty(&self, slot: usize) -> Dirty {
        self.dirty.get(slot).copied().unwrap_or_default()
    }

    pub fn iter_alive(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.slot_count()).filter(|&slot| self.alive[slot])
    }

    // Live, visible instances whose layers meet `mask` (a camera's or a light's).
    pub fn iter_visible(&self, mask: LayerMask) -> impl Iterator<Item = usize> + '_ {
        (0..self.slot_count()).filter(move |&slot| {
            self.alive[slot] && self.flags[slot].contains(InstanceFlags::VISIBLE) && self.layers[slot].intersects(mask)
        })
    }

    fn resolve(&self, id: InstanceId) -> Result<usize, SceneError> {
        self.slot_of(id)
            .ok_or(SceneError::StaleHandle { kind: InstanceId::KIND })
    }

    fn mark(&mut self, slot: usize, what: Dirty) {
        let dirty = &mut self.dirty[slot];
        if dirty.is_empty() {
            self.changes.push(slot_index(slot));
        }
        *dirty |= what;
    }

    fn push_slot(&mut self) -> usize {
        self.generation.push(0);
        self.alive.push(false);
        self.transform.push(Affine3A::IDENTITY);
        self.previous_transform.push(Affine3A::IDENTITY);
        self.mesh.push(MeshHandle::NONE);
        self.material.push(MaterialHandle::DEFAULT);
        self.layers.push(LayerMask::NONE);
        self.flags.push(InstanceFlags::empty());
        self.local_bounds.push(Aabb::UNBOUNDED);
        self.dirty.push(Dirty::empty());
        self.generation.len() - 1
    }

    pub(crate) fn spawn(&mut self, free: &mut Vec<u32>, desc: &InstanceDesc) -> Result<InstanceId, SceneError> {
        if !desc.transform.is_finite() {
            return Err(SceneError::NotFinite {
                what: "instance transform",
            });
        }
        let slot = free.pop().map_or_else(|| self.push_slot(), |slot| slot as usize);
        self.alive[slot] = true;
        self.transform[slot] = desc.transform;
        self.previous_transform[slot] = desc.transform;
        self.mesh[slot] = desc.mesh;
        self.material[slot] = desc.material;
        self.layers[slot] = desc.layers;
        self.flags[slot] = desc.flags;
        self.local_bounds[slot] = desc.local_bounds;
        self.live += 1;
        self.mark(slot, Dirty::SPAWNED);
        Ok(InstanceId::from_raw(RawHandle::new(
            slot_index(slot),
            self.generation[slot],
        )))
    }

    pub(crate) fn despawn(&mut self, free: &mut Vec<u32>, id: InstanceId) -> Result<(), SceneError> {
        let slot = self.resolve(id)?;
        self.alive[slot] = false;
        self.generation[slot] = self.generation[slot].wrapping_add(1);
        self.live -= 1;
        free.push(slot_index(slot));
        self.mark(slot, Dirty::DESPAWNED);
        Ok(())
    }

    // Returns whether the value differed; an equal value is not written and not marked.
    pub(crate) fn set_transform(&mut self, id: InstanceId, transform: Affine3A) -> Result<bool, SceneError> {
        let slot = self.resolve(id)?;
        if !transform.is_finite() {
            return Err(SceneError::NotFinite {
                what: "instance transform",
            });
        }
        if self.transform[slot] == transform {
            return Ok(false);
        }
        self.transform[slot] = transform;
        self.mark(slot, Dirty::TRANSFORM);
        Ok(true)
    }

    pub(crate) fn set_mesh(&mut self, id: InstanceId, mesh: MeshHandle) -> Result<bool, SceneError> {
        let slot = self.resolve(id)?;
        if self.mesh[slot] == mesh {
            return Ok(false);
        }
        self.mesh[slot] = mesh;
        self.mark(slot, Dirty::MESH);
        Ok(true)
    }

    pub(crate) fn set_material(&mut self, id: InstanceId, material: MaterialHandle) -> Result<bool, SceneError> {
        let slot = self.resolve(id)?;
        if self.material[slot] == material {
            return Ok(false);
        }
        self.material[slot] = material;
        self.mark(slot, Dirty::MATERIAL);
        Ok(true)
    }

    pub(crate) fn set_layers(&mut self, id: InstanceId, layers: LayerMask) -> Result<bool, SceneError> {
        let slot = self.resolve(id)?;
        if self.layers[slot] == layers {
            return Ok(false);
        }
        self.layers[slot] = layers;
        self.mark(slot, Dirty::LAYERS);
        Ok(true)
    }

    pub(crate) fn set_flags(&mut self, id: InstanceId, flags: InstanceFlags) -> Result<bool, SceneError> {
        let slot = self.resolve(id)?;
        if self.flags[slot] == flags {
            return Ok(false);
        }
        self.flags[slot] = flags;
        self.mark(slot, Dirty::FLAGS);
        Ok(true)
    }

    pub(crate) fn set_local_bounds(&mut self, id: InstanceId, bounds: Aabb) -> Result<bool, SceneError> {
        let slot = self.resolve(id)?;
        if self.local_bounds[slot] == bounds {
            return Ok(false);
        }
        self.local_bounds[slot] = bounds;
        self.mark(slot, Dirty::BOUNDS);
        Ok(true)
    }

    // Start of an extract, after catch-up: an object that moved in the previous extract takes its model as the
    // previous one, so one that stopped moving has zero motion. Only those objects are touched. Returns their count.
    pub(crate) fn settle_moved_in(&mut self, front: &Self) -> usize {
        let mut settled = 0;
        for &slot in &front.changes {
            let slot = slot as usize;
            if front.dirty[slot].contains(Dirty::TRANSFORM)
                && self.alive[slot]
                && self.previous_transform[slot] != self.transform[slot]
            {
                self.previous_transform[slot] = self.transform[slot];
                self.mark(slot, Dirty::TRANSFORM);
                settled += 1;
            }
        }
        settled
    }

    pub(crate) fn clear_changes(&mut self) {
        for slot in self.changes.drain(..) {
            self.dirty[slot as usize] = Dirty::empty();
        }
    }

    // Brings this (older) copy to the state of `front`: only the columns `front` marked in the slots it changed.
    pub(crate) fn catch_up(&mut self, front: &Self) {
        while self.slot_count() < front.slot_count() {
            self.push_slot();
        }
        for &slot in &front.changes {
            let slot = slot as usize;
            let what = front.dirty[slot];
            self.generation[slot] = front.generation[slot];
            self.alive[slot] = front.alive[slot];
            if what.intersects(Dirty::SPAWNED | Dirty::DESPAWNED) {
                self.copy_all_columns(front, slot);
                continue;
            }
            if what.contains(Dirty::TRANSFORM) {
                self.transform[slot] = front.transform[slot];
                self.previous_transform[slot] = front.previous_transform[slot];
            }
            if what.contains(Dirty::MESH) {
                self.mesh[slot] = front.mesh[slot];
            }
            if what.contains(Dirty::MATERIAL) {
                self.material[slot] = front.material[slot];
            }
            if what.contains(Dirty::LAYERS) {
                self.layers[slot] = front.layers[slot];
            }
            if what.contains(Dirty::FLAGS) {
                self.flags[slot] = front.flags[slot];
            }
            if what.contains(Dirty::BOUNDS) {
                self.local_bounds[slot] = front.local_bounds[slot];
            }
        }
        self.live = front.live;
    }

    fn copy_all_columns(&mut self, front: &Self, slot: usize) {
        self.transform[slot] = front.transform[slot];
        self.previous_transform[slot] = front.previous_transform[slot];
        self.mesh[slot] = front.mesh[slot];
        self.material[slot] = front.material[slot];
        self.layers[slot] = front.layers[slot];
        self.flags[slot] = front.flags[slot];
        self.local_bounds[slot] = front.local_bounds[slot];
    }

    // Free slots, lowest on top of the stack: the allocator state after a rebuild.
    pub(crate) fn free_slots(&self) -> Vec<u32> {
        (0..self.slot_count())
            .rev()
            .filter(|&slot| !self.alive[slot])
            .map(slot_index)
            .collect()
    }
}

pub(crate) fn slot_index(slot: usize) -> u32 {
    u32::try_from(slot).expect("a scene holds fewer than u32::MAX slots")
}

#[cfg(test)]
mod tests {
    use pathon_api::glam::Vec3;
    use pathon_api::rhi::BufferId;

    use super::*;

    fn mesh(index: u32) -> MeshHandle {
        MeshHandle::from_id(BufferId::from_raw(RawHandle::new(index, 0)))
    }

    fn desc(x: f32) -> InstanceDesc {
        InstanceDesc::new(
            mesh(1),
            MaterialHandle::DEFAULT,
            Affine3A::from_translation(Vec3::new(x, 0.0, 0.0)),
        )
    }

    #[test]
    fn spawning_fills_every_column_without_motion() {
        let mut instances = Instances::default();
        let mut free = Vec::new();

        let id = instances
            .spawn(&mut free, &desc(2.0).with_layers(LayerMask::layer(4)))
            .expect("finite");
        let slot = instances.slot_of(id).expect("live");

        assert_eq!(instances.transforms()[slot], instances.previous_transforms()[slot]);
        assert_eq!(instances.meshes()[slot], mesh(1));
        assert_eq!(instances.layers()[slot], LayerMask::layer(4));
        assert_eq!(instances.dirty(slot), Dirty::SPAWNED);
        assert_eq!(instances.changes(), [0]);
    }

    #[test]
    fn a_stale_handle_is_rejected_after_the_slot_is_reused() {
        let mut instances = Instances::default();
        let mut free = Vec::new();
        let old = instances.spawn(&mut free, &desc(0.0)).expect("finite");
        instances.despawn(&mut free, old).expect("live");
        let new = instances.spawn(&mut free, &desc(1.0)).expect("finite");

        assert_eq!(old.raw().index(), new.raw().index());
        assert!(!instances.contains(old));
        assert!(instances.contains(new));
        assert!(matches!(
            instances.set_transform(old, Affine3A::IDENTITY),
            Err(SceneError::StaleHandle { kind: "instance" })
        ));
        assert!(matches!(
            instances.despawn(&mut free, old),
            Err(SceneError::StaleHandle { .. })
        ));
    }

    #[test]
    fn writing_an_equal_value_is_not_a_change() {
        let mut instances = Instances::default();
        let mut free = Vec::new();
        let id = instances.spawn(&mut free, &desc(1.0)).expect("finite");
        instances.clear_changes();

        assert_eq!(instances.set_transform(id, desc(1.0).transform), Ok(false));
        assert_eq!(instances.set_mesh(id, mesh(1)), Ok(false));
        assert_eq!(instances.set_layers(id, LayerMask::DEFAULT), Ok(false));
        assert_eq!(instances.changes().len(), 0);
    }

    #[test]
    fn several_writes_to_one_slot_list_it_once_with_all_flags() {
        let mut instances = Instances::default();
        let mut free = Vec::new();
        let id = instances.spawn(&mut free, &desc(0.0)).expect("finite");
        instances.clear_changes();

        instances.set_transform(id, desc(1.0).transform).expect("live");
        instances.set_mesh(id, mesh(2)).expect("live");
        instances.set_transform(id, desc(2.0).transform).expect("live");

        assert_eq!(instances.changes(), [0]);
        assert_eq!(instances.dirty(0), Dirty::TRANSFORM | Dirty::MESH);
    }

    #[test]
    fn non_finite_transforms_are_rejected_and_leave_the_old_value() {
        let mut instances = Instances::default();
        let mut free = Vec::new();
        let id = instances.spawn(&mut free, &desc(1.0)).expect("finite");
        let nan = Affine3A::from_translation(Vec3::NAN);

        assert!(matches!(
            instances.set_transform(id, nan),
            Err(SceneError::NotFinite { .. })
        ));
        assert!(matches!(
            instances.spawn(&mut free, &InstanceDesc::new(mesh(1), MaterialHandle::DEFAULT, nan)),
            Err(SceneError::NotFinite { .. })
        ));
        assert_eq!(instances.transforms()[0], desc(1.0).transform);
        assert_eq!(instances.len(), 1);
    }

    #[test]
    fn visibility_needs_the_flag_and_a_shared_layer() {
        let mut instances = Instances::default();
        let mut free = Vec::new();
        let shown = instances.spawn(&mut free, &desc(0.0)).expect("finite");
        let hidden = instances
            .spawn(&mut free, &desc(1.0).with_flags(InstanceFlags::CASTS_SHADOWS))
            .expect("finite");
        instances
            .spawn(&mut free, &desc(2.0).with_layers(LayerMask::layer(2)))
            .expect("finite");

        let visible: Vec<usize> = instances.iter_visible(LayerMask::DEFAULT).collect();

        assert_eq!(visible, [instances.slot_of(shown).expect("live")]);
        assert!(instances.contains(hidden));
        assert_eq!(instances.iter_visible(LayerMask::ALL).count(), 2);
    }

    #[test]
    fn catch_up_reproduces_the_front_from_its_changes() {
        let mut front = Instances::default();
        let mut free = Vec::new();
        let a = front.spawn(&mut free, &desc(0.0)).expect("finite");
        let b = front.spawn(&mut free, &desc(1.0)).expect("finite");
        let mut back = front.clone();
        front.clear_changes();
        back.clear_changes();
        front.set_transform(a, desc(5.0).transform).expect("live");
        front
            .set_material(b, MaterialHandle::from_id(BufferId::from_raw(RawHandle::new(3, 1))))
            .expect("live");
        front.despawn(&mut free, b).expect("live");
        let c = front.spawn(&mut free, &desc(9.0)).expect("finite");
        let d = front.spawn(&mut free, &desc(10.0)).expect("finite");

        back.catch_up(&front);

        assert_eq!(back.slot_count(), front.slot_count());
        assert_eq!(back.len(), front.len());
        for slot in 0..front.slot_count() {
            assert_eq!(back.is_alive(slot), front.is_alive(slot));
            assert_eq!(back.id_at(slot), front.id_at(slot));
            assert_eq!(back.transforms()[slot], front.transforms()[slot]);
            assert_eq!(back.materials()[slot], front.materials()[slot]);
        }
        assert!(back.contains(a) && back.contains(c) && back.contains(d) && !back.contains(b));
    }

    #[test]
    fn settling_gives_an_object_that_stopped_moving_zero_motion() {
        let mut front = Instances::default();
        let mut free = Vec::new();
        let id = front.spawn(&mut free, &desc(0.0)).expect("finite");
        front.clear_changes();
        front.set_transform(id, desc(1.0).transform).expect("live");
        let mut back = front.clone();
        back.clear_changes();

        let settled = back.settle_moved_in(&front);

        assert_eq!(settled, 1);
        assert_eq!(back.previous_transforms()[0], desc(1.0).transform);
        assert_eq!(back.dirty(0), Dirty::TRANSFORM);
        assert_eq!(front.previous_transforms()[0], desc(0.0).transform);
    }

    #[test]
    fn free_slots_are_rebuilt_lowest_first() {
        let mut instances = Instances::default();
        let mut free = Vec::new();
        let ids: Vec<_> = (0..4)
            .map(|x| instances.spawn(&mut free, &desc(x as f32)).expect("finite"))
            .collect();
        instances.despawn(&mut free, ids[0]).expect("live");
        instances.despawn(&mut free, ids[2]).expect("live");

        assert_eq!(instances.free_slots(), [2, 0]);
    }
}
