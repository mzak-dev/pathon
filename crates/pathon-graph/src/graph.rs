use pathon_rhi_types::{
    Access, BufferId, BufferUsage, Extent3d, Format, MaybeSend, PhysicalSize, RhiError, SubresourceRange, TextureDesc,
    TextureId,
};

use crate::execute::PassContext;
use crate::history::{HistoryStore, HistoryTexture};

// Coarse frame order. Ties inside a stage are broken by priority, then by registration order (with G0006).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
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
#[non_exhaustive]
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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GraphBufferDesc {
    pub size: u64,
    pub usage: BufferUsage,
}

impl GraphBufferDesc {
    pub const fn new(size: u64, usage: BufferUsage) -> Self {
        Self { size, usage }
    }
}

// A resource plus the version a pass saw. Every write creates a new version, so readers name exactly the contents they
// depend on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TextureHandle {
    pub(crate) resource: u32,
    pub(crate) version: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BufferHandle {
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

// `MaybeSend` is not an auto trait, so a trait object names the native `Send` bound directly (ADR-0004, ADR-0012).
#[cfg(not(target_arch = "wasm32"))]
pub type RecordFn = Box<dyn FnOnce(&mut PassContext<'_>) -> Result<(), RhiError> + Send>;
#[cfg(target_arch = "wasm32")]
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
    // Backed by a `HistoryStore` texture: its contents come from, and go to, other frames.
    History {
        desc: GraphTextureDesc,
        role: HistoryRole,
    },
    TransientBuffer(GraphBufferDesc),
    ImportedBuffer {
        buffer: BufferId,
        size: u64,
        initial: Access,
        final_access: Access,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HistoryRole {
    Previous,
    Current,
}

// The two resources of one `history_texture` call.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HistoryDecl {
    pub(crate) name: &'static str,
    pub(crate) desc: GraphTextureDesc,
    pub(crate) previous: u32,
    pub(crate) current: u32,
    pub(crate) previous_valid: bool,
}

impl HistoryDecl {
    fn texture(&self) -> HistoryTexture {
        HistoryTexture {
            previous: TextureHandle {
                resource: self.previous,
                version: 0,
            },
            current: TextureHandle {
                resource: self.current,
                version: 0,
            },
            previous_valid: self.previous_valid,
        }
    }
}

impl ResourceKind {
    pub(crate) fn is_imported(self) -> bool {
        matches!(
            self,
            Self::Imported { .. } | Self::ImportedBuffer { .. } | Self::History { .. }
        )
    }

    // What the host handed in; a history texture belongs to the store, so a capture does not list it as an import.
    pub(crate) fn is_host_import(self) -> bool {
        matches!(self, Self::Imported { .. } | Self::ImportedBuffer { .. })
    }

    pub(crate) fn is_transient(self) -> bool {
        matches!(self, Self::Transient(_) | Self::TransientBuffer(_))
    }

    pub(crate) fn is_buffer(self) -> bool {
        matches!(self, Self::TransientBuffer(_) | Self::ImportedBuffer { .. })
    }
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

pub struct FrameGraph {
    pub(crate) resources: Vec<ResourceNode>,
    pub(crate) passes: Vec<PassNode>,
    pub(crate) capture: bool,
    pub(crate) histories: Vec<HistoryDecl>,
    // Names declared again with a different description.
    pub(crate) history_conflicts: Vec<&'static str>,
    known_history: Vec<(&'static str, GraphTextureDesc, Extent3d)>,
    sizes: Option<FrameSizes>,
}

impl std::fmt::Debug for FrameGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let passes: Vec<&str> = self.passes.iter().map(|pass| pass.desc.name).collect();
        f.debug_struct("FrameGraph")
            .field("resources", &self.resources)
            .field("passes", &passes)
            .field("capture", &self.capture)
            .field("histories", &self.histories)
            .field("history_conflicts", &self.history_conflicts)
            .field("known_history", &self.known_history)
            .field("sizes", &self.sizes)
            .finish()
    }
}

impl FrameGraph {
    #[expect(
        clippy::new_without_default,
        reason = "a pack could infer a Default FrameGraph and reach compile"
    )]
    pub fn new() -> Self {
        Self {
            resources: Vec::new(),
            passes: Vec::new(),
            capture: false,
            histories: Vec::new(),
            history_conflicts: Vec::new(),
            known_history: Vec::new(),
            sizes: None,
        }
    }

    // A graph that knows which history the view kept from earlier frames.
    pub fn for_view(history: &HistoryStore, sizes: FrameSizes) -> Self {
        Self {
            known_history: history.signatures(),
            sizes: Some(sizes),
            ..Self::new()
        }
    }

    // One pair per name and frame: declaring the name again returns the same pair, and a different description is
    // G0007.
    pub fn history_texture(&mut self, name: &'static str, desc: GraphTextureDesc) -> HistoryTexture {
        if let Some(declared) = self.histories.iter().find(|declared| declared.name == name) {
            if declared.desc == desc {
                return declared.texture();
            }
            self.history_conflicts.push(name);
            return self.history_resources(name, desc, false).0;
        }
        let previous_valid = self.sizes.is_some_and(|sizes| {
            let extent = desc.extent(sizes);
            self.known_history.iter().any(|(known, known_desc, known_extent)| {
                *known == name && *known_desc == desc && *known_extent == extent
            })
        });
        let (texture, decl) = self.history_resources(name, desc, previous_valid);
        self.histories.push(decl);
        texture
    }

    fn history_resources(
        &mut self,
        name: &'static str,
        desc: GraphTextureDesc,
        previous_valid: bool,
    ) -> (HistoryTexture, HistoryDecl) {
        let previous = self.add_resource(
            name,
            ResourceKind::History {
                desc,
                role: HistoryRole::Previous,
            },
        );
        let current = self.add_resource(
            name,
            ResourceKind::History {
                desc,
                role: HistoryRole::Current,
            },
        );
        let decl = HistoryDecl {
            name,
            desc,
            previous,
            current,
            previous_valid,
        };
        (decl.texture(), decl)
    }

    pub fn create_texture(&mut self, name: &'static str, desc: GraphTextureDesc) -> TextureHandle {
        let resource = self.add_resource(name, ResourceKind::Transient(desc));
        TextureHandle { resource, version: 0 }
    }

    pub fn import_texture(
        &mut self,
        name: &'static str,
        texture: TextureId,
        desc: TextureDesc,
        initial: Access,
        final_access: Access,
    ) -> TextureHandle {
        let resource = self.add_resource(
            name,
            ResourceKind::Imported {
                texture,
                desc,
                initial,
                final_access,
            },
        );
        TextureHandle { resource, version: 0 }
    }

    pub fn create_buffer(&mut self, name: &'static str, desc: GraphBufferDesc) -> BufferHandle {
        let resource = self.add_resource(name, ResourceKind::TransientBuffer(desc));
        BufferHandle { resource, version: 0 }
    }

    pub fn import_buffer(
        &mut self,
        name: &'static str,
        buffer: BufferId,
        size: u64,
        initial: Access,
        final_access: Access,
    ) -> BufferHandle {
        let resource = self.add_resource(
            name,
            ResourceKind::ImportedBuffer {
                buffer,
                size,
                initial,
                final_access,
            },
        );
        BufferHandle { resource, version: 0 }
    }

    fn add_resource(&mut self, name: &'static str, kind: ResourceKind) -> u32 {
        let resource = u32::try_from(self.resources.len()).expect("fewer than u32::MAX graph resources");
        self.resources.push(ResourceNode {
            name,
            kind,
            producers: vec![None],
        });
        resource
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
            ResourceKind::Transient(desc) | ResourceKind::History { desc, .. } => Some(desc.format),
            ResourceKind::Imported { desc, .. } => Some(desc.format),
            ResourceKind::TransientBuffer(_) | ResourceKind::ImportedBuffer { .. } => None,
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
        self.push(handle.resource, Some(handle.version), None, access, range);
        handle
    }

    // Discards the previous contents: passes that only produced them can be culled.
    pub fn write(&mut self, handle: TextureHandle, access: Access) -> TextureHandle {
        self.write_range(handle, access, SubresourceRange::ALL)
    }

    pub fn write_range(&mut self, handle: TextureHandle, access: Access, range: SubresourceRange) -> TextureHandle {
        let version = self.new_version(handle.resource);
        self.push(handle.resource, None, Some(version), access, range);
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
        let version = self.new_version(handle.resource);
        self.push(handle.resource, Some(handle.version), Some(version), access, range);
        TextureHandle {
            resource: handle.resource,
            version,
        }
    }

    pub fn read_buffer(&mut self, handle: BufferHandle, access: Access) -> BufferHandle {
        self.push(
            handle.resource,
            Some(handle.version),
            None,
            access,
            SubresourceRange::ALL,
        );
        handle
    }

    // Discards the previous contents, like `write` on a texture.
    pub fn write_buffer(&mut self, handle: BufferHandle, access: Access) -> BufferHandle {
        let version = self.new_version(handle.resource);
        self.push(handle.resource, None, Some(version), access, SubresourceRange::ALL);
        BufferHandle {
            resource: handle.resource,
            version,
        }
    }

    pub fn modify_buffer(&mut self, handle: BufferHandle, access: Access) -> BufferHandle {
        let version = self.new_version(handle.resource);
        self.push(
            handle.resource,
            Some(handle.version),
            Some(version),
            access,
            SubresourceRange::ALL,
        );
        BufferHandle {
            resource: handle.resource,
            version,
        }
    }

    pub fn record(self, record: impl FnOnce(&mut PassContext<'_>) -> Result<(), RhiError> + MaybeSend + 'static) {
        self.graph.passes[self.pass].record = Some(Box::new(record));
    }

    fn new_version(&mut self, resource: u32) -> u32 {
        let resource = &mut self.graph.resources[resource as usize];
        let version = u32::try_from(resource.producers.len()).expect("fewer than u32::MAX versions");
        resource.producers.push(Some(self.pass));
        version
    }

    fn push(
        &mut self,
        resource: u32,
        read_version: Option<u32>,
        write_version: Option<u32>,
        access: Access,
        range: SubresourceRange,
    ) {
        self.graph.passes[self.pass].accesses.push(PassAccess {
            resource,
            read_version,
            write_version,
            access,
            range,
        });
    }
}
