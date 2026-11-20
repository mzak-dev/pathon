use pathon_diag::Diagnostic;
use pathon_resources::DeferredRelease;
use pathon_rhi_types::{
    Access, BindGroupDesc, BindGroupId, CommandList, DeviceApi, Resource, RhiError, SubresourceRange, TextureDesc,
    TextureDimension, TextureId, TextureUsage, TextureViewDesc, TextureViewId,
};

use crate::compile::{CompiledGraph, GraphStats};
use crate::graph::{FrameGraph, PassAccess, PassNode, ResourceKind, TextureHandle};
use crate::pool::TexturePool;

#[derive(Clone, Copy, Debug)]
struct PhysicalTexture {
    texture: TextureId,
    desc: TextureDesc,
}

#[derive(Debug)]
pub struct Execution {
    pub commands: CommandList,
    pub stats: GraphStats,
    pub warnings: Vec<Diagnostic>,
}

pub struct PassContext<'exec> {
    device: &'exec dyn DeviceApi,
    commands: &'exec mut CommandList,
    pool: &'exec mut TexturePool,
    physical: &'exec [Option<PhysicalTexture>],
    frame_resources: &'exec mut Vec<Resource>,
}

impl std::fmt::Debug for PassContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassContext").finish_non_exhaustive()
    }
}

impl PassContext<'_> {
    pub fn device(&self) -> &dyn DeviceApi {
        self.device
    }

    pub fn commands(&mut self) -> &mut CommandList {
        self.commands
    }

    fn physical(&self, handle: TextureHandle) -> PhysicalTexture {
        self.physical[handle.resource as usize].expect("a pass only sees resources it declared, which are allocated")
    }

    pub fn texture(&self, handle: TextureHandle) -> TextureId {
        self.physical(handle).texture
    }

    pub fn desc(&self, handle: TextureHandle) -> TextureDesc {
        self.physical(handle).desc
    }

    pub fn view(&mut self, handle: TextureHandle) -> Result<TextureViewId, RhiError> {
        self.view_with(handle, &TextureViewDesc::default())
    }

    pub fn view_with(&mut self, handle: TextureHandle, desc: &TextureViewDesc) -> Result<TextureViewId, RhiError> {
        let texture = self.texture(handle);
        self.pool.view(self.device, texture, desc)
    }

    // Lives for this frame only; released after the frames in flight.
    pub fn create_bind_group(&mut self, desc: &BindGroupDesc) -> Result<BindGroupId, RhiError> {
        let group = self.device.create_bind_group(desc)?;
        self.frame_resources.push(group.into());
        Ok(group)
    }
}

struct StateTracker {
    states: Vec<(TextureId, Vec<Access>)>,
}

impl StateTracker {
    fn states_mut(&mut self, physical: PhysicalTexture, initial: Access) -> &mut Vec<Access> {
        let existing = self.states.iter().position(|(texture, _)| *texture == physical.texture);
        let index = existing.unwrap_or_else(|| {
            let count = physical.desc.mip_level_count * physical.desc.array_layers();
            self.states.push((physical.texture, vec![initial; count as usize]));
            self.states.len() - 1
        });
        &mut self.states[index].1
    }

    // A barrier is emitted when the state changes and also after any write, so a native backend gets the memory
    // dependency between two writers or a writer and the next reader.
    fn transition(&mut self, physical: PhysicalTexture, needed: &[Access], commands: &mut CommandList) {
        let layers = physical.desc.array_layers();
        let current = self.states_mut(physical, Access::empty());
        let changed: Vec<usize> = (0..needed.len())
            .filter(|index| {
                let need = needed[*index];
                !need.is_empty() && (need != current[*index] || !current[*index].writes().is_empty())
            })
            .collect();
        if changed.is_empty() {
            return;
        }
        let uniform = changed.len() == needed.len() && needed.iter().all(|need| *need == needed[0]);
        if uniform {
            commands.texture_access(physical.texture, SubresourceRange::ALL, needed[0]);
        } else {
            for index in &changed {
                let index = u32::try_from(*index).expect("subresource count fits u32");
                let range = SubresourceRange {
                    base_mip: index / layers,
                    mip_count: 1,
                    base_layer: index % layers,
                    layer_count: 1,
                };
                commands.texture_access(physical.texture, range, needed[index as usize]);
            }
        }
        for index in changed {
            current[index] = needed[index];
        }
    }
}

fn needed_states(accesses: &[PassAccess], physical: &[Option<PhysicalTexture>]) -> Vec<(PhysicalTexture, Vec<Access>)> {
    let mut needed: Vec<(PhysicalTexture, Vec<Access>)> = Vec::new();
    for access in accesses {
        let Some(texture) = physical[access.resource as usize] else {
            continue;
        };
        let mips = texture.desc.mip_level_count;
        let layers = texture.desc.array_layers();
        let existing = needed.iter().position(|(entry, _)| entry.texture == texture.texture);
        let index = existing.unwrap_or_else(|| {
            needed.push((texture, vec![Access::empty(); (mips * layers) as usize]));
            needed.len() - 1
        });
        for (mip, layer) in access.range.subresources(mips, layers) {
            needed[index].1[(mip * layers + layer) as usize] |= access.access;
        }
    }
    needed
}

