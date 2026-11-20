use std::any::Any;
use std::fmt;
use std::ops::Range;

use crate::access::{Access, SubresourceRange};
use crate::desc::{BufferCopy, Extent3d, TextureCopy};
use crate::id::{BindGroupId, BufferId, ComputePipelineId, QuerySetId, RenderPipelineId, TextureId, TextureViewId};
use crate::maybe_send::MaybeSend;
use crate::pipeline::IndexFormat;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Color {
    pub const BLACK: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    pub const fn rgba(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LoadOp<V> {
    Clear(V),
    Load,
    DontCare,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StoreOp {
    Store,
    Discard,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ColorAttachment {
    pub view: TextureViewId,
    pub resolve_target: Option<TextureViewId>,
    pub load: LoadOp<Color>,
    pub store: StoreOp,
}

impl ColorAttachment {
    pub const fn clear(view: TextureViewId, color: Color) -> Self {
        Self {
            view,
            resolve_target: None,
            load: LoadOp::Clear(color),
            store: StoreOp::Store,
        }
    }

    pub const fn load(view: TextureViewId) -> Self {
        Self {
            view,
            resolve_target: None,
            load: LoadOp::Load,
            store: StoreOp::Store,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DepthAccess {
    ReadWrite,
    ReadOnly,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DepthAttachment {
    pub view: TextureViewId,
    pub load: LoadOp<f32>,
    pub store: StoreOp,
    pub access: DepthAccess,
}

impl DepthAttachment {
    // Reverse-Z clears to the far plane at 0.
    pub const fn clear_reverse_z(view: TextureViewId) -> Self {
        Self {
            view,
            load: LoadOp::Clear(0.0),
            store: StoreOp::Store,
            access: DepthAccess::ReadWrite,
        }
    }
}

// Timestamps belong to pass boundaries: Apple GPUs cannot write them inside a pass.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PassTimestamps {
    pub query_set: QuerySetId,
    pub begin_index: u32,
    pub end_index: u32,
}

#[derive(Clone, PartialEq, Debug)]
pub struct RenderPassDesc {
    pub label: &'static str,
    pub color: Vec<ColorAttachment>,
    pub depth: Option<DepthAttachment>,
    pub timestamps: Option<PassTimestamps>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ComputePassDesc {
    pub label: &'static str,
    pub timestamps: Option<PassTimestamps>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ScissorRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ResourceRef {
    Texture(TextureId, SubresourceRange),
    Buffer(BufferId),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DeclaredAccess {
    pub resource: ResourceRef,
    pub access: Access,
}

// The backend passes its native recording context (for wgpu: `pathon_rhi_wgpu::interop::ExternalContext`).
pub trait ExternalCallback: MaybeSend {
    fn call(self: Box<Self>, native: &mut dyn Any);
}

impl<F: FnOnce(&mut dyn Any) + MaybeSend> ExternalCallback for F {
    fn call(self: Box<Self>, native: &mut dyn Any) {
        (*self)(native);
    }
}

pub struct ExternalCommand {
    pub label: &'static str,
    pub accesses: Vec<DeclaredAccess>,
    pub callback: Box<dyn ExternalCallback>,
}

impl fmt::Debug for ExternalCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExternalCommand")
            .field("label", &self.label)
            .field("accesses", &self.accesses)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum Command {
    TextureAccess {
        texture: TextureId,
        range: SubresourceRange,
        access: Access,
    },
    BufferAccess {
        buffer: BufferId,
        access: Access,
    },
    BeginRenderPass(Box<RenderPassDesc>),
    EndRenderPass,
    BeginComputePass(Box<ComputePassDesc>),
    EndComputePass,
    SetRenderPipeline(RenderPipelineId),
    SetComputePipeline(ComputePipelineId),
    SetBindGroup {
        index: u32,
        group: BindGroupId,
    },
    SetVertexBuffer {
        slot: u32,
        buffer: BufferId,
        offset: u64,
    },
    SetIndexBuffer {
        buffer: BufferId,
        format: IndexFormat,
        offset: u64,
    },
    SetViewport(Viewport),
    SetScissor(ScissorRect),
    Draw {
        vertices: Range<u32>,
        instances: Range<u32>,
    },
    DrawIndexed {
        indices: Range<u32>,
        base_vertex: i32,
        instances: Range<u32>,
    },
    DrawIndexedIndirect {
        buffer: BufferId,
        offset: u64,
    },
    Dispatch {
        x: u32,
        y: u32,
        z: u32,
    },
    // Rare and large, so boxed: the common commands (binds, draws) stay small.
    Transfer(Box<Transfer>),
    PushDebugGroup(&'static str),
    PopDebugGroup,
    External(Box<ExternalCommand>),
}

#[derive(Debug)]
pub enum Transfer {
    BufferToBuffer {
        src: BufferId,
        src_offset: u64,
        dst: BufferId,
        dst_offset: u64,
        size: u64,
    },
    BufferToTexture {
        src: BufferCopy,
        dst: TextureCopy,
        size: Extent3d,
    },
    TextureToBuffer {
        src: TextureCopy,
        dst: BufferCopy,
        size: Extent3d,
    },
    TextureToTexture {
        src: TextureCopy,
        dst: TextureCopy,
        size: Extent3d,
    },
    ResolveQueries {
        query_set: QuerySetId,
        queries: Range<u32>,
        dst: BufferId,
        dst_offset: u64,
    },
}

#[derive(Debug, Default)]
pub struct CommandList {
    commands: Vec<Command>,
}

impl CommandList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    pub fn into_commands(self) -> Vec<Command> {
        self.commands
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn push(&mut self, command: Command) {
        self.commands.push(command);
    }

    pub fn append(&mut self, other: Self) {
        self.commands.extend(other.commands);
    }

    pub fn texture_access(&mut self, texture: TextureId, range: SubresourceRange, access: Access) {
        self.push(Command::TextureAccess { texture, range, access });
    }

    pub fn buffer_access(&mut self, buffer: BufferId, access: Access) {
        self.push(Command::BufferAccess { buffer, access });
    }

    pub fn render_pass(&mut self, desc: RenderPassDesc) -> RenderPassEncoder<'_> {
        self.push(Command::BeginRenderPass(Box::new(desc)));
        RenderPassEncoder { list: self }
    }

    pub fn compute_pass(&mut self, desc: ComputePassDesc) -> ComputePassEncoder<'_> {
        self.push(Command::BeginComputePass(Box::new(desc)));
        ComputePassEncoder { list: self }
    }

    pub fn copy_buffer_to_buffer(&mut self, src: BufferId, src_offset: u64, dst: BufferId, dst_offset: u64, size: u64) {
        self.transfer(Transfer::BufferToBuffer {
            src,
            src_offset,
            dst,
            dst_offset,
            size,
        });
    }

    pub fn copy_buffer_to_texture(&mut self, src: BufferCopy, dst: TextureCopy, size: Extent3d) {
        self.transfer(Transfer::BufferToTexture { src, dst, size });
    }

    pub fn copy_texture_to_buffer(&mut self, src: TextureCopy, dst: BufferCopy, size: Extent3d) {
        self.transfer(Transfer::TextureToBuffer { src, dst, size });
    }

    pub fn copy_texture_to_texture(&mut self, src: TextureCopy, dst: TextureCopy, size: Extent3d) {
        self.transfer(Transfer::TextureToTexture { src, dst, size });
    }

    pub fn resolve_queries(&mut self, query_set: QuerySetId, queries: Range<u32>, dst: BufferId, dst_offset: u64) {
        self.transfer(Transfer::ResolveQueries {
            query_set,
            queries,
            dst,
            dst_offset,
        });
    }

    pub fn push_debug_group(&mut self, label: &'static str) {
        self.push(Command::PushDebugGroup(label));
    }

    pub fn pop_debug_group(&mut self) {
        self.push(Command::PopDebugGroup);
    }

    pub fn external(&mut self, command: ExternalCommand) {
        self.push(Command::External(Box::new(command)));
    }

    fn transfer(&mut self, transfer: Transfer) {
        self.push(Command::Transfer(Box::new(transfer)));
    }
}

#[derive(Debug)]
pub struct RenderPassEncoder<'list> {
    list: &'list mut CommandList,
}

impl RenderPassEncoder<'_> {
    pub fn set_pipeline(&mut self, pipeline: RenderPipelineId) {
        self.list.push(Command::SetRenderPipeline(pipeline));
    }

    pub fn set_bind_group(&mut self, index: u32, group: BindGroupId) {
        self.list.push(Command::SetBindGroup { index, group });
    }

    pub fn set_vertex_buffer(&mut self, slot: u32, buffer: BufferId, offset: u64) {
        self.list.push(Command::SetVertexBuffer { slot, buffer, offset });
    }

    pub fn set_index_buffer(&mut self, buffer: BufferId, format: IndexFormat, offset: u64) {
        self.list.push(Command::SetIndexBuffer { buffer, format, offset });
    }

    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.list.push(Command::SetViewport(viewport));
    }

    pub fn set_scissor(&mut self, scissor: ScissorRect) {
        self.list.push(Command::SetScissor(scissor));
    }

    pub fn draw(&mut self, vertices: Range<u32>, instances: Range<u32>) {
        self.list.push(Command::Draw { vertices, instances });
    }

    pub fn draw_indexed(&mut self, indices: Range<u32>, base_vertex: i32, instances: Range<u32>) {
        self.list.push(Command::DrawIndexed {
            indices,
            base_vertex,
            instances,
        });
    }

    pub fn draw_indexed_indirect(&mut self, buffer: BufferId, offset: u64) {
        self.list.push(Command::DrawIndexedIndirect { buffer, offset });
    }
}

impl Drop for RenderPassEncoder<'_> {
    fn drop(&mut self) {
        self.list.push(Command::EndRenderPass);
    }
}

#[derive(Debug)]
pub struct ComputePassEncoder<'list> {
    list: &'list mut CommandList,
}

impl ComputePassEncoder<'_> {
    pub fn set_pipeline(&mut self, pipeline: ComputePipelineId) {
        self.list.push(Command::SetComputePipeline(pipeline));
    }

    pub fn set_bind_group(&mut self, index: u32, group: BindGroupId) {
        self.list.push(Command::SetBindGroup { index, group });
    }

    pub fn dispatch(&mut self, x: u32, y: u32, z: u32) {
        self.list.push(Command::Dispatch { x, y, z });
    }
}

impl Drop for ComputePassEncoder<'_> {
    fn drop(&mut self) {
        self.list.push(Command::EndComputePass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ADR-0009: every pack records thousands of these per frame; keep the common ones small.
    #[test]
    fn command_stays_within_32_bytes() {
        assert!(
            std::mem::size_of::<Command>() <= 32,
            "Command is {} bytes",
            std::mem::size_of::<Command>()
        );
    }
}
