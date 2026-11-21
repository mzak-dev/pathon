use pathon_diag::Diagnostic;
use pathon_rhi_types::{Access, BufferUsage, PhysicalSize, TextureUsage};

use crate::graph::{FrameGraph, FrameSizes, GraphSize, PassAccess, ResourceKind};
use crate::order::{OrderNode, sort_nodes};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct GraphStats {
    pub passes_total: usize,
    pub passes_live: usize,
    pub logical_textures: usize,
    pub physical_textures: usize,
    pub logical_buffers: usize,
    pub physical_buffers: usize,
}

#[derive(Clone, Debug)]
pub struct CompiledGraph {
    pub(crate) order: Vec<usize>,
    pub(crate) names: Vec<&'static str>,
    pub(crate) sizes: FrameSizes,
    pub(crate) usages: Vec<TextureUsage>,
    pub(crate) buffer_usages: Vec<BufferUsage>,
    // First and last position in `order` at which a resource is touched.
    pub(crate) lifetimes: Vec<Option<(usize, usize)>>,
    // Dependencies between live passes (data and before/after), by pass index.
    pub(crate) edges: Vec<(usize, usize)>,
    pub(crate) warnings: Vec<Diagnostic>,
    pub(crate) stats: GraphStats,
}

impl CompiledGraph {
    pub fn order(&self) -> &[&'static str] {
        &self.names
    }

    pub fn edges(&self) -> &[(usize, usize)] {
        &self.edges
    }

    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    pub fn stats(&self) -> GraphStats {
        self.stats
    }
}

pub(crate) fn usage_for(access: Access) -> TextureUsage {
    let mut usage = TextureUsage::empty();
    let pairs = [
        (Access::COPY_SRC, TextureUsage::COPY_SRC),
        (Access::COPY_DST, TextureUsage::COPY_DST),
        (Access::SAMPLED, TextureUsage::SAMPLED),
        (Access::STORAGE_READ, TextureUsage::STORAGE),
        (Access::STORAGE_WRITE, TextureUsage::STORAGE),
        (Access::COLOR_ATTACHMENT, TextureUsage::RENDER_TARGET),
        (Access::DEPTH_READ, TextureUsage::RENDER_TARGET),
        (Access::DEPTH_WRITE, TextureUsage::RENDER_TARGET),
    ];
    for (flag, needed) in pairs {
        if access.intersects(flag) {
            usage |= needed;
        }
    }
    usage
}

pub(crate) fn buffer_usage_for(access: Access) -> BufferUsage {
    let mut usage = BufferUsage::empty();
    let pairs = [
        (Access::VERTEX, BufferUsage::VERTEX),
        (Access::INDEX, BufferUsage::INDEX),
        (Access::INDIRECT, BufferUsage::INDIRECT),
        (Access::UNIFORM, BufferUsage::UNIFORM),
        (Access::COPY_SRC, BufferUsage::COPY_SRC),
        (Access::COPY_DST, BufferUsage::COPY_DST),
        (Access::STORAGE_READ, BufferUsage::STORAGE),
        (Access::STORAGE_WRITE, BufferUsage::STORAGE),
    ];
    for (flag, needed) in pairs {
        if access.intersects(flag) {
            usage |= needed;
        }
    }
    usage
}

// A version is written before it is read, so `Write` sorts first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Hazard {
    Write,
    Read,
}

// A read, a modify or a partial write depends on the previous version.
fn read_dependency(access: &PassAccess) -> Option<u32> {
    match (access.read_version, access.write_version) {
        (Some(read), _) => Some(read),
        (None, Some(write)) if access.range != pathon_rhi_types::SubresourceRange::ALL => write.checked_sub(1),
        _ => None,
    }
}

fn is_attachment(access: Access) -> bool {
    access.intersects(Access::COLOR_ATTACHMENT | Access::DEPTH_READ | Access::DEPTH_WRITE)
}

