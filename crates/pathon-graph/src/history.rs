use std::collections::HashMap;

use pathon_resources::DeferredRelease;
use pathon_rhi_types::{Access, Extent3d, TextureDesc, TextureId, TextureUsage};

use crate::graph::{GraphTextureDesc, TextureHandle};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct HistoryTexture {
    pub previous: TextureHandle,
    pub current: TextureHandle,
    pub previous_valid: bool,
}

#[derive(Debug)]
pub(crate) struct HistoryEntry {
    pub(crate) desc: GraphTextureDesc,
    pub(crate) texture_desc: TextureDesc,
    pub(crate) previous: TextureId,
    pub(crate) current: TextureId,
    pub(crate) previous_states: Vec<Access>,
    pub(crate) current_states: Vec<Access>,
}

impl HistoryEntry {
    pub(crate) fn fits(&self, desc: GraphTextureDesc, extent: Extent3d, usage: TextureUsage) -> bool {
        self.desc == desc && self.texture_desc.size == extent && self.texture_desc.usage.contains(usage)
    }

    pub(crate) fn swap(&mut self) {
        std::mem::swap(&mut self.previous, &mut self.current);
        std::mem::swap(&mut self.previous_states, &mut self.current_states);
    }
}

// Textures that outlive one frame graph: a previous/current pair per name, with the `Access` state each was left in.
#[derive(Debug, Default)]
pub struct HistoryStore {
    entries: HashMap<&'static str, HistoryEntry>,
    created: usize,
}

impl HistoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn created_total(&self) -> usize {
        self.created
    }

    pub fn textures(&self) -> impl Iterator<Item = TextureId> + '_ {
        self.entries.values().flat_map(|entry| [entry.previous, entry.current])
    }

    // After device loss every handle is dead; drop them without destroying.
    pub fn forget_all(&mut self) {
        self.entries.clear();
    }

    pub fn release_all(&mut self, release: &mut DeferredRelease, frame: u64) {
        for (_, entry) in self.entries.drain() {
            release.release(frame, entry.previous);
            release.release(frame, entry.current);
        }
    }

    pub(crate) fn signatures(&self) -> Vec<(&'static str, GraphTextureDesc, Extent3d)> {
        self.entries
            .iter()
            .map(|(name, entry)| (*name, entry.desc, entry.texture_desc.size))
            .collect()
    }

    pub(crate) fn entry_mut(&mut self, name: &'static str) -> Option<&mut HistoryEntry> {
        self.entries.get_mut(name)
    }

    pub(crate) fn remove(&mut self, name: &'static str) -> Option<HistoryEntry> {
        self.entries.remove(name)
    }

    pub(crate) fn insert(&mut self, name: &'static str, entry: HistoryEntry) {
        self.created += 2;
        self.entries.insert(name, entry);
    }
}
