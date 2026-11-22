use pathon_api::resources::{UniformRing, UniformSlice};
use pathon_api::rhi::{
    BindGroupLayoutEntry, BindingType, ColorTargetState, DeviceApi, Format, OverrideValue, PrimitiveState,
    RenderPipelineDesc, RenderPipelineId, RhiError, SamplerBindingType, ShaderModuleId, ShaderStage, ShaderStages,
    TextureSampleType,
};
use pathon_api::{Diagnostic, RecordCtx};

use crate::settings::PostSettings;

pub(crate) const FULLSCREEN_WGSL: &str = include_str!("fullscreen.wgsl");
pub(crate) const COMMON_WGSL: &str = include_str!("common.wgsl");

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
            dimension: pathon_api::rhi::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

pub(crate) const fn sampler_entry(binding: u32, visibility: ShaderStages) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility,
        ty: BindingType::Sampler(SamplerBindingType::Filtering),
        count: None,
    }
}

pub(crate) fn fullscreen_pipeline(
    device: &dyn DeviceApi,
    label: &'static str,
    layout: pathon_api::rhi::BindGroupLayoutId,
    module: ShaderModuleId,
    fragment_entry: &'static str,
    constants: Vec<(&'static str, OverrideValue)>,
    format: Format,
) -> Result<RenderPipelineId, RhiError> {
    device.create_render_pipeline(&RenderPipelineDesc {
        label,
        bind_group_layouts: vec![layout],
        vertex: ShaderStage {
            module,
            entry_point: "vs_fullscreen",
            constants: vec![],
        },
        vertex_buffers: vec![],
        fragment: Some(ShaderStage {
            module,
            entry_point: fragment_entry,
            constants,
        }),
        color_targets: vec![ColorTargetState::opaque(format)],
        primitive: PrimitiveState::default(),
        depth: None,
        sample_count: 1,
        immediate_size: 0,
    })
}

// Targets whose numbers are not already sRGB-encoded by the hardware and are not floating point get the OETF in
// the shader; float targets keep display-linear values.
pub(crate) fn needs_manual_oetf(format: Format) -> bool {
    matches!(
        format,
        Format::Rgba8Unorm | Format::Bgra8Unorm | Format::Rgb10a2Unorm | Format::Rgba16Unorm
    )
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
            ring: UniformRing::new(device, label, 16 << 10)?,
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

// Settings come from `Shared` every frame, so the application changes them live. Out-of-range values are repaired
// and reported once, not every frame.
#[derive(Debug, Default)]
pub(crate) struct SettingsGate {
    reports_problems: bool,
    problems_reported: bool,
}

impl SettingsGate {
    pub(crate) fn new(reports_problems: bool) -> Self {
        Self {
            reports_problems,
            problems_reported: false,
        }
    }

    pub(crate) fn resolve(&mut self, ctx: &mut RecordCtx<'_>) -> PostSettings {
        let requested = ctx.shared().get::<PostSettings>().cloned().unwrap_or_default();
        let (settings, problems) = requested.sanitized();
        if problems.is_empty() {
            self.problems_reported = false;
        } else if self.reports_problems && !self.problems_reported {
            self.problems_reported = true;
            ctx.report(
                Diagnostic::warning("Q0001", format!("post settings repaired: {}", problems.join("; ")))
                    .with_hint("keep PostSettings inside the documented ranges"),
            );
        }
        settings
    }
}