impl FrameGraph {
    pub fn compile(&self, sizes: FrameSizes) -> Result<CompiledGraph, Vec<Diagnostic>> {
        let errors = self.validate(sizes);
        if !errors.is_empty() {
            return Err(errors);
        }
        let live = self.live_passes();
        let mut warnings = Vec::new();
        let (order, edges) = self.order(&live, &mut warnings)?;

        let mut usages = vec![TextureUsage::empty(); self.resources.len()];
        let mut buffer_usages = vec![BufferUsage::empty(); self.resources.len()];
        let mut lifetimes: Vec<Option<(usize, usize)>> = vec![None; self.resources.len()];
        for (position, pass) in order.iter().enumerate() {
            for access in &self.passes[*pass].accesses {
                let resource = access.resource as usize;
                if self.resources[resource].kind.is_buffer() {
                    buffer_usages[resource] |= buffer_usage_for(access.access);
                } else {
                    usages[resource] |= usage_for(access.access);
                }
                lifetimes[resource] = Some(match lifetimes[resource] {
                    Some((first, _)) => (first, position),
                    None => (position, position),
                });
            }
        }
        let logical = |buffers: bool| {
            self.resources
                .iter()
                .zip(&lifetimes)
                .filter(|(resource, lifetime)| {
                    resource.kind.is_transient() && resource.kind.is_buffer() == buffers && lifetime.is_some()
                })
                .count()
        };
        let stats = GraphStats {
            passes_total: self.passes.len(),
            passes_live: order.len(),
            logical_textures: logical(false),
            physical_textures: 0,
            logical_buffers: logical(true),
            physical_buffers: 0,
        };
        let names = order.iter().map(|pass| self.passes[*pass].desc.name).collect();
        Ok(CompiledGraph {
            order,
            names,
            sizes,
            usages,
            buffer_usages,
            lifetimes,
            edges,
            warnings,
            stats,
        })
    }

    fn validate(&self, sizes: FrameSizes) -> Vec<Diagnostic> {
        let mut errors = Vec::new();
        for name in &self.history_conflicts {
            errors.push(
                Diagnostic::error(
                    "G0007",
                    format!("history '{name}' is declared twice with different descriptions"),
                )
                .with_hint("declare it once, or give the second one another name"),
            );
        }
        for pass in &self.passes {
            let mut render_sized = false;
            let mut output_sized = false;
            for access in &pass.accesses {
                let resource = &self.resources[access.resource as usize];
                if let (Some(version), true) = (access.read_version, resource.kind.is_transient())
                    && resource.producers.get(version as usize).copied().flatten().is_none()
                {
                    errors.push(
                        Diagnostic::error(
                            "G0003",
                            format!(
                                "pass '{}' reads '{}', but no pass writes it",
                                pass.desc.name, resource.name
                            ),
                        )
                        .with_hint("write the resource in an earlier pass, or import it"),
                    );
                }
                if is_attachment(access.access) {
                    match resource.kind {
                        ResourceKind::Transient(desc) | ResourceKind::History { desc, .. }
                            if desc.size == GraphSize::Render =>
                        {
                            render_sized = true;
                        }
                        ResourceKind::Transient(desc) | ResourceKind::History { desc, .. }
                            if desc.size == GraphSize::Output =>
                        {
                            output_sized = true;
                        }
                        // An import has no size class, only a size; it counts once that size tells the classes apart.
                        ResourceKind::Imported { desc, .. } if sizes.render != sizes.output => {
                            let size = PhysicalSize::new(desc.size.width, desc.size.height);
                            render_sized |= size == sizes.render;
                            output_sized |= size == sizes.output;
                        }
                        ResourceKind::Imported { .. }
                        | ResourceKind::Transient(_)
                        | ResourceKind::History { .. }
                        | ResourceKind::TransientBuffer(_)
                        | ResourceKind::ImportedBuffer { .. } => {}
                    }
                }
            }
            if render_sized && output_sized {
                errors.push(
                    Diagnostic::error(
                        "G0004",
                        format!(
                            "pass '{}' mixes render-size and output-size attachments",
                            pass.desc.name
                        ),
                    )
                    .with_hint("move the pass before or after the upscaler, or use one size class"),
                );
            }
        }
        errors
    }

