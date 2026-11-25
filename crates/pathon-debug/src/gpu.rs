use std::collections::HashMap;

use pathon_api::resources::{UniformRing, UniformSlice};
use pathon_api::rhi::{
    BindGroupLayoutEntry, BindingType, BufferDesc, BufferId, BufferUsage, DeviceApi, Format, PhysicalSize, RhiError,
    ShaderStages, TextureSampleType, TextureViewDimension,
};
use pathon_api::{Diagnostic, RecordCtx};

use crate::settings::DebugSettings;

pub(crate) const FRAME_WGSL: &str = include_str!("frame.wgsl");

pub(crate) fn joined_wgsl(parts: &[&str]) -> String {
    parts.join("\n")
}

pub(crate) const fn uniform_entry(binding: u32, visibility: ShaderStages) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility,
        ty: BindingType::UniformBuffer,
        count: None,
    }
}

pub(crate) const fn texture_entry(
    binding: u32,
    visibility: ShaderStages,
    sample_type: TextureSampleType,
) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility,
        ty: BindingType::SampledTexture {
            sample_type,
            dimension: TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// Targets whose bytes are not decoded as sRGB by the hardware and are not floating point take the colours as they are
/// (they are already display-encoded); the others want linear values and the shader linearizes first.
pub(crate) fn linearizes_for(format: Format) -> bool {
    !matches!(
        format,
        Format::Rgba8Unorm | Format::Bgra8Unorm | Format::Rgb10a2Unorm | Format::Rgba16Unorm
    )
}

/// The `Frame` uniform of `frame.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct FrameUniform {
    pub view_projection: [f32; 16],
    pub viewport: [f32; 2],
    pub depth_scale: [f32; 2],
    pub depth_bias: f32,
    pub padding: [f32; 3],
}

const _: () = assert!(size_of::<FrameUniform>() == 96);

impl FrameUniform {
    pub(crate) fn new(
        view_projection: pathon_api::glam::Mat4,
        output: PhysicalSize,
        render: PhysicalSize,
        depth_bias: f32,
    ) -> Self {
        let (output_width, output_height) = (output.width.max(1) as f32, output.height.max(1) as f32);
        Self {
            view_projection: view_projection.to_cols_array(),
            viewport: [output_width, output_height],
            depth_scale: [render.width as f32 / output_width, render.height as f32 / output_height],
            depth_bias,
            padding: [0.0; 3],
        }
    }
}

// Uniforms are written while the graph is built, so every record attempt (ADR-0022) starts the ring again.
#[derive(Debug)]
pub(crate) struct PerRecordUniforms {
    ring: UniformRing,
    epoch: Option<u64>,
}

impl PerRecordUniforms {
    pub(crate) fn new(device: &dyn DeviceApi, label: &'static str) -> Result<Self, RhiError> {
        Ok(Self {
            ring: UniformRing::new(device, label, 64 << 10)?,
            epoch: None,
        })
    }

    pub(crate) fn write(
        &mut self,
        device: &dyn DeviceApi,
        record_epoch: u64,
        bytes: &[u8],
    ) -> Result<UniformSlice, RhiError> {
        if self.epoch != Some(record_epoch) {
            self.ring.begin_frame();
            self.epoch = Some(record_epoch);
        }
        self.ring.write(device, bytes)
    }
}

/// A vertex buffer that grows by doubling and hands out a fresh region per write, so several views of one frame never
/// overwrite each other (queue writes land before the next submission, the regions restart each record attempt).
#[derive(Debug, Default)]
pub(crate) struct VertexArena {
    buffer: Option<(BufferId, u64)>,
    cursor: u64,
    epoch: Option<u64>,
    label: &'static str,
}

impl VertexArena {
    pub(crate) fn new(label: &'static str) -> Self {
        Self {
            label,
            ..Self::default()
        }
    }

    pub(crate) fn forget(&mut self) {
        self.buffer = None;
        self.cursor = 0;
        self.epoch = None;
    }

    /// Writes `bytes` at a fresh offset; the buffer it replaces, if it had to grow, goes to `ctx.release_later`.
    pub(crate) fn write(&mut self, ctx: &mut RecordCtx<'_>, bytes: &[u8]) -> Result<(BufferId, u64), RhiError> {
        let record_epoch = ctx.view().record_epoch;
        if self.epoch != Some(record_epoch) {
            self.epoch = Some(record_epoch);
            self.cursor = 0;
        }
        let offset = self.cursor.next_multiple_of(4);
        let needed = offset + bytes.len() as u64;
        let buffer = match self.buffer {
            Some((buffer, capacity)) if capacity >= needed => buffer,
            existing => {
                if let Some((old, _)) = existing {
                    ctx.release_later(old);
                }
                let capacity = needed.next_power_of_two().max(4096);
                let buffer = ctx.device().create_buffer(&BufferDesc {
                    label: self.label,
                    size: capacity,
                    usage: BufferUsage::VERTEX | BufferUsage::COPY_DST,
                })?;
                self.buffer = Some((buffer, capacity));
                buffer
            }
        };
        ctx.device().write_buffer(buffer, offset, bytes)?;
        self.cursor = needed;
        Ok((buffer, offset))
    }
}

/// Settings come from `Shared` every frame; a value outside its range is repaired and reported once.
#[derive(Debug, Default)]
pub(crate) struct ReportOnce {
    reported: HashMap<&'static str, bool>,
}

impl ReportOnce {
    /// Reports `diagnostic` the first time `active` is true after it was false.
    pub(crate) fn when(&mut self, key: &'static str, active: bool, ctx: &mut RecordCtx<'_>, diagnostic: impl FnOnce() -> Diagnostic) {
        let reported = self.reported.entry(key).or_default();
        if active && !*reported {
            ctx.report(diagnostic());
        }
        *reported = active;
    }
}

pub(crate) fn settings(ctx: &RecordCtx<'_>) -> DebugSettings {
    ctx.shared().get::<DebugSettings>().cloned().unwrap_or_default()
}
