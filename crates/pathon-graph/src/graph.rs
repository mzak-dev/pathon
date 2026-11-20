use pathon_rhi_types::{Access, Extent3d, Format, PhysicalSize, RhiError, SubresourceRange, TextureDesc, TextureId};

use crate::execute::PassContext;

// Coarse frame order. Ties inside a stage are broken by priority, then by registration order (with G0006).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Stage {
    Setup,
    Shadows,
    Prepass,
    Opaque,
    Sky,
    Transparent,
    PostProcess,
    Resolve,
    Tonemap,
    Overlay,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FrameSizes {
    pub render: PhysicalSize,
    pub output: PhysicalSize,
}

impl FrameSizes {
    pub const fn same(size: PhysicalSize) -> Self {
        Self {
            render: size,
            output: size,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GraphSize {
    Render,
    Output,
    Fixed(u32, u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GraphTextureDesc {
    pub format: Format,
    pub size: GraphSize,
    pub mip_level_count: u32,
    pub layers: u32,
}

impl GraphTextureDesc {
    pub const fn render(format: Format) -> Self {
        Self {
            format,
            size: GraphSize::Render,
            mip_level_count: 1,
            layers: 1,
        }
    }

    pub const fn output(format: Format) -> Self {
        Self {
            format,
            size: GraphSize::Output,
            mip_level_count: 1,
            layers: 1,
        }
    }

    pub const fn fixed(width: u32, height: u32, format: Format) -> Self {
        Self {
            format,
            size: GraphSize::Fixed(width, height),
            mip_level_count: 1,
            layers: 1,
        }
    }

    #[must_use]
    pub const fn with_mips(self, mip_level_count: u32) -> Self {
        Self {
            mip_level_count,
            ..self
        }
    }

    pub(crate) fn extent(self, sizes: FrameSizes) -> Extent3d {
        let (width, height) = match self.size {
            GraphSize::Render => (sizes.render.width, sizes.render.height),
            GraphSize::Output => (sizes.output.width, sizes.output.height),
            GraphSize::Fixed(width, height) => (width, height),
        };
        Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_layers: self.layers,
        }
    }
}

// A resource plus the version a pass saw. Every write creates a new version, so readers name exactly the contents they
// depend on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TextureHandle {
    pub(crate) resource: u32,
    pub(crate) version: u32,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PassDesc {
    pub name: &'static str,
    pub stage: Stage,
    pub priority: i32,
    pub before: Vec<&'static str>,
    pub after: Vec<&'static str>,
    pub side_effect: bool,
}

impl PassDesc {
    pub fn new(name: &'static str, stage: Stage) -> Self {
        Self {
            name,
            stage,
            priority: 0,
            before: Vec::new(),
            after: Vec::new(),
            side_effect: false,
        }
    }

    #[must_use]
    pub fn priority(self, priority: i32) -> Self {
        Self { priority, ..self }
    }

    #[must_use]
    pub fn before(mut self, pass: &'static str) -> Self {
        self.before.push(pass);
        self
    }

    #[must_use]
    pub fn after(mut self, pass: &'static str) -> Self {
        self.after.push(pass);
        self
    }

    // Kept even when nothing reads its outputs (readbacks, timestamp resolves).
    #[must_use]
    pub fn side_effect(self) -> Self {
        Self {
            side_effect: true,
            ..self
        }
    }
}

pub type RecordFn = Box<dyn FnOnce(&mut PassContext<'_>) -> Result<(), RhiError>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ResourceKind {
    Transient(GraphTextureDesc),
    Imported {
        texture: TextureId,
        desc: TextureDesc,
        initial: Access,
        final_access: Access,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct ResourceNode {
    pub(crate) name: &'static str,
    pub(crate) kind: ResourceKind,
    // Pass that produced each version; version 0 comes from outside the graph (imported) or from nowhere (transient).
    pub(crate) producers: Vec<Option<usize>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct PassAccess {
    pub(crate) resource: u32,
    pub(crate) read_version: Option<u32>,
    pub(crate) write_version: Option<u32>,
    pub(crate) access: Access,
    pub(crate) range: SubresourceRange,
}

pub(crate) struct PassNode {
    pub(crate) desc: PassDesc,
    pub(crate) accesses: Vec<PassAccess>,
    pub(crate) record: Option<RecordFn>,
}

#[derive(Default)]
pub struct FrameGraph {
    pub(crate) resources: Vec<ResourceNode>,
    pub(crate) passes: Vec<PassNode>,
    pub(crate) capture: bool,
}

impl std::fmt::Debug for FrameGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let passes: Vec<&str> = self.passes.iter().map(|pass| pass.desc.name).collect();
        f.debug_struct("FrameGraph")
            .field("resources", &self.resources)
            .field("passes", &passes)
            .field("capture", &self.capture)
            .finish()
    }
}

impl FrameGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_texture(&mut self, name: &'static str, desc: GraphTextureDesc) -> TextureHandle {
        self.add_resource(name, ResourceKind::Transient(desc))
    }

    pub fn import_texture(
        &mut self,
        name: &'static str,
        texture: TextureId,
        desc: TextureDesc,
        initial: Access,
        final_access: Access,
    ) -> TextureHandle {
        self.add_resource(
            name,
            ResourceKind::Imported {
                texture,
                desc,
                initial,
                final_access,
            },
        )
    }

    fn add_resource(&mut self, name: &'static str, kind: ResourceKind) -> TextureHandle {
        let resource = u32::try_from(self.resources.len()).expect("fewer than u32::MAX graph resources");
        self.resources.push(ResourceNode {
            name,
            kind,
            producers: vec![None],
        });
        TextureHandle { resource, version: 0 }
    }

    pub fn add_pass(&mut self, desc: PassDesc) -> PassBuilder<'_> {
        let pass = self.passes.len();
        self.passes.push(PassNode {
            desc,
            accesses: Vec::new(),
            record: None,
        });
        PassBuilder { graph: self, pass }
    }

    // Ask `execute` for a `GraphCapture` (tools only; it allocates).
    pub fn set_capture(&mut self, enabled: bool) {
        self.capture = enabled;
    }

    pub fn pass_count(&self) -> usize {
        self.passes.len()
    }

    pub fn resource_name(&self, handle: TextureHandle) -> Option<&'static str> {
        self.resources
            .get(handle.resource as usize)
            .map(|resource| resource.name)
    }

    pub fn texture_format(&self, handle: TextureHandle) -> Option<Format> {
        match self.resources.get(handle.resource as usize)?.kind {
            ResourceKind::Transient(desc) => Some(desc.format),
            ResourceKind::Imported { desc, .. } => Some(desc.format),
        }
    }

    pub fn is_latest(&self, handle: TextureHandle) -> bool {
        self.resources
            .get(handle.resource as usize)
            .is_some_and(|resource| resource.producers.len() == handle.version as usize + 1)
    }
}

#[derive(Debug)]
pub struct PassBuilder<'graph> {
    graph: &'graph mut FrameGraph,
    pass: usize,
}

impl PassBuilder<'_> {
    pub fn read(&mut self, handle: TextureHandle, access: Access) -> TextureHandle {
        self.read_range(handle, access, SubresourceRange::ALL)
    }

