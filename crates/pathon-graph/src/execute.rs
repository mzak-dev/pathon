use pathon_diag::Diagnostic;
use pathon_resources::DeferredRelease;
use pathon_rhi_types::{
    Access, BindGroupDesc, BindGroupId, BufferDesc, BufferId, Color, ColorAttachment, Command, CommandList,
    DepthAttachment, DeviceApi, FormatKind, RenderPassDesc, Resource, RhiError, SubresourceRange, TextureDesc,
    TextureDimension, TextureId, TextureUsage, TextureViewDesc, TextureViewDimension, TextureViewId,
};

use crate::capture::{AccessCapture, BarrierCapture, GraphCapture, PassCapture, ResourceCapture};
use crate::compile::{CompiledGraph, GraphStats};
use crate::graph::{BufferHandle, FrameGraph, HistoryDecl, PassAccess, PassNode, ResourceKind, TextureHandle};
use crate::history::{HistoryEntry, HistoryStore};
use crate::pool::ResourcePool;

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
    pub capture: Option<GraphCapture>,
}

pub struct PassContext<'exec> {
    device: &'exec dyn DeviceApi,
    commands: &'exec mut CommandList,
    pool: &'exec mut ResourcePool,
    physical: &'exec [Option<PhysicalTexture>],
    physical_buffers: &'exec [Option<BufferId>],
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

    pub fn buffer(&self, handle: BufferHandle) -> BufferId {
        self.physical_buffers[handle.resource as usize]
            .expect("a pass only sees resources it declared, which are allocated")
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
    fn seed(&mut self, texture: TextureId, states: Vec<Access>) {
        self.states.push((texture, states));
    }

    fn states_of(&self, texture: TextureId) -> Vec<Access> {
        self.states
            .iter()
            .find(|(known, _)| *known == texture)
            .map(|(_, states)| states.clone())
            .expect("history textures are seeded before the first pass")
    }

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

struct BufferStateTracker {
    states: Vec<(BufferId, Access)>,
}

impl BufferStateTracker {
    fn state_mut(&mut self, buffer: BufferId, initial: Access) -> &mut Access {
        let existing = self.states.iter().position(|(known, _)| *known == buffer);
        let index = existing.unwrap_or_else(|| {
            self.states.push((buffer, initial));
            self.states.len() - 1
        });
        &mut self.states[index].1
    }

    // Same rule as for textures: a barrier on any state change and after any write.
    fn transition(&mut self, buffer: BufferId, needed: Access, commands: &mut CommandList) {
        let current = self.state_mut(buffer, Access::empty());
        if needed.is_empty() || (needed == *current && current.writes().is_empty()) {
            return;
        }
        commands.buffer_access(buffer, needed);
        *current = needed;
    }
}

fn needed_buffer_states(accesses: &[PassAccess], physical: &[Option<BufferId>]) -> Vec<(BufferId, Access)> {
    let mut needed: Vec<(BufferId, Access)> = Vec::new();
    for access in accesses {
        let Some(buffer) = physical[access.resource as usize] else {
            continue;
        };
        match needed.iter_mut().find(|(known, _)| *known == buffer) {
            Some((_, state)) => *state |= access.access,
            None => needed.push((buffer, access.access)),
        }
    }
    needed
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
        mut self,
        compiled: &CompiledGraph,
        device: &dyn DeviceApi,
        pool: &mut ResourcePool,
        history: &mut HistoryStore,
        release: &mut DeferredRelease,
        frame: u64,
    ) -> Result<Execution, RhiError> {
        pool.begin_frame();
        let (mut physical, physical_count) = self.allocate(compiled, device, pool, frame)?;
        let fresh_history = self.prepare_history(compiled, device, pool, history, release, frame)?;
        self.bind_history(history, &mut physical);
        let (physical_buffers, physical_buffer_count) = self.allocate_buffers(compiled, device, pool, frame)?;
        let mut tracker = StateTracker { states: Vec::new() };
        let mut buffer_tracker = BufferStateTracker { states: Vec::new() };
        for (resource, buffer) in self.resources.iter().zip(&physical_buffers) {
            if let (ResourceKind::ImportedBuffer { initial, .. }, Some(buffer)) = (resource.kind, buffer) {
                *buffer_tracker.state_mut(*buffer, initial) = initial;
            }
        }
        for (resource, texture) in self.resources.iter().zip(&physical) {
            if let (ResourceKind::Imported { initial, .. }, Some(texture)) = (resource.kind, texture) {
                tracker.states_mut(*texture, initial);
            }
        }

        for decl in &self.histories {
            let entry = history.entry_mut(decl.name).expect("prepared above");
            tracker.seed(entry.previous, entry.previous_states.clone());
            tracker.seed(entry.current, entry.current_states.clone());
        }

        let mut capture = self.capture.then(|| self.begin_capture(compiled, &physical));
        let mut commands = CommandList::new();
        let mut failure = self
            .zero_fresh_history(&fresh_history, &physical, pool, &mut tracker, &mut commands, device)
            .err();
        let mut frame_resources = Vec::new();
        let mut passes: Vec<Option<PassNode>> = std::mem::take(&mut self.passes).into_iter().map(Some).collect();
        for index in &compiled.order {
            if failure.is_some() {
                break;
            }
            let pass = passes[*index].take().expect("each pass appears once in the order");
            commands.push_debug_group(pass.desc.name);
            let barriers_start = commands.commands().len();
            for (texture, needed) in needed_states(&pass.accesses, &physical) {
                tracker.transition(texture, &needed, &mut commands);
            }
            for (buffer, needed) in needed_buffer_states(&pass.accesses, &physical_buffers) {
                buffer_tracker.transition(buffer, needed, &mut commands);
            }
            if let Some(capture) = &mut capture {
                capture.passes[*index].barriers = barriers_since(&commands, barriers_start);
            }
            if let Some(record) = pass.record {
                let mut context = PassContext {
                    device,
                    commands: &mut commands,
                    pool: &mut *pool,
                    physical: &physical,
                    physical_buffers: &physical_buffers,
                    frame_resources: &mut frame_resources,
                };
                if let Err(error) = record(&mut context) {
                    failure = Some(error);
                    break;
                }
            }
            commands.pop_debug_group();
        }

        let final_start = commands.commands().len();
        if failure.is_some() {
            self.discard_history(history, pool, release, frame);
        } else {
            self.keep_history(history, &tracker);
        }
        for (resource, texture) in self.resources.iter().zip(&physical) {
            if let (ResourceKind::Imported { final_access, .. }, Some(texture)) = (resource.kind, texture) {
                let count = (texture.desc.mip_level_count * texture.desc.array_layers()) as usize;
                tracker.transition(*texture, &vec![final_access; count], &mut commands);
                pool.release_views_of(texture.texture, release, frame);
            }
        }
        for (resource, buffer) in self.resources.iter().zip(&physical_buffers) {
            if let (ResourceKind::ImportedBuffer { final_access, .. }, Some(buffer)) = (resource.kind, buffer) {
                buffer_tracker.transition(*buffer, final_access, &mut commands);
            }
        }
        if let Some(capture) = &mut capture {
            capture.final_barriers = barriers_since(&commands, final_start);
            capture.stats.physical_textures = physical_count;
        }
        for resource in frame_resources {
            release.release(frame, resource);
        }
        pool.end_frame(frame, release);
        if let Some(error) = failure {
            return Err(error);
        }

        let stats = GraphStats {
            physical_textures: physical_count,
            physical_buffers: physical_buffer_count,
            ..compiled.stats
        };
        Ok(Execution {
            commands,
            stats,
            warnings: compiled.warnings.clone(),
            capture,
        })
    }

    fn begin_capture(&self, compiled: &CompiledGraph, physical: &[Option<PhysicalTexture>]) -> GraphCapture {
        let mut positions = vec![None; self.passes.len()];
        for (position, index) in compiled.order.iter().enumerate() {
            positions[*index] = Some(position);
        }
        let passes = self
            .passes
            .iter()
            .zip(positions)
            .map(|(pass, order)| PassCapture {
                name: pass.desc.name,
                stage: pass.desc.stage,
                priority: pass.desc.priority,
                order,
                side_effect: pass.desc.side_effect,
                accesses: pass
                    .accesses
                    .iter()
                    .map(|access| AccessCapture {
                        resource: access.resource as usize,
                        read_version: access.read_version,
                        write_version: access.write_version,
                        access: access.access,
                        range: access.range,
                    })
                    .collect(),
                barriers: Vec::new(),
            })
            .collect();
        let resources = self
            .resources
            .iter()
            .zip(physical)
            .zip(&compiled.lifetimes)
            .map(|((resource, physical), lifetime)| ResourceCapture {
                name: resource.name,
                imported: resource.kind.is_imported(),
                desc: physical.map(|physical| physical.desc),
                physical: physical.map(|physical| physical.texture),
                lifetime: *lifetime,
                versions: resource.producers.len(),
            })
            .collect();
        GraphCapture {
            passes,
            resources,
            edges: compiled.edges.clone(),
            final_barriers: Vec::new(),
            stats: compiled.stats,
            warnings: compiled.warnings.clone(),
        }
    }
}