    // Roots are side-effect passes and the producers of each imported resource's final version; everything they
    // depend on stays, the rest is culled.
    fn live_passes(&self) -> Vec<bool> {
        let mut live = vec![false; self.passes.len()];
        let mut stack = Vec::new();
        for (index, pass) in self.passes.iter().enumerate() {
            if pass.desc.side_effect {
                stack.push(index);
            }
        }
        for resource in &self.resources {
            if resource.kind.is_imported()
                && let Some(Some(producer)) = resource.producers.last()
            {
                stack.push(*producer);
            }
        }
        while let Some(index) = stack.pop() {
            if live[index] {
                continue;
            }
            live[index] = true;
            for access in &self.passes[index].accesses {
                if let Some(producer) = self.dependency_producer(access) {
                    stack.push(producer);
                }
            }
        }
        live
    }

    fn dependency_producer(&self, access: &PassAccess) -> Option<usize> {
        let resource = &self.resources[access.resource as usize];
        let version = read_dependency(access)?;
        resource.producers.get(version as usize).copied().flatten()
    }

    // Per resource, the live accesses in version order: a write follows the previous live write and every live read
    // since it, a read follows the last live write. Culled passes drop out of the chain without breaking it.
    fn hazard_edges(&self, live: &[bool]) -> Vec<(usize, usize)> {
        let mut chains: Vec<Vec<(u32, Hazard, usize)>> = vec![Vec::new(); self.resources.len()];
        for (index, pass) in self.passes.iter().enumerate() {
            if !live[index] {
                continue;
            }
            for access in &pass.accesses {
                let chain = &mut chains[access.resource as usize];
                if let Some(read) = read_dependency(access) {
                    chain.push((read, Hazard::Read, index));
                }
                if let Some(write) = access.write_version {
                    chain.push((write, Hazard::Write, index));
                }
            }
        }
        let mut edges = Vec::new();
        for mut chain in chains {
            chain.sort_unstable();
            let mut last_write = None;
            let mut reads_since_write = Vec::new();
            for (_, hazard, pass) in chain {
                match hazard {
                    Hazard::Write => {
                        let earlier = last_write.into_iter().chain(reads_since_write.drain(..));
                        edges.extend(earlier.map(|from| (from, pass)));
                        last_write = Some(pass);
                    }
                    Hazard::Read => {
                        edges.extend(last_write.map(|from| (from, pass)));
                        reads_since_write.push(pass);
                    }
                }
            }
        }
        edges
    }

    #[expect(clippy::type_complexity, reason = "order plus the edges it was derived from")]
    fn order(
        &self,
        live: &[bool],
        warnings: &mut Vec<Diagnostic>,
    ) -> Result<(Vec<usize>, Vec<(usize, usize)>), Vec<Diagnostic>> {
        let data_edges = self.hazard_edges(live);
        let nodes: Vec<OrderNode<'_>> = self
            .passes
            .iter()
            .zip(live)
            .map(|(pass, live)| OrderNode {
                name: pass.desc.name,
                stage: pass.desc.stage,
                priority: pass.desc.priority,
                before: &pass.desc.before,
                after: &pass.desc.after,
                included: *live,
            })
            .collect();
        let (order, order_warnings) = sort_nodes(&nodes, &data_edges)?;
        warnings.extend(order_warnings);
        let mut edges: Vec<(usize, usize)> = data_edges
            .into_iter()
            .filter(|(from, to)| from != to && live[*from] && live[*to])
            .collect();
        for (index, pass) in self.passes.iter().enumerate() {
            for name in &pass.desc.after {
                edges.extend(self.live_named(live, name).map(|other| (other, index)));
            }
            for name in &pass.desc.before {
                edges.extend(self.live_named(live, name).map(|other| (index, other)));
            }
        }
        edges.retain(|(from, to)| live[*from] && live[*to]);
        edges.sort_unstable();
        edges.dedup();
        Ok((order, edges))
    }

    fn live_named<'graph>(
        &'graph self,
        live: &'graph [bool],
        name: &'graph str,
    ) -> impl Iterator<Item = usize> + 'graph {
        self.passes
            .iter()
            .enumerate()
            .filter(move |(index, pass)| live[*index] && pass.desc.name == name)
            .map(|(index, _)| index)
    }
}
