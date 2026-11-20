use pathon_diag::Diagnostic;
use pathon_rhi_types::{Access, TextureUsage};

use crate::graph::{FrameGraph, FrameSizes, GraphSize, PassAccess, ResourceKind};
use crate::order::{OrderNode, sort_nodes};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct GraphStats {
    pub passes_total: usize,
    pub passes_live: usize,
    pub logical_textures: usize,
    pub physical_textures: usize,
}

#[derive(Clone, Debug)]
pub struct CompiledGraph {
    pub(crate) order: Vec<usize>,
    pub(crate) names: Vec<&'static str>,
    pub(crate) sizes: FrameSizes,
    pub(crate) usages: Vec<TextureUsage>,
    // First and last position in `order` at which a resource is touched.
    pub(crate) lifetimes: Vec<Option<(usize, usize)>>,
    pub(crate) warnings: Vec<Diagnostic>,
    pub(crate) stats: GraphStats,
}

impl CompiledGraph {
    pub fn order(&self) -> &[&'static str] {
        &self.names
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
        (Access::SHADER_READ, TextureUsage::SAMPLED),
        (Access::SHADER_WRITE, TextureUsage::STORAGE),
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

fn is_attachment(access: Access) -> bool {
    access.intersects(Access::COLOR_ATTACHMENT | Access::DEPTH_READ | Access::DEPTH_WRITE)
}

impl FrameGraph {
    pub fn compile(&self, sizes: FrameSizes) -> Result<CompiledGraph, Vec<Diagnostic>> {
        let errors = self.validate();
        if !errors.is_empty() {
            return Err(errors);
        }
        let live = self.live_passes();
        let mut warnings = Vec::new();
        let order = self.order(&live, &mut warnings)?;

        let mut usages = vec![TextureUsage::empty(); self.resources.len()];
        let mut lifetimes: Vec<Option<(usize, usize)>> = vec![None; self.resources.len()];
        for (position, pass) in order.iter().enumerate() {
            for access in &self.passes[*pass].accesses {
                let resource = access.resource as usize;
                usages[resource] |= usage_for(access.access);
                lifetimes[resource] = Some(match lifetimes[resource] {
                    Some((first, _)) => (first, position),
                    None => (position, position),
                });
            }
        }
        let logical_textures = self
            .resources
            .iter()
            .zip(&lifetimes)
            .filter(|(resource, lifetime)| matches!(resource.kind, ResourceKind::Transient(_)) && lifetime.is_some())
            .count();
        let stats = GraphStats {
            passes_total: self.passes.len(),
            passes_live: order.len(),
            logical_textures,
            physical_textures: 0,
        };
        let names = order.iter().map(|pass| self.passes[*pass].desc.name).collect();
        Ok(CompiledGraph {
            order,
            names,
            sizes,
            usages,
            lifetimes,
            warnings,
            stats,
        })
    }

    fn validate(&self) -> Vec<Diagnostic> {
        let mut errors = Vec::new();
        for pass in &self.passes {
            let mut render_sized = false;
            let mut output_sized = false;
            for access in &pass.accesses {
                let resource = &self.resources[access.resource as usize];
                if let (Some(version), ResourceKind::Transient(_)) = (access.read_version, resource.kind)
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
                        .with_hint("write the texture in an earlier pass, or import it"),
                    );
                }
                if is_attachment(access.access) {
                    match resource.kind {
                        ResourceKind::Transient(desc) if desc.size == GraphSize::Render => render_sized = true,
                        ResourceKind::Transient(desc) if desc.size == GraphSize::Output => output_sized = true,
                        ResourceKind::Imported { .. } => output_sized = true,
                        ResourceKind::Transient(_) => {}
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
            if let ResourceKind::Imported { .. } = resource.kind
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

    // A read, a modify or a partial write depends on the previous version.
    fn dependency_producer(&self, access: &PassAccess) -> Option<usize> {
        let resource = &self.resources[access.resource as usize];
        let version = match (access.read_version, access.write_version) {
            (Some(read), _) => read,
            (None, Some(write)) if access.range != pathon_rhi_types::SubresourceRange::ALL => write.checked_sub(1)?,
            _ => return None,
        };
        resource.producers.get(version as usize).copied().flatten()
    }

    fn order(&self, live: &[bool], warnings: &mut Vec<Diagnostic>) -> Result<Vec<usize>, Vec<Diagnostic>> {
        let mut data_edges = Vec::new();
        for (index, pass) in self.passes.iter().enumerate() {
            for access in &pass.accesses {
                let resource = &self.resources[access.resource as usize];
                if let Some(producer) = self.dependency_producer(access) {
                    data_edges.push((producer, index));
                }
                if let Some(write) = access.write_version
                    && let Some(Some(previous)) = resource.producers.get((write - 1) as usize)
                {
                    data_edges.push((*previous, index));
                }
                if let Some(read) = access.read_version
                    && let Some(Some(next_writer)) = resource.producers.get(read as usize + 1)
                {
                    data_edges.push((index, *next_writer));
                }
            }
        }
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
        Ok(order)
    }
}
