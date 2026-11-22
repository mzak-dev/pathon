use std::marker::PhantomData;

use pathon_api::rhi::{Id, RawHandle};

use crate::error::SceneError;
use crate::instance::slot_index;

// Few objects with generational handles (cameras, lights), stored by slot. Writes mark the slot changed; the list of
// changed slots is what the other buffer copies on catch-up and what a consumer uploads.
#[derive(Clone, Debug)]
pub struct Table<I, T> {
    generation: Vec<u32>,
    values: Vec<Option<T>>,
    changed: Vec<bool>,
    changes: Vec<u32>,
    live: usize,
    _id: PhantomData<fn() -> I>,
}

impl<I, T> Default for Table<I, T> {
    fn default() -> Self {
        Self {
            generation: Vec::new(),
            values: Vec::new(),
            changed: Vec::new(),
            changes: Vec::new(),
            live: 0,
            _id: PhantomData,
        }
    }
}

impl<I: Id, T> Table<I, T> {
    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    pub fn get(&self, id: I) -> Option<&T> {
        let raw = id.raw();
        let slot = raw.index() as usize;
        if *self.generation.get(slot)? != raw.generation() {
            return None;
        }
        self.values.get(slot)?.as_ref()
    }

    pub fn iter(&self) -> impl Iterator<Item = (I, &T)> {
        self.values.iter().enumerate().filter_map(|(slot, value)| {
            let slot = u32::try_from(slot).ok()?;
            Some((
                I::from_raw(RawHandle::new(slot, self.generation[slot as usize])),
                value.as_ref()?,
            ))
        })
    }

    // Slots written since the previous extract (spawned, changed or despawned).
    pub fn changes(&self) -> &[u32] {
        &self.changes
    }

    pub fn value_at(&self, slot: u32) -> Option<&T> {
        self.values.get(slot as usize)?.as_ref()
    }

    pub fn id_at(&self, slot: u32) -> Option<I> {
        self.value_at(slot)?;
        Some(I::from_raw(RawHandle::new(slot, self.generation[slot as usize])))
    }

    fn resolve(&self, id: I) -> Result<usize, SceneError> {
        let raw = id.raw();
        let slot = raw.index() as usize;
        match (self.generation.get(slot), self.values.get(slot)) {
            (Some(&generation), Some(Some(_))) if generation == raw.generation() => Ok(slot),
            _ => Err(SceneError::StaleHandle { kind: I::KIND }),
        }
    }

    fn mark_changed(&mut self, slot: usize) {
        if !self.changed[slot] {
            self.changed[slot] = true;
            self.changes.push(slot_index(slot));
        }
    }

    pub(crate) fn insert(&mut self, free: &mut Vec<u32>, value: T) -> I {
        let slot = if let Some(slot) = free.pop() {
            slot as usize
        } else {
            self.generation.push(0);
            self.values.push(None);
            self.changed.push(false);
            self.values.len() - 1
        };
        self.values[slot] = Some(value);
        self.live += 1;
        self.mark_changed(slot);
        I::from_raw(RawHandle::new(slot_index(slot), self.generation[slot]))
    }

    pub(crate) fn remove(&mut self, free: &mut Vec<u32>, id: I) -> Result<(), SceneError> {
        let slot = self.resolve(id)?;
        self.values[slot] = None;
        self.generation[slot] = self.generation[slot].wrapping_add(1);
        self.live -= 1;
        free.push(slot_index(slot));
        self.mark_changed(slot);
        Ok(())
    }

    // `update` returns whether it changed the value; only then the slot is marked.
    pub(crate) fn update(&mut self, id: I, update: impl FnOnce(&mut T) -> bool) -> Result<bool, SceneError> {
        let slot = self.resolve(id)?;
        let value = self.values[slot].as_mut().expect("resolve checked the slot is live");
        let changed = update(value);
        if changed {
            self.mark_changed(slot);
        }
        Ok(changed)
    }

