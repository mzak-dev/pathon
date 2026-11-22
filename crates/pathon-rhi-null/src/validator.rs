use std::sync::Arc;

use pathon_rhi_types::{
    Access, BindGroupId, BufferId, BufferUsage, Command, CommandList, DepthAccess, Extent3d, FormatKind, LoadOp,
    PassTimestamps, ResourceRef, RhiError, StoreOp, SubresourceRange, TexelLayout, TextureCopy, TextureDimension,
    TextureId, TextureUsage, TextureViewId, Transfer,
};

use crate::device::{BindingUse, State};
use crate::texel_layout::{RowAlignment, check_texel_layout};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PassKind {
    None,
    Render,
    Compute,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ContentEffect {
    Keep,
    Written,
    Discarded,
}

fn structure_error(message: impl Into<String>) -> RhiError {
    RhiError::validation("R0104", message)
}

// Walks a command list the way a GPU would and rejects every use of a texture subresource whose declared `Access`
// does not allow it, or whose contents were never written.
pub(crate) struct Validator<'state> {
    state: &'state mut State,
    pass: PassKind,
    // One flag per 4-byte word of the current pipeline's immediates; `None` until a pipeline is set.
    immediates_written: Option<Vec<bool>>,
    bound: Vec<(u32, BindGroupId)>,
    debug_depth: u32,
}

impl<'state> Validator<'state> {
    pub(crate) fn new(state: &'state mut State) -> Self {
        Self {
            state,
            pass: PassKind::None,
            immediates_written: None,
            bound: Vec::new(),
            debug_depth: 0,
        }
    }

    pub(crate) fn run(mut self, list: CommandList) -> Result<(), RhiError> {
        for command in list.into_commands() {
            self.apply(command)?;
        }
        if self.pass != PassKind::None {
            return Err(structure_error("command list ends inside an open pass"));
        }
        if self.debug_depth != 0 {
            return Err(structure_error("command list ends with an open debug group"));
        }
        Ok(())
    }

