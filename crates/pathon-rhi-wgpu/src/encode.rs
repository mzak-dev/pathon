use pathon_rhi_types::{
    BufferCopy, BufferId, Command, CommandList, ComputePassDesc, DepthAccess, Extent3d, LoadOp, RenderPassDesc,
    RhiError, StoreOp, TextureCopy, TextureViewId,
};

use crate::convert;
use crate::device::Objects;
use crate::interop::ExternalContext;

fn structure_error(message: String) -> RhiError {
    RhiError::validation("R0104", message)
}

fn new_encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pathon") })
}

// External callbacks get their own encoder, so the list is split into several command buffers that keep the
// recorded order when submitted together.
pub(crate) fn encode_list(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    objects: &Objects,
    list: CommandList,
) -> Result<Vec<wgpu::CommandBuffer>, RhiError> {
    let mut buffers = Vec::new();
    let mut encoder = new_encoder(device);
    let mut commands = list.into_commands().into_iter();
    while let Some(command) = commands.next() {
        if let Command::External(external) = command {
            buffers.push(encoder.finish());
            let mut context = ExternalContext::new(device, queue, objects, &external.accesses);
            external.callback.call(&mut context);
            buffers.push(context.finish());
            encoder = new_encoder(device);
        } else {
            encode_command(objects, &mut encoder, command, &mut commands)?;
        }
    }
    buffers.push(encoder.finish());
    Ok(buffers)
}

pub(crate) fn encode_into(
    objects: &Objects,
    encoder: &mut wgpu::CommandEncoder,
    list: CommandList,
) -> Result<(), RhiError> {
    let mut commands = list.into_commands().into_iter();
    while let Some(command) = commands.next() {
        if let Command::External(_) = command {
            return Err(RhiError::Unsupported("external commands need their own submission"));
        }
        encode_command(objects, encoder, command, &mut commands)?;
    }
    Ok(())
}

fn encode_command(
    objects: &Objects,
    encoder: &mut wgpu::CommandEncoder,
    command: Command,
    rest: &mut impl Iterator<Item = Command>,
) -> Result<(), RhiError> {
    match command {
        // wgpu tracks resource states itself; the plan only feeds validation on the null backend.
        Command::TextureAccess { .. } | Command::BufferAccess { .. } => {}
        Command::BeginRenderPass(desc) => encode_render_pass(objects, encoder, &desc, rest)?,
        Command::BeginComputePass(desc) => encode_compute_pass(objects, encoder, desc, rest)?,
        Command::CopyBufferToBuffer {
            src,
            src_offset,
            dst,
            dst_offset,
            size,
        } => {
            encoder.copy_buffer_to_buffer(
                buffer(objects, src)?,
                src_offset,
                buffer(objects, dst)?,
                dst_offset,
                size,
            );
        }
        Command::CopyBufferToTexture { src, dst, size } => {
            encoder.copy_buffer_to_texture(buffer_copy(objects, src)?, texture_copy(objects, dst)?, extent(size));
        }
        Command::CopyTextureToBuffer { src, dst, size } => {
            encoder.copy_texture_to_buffer(texture_copy(objects, src)?, buffer_copy(objects, dst)?, extent(size));
        }
        Command::CopyTextureToTexture { src, dst, size } => {
            encoder.copy_texture_to_texture(texture_copy(objects, src)?, texture_copy(objects, dst)?, extent(size));
        }
        Command::ResolveQueries {
            query_set,
            queries,
            dst,
            dst_offset,
        } => {
            let query_set = objects
                .query_sets
                .get(query_set)
                .ok_or(RhiError::InvalidHandle("query set"))?;
            encoder.resolve_query_set(query_set, queries, buffer(objects, dst)?, dst_offset);
        }
        Command::PushDebugGroup(label) => encoder.push_debug_group(label),
        Command::PopDebugGroup => encoder.pop_debug_group(),
        other => return Err(structure_error(format!("{other:?} recorded outside a pass"))),
    }
    Ok(())
}