    pub fn read_range(&mut self, handle: TextureHandle, access: Access, range: SubresourceRange) -> TextureHandle {
        self.push(handle, Some(handle.version), None, access, range);
        handle
    }

    // Discards the previous contents: passes that only produced them can be culled.
    pub fn write(&mut self, handle: TextureHandle, access: Access) -> TextureHandle {
        self.write_range(handle, access, SubresourceRange::ALL)
    }

    pub fn write_range(&mut self, handle: TextureHandle, access: Access, range: SubresourceRange) -> TextureHandle {
        let version = self.new_version(handle);
        self.push(handle, None, Some(version), access, range);
        TextureHandle {
            resource: handle.resource,
            version,
        }
    }

    // Reads and then overwrites (load-and-store attachments, read-write storage).
    pub fn modify(&mut self, handle: TextureHandle, access: Access) -> TextureHandle {
        self.modify_range(handle, access, SubresourceRange::ALL)
    }

    pub fn modify_range(&mut self, handle: TextureHandle, access: Access, range: SubresourceRange) -> TextureHandle {
        let version = self.new_version(handle);
        self.push(handle, Some(handle.version), Some(version), access, range);
        TextureHandle {
            resource: handle.resource,
            version,
        }
    }

    pub fn record(self, record: impl FnOnce(&mut PassContext<'_>) -> Result<(), RhiError> + 'static) {
        self.graph.passes[self.pass].record = Some(Box::new(record));
    }

    fn new_version(&mut self, handle: TextureHandle) -> u32 {
        let resource = &mut self.graph.resources[handle.resource as usize];
        let version = u32::try_from(resource.producers.len()).expect("fewer than u32::MAX versions");
        resource.producers.push(Some(self.pass));
        version
    }

    fn push(
        &mut self,
        handle: TextureHandle,
        read_version: Option<u32>,
        write_version: Option<u32>,
        access: Access,
        range: SubresourceRange,
    ) {
        self.graph.passes[self.pass].accesses.push(PassAccess {
            resource: handle.resource,
            read_version,
            write_version,
            access,
            range,
        });
    }
}