    fn apply(&mut self, command: Command) -> Result<(), RhiError> {
        match command {
            Command::TextureAccess {
                texture,
                range,
                from,
                to,
            } => {
                self.require_pass(PassKind::None, "texture access")?;
                Self::require_valid_texture_state(to)?;
                let target = self
                    .state
                    .textures
                    .get_mut(texture)
                    .ok_or(RhiError::InvalidHandle("texture"))?;
                let label = target.desc.label;
                let subresources = target.checked_subresources(range)?;
                for &(mip, layer) in &subresources {
                    let tracked = target.subresource_mut(mip, layer).access;
                    if tracked != from {
                        return Err(RhiError::validation(
                            "R0103",
                            format!(
                                "barrier on texture '{label}' mip {mip} layer {layer} starts from {from:?} but the subresource is in {tracked:?}"
                            ),
                        ));
                    }
                }
                for (mip, layer) in subresources {
                    target.subresource_mut(mip, layer).access = to;
                }
            }
            Command::BufferAccess { buffer, access } => {
                self.require_pass(PassKind::None, "buffer access")?;
                Self::require_valid_state(access)?;
                self.buffer_usage(buffer, BufferUsage::empty())?;
            }
            Command::BeginRenderPass(desc) => {
                self.require_pass(PassKind::None, "render pass begin")?;
                self.check_pass_timestamps(desc.timestamps)?;
                for attachment in &desc.color {
                    let reads = attachment.load == LoadOp::Load;
                    self.attachment(attachment.view, Access::COLOR_ATTACHMENT, reads, attachment.store)?;
                    if let Some(resolve) = attachment.resolve_target {
                        self.attachment(resolve, Access::COLOR_ATTACHMENT, false, StoreOp::Store)?;
                    }
                }
                if let Some(depth) = desc.depth {
                    let (needed, store) = match depth.access {
                        DepthAccess::ReadWrite => (Access::DEPTH_WRITE, depth.store),
                        DepthAccess::ReadOnly => (Access::DEPTH_READ, StoreOp::Store),
                    };
                    let reads = depth.load == LoadOp::Load || depth.access == DepthAccess::ReadOnly;
                    let effect = if depth.access == DepthAccess::ReadOnly {
                        None
                    } else {
                        Some(store)
                    };
                    self.depth_attachment(depth.view, needed, reads, effect)?;
                }
                self.enter_pass(PassKind::Render);
            }
            Command::BeginComputePass(desc) => {
                self.require_pass(PassKind::None, "compute pass begin")?;
                self.check_pass_timestamps(desc.timestamps)?;
                self.enter_pass(PassKind::Compute);
            }
            Command::EndRenderPass => {
                self.require_pass(PassKind::Render, "render pass end")?;
                self.pass = PassKind::None;
            }
            Command::EndComputePass => {
                self.require_pass(PassKind::Compute, "compute pass end")?;
                self.pass = PassKind::None;
            }
            Command::SetRenderPipeline(pipeline) => {
                self.require_pass(PassKind::Render, "set render pipeline")?;
                let immediate_size = self
                    .state
                    .render_pipelines
                    .get(pipeline)
                    .ok_or(RhiError::InvalidHandle("render pipeline"))?
                    .immediate_size;
                self.immediates_written = Some(vec![false; (immediate_size / 4) as usize]);
            }
            Command::SetComputePipeline(pipeline) => {
                self.require_pass(PassKind::Compute, "set compute pipeline")?;
                let immediate_size = self
                    .state
                    .compute_pipelines
                    .get(pipeline)
                    .ok_or(RhiError::InvalidHandle("compute pipeline"))?
                    .immediate_size;
                self.immediates_written = Some(vec![false; (immediate_size / 4) as usize]);
            }
            Command::SetBindGroup { index, group } => {
                if self.pass == PassKind::None {
                    return Err(structure_error("bind group set outside a pass"));
                }
                if !self.state.bind_groups.contains(group) {
                    return Err(RhiError::InvalidHandle("bind group"));
                }
                self.bound.retain(|(bound_index, _)| *bound_index != index);
                self.bound.push((index, group));
            }
            Command::SetImmediates { offset, data } => self.set_immediates(offset, data.len())?,
            Command::SetVertexBuffer { buffer, .. } => {
                self.require_pass(PassKind::Render, "set vertex buffer")?;
                self.buffer_usage(buffer, BufferUsage::VERTEX)?;
            }
            Command::SetIndexBuffer { buffer, .. } => {
                self.require_pass(PassKind::Render, "set index buffer")?;
                self.buffer_usage(buffer, BufferUsage::INDEX)?;
            }
            Command::SetViewport(_) | Command::SetScissor(_) => {
                self.require_pass(PassKind::Render, "viewport or scissor")?;
            }
            Command::Draw { .. } | Command::DrawIndexed { .. } => self.draw_or_dispatch(PassKind::Render)?,
            Command::DrawIndexedIndirect { buffer, .. } => {
                self.buffer_usage(buffer, BufferUsage::INDIRECT)?;
                self.draw_or_dispatch(PassKind::Render)?;
            }
            Command::Dispatch { .. } => self.draw_or_dispatch(PassKind::Compute)?,
            Command::Transfer(transfer) => {
                self.require_pass(PassKind::None, "transfer")?;
                self.transfer(*transfer)?;
            }
            Command::PushDebugGroup(_) => self.debug_depth += 1,
            Command::PopDebugGroup => {
                self.debug_depth = self
                    .debug_depth
                    .checked_sub(1)
                    .ok_or_else(|| structure_error("debug group popped twice"))?;
            }
            Command::External(external) => {
                self.require_pass(PassKind::None, "external command")?;
                for declared in &external.accesses {
                    if let ResourceRef::Texture(texture, range) = declared.resource {
                        let effect = if declared.access.writes().is_empty() {
                            ContentEffect::Keep
                        } else {
                            ContentEffect::Written
                        };
                        self.texture_use(texture, range, declared.access, false, effect, TextureUsage::empty())?;
                    }
                }
                external.callback.call(&mut ());
            }
            _ => return Err(RhiError::Unsupported("command not handled by the null validator")),
        }
        Ok(())
    }