fn encode_render_pass(
    objects: &Objects,
    encoder: &mut wgpu::CommandEncoder,
    desc: &RenderPassDesc,
    rest: &mut impl Iterator<Item = Command>,
) -> Result<(), RhiError> {
    let mut color_attachments = Vec::with_capacity(desc.color.len());
    for attachment in &desc.color {
        let load = match attachment.load {
            LoadOp::Clear(color) => wgpu::LoadOp::Clear(wgpu::Color {
                r: color.r,
                g: color.g,
                b: color.b,
                a: color.a,
            }),
            LoadOp::Load => wgpu::LoadOp::Load,
            LoadOp::DontCare => wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        };
        let resolve_target = match attachment.resolve_target {
            Some(resolve) => Some(view(objects, resolve)?),
            None => None,
        };
        color_attachments.push(Some(wgpu::RenderPassColorAttachment {
            view: view(objects, attachment.view)?,
            depth_slice: None,
            resolve_target,
            ops: wgpu::Operations {
                load,
                store: store_op(attachment.store),
            },
        }));
    }
    let depth_stencil_attachment = match desc.depth {
        Some(depth) => {
            let depth_ops = match depth.access {
                DepthAccess::ReadOnly => None,
                DepthAccess::ReadWrite => Some(wgpu::Operations {
                    load: match depth.load {
                        LoadOp::Clear(value) => wgpu::LoadOp::Clear(value),
                        LoadOp::Load => wgpu::LoadOp::Load,
                        LoadOp::DontCare => wgpu::LoadOp::Clear(0.0),
                    },
                    store: store_op(depth.store),
                }),
            };
            Some(wgpu::RenderPassDepthStencilAttachment {
                view: view(objects, depth.view)?,
                depth_ops,
                stencil_ops: None,
            })
        }
        None => None,
    };
    let timestamp_writes = match desc.timestamps {
        Some(timestamps) => Some(wgpu::RenderPassTimestampWrites {
            query_set: objects
                .query_sets
                .get(timestamps.query_set)
                .ok_or(RhiError::InvalidHandle("query set"))?,
            beginning_of_pass_write_index: Some(timestamps.begin_index),
            end_of_pass_write_index: Some(timestamps.end_index),
        }),
        None => None,
    };
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(desc.label),
        color_attachments: &color_attachments,
        depth_stencil_attachment,
        timestamp_writes,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    for command in rest.by_ref() {
        match command {
            Command::EndRenderPass => return Ok(()),
            Command::SetRenderPipeline(pipeline) => pass.set_pipeline(
                objects
                    .render_pipelines
                    .get(pipeline)
                    .ok_or(RhiError::InvalidHandle("render pipeline"))?,
            ),
            Command::SetBindGroup { index, group } => pass.set_bind_group(
                index,
                objects
                    .bind_groups
                    .get(group)
                    .ok_or(RhiError::InvalidHandle("bind group"))?,
                &[],
            ),
            Command::SetVertexBuffer {
                slot,
                buffer: id,
                offset,
            } => {
                pass.set_vertex_buffer(slot, buffer(objects, id)?.slice(offset..));
            }
            Command::SetIndexBuffer {
                buffer: id,
                format,
                offset,
            } => {
                pass.set_index_buffer(
                    buffer(objects, id)?.slice(offset..),
                    convert::index_format_to_wgpu(format),
                );
            }
            Command::SetViewport(viewport) => pass.set_viewport(
                viewport.x,
                viewport.y,
                viewport.width,
                viewport.height,
                viewport.min_depth,
                viewport.max_depth,
            ),
            Command::SetScissor(rect) => pass.set_scissor_rect(rect.x, rect.y, rect.width, rect.height),
            Command::Draw { vertices, instances } => pass.draw(vertices, instances),
            Command::DrawIndexed {
                indices,
                base_vertex,
                instances,
            } => pass.draw_indexed(indices, base_vertex, instances),
            Command::DrawIndexedIndirect { buffer: id, offset } => {
                pass.draw_indexed_indirect(buffer(objects, id)?, offset);
            }
            Command::PushDebugGroup(label) => pass.push_debug_group(label),
            Command::PopDebugGroup => pass.pop_debug_group(),
            other => {
                return Err(structure_error(format!(
                    "{other:?} is not allowed inside a render pass"
                )));
            }
        }
    }
    Err(structure_error(format!("render pass '{}' never ended", desc.label)))
}

