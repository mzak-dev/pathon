use std::collections::HashMap;

use pathon_api::glam::Mat4;
use pathon_api::graph::{PassDesc, Stage, TextureHandle};
use pathon_api::resources::UniformSlice;
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutId, BindingResource, Caps, Color,
    ColorAttachment, ColorTargetState, Format, FormatCaps, LoadOp, OverrideValue, PhysicalSize, PrimitiveState,
    RenderPassDesc, RenderPipelineDesc, RenderPipelineId, RhiError, ShaderModuleDesc, ShaderModuleId, ShaderStage,
    ShaderStages, TextureSampleType, Viewport,
};
use pathon_api::{Diagnostic, Negotiation, PrepareCtx, RecordCtx, RenderFeature, SetupCtx, Slot};

use crate::gpu::{PerRecordUniforms, ReportOnce, joined_wgsl, linearizes_for, settings, texture_entry, uniform_entry};
use crate::reference;
use crate::settings::{Corner, DebugSettings, PreviewLayout, PreviewMode, PreviewSlot, SlotPreview, SourceKind};

const COMMON_WGSL: &str = include_str!("preview_common.wgsl");
const SHADE_WGSL: &str = include_str!("preview_shade.wgsl");
const FLOAT_WGSL: &str = include_str!("preview_float.wgsl");
const DEPTH_WGSL: &str = include_str!("preview_depth.wgsl");
const UINT_WGSL: &str = include_str!("preview_uint.wgsl");

const CORNER_MARGIN: u32 = 8;
const GRID_GAP: u32 = 2;
const GRID_BACKGROUND: f32 = 0.04;

// A clear value does not go through the shader: targets that decode their bytes as linear need it decoded here too.
fn grid_background(target: Format) -> Color {
    let level = if linearizes_for(target) {
        reference::srgb_eotf(GRID_BACKGROUND)
    } else {
        GRID_BACKGROUND
    };
    let level = f64::from(level);
    Color::rgba(level, level, level, 1.0)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PreviewParams {
    inverse_projection: [f32; 16],
    mode: u32,
    level: u32,
    use_exposure_texture: u32,
    exposure: f32,
    depth_near: f32,
    depth_far: f32,
    motion_scale: f32,
    padding: f32,
}

const _: () = assert!(size_of::<PreviewParams>() == 96);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    fn viewport(self) -> Viewport {
        Viewport {
            x: self.x as f32,
            y: self.y as f32,
            width: self.width as f32,
            height: self.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        }
    }
}