impl FrameGraph {
    pub fn execute(
        self,
        compiled: CompiledGraph,
        device: &dyn DeviceApi,
        pool: &mut TexturePool,
        release: &mut DeferredRelease,
        frame: u64,
    ) -> Result<Execution, RhiError> {
        pool.begin_frame();
        let (physical, physical_count) = self.allocate(&compiled, device, pool, frame)?;
        let mut tracker = StateTracker { states: Vec::new() };
        for (resource, texture) in self.resources.iter().zip(&physical) {
            if let (ResourceKind::Imported { initial, .. }, Some(texture)) = (resource.kind, texture) {
                tracker.states_mut(*texture, initial);
            }
        }

        let mut commands = CommandList::new();
        let mut frame_resources = Vec::new();
        let mut passes: Vec<Option<PassNode>> = self.passes.into_iter().map(Some).collect();
        for index in &compiled.order {
            let pass = passes[*index].take().expect("each pass appears once in the order");
            commands.push_debug_group(pass.desc.name);
            for (texture, needed) in needed_states(&pass.accesses, &physical) {
                tracker.transition(texture, &needed, &mut commands);
            }
            if let Some(record) = pass.record {
                let mut context = PassContext {
                    device,
                    commands: &mut commands,
                    pool: &mut *pool,
                    physical: &physical,
                    frame_resources: &mut frame_resources,
                };
                record(&mut context)?;
            }
            commands.pop_debug_group();
        }

        for (resource, texture) in self.resources.iter().zip(&physical) {
            if let (ResourceKind::Imported { final_access, .. }, Some(texture)) = (resource.kind, texture) {
                let count = (texture.desc.mip_level_count * texture.desc.array_layers()) as usize;
                tracker.transition(*texture, &vec![final_access; count], &mut commands);
                pool.release_views_of(texture.texture, release, frame);
            }
        }
        for resource in frame_resources {
            release.release(frame, resource);
        }
        pool.end_frame(frame, release);

        let stats = GraphStats {
            physical_textures: physical_count,
            ..compiled.stats
        };
        Ok(Execution {
            commands,
            stats,
            warnings: compiled.warnings,
        })
    }

    // Transient textures with identical descriptions and disjoint lifetimes share one physical texture.
    fn allocate(
        &self,
        compiled: &CompiledGraph,
        device: &dyn DeviceApi,
        pool: &mut TexturePool,
        frame: u64,
    ) -> Result<(Vec<Option<PhysicalTexture>>, usize), RhiError> {
        let mut physical: Vec<Option<PhysicalTexture>> = vec![None; self.resources.len()];
        let mut transients = Vec::new();
        for (index, resource) in self.resources.iter().enumerate() {
            match (resource.kind, compiled.lifetimes[index]) {
                (ResourceKind::Imported { texture, desc, .. }, _) => {
                    physical[index] = Some(PhysicalTexture { texture, desc });
                }
                (ResourceKind::Transient(graph_desc), Some((first, last))) => {
                    let size = graph_desc.extent(compiled.sizes);
                    let desc = TextureDesc {
                        label: resource.name,
                        size,
                        mip_level_count: graph_desc.mip_level_count,
                        sample_count: 1,
                        dimension: TextureDimension::D2,
                        format: graph_desc.format,
                        usage: compiled.usages[index],
                    };
                    transients.push((index, first, last, desc));
                }
                (ResourceKind::Transient(_), None) => {}
            }
        }
        transients.sort_by_key(|(_, first, _, _)| *first);

        // Aliases share a shape; the physical texture carries the union of their usages.
        let mut slots: Vec<(TextureDesc, usize, Vec<usize>)> = Vec::new();
        for (index, first, last, desc) in transients {
            let reusable = slots
                .iter_mut()
                .find(|(slot, busy_until, _)| *busy_until < first && same_shape(slot, &desc));
            if let Some((slot, busy_until, members)) = reusable {
                slot.usage |= desc.usage;
                *busy_until = last;
                members.push(index);
            } else {
                slots.push((desc, last, vec![index]));
            }
        }
        for (desc, _, members) in &slots {
            let texture = pool.acquire(device, desc, frame)?;
            for member in members {
                physical[*member] = Some(PhysicalTexture { texture, desc: *desc });
            }
        }
        Ok((physical, slots.len()))
    }
}

fn same_shape(a: &TextureDesc, b: &TextureDesc) -> bool {
    let strip = |desc: &TextureDesc| TextureDesc {
        label: "",
        usage: TextureUsage::empty(),
        ..*desc
    };
    strip(a) == strip(b)
}