    // Start of an extract: `settle` runs on every slot the previous extract wrote.
    pub(crate) fn settle_changed_in(&mut self, front: &Self, mut settle: impl FnMut(&mut T) -> bool) -> usize {
        let mut settled = 0;
        for &slot in &front.changes {
            let slot = slot as usize;
            if let Some(value) = self.values[slot].as_mut()
                && settle(value)
            {
                self.mark_changed(slot);
                settled += 1;
            }
        }
        settled
    }

    pub(crate) fn clear_changes(&mut self) {
        for slot in self.changes.drain(..) {
            self.changed[slot as usize] = false;
        }
    }

    // Brings this (older) copy to the state of `front` by copying the slots `front` changed.
    pub(crate) fn catch_up(&mut self, front: &Self)
    where
        T: Clone,
    {
        self.generation.resize(front.generation.len(), 0);
        self.values.resize_with(front.values.len(), || None);
        self.changed.resize(front.values.len(), false);
        for &slot in &front.changes {
            let slot = slot as usize;
            self.generation[slot] = front.generation[slot];
            self.values[slot].clone_from(&front.values[slot]);
        }
        self.live = front.live;
    }

    pub(crate) fn free_slots(&self) -> Vec<u32> {
        let free = (0..self.values.len()).rev().filter(|&slot| self.values[slot].is_none());
        free.map(slot_index).collect()
    }
}

#[cfg(test)]
mod tests {
    use pathon_api::rhi::BufferId;

    use super::*;

    type Numbers = Table<BufferId, u32>;

    #[test]
    fn a_removed_handle_does_not_reach_the_reused_slot() {
        let mut table = Numbers::default();
        let mut free = Vec::new();
        let first = table.insert(&mut free, 1);
        table.remove(&mut free, first).expect("live");
        let second = table.insert(&mut free, 2);

        assert_eq!(first.raw().index(), second.raw().index());
        assert_eq!(table.get(first), None);
        assert_eq!(table.get(second), Some(&2));
        assert!(matches!(
            table.remove(&mut free, first),
            Err(SceneError::StaleHandle { kind: "buffer" })
        ));
    }

    #[test]
    fn an_update_that_changes_nothing_is_not_a_change() {
        let mut table = Numbers::default();
        let mut free = Vec::new();
        let id = table.insert(&mut free, 5);
        table.clear_changes();

        assert_eq!(table.update(id, |_| false), Ok(false));
        assert_eq!(table.changes().len(), 0);
        assert_eq!(
            table.update(id, |value| {
                *value = 6;
                true
            }),
            Ok(true)
        );
        assert_eq!(table.changes(), [id.raw().index()]);
    }

    #[test]
    fn catch_up_copies_only_what_the_front_changed() {
        let mut front = Numbers::default();
        let mut free = Vec::new();
        let kept = front.insert(&mut free, 1);
        let gone = front.insert(&mut free, 2);
        let mut back = front.clone();
        back.clear_changes();
        front.clear_changes();
        front.remove(&mut free, gone).expect("live");
        let added = front.insert(&mut free, 9);
        front
            .update(kept, |value| {
                *value = 10;
                true
            })
            .expect("live");

        back.catch_up(&front);

        assert_eq!(back.get(kept), Some(&10));
        assert_eq!(back.get(gone), None);
        assert_eq!(back.get(added), Some(&9));
        assert_eq!(back.len(), 2);
    }

    #[test]
    fn free_slots_are_rebuilt_lowest_first() {
        let mut table = Numbers::default();
        let mut free = Vec::new();
        let ids: Vec<_> = (0..4).map(|value| table.insert(&mut free, value)).collect();
        table.remove(&mut free, ids[1]).expect("live");
        table.remove(&mut free, ids[3]).expect("live");

        assert_eq!(table.free_slots(), [3, 1]);
    }
}