    fn transfer(&mut self, transfer: Transfer) -> Result<(), RhiError> {
        match transfer {
            Transfer::BufferToBuffer {
                src,
                src_offset,
                dst,
                dst_offset,
                size,
            } => {
                self.copy_buffer_data(src, src_offset, dst, dst_offset, size)?;
            }
            Transfer::BufferToTexture { src, dst, size } => {
                self.check_copy_layout(dst.texture, src.layout, size)?;
                self.buffer_usage(src.buffer, BufferUsage::COPY_SRC)?;
                self.copy_texture(dst, size, Access::COPY_DST, TextureUsage::COPY_DST)?;
            }
            Transfer::TextureToBuffer { src, dst, size } => {
                self.check_copy_layout(src.texture, dst.layout, size)?;
                self.copy_texture(src, size, Access::COPY_SRC, TextureUsage::COPY_SRC)?;
                self.buffer_usage(dst.buffer, BufferUsage::COPY_DST)?;
            }
            Transfer::TextureToTexture { src, dst, size } => {
                self.copy_texture(src, size, Access::COPY_SRC, TextureUsage::COPY_SRC)?;
                self.copy_texture(dst, size, Access::COPY_DST, TextureUsage::COPY_DST)?;
            }
            Transfer::ResolveQueries {
                query_set,
                queries,
                dst,
                ..
            } => {
                let count = *self
                    .state
                    .query_sets
                    .get(query_set)
                    .ok_or(RhiError::InvalidHandle("query set"))?;
                if queries.end > count {
                    return Err(structure_error("query resolve range exceeds the query set"));
                }
                self.buffer_usage(dst, BufferUsage::QUERY_RESOLVE)?;
            }
            _ => return Err(RhiError::Unsupported("transfer not handled by the null validator")),
        }
        Ok(())
    }

    fn check_pass_timestamps(&self, timestamps: Option<PassTimestamps>) -> Result<(), RhiError> {
        let Some(timestamps) = timestamps else {
            return Ok(());
        };
        let count = *self
            .state
            .query_sets
            .get(timestamps.query_set)
            .ok_or(RhiError::InvalidHandle("query set"))?;
        if timestamps.begin_index == timestamps.end_index {
            return Err(structure_error(
                "pass timestamps write the begin and end to the same query",
            ));
        }
        if timestamps.begin_index.max(timestamps.end_index) >= count {
            return Err(structure_error(format!(
                "pass timestamps write query {} of a set with {count}",
                timestamps.begin_index.max(timestamps.end_index)
            )));
        }
        Ok(())
    }

    fn check_copy_layout(&self, texture: TextureId, layout: TexelLayout, size: Extent3d) -> Result<(), RhiError> {
        let format = self
            .state
            .textures
            .get(texture)
            .ok_or(RhiError::InvalidHandle("texture"))?
            .desc
            .format;
        check_texel_layout(format, layout, size, RowAlignment::Required)
    }

    fn require_valid_texture_state(access: Access) -> Result<(), RhiError> {
        if access.is_valid_texture_state() {
            Ok(())
        } else {
            Err(RhiError::validation(
                "R0103",
                format!("{access:?} is not a valid texture state"),
            ))
        }
    }

    fn require_valid_state(access: Access) -> Result<(), RhiError> {
        if access.is_valid_state() {
            Ok(())
        } else {
            Err(RhiError::validation(
                "R0103",
                format!("{access:?} is not a valid resource state"),
            ))
        }
    }

    fn require_pass(&self, expected: PassKind, what: &str) -> Result<(), RhiError> {
        if self.pass == expected {
            Ok(())
        } else {
            Err(structure_error(format!(
                "{what} recorded in {:?} pass, expected {expected:?}",
                self.pass
            )))
        }
    }