fn encode_compute_pass(
    objects: &Objects,
    encoder: &mut wgpu::CommandEncoder,
    desc: ComputePassDesc,
    rest: &mut impl Iterator<Item = Command>,
) -> Result<(), RhiError> {
    let timestamp_writes = match desc.timestamps {
        Some(timestamps) => Some(wgpu::ComputePassTimestampWrites {
            query_set: objects
                .query_sets
                .get(timestamps.query_set)
                .ok_or(RhiError::InvalidHandle("query set"))?,
            beginning_of_pass_write_index: Some(timestamps.begin_index),
            end_of_pass_write_index: Some(timestamps.end_index),
        }),
        None => None,
    };
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some(desc.label),
        timestamp_writes,
    });
    for command in rest.by_ref() {
        match command {
            Command::EndComputePass => return Ok(()),
            Command::SetComputePipeline(pipeline) => pass.set_pipeline(
                objects
                    .compute_pipelines
                    .get(pipeline)
                    .ok_or(RhiError::InvalidHandle("compute pipeline"))?,
            ),
            Command::SetBindGroup { index, group } => pass.set_bind_group(
                index,
                objects
                    .bind_groups
                    .get(group)
                    .ok_or(RhiError::InvalidHandle("bind group"))?,
                &[],
            ),
            Command::Dispatch { x, y, z } => pass.dispatch_workgroups(x, y, z),
            Command::PushDebugGroup(label) => pass.push_debug_group(label),
            Command::PopDebugGroup => pass.pop_debug_group(),
            other => {
                return Err(structure_error(format!(
                    "{other:?} is not allowed inside a compute pass"
                )));
            }
        }
    }
    Err(structure_error(format!("compute pass '{}' never ended", desc.label)))
}

fn store_op(store: StoreOp) -> wgpu::StoreOp {
    match store {
        StoreOp::Store => wgpu::StoreOp::Store,
        StoreOp::Discard => wgpu::StoreOp::Discard,
    }
}

fn view(objects: &Objects, id: TextureViewId) -> Result<&wgpu::TextureView, RhiError> {
    objects.views.get(id).ok_or(RhiError::InvalidHandle("texture view"))
}

fn buffer(objects: &Objects, id: BufferId) -> Result<&wgpu::Buffer, RhiError> {
    objects.buffers.get(id).ok_or(RhiError::InvalidHandle("buffer"))
}

fn extent(size: Extent3d) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size.width,
        height: size.height,
        depth_or_array_layers: size.depth_or_layers,
    }
}

fn buffer_copy(objects: &Objects, copy: BufferCopy) -> Result<wgpu::TexelCopyBufferInfo<'_>, RhiError> {
    Ok(wgpu::TexelCopyBufferInfo {
        buffer: buffer(objects, copy.buffer)?,
        layout: wgpu::TexelCopyBufferLayout {
            offset: copy.layout.offset,
            bytes_per_row: Some(copy.layout.bytes_per_row),
            rows_per_image: Some(copy.layout.rows_per_image),
        },
    })
}

fn texture_copy(objects: &Objects, copy: TextureCopy) -> Result<wgpu::TexelCopyTextureInfo<'_>, RhiError> {
    let entry = objects
        .textures
        .get(copy.texture)
        .ok_or(RhiError::InvalidHandle("texture"))?;
    Ok(wgpu::TexelCopyTextureInfo {
        texture: &entry.texture,
        mip_level: copy.mip,
        origin: wgpu::Origin3d {
            x: copy.origin.x,
            y: copy.origin.y,
            z: copy.origin.z,
        },
        aspect: convert::aspect_to_wgpu(copy.aspect),
    })
}