/// Where each of `count` previews goes on an output of `size`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "sizes are rounded, non-negative and not larger than the output"
)]
pub(crate) fn cell_rects(layout: PreviewLayout, count: usize, size: PhysicalSize, corner_size: f32) -> Vec<Rect> {
    let full = Rect {
        x: 0,
        y: 0,
        width: size.width.max(1),
        height: size.height.max(1),
    };
    match layout {
        PreviewLayout::Fullscreen => vec![full],
        PreviewLayout::Corner(corner) => {
            let width = ((full.width as f32 * corner_size).round() as u32).clamp(1, full.width);
            let height = ((full.height as f32 * corner_size).round() as u32).clamp(1, full.height);
            let margin = |room: u32| CORNER_MARGIN.min(room / 2);
            let (margin_x, margin_y) = (margin(full.width - width), margin(full.height - height));
            let (left, top) = match corner {
                Corner::TopLeft => (true, true),
                Corner::TopRight => (false, true),
                Corner::BottomLeft => (true, false),
                Corner::BottomRight => (false, false),
            };
            vec![Rect {
                x: if left { margin_x } else { full.width - width - margin_x },
                y: if top { margin_y } else { full.height - height - margin_y },
                width,
                height,
            }]
        }
        PreviewLayout::Grid => {
            let count = count.max(1) as u32;
            let columns = (f64::from(count).sqrt().ceil() as u32).max(1);
            let rows = count.div_ceil(columns);
            let (cell_width, cell_height) = (full.width / columns, full.height / rows);
            let gap = if cell_width > 4 * GRID_GAP && cell_height > 4 * GRID_GAP {
                GRID_GAP
            } else {
                0
            };
            (0..count)
                .map(|index| Rect {
                    x: (index % columns) * cell_width + gap,
                    y: (index / columns) * cell_height + gap,
                    width: (cell_width - 2 * gap).max(1),
                    height: (cell_height - 2 * gap).max(1),
                })
                .collect()
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct KindGpu {
    module: ShaderModuleId,
    layout: BindGroupLayoutId,
}

#[derive(Clone, Copy, Debug)]
struct PreviewGpu {
    float: KindGpu,
    depth: KindGpu,
    uint: KindGpu,
    missing: KindGpu,
}

impl PreviewGpu {
    fn kind(&self, kind: Option<SourceKind>) -> KindGpu {
        match kind {
            Some(SourceKind::Float) => self.float,
            Some(SourceKind::Depth) => self.depth,
            Some(SourceKind::Uint) => self.uint,
            None => self.missing,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct CellDraw {
    kind: Option<SourceKind>,
    pipeline: RenderPipelineId,
    layout: BindGroupLayoutId,
    source: Option<TextureHandle>,
    exposure: Option<TextureHandle>,
    uniform: UniformSlice,
    viewport: Viewport,
}

/// Shows base slots in `Slot::OUTPUT`: fullscreen, in a corner or as a grid. It reads the slots other packs write and
/// asks for the on-demand ones in `prepare_view`; a slot nobody writes is a checkered tile and a U0001.
#[derive(Debug)]
pub struct SlotPreviewFeature {
    gpu: Option<PreviewGpu>,
    pipelines: HashMap<(Option<SourceKind>, Format), RenderPipelineId>,
    uniforms: Option<PerRecordUniforms>,
    reports: ReportOnce,
}

impl Default for SlotPreviewFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl SlotPreviewFeature {
    pub fn new() -> Self {
        Self {
            gpu: None,
            pipelines: HashMap::new(),
            uniforms: None,
            reports: ReportOnce::default(),
        }
    }

    fn pipeline(
        &mut self,
        ctx: &RecordCtx<'_>,
        gpu: &PreviewGpu,
        kind: Option<SourceKind>,
        target: Format,
    ) -> Result<RenderPipelineId, RhiError> {
        if let Some(pipeline) = self.pipelines.get(&(kind, target)) {
            return Ok(*pipeline);
        }
        let kind_gpu = gpu.kind(kind);
        let pipeline = ctx.device().create_render_pipeline(&RenderPipelineDesc {
            label: "debug slot preview",
            bind_group_layouts: vec![kind_gpu.layout],
            vertex: ShaderStage {
                module: kind_gpu.module,
                entry_point: "vs_fullscreen",
                constants: vec![],
            },
            vertex_buffers: vec![],
            fragment: Some(ShaderStage {
                module: kind_gpu.module,
                entry_point: if kind.is_some() { "fs_preview" } else { "fs_missing" },
                constants: vec![("LINEARIZE", OverrideValue::from(linearizes_for(target)))],
            }),
            color_targets: vec![ColorTargetState::opaque(target)],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        })?;
        self.pipelines.insert((kind, target), pipeline);
        Ok(pipeline)
    }
}

fn params_for(
    slot: PreviewSlot,
    preview: &SlotPreview,
    inverse_projection: Mat4,
    uses_exposure_texture: bool,
) -> PreviewParams {
    PreviewParams {
        inverse_projection: inverse_projection.to_cols_array(),
        mode: slot.mode() as u32,
        level: if slot == PreviewSlot::DepthPyramid {
            preview.pyramid_level
        } else {
            0
        },
        use_exposure_texture: u32::from(uses_exposure_texture && slot.mode() == PreviewMode::ExposedHdr),
        exposure: preview.exposure,
        depth_near: preview.depth_range.0,
        depth_far: preview.depth_range.1,
        motion_scale: preview.motion_scale,
        padding: 0.0,
    }
}

impl RenderFeature for SlotPreviewFeature {
    fn name(&self) -> &'static str {
        "debug slot preview"
    }

    fn stage(&self) -> Stage {
        Stage::Overlay
    }

    fn priority(&self) -> i32 {
        0
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        let sampled = |format| caps.format(format).contains(FormatCaps::SAMPLED);
        if sampled(Format::Rgba16Float) && sampled(Format::R32Float) {
            Negotiation::Full
        } else {
            Negotiation::Disabled {
                reason: "slot previews need sampled Rgba16Float and R32Float textures".to_owned(),
            }
        }
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        let fragment = ShaderStages::FRAGMENT;
        let make = |label: &'static str, kind: Option<(&str, Vec<TextureSampleType>)>| -> Result<KindGpu, RhiError> {
            let mut parts = vec![COMMON_WGSL];
            let mut entries = vec![uniform_entry(0, fragment)];
            if let Some((snippet, sample_types)) = &kind {
                parts.extend([*snippet, SHADE_WGSL]);
                for (binding, sample_type) in (1_u32..).zip(sample_types) {
                    entries.push(texture_entry(binding, fragment, *sample_type));
                }
            }
            let module = device.create_shader_module(&ShaderModuleDesc {
                label,
                wgsl: &joined_wgsl(&parts),
            })?;
            let layout = device.create_bind_group_layout(&BindGroupLayoutDesc { label, entries })?;
            Ok(KindGpu { module, layout })
        };
        self.gpu = Some(PreviewGpu {
            float: make(
                "debug preview float",
                Some((
                    FLOAT_WGSL,
                    vec![
                        TextureSampleType::UnfilterableFloat,
                        TextureSampleType::UnfilterableFloat,
                    ],
                )),
            )?,
            depth: make(
                "debug preview depth",
                Some((DEPTH_WGSL, vec![TextureSampleType::Depth])),
            )?,
            uint: make("debug preview uint", Some((UINT_WGSL, vec![TextureSampleType::Uint])))?,
            missing: make("debug preview missing", None)?,
        });
        self.pipelines.clear();
        self.uniforms = Some(PerRecordUniforms::new(device, "debug preview uniforms")?);
        Ok(())
    }

    fn prepare_view(&mut self, ctx: &mut PrepareCtx<'_>) {
        let Some(preview) = ctx
            .shared()
            .get::<DebugSettings>()
            .and_then(|settings| settings.preview.clone())
        else {
            return;
        };
        for slot in preview.visible_slots() {
            if slot.slot().is_on_demand() {
                ctx.request(slot.slot());
            }
        }
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(requested) = settings(ctx).preview else {
            return Ok(());
        };
        let (preview, problems) = requested.sanitized();
        let no_slots = preview.visible_slots().is_empty();
        self.reports
            .when("preview settings", !problems.is_empty() || no_slots, ctx, || {
                let message = if no_slots {
                    "slot preview has no slots".to_owned()
                } else {
                    format!("slot preview settings repaired: {}", problems.join("; "))
                };
                Diagnostic::warning("U0003", message)
                    .with_hint("keep SlotPreview inside the documented ranges and list at least one slot")
            });
        let Some(output) = ctx.slot(Slot::OUTPUT) else {
            return Ok(());
        };
        if no_slots {
            return Ok(());
        }
        let gpu = self
            .gpu
            .ok_or(RhiError::Unsupported("slot preview used before setup"))?;
        let target = ctx
            .graph()
            .texture_format(output)
            .ok_or(RhiError::InvalidHandle("output"))?;
        let view = ctx.view();
        let (output_size, inverse_projection, record_epoch) =
            (view.output_size, view.camera.projection.inverse(), view.record_epoch);
        let exposure_slot = if preview.exposure_from_slot {
            ctx.slot(Slot::EXPOSURE)
        } else {
            None
        };
        let rects = cell_rects(
            preview.layout,
            preview.visible_slots().len(),
            output_size,
            preview.corner_size,
        );

        let mut cells = Vec::with_capacity(rects.len());
        for (slot, rect) in preview.visible_slots().iter().zip(rects) {
            let source = ctx.slot(slot.slot());
            let slot_name = slot.slot().name();
            self.reports.when(slot_name, source.is_none(), ctx, || {
                Diagnostic::warning("U0001", format!("slot '{slot_name}' is not available, the preview shows a checkered tile"))
                    .with_hint("register the pack that writes this slot; on-demand slots are requested and produced only if some pack writes them")
            });
            let kind = source.map(|_| slot.kind());
            let params = params_for(*slot, &preview, inverse_projection, exposure_slot.is_some());
            let uniform = self
                .uniforms
                .as_mut()
                .ok_or(RhiError::Unsupported("slot preview used before setup"))?
                .write(ctx.device(), record_epoch, bytemuck::bytes_of(&params))?;
            cells.push(CellDraw {
                kind,
                pipeline: self.pipeline(ctx, &gpu, kind, target)?,
                layout: gpu.kind(kind).layout,
                source,
                exposure: exposure_slot.filter(|_| source.is_some() && slot.mode() == PreviewMode::ExposedHdr),
                uniform,
                viewport: rect.viewport(),
            });
        }

        let load = match preview.layout {
            PreviewLayout::Fullscreen => LoadOp::Clear(Color::BLACK),
            PreviewLayout::Grid => LoadOp::Clear(grid_background(target)),
            PreviewLayout::Corner(_) => LoadOp::Load,
        };
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("debug slot preview", Stage::Overlay));
        for cell in &mut cells {
            let access = if cell.kind == Some(SourceKind::Depth) {
                Access::SAMPLED | Access::DEPTH_READ
            } else {
                Access::SAMPLED
            };
            cell.source = cell.source.map(|source| pass.read(source, access));
            cell.exposure = cell.exposure.map(|exposure| pass.read(exposure, Access::SAMPLED));
        }
        let output = pass.modify(output, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let target = pass.view(output)?;
            let mut groups = Vec::with_capacity(cells.len());
            for cell in &cells {
                let mut entries = vec![BindGroupEntry {
                    binding: 0,
                    resource: cell.uniform.binding(),
                }];
                if let Some(source) = cell.source {
                    let source_view = pass.view(source)?;
                    entries.push(BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::TextureView(source_view),
                    });
                    // Only the float layout has a second texture: the exposure, or the source again when unused.
                    if cell.kind == Some(SourceKind::Float) {
                        let exposure_view = match cell.exposure {
                            Some(exposure) => pass.view(exposure)?,
                            None => source_view,
                        };
                        entries.push(BindGroupEntry {
                            binding: 2,
                            resource: BindingResource::TextureView(exposure_view),
                        });
                    }
                }
                groups.push(pass.create_bind_group(&BindGroupDesc {
                    label: "debug slot preview",
                    layout: cell.layout,
                    entries,
                })?);
            }
            let mut render = pass.commands().render_pass(RenderPassDesc {
                label: "debug slot preview",
                color: vec![ColorAttachment {
                    load,
                    ..ColorAttachment::load(target)
                }],
                depth: None,
                timestamps: None,
            });
            for (cell, group) in cells.iter().zip(groups) {
                render.set_viewport(cell.viewport);
                render.set_pipeline(cell.pipeline);
                render.set_bind_group(0, group);
                render.draw(0..3, 0..1);
            }
            Ok(())
        });
        ctx.set_slot(Slot::OUTPUT, output);
        Ok(())
    }
}
