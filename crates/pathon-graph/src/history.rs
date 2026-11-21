use std::collections::HashMap;

use pathon_resources::DeferredRelease;
use pathon_rhi_types::{
    Access, Color, ColorAttachment, CommandList, DepthAttachment, DeviceApi, Extent3d, FormatKind, RenderPassDesc,
    RhiError, TextureDesc, TextureDimension, TextureId, TextureUsage, TextureViewDesc, TextureViewDimension,
};

use crate::compile::CompiledGraph;
use crate::execute::{PhysicalTexture, StateTracker};
use crate::graph::{FrameGraph, GraphTextureDesc, HistoryDecl, TextureHandle};
use crate::pool::ResourcePool;

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

const HISTORY_ZERO_COLOR: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

impl FrameGraph {
    // A missing, resized or under-used pair is created anew and the old one released. Returns the declarations whose
    // `previous` has no valid contents and is read this frame.
    pub(crate) fn prepare_history(
        &self,
        compiled: &CompiledGraph,
        device: &dyn DeviceApi,
        pool: &mut ResourcePool,
        history: &mut HistoryStore,
        release: &mut DeferredRelease,
        frame: u64,
    ) -> Result<Vec<usize>, RhiError> {
        let mut fresh = Vec::new();
        for (index, decl) in self.histories.iter().enumerate() {
            let extent = decl.desc.extent(compiled.sizes);
            let usage = compiled.usages[decl.previous as usize]
                | compiled.usages[decl.current as usize]
                | TextureUsage::RENDER_TARGET;
            let fits = history
                .entry_mut(decl.name)
                .is_some_and(|entry| entry.fits(decl.desc, extent, usage));
            if fits {
                continue;
            }
            discard_entry(history, decl.name, pool, release, frame);
            let texture_desc = TextureDesc {
                label: decl.name,
                size: extent,
                mip_level_count: decl.desc.mip_level_count,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: decl.desc.format,
                usage,
            };
            let previous = device.create_texture(&texture_desc)?;
            let current = match device.create_texture(&texture_desc) {
                Ok(current) => current,
                Err(error) => {
                    release.release(frame, previous);
                    return Err(error);
                }
            };
            let count = (texture_desc.mip_level_count * texture_desc.array_layers()) as usize;
            history.insert(
                decl.name,
                HistoryEntry {
                    desc: decl.desc,
                    texture_desc,
                    previous,
                    current,
                    previous_states: vec![Access::empty(); count],
                    current_states: vec![Access::empty(); count],
                },
            );
            if compiled.lifetimes[decl.previous as usize].is_some() {
                fresh.push(index);
            }
        }
        Ok(fresh)
    }

    pub(crate) fn bind_history(&self, history: &mut HistoryStore, physical: &mut [Option<PhysicalTexture>]) {
        for decl in &self.histories {
            let entry = history.entry_mut(decl.name).expect("prepared above");
            let desc = entry.texture_desc;
            physical[decl.previous as usize] = Some(PhysicalTexture {
                texture: entry.previous,
                desc,
            });
            physical[decl.current as usize] = Some(PhysicalTexture {
                texture: entry.current,
                desc,
            });
        }
    }

    // Reading a fresh `previous` must see zeros, not whatever memory the allocator handed out.
    pub(crate) fn zero_fresh_history(
        &self,
        fresh: &[usize],
        physical: &[Option<PhysicalTexture>],
        pool: &mut ResourcePool,
        tracker: &mut StateTracker,
        commands: &mut CommandList,
        device: &dyn DeviceApi,
    ) -> Result<(), RhiError> {
        for index in fresh {
            let HistoryDecl { previous, .. } = self.histories[*index];
            let texture = physical[previous as usize].expect("bound above");
            let is_color = texture.desc.format.kind() == FormatKind::Color;
            let attachment = if is_color {
                Access::COLOR_ATTACHMENT
            } else {
                Access::DEPTH_WRITE
            };
            let (mips, layers) = (texture.desc.mip_level_count, texture.desc.array_layers());
            tracker.transition(texture, &vec![attachment; (mips * layers) as usize], commands);
            for mip in 0..mips {
                for layer in 0..layers {
                    let view = pool.view(
                        device,
                        texture.texture,
                        &TextureViewDesc {
                            dimension: Some(TextureViewDimension::D2),
                            base_mip: mip,
                            mip_count: Some(1),
                            base_layer: layer,
                            layer_count: Some(1),
                            ..TextureViewDesc::depth_only(texture.desc.format)
                        },
                    )?;
                    drop(commands.render_pass(RenderPassDesc {
                        label: "zero history",
                        color: if is_color {
                            vec![ColorAttachment::clear(view, HISTORY_ZERO_COLOR)]
                        } else {
                            Vec::new()
                        },
                        depth: (!is_color).then(|| DepthAttachment::clear_reverse_z(view)),
                        timestamps: None,
                    }));
                }
            }
        }
        Ok(())
    }

    // The next frame reads what this one wrote: remember the states and swap the roles of the pair.
    pub(crate) fn keep_history(&self, history: &mut HistoryStore, tracker: &StateTracker) {
        for decl in &self.histories {
            let entry = history.entry_mut(decl.name).expect("prepared above");
            entry.previous_states = tracker.states_of(entry.previous);
            entry.current_states = tracker.states_of(entry.current);
            if self.resources[decl.current as usize].producers.len() > 1 {
                entry.swap();
            }
        }
    }

    // After a failed frame the commands never run, so the pair's contents and states are unknown.
    pub(crate) fn discard_history(
        &self,
        history: &mut HistoryStore,
        pool: &mut ResourcePool,
        release: &mut DeferredRelease,
        frame: u64,
    ) {
        for decl in &self.histories {
            discard_entry(history, decl.name, pool, release, frame);
        }
    }
}

fn discard_entry(
    history: &mut HistoryStore,
    name: &'static str,
    pool: &mut ResourcePool,
    release: &mut DeferredRelease,
    frame: u64,
) {
    if let Some(entry) = history.remove(name) {
        for texture in [entry.previous, entry.current] {
            pool.release_views_of(texture, release, frame);
            release.release(frame, texture);
        }
    }
}