fn barriers_since(commands: &CommandList, start: usize) -> Vec<BarrierCapture> {
    commands.commands()[start..]
        .iter()
        .filter_map(|command| match command {
            Command::TextureAccess { texture, range, access } => Some(BarrierCapture {
                texture: *texture,
                range: *range,
                access: *access,
            }),
            _ => None,
        })
        .collect()
}

impl FrameGraph {
    // Transient textures with identical descriptions and disjoint lifetimes share one physical texture.
    fn allocate(
        &self,
        compiled: &CompiledGraph,
        device: &dyn DeviceApi,
        pool: &mut ResourcePool,
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
                (ResourceKind::Transient(_), None)
                | (
                    ResourceKind::History { .. }
                    | ResourceKind::TransientBuffer(_)
                    | ResourceKind::ImportedBuffer { .. },
                    _,
                ) => {}
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

impl FrameGraph {
    // Same aliasing as for textures: equal sizes with disjoint lifetimes share one buffer carrying the union of usages.
    fn allocate_buffers(
        &self,
        compiled: &CompiledGraph,
        device: &dyn DeviceApi,
        pool: &mut ResourcePool,
        frame: u64,
    ) -> Result<(Vec<Option<BufferId>>, usize), RhiError> {
        let mut physical: Vec<Option<BufferId>> = vec![None; self.resources.len()];
        let mut transients = Vec::new();
        for (index, resource) in self.resources.iter().enumerate() {
            match (resource.kind, compiled.lifetimes[index]) {
                (ResourceKind::ImportedBuffer { buffer, .. }, _) => physical[index] = Some(buffer),
                (ResourceKind::TransientBuffer(graph_desc), Some((first, last))) => {
                    let desc = BufferDesc {
                        label: resource.name,
                        size: graph_desc.size,
                        usage: graph_desc.usage | compiled.buffer_usages[index],
                    };
                    transients.push((index, first, last, desc));
                }
                _ => {}
            }
        }
        transients.sort_by_key(|(_, first, _, _)| *first);

        let mut slots: Vec<(BufferDesc, usize, Vec<usize>)> = Vec::new();
        for (index, first, last, desc) in transients {
            let reusable = slots
                .iter_mut()
                .find(|(slot, busy_until, _)| *busy_until < first && slot.size == desc.size);
            if let Some((slot, busy_until, members)) = reusable {
                slot.usage |= desc.usage;
                *busy_until = last;
                members.push(index);
            } else {
                slots.push((desc, last, vec![index]));
            }
        }
        for (desc, _, members) in &slots {
            let buffer = pool.acquire_buffer(device, desc, frame)?;
            for member in members {
                physical[*member] = Some(buffer);
            }
        }
        Ok((physical, slots.len()))
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
    fn prepare_history(
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

    fn bind_history(&self, history: &mut HistoryStore, physical: &mut [Option<PhysicalTexture>]) {
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
    fn zero_fresh_history(
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
    fn keep_history(&self, history: &mut HistoryStore, tracker: &StateTracker) {
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
    fn discard_history(
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
