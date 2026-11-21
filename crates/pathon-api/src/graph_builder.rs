use pathon_graph::{
    BufferHandle, FrameGraph, GraphBufferDesc, GraphTextureDesc, HistoryTexture, PassBuilder, PassDesc, TextureHandle,
};
use pathon_rhi_types::{Access, BufferId, Format, TextureDesc, TextureId};

#[derive(Debug)]
pub struct GraphBuilder<'graph> {
    graph: &'graph mut FrameGraph,
}

impl<'graph> GraphBuilder<'graph> {
    pub(crate) fn new(graph: &'graph mut FrameGraph) -> Self {
        Self { graph }
    }

    pub fn create_texture(&mut self, name: &'static str, desc: GraphTextureDesc) -> TextureHandle {
        self.graph.create_texture(name, desc)
    }

    pub fn import_texture(
        &mut self,
        name: &'static str,
        texture: TextureId,
        desc: TextureDesc,
        initial: Access,
        final_access: Access,
    ) -> TextureHandle {
        self.graph.import_texture(name, texture, desc, initial, final_access)
    }

    pub fn create_buffer(&mut self, name: &'static str, desc: GraphBufferDesc) -> BufferHandle {
        self.graph.create_buffer(name, desc)
    }

    pub fn import_buffer(
        &mut self,
        name: &'static str,
        buffer: BufferId,
        size: u64,
        initial: Access,
        final_access: Access,
    ) -> BufferHandle {
        self.graph.import_buffer(name, buffer, size, initial, final_access)
    }

    pub fn history_texture(&mut self, name: &'static str, desc: GraphTextureDesc) -> HistoryTexture {
        self.graph.history_texture(name, desc)
    }

    pub fn add_pass(&mut self, desc: PassDesc) -> PassBuilder<'_> {
        self.graph.add_pass(desc)
    }

    pub fn resource_name(&self, handle: TextureHandle) -> Option<&'static str> {
        self.graph.resource_name(handle)
    }

    pub fn texture_format(&self, handle: TextureHandle) -> Option<Format> {
        self.graph.texture_format(handle)
    }

    pub fn is_latest(&self, handle: TextureHandle) -> bool {
        self.graph.is_latest(handle)
    }
}