    fn set_immediates(&mut self, offset: u32, len: usize) -> Result<(), RhiError> {
        if self.pass == PassKind::None {
            return Err(structure_error("immediates set outside a pass"));
        }
        let Some(written) = &mut self.immediates_written else {
            return Err(structure_error("immediates set without a pipeline"));
        };
        let size = written.len() as u64 * 4;
        let end = u64::from(offset) + len as u64;
        if !offset.is_multiple_of(4) || !len.is_multiple_of(4) {
            return Err(structure_error(format!(
                "immediates at offset {offset} with {len} bytes are not 4-byte aligned"
            )));
        }
        if end > size {
            return Err(structure_error(format!(
                "immediates {offset}..{end} reach past the pipeline's {size} bytes"
            )));
        }
        written[(offset / 4) as usize..(end / 4) as usize].fill(true);
        Ok(())
    }

    fn enter_pass(&mut self, kind: PassKind) {
        self.pass = kind;
        self.immediates_written = None;
        self.bound.clear();
    }

    fn draw_or_dispatch(&mut self, kind: PassKind) -> Result<(), RhiError> {
        self.require_pass(kind, "draw or dispatch")?;
        let Some(written) = &self.immediates_written else {
            return Err(structure_error("draw or dispatch without a pipeline"));
        };
        if written.iter().any(|word| !word) {
            return Err(structure_error(
                "draw or dispatch before every immediate byte of the pipeline was written",
            ));
        }
        let groups: Vec<BindGroupId> = self.bound.iter().map(|(_, group)| *group).collect();
        for group in groups {
            let uses = Arc::clone(
                self.state
                    .bind_groups
                    .get(group)
                    .ok_or(RhiError::InvalidHandle("bind group"))?,
            );
            for &binding_use in uses.iter() {
                if let BindingUse::Texture {
                    view,
                    mut access,
                    reads,
                } = binding_use
                {
                    if access == Access::SAMPLED && self.view_is_depth(view)? {
                        access |= Access::DEPTH_READ;
                    }
                    let effect = if access.contains(Access::STORAGE_WRITE) {
                        ContentEffect::Written
                    } else {
                        ContentEffect::Keep
                    };
                    self.view_use(view, access, reads, effect)?;
                }
            }
        }
        Ok(())
    }

    fn view_is_depth(&self, view: TextureViewId) -> Result<bool, RhiError> {
        let view = self
            .state
            .views
            .get(view)
            .ok_or(RhiError::InvalidHandle("texture view"))?;
        let texture = self
            .state
            .textures
            .get(view.texture)
            .ok_or(RhiError::InvalidHandle("texture"))?;
        Ok(texture.desc.format.kind() != FormatKind::Color)
    }

    fn attachment(&mut self, view: TextureViewId, needed: Access, reads: bool, store: StoreOp) -> Result<(), RhiError> {
        let effect = match store {
            StoreOp::Store => ContentEffect::Written,
            StoreOp::Discard => ContentEffect::Discarded,
        };
        self.view_use_with_usage(view, needed, reads, effect, TextureUsage::RENDER_TARGET)
    }

    fn depth_attachment(
        &mut self,
        view: TextureViewId,
        needed: Access,
        reads: bool,
        store: Option<StoreOp>,
    ) -> Result<(), RhiError> {
        let effect = match store {
            None => ContentEffect::Keep,
            Some(StoreOp::Store) => ContentEffect::Written,
            Some(StoreOp::Discard) => ContentEffect::Discarded,
        };
        self.view_use_with_usage(view, needed, reads, effect, TextureUsage::RENDER_TARGET)
    }

    fn view_use(
        &mut self,
        view: TextureViewId,
        needed: Access,
        reads: bool,
        effect: ContentEffect,
    ) -> Result<(), RhiError> {
        self.view_use_with_usage(view, needed, reads, effect, TextureUsage::empty())
    }

    fn view_use_with_usage(
        &mut self,
        view: TextureViewId,
        needed: Access,
        reads: bool,
        effect: ContentEffect,
        usage: TextureUsage,
    ) -> Result<(), RhiError> {
        let view = *self
            .state
            .views
            .get(view)
            .ok_or(RhiError::InvalidHandle("texture view"))?;
        self.texture_use(view.texture, view.range, needed, reads, effect, usage)
    }

    fn copy_texture(
        &mut self,
        copy: TextureCopy,
        size: Extent3d,
        needed: Access,
        usage: TextureUsage,
    ) -> Result<(), RhiError> {
        let texture = self
            .state
            .textures
            .get(copy.texture)
            .ok_or(RhiError::InvalidHandle("texture"))?;
        let (base_layer, layer_count) = if texture.desc.dimension == TextureDimension::D3 {
            (0, 1)
        } else {
            (copy.origin.z, size.depth_or_layers)
        };
        let range = SubresourceRange {
            base_mip: copy.mip,
            mip_count: 1,
            base_layer,
            layer_count,
        };
        let (reads, effect) = if needed == Access::COPY_SRC {
            (true, ContentEffect::Keep)
        } else {
            (false, ContentEffect::Written)
        };
        self.texture_use(copy.texture, range, needed, reads, effect, usage)
    }

    fn texture_use(
        &mut self,
        texture: TextureId,
        range: SubresourceRange,
        needed: Access,
        reads: bool,
        effect: ContentEffect,
        usage: TextureUsage,
    ) -> Result<(), RhiError> {
        let target = self
            .state
            .textures
            .get_mut(texture)
            .ok_or(RhiError::InvalidHandle("texture"))?;
        let label = target.desc.label;
        if !target.desc.usage.contains(usage) {
            return Err(RhiError::validation(
                "R0105",
                format!("texture '{label}' used without {usage:?} usage"),
            ));
        }
        for (mip, layer) in target.checked_subresources(range)? {
            let subresource = target.subresource_mut(mip, layer);
            if !subresource.access.contains(needed) {
                return Err(RhiError::validation(
                    "R0101",
                    format!(
                        "texture '{label}' mip {mip} layer {layer} used as {needed:?} while in state {:?}",
                        subresource.access
                    ),
                ));
            }
            if reads && !subresource.initialized {
                return Err(RhiError::validation(
                    "R0102",
                    format!("texture '{label}' mip {mip} layer {layer} read before anything wrote it"),
                ));
            }
            match effect {
                ContentEffect::Keep => {}
                ContentEffect::Written => subresource.initialized = true,
                ContentEffect::Discarded => subresource.initialized = false,
            }
        }
        Ok(())
    }

    fn buffer_usage(&self, buffer: BufferId, usage: BufferUsage) -> Result<(), RhiError> {
        let target = self
            .state
            .buffers
            .get(buffer)
            .ok_or(RhiError::InvalidHandle("buffer"))?;
        if target.desc.usage.contains(usage) {
            Ok(())
        } else {
            Err(RhiError::validation(
                "R0105",
                format!("buffer '{}' used without {usage:?} usage", target.desc.label),
            ))
        }
    }

    fn copy_buffer_data(
        &mut self,
        src: BufferId,
        src_offset: u64,
        dst: BufferId,
        dst_offset: u64,
        size: u64,
    ) -> Result<(), RhiError> {
        self.buffer_usage(src, BufferUsage::COPY_SRC)?;
        self.buffer_usage(dst, BufferUsage::COPY_DST)?;
        let overflow = || RhiError::validation("R0108", "buffer copy out of bounds");
        let to_usize = |value: u64| usize::try_from(value).map_err(|_| overflow());
        let (src_start, dst_start, len) = (to_usize(src_offset)?, to_usize(dst_offset)?, to_usize(size)?);
        let bytes = self
            .state
            .buffers
            .get(src)
            .and_then(|buffer| buffer.data.get(src_start..src_start + len))
            .ok_or_else(overflow)?
            .to_vec();
        let target = self
            .state
            .buffers
            .get_mut(dst)
            .and_then(|buffer| buffer.data.get_mut(dst_start..dst_start + len))
            .ok_or_else(overflow)?;
        target.copy_from_slice(&bytes);
        Ok(())
    }
}
