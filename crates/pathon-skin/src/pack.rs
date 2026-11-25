use pathon_api::glam::Mat4;
use pathon_api::graph::{BufferHandle, PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindingResource, BufferId, BufferUsage, Caps, ComputePassDesc, DeviceApi,
    RhiError,
};
use pathon_api::{Negotiation, RecordCtx, RenderFeature, SetupCtx};
use pathon_mesh::{DEFORMED_POSITION_STRIDE, DeformedPositions};

use crate::deformed::{DEFORMED_SURFACE_STRIDE, SkinTargets};
use crate::error::SkinError;
use crate::frame::{MotionHistory, SkinFrame, SkinInstance, SkinOutput};
use crate::gpu::{FLAG_SKINNED, FLAG_WRITE_PREVIOUS, GrowBuffer, Params, SkinGpu, WORKGROUP_SIZE};

// Every way the deformed buffers are used around the pass; the pass itself moves them to `STORAGE_WRITE` and back.
const RESTING: Access = Access::VERTEX.union(Access::STORAGE_READ);
// Ahead of the core's "clear output" (priority 0) in the same stage, so their order is not left to registration.
const PASS_PRIORITY: i32 = -10;
const MATRIX_FLOATS: usize = 16;

/// Everything one dispatch needs, resolved while the graph is built.
#[derive(Clone, Debug)]
struct Job {
    params_offset: u64,
    positions: BufferId,
    surface: BufferId,
    skin: BufferId,
    morph: BufferId,
    groups: (u32, u32),
}

struct Prepared {
    jobs: Vec<Job>,
    pose: BufferId,
    params: BufferId,
}

/// Deforms the instances of the [`SkinFrame`] in `Shared` in one compute pass at the start of the frame, before
/// shadows and the depth prepass, and publishes a [`SkinOutput`] for the passes that read the result. The work runs
/// once per frame however many views there are (docs/crates/pathon-skin.md).
#[derive(Debug)]
pub struct SkinPack {
    gpu: Option<SkinGpu>,
    pose: GrowBuffer,
    params: GrowBuffer,
    // The record attempt that already holds the compute pass, and whether it had work.
    recorded: Option<(u64, bool)>,
    reported: Vec<String>,
}

impl Default for SkinPack {
    fn default() -> Self {
        Self::new()
    }
}

impl SkinPack {
    pub fn new() -> Self {
        Self {
            gpu: None,
            pose: GrowBuffer::new("skin pose", BufferUsage::STORAGE),
            params: GrowBuffer::new("skin params", BufferUsage::UNIFORM),
            recorded: None,
            reported: Vec::new(),
        }
    }

    fn prepare(
        &mut self,
        device: &dyn DeviceApi,
        gpu: &SkinGpu,
        frame: &SkinFrame,
        frame_number: u64,
        retired: &mut Vec<BufferId>,
    ) -> Result<(Option<Prepared>, Vec<SkinError>), RhiError> {
        let mut problems = Vec::new();
        let valid: Vec<&SkinInstance> = frame
            .instances
            .iter()
            .filter(|instance| match instance.validate(frame) {
                Ok(()) => true,
                Err(problem) => {
                    problems.push(problem);
                    false
                }
            })
            .collect();
        if valid.is_empty() {
            return Ok((None, problems));
        }
        let limits = &device.caps().limits;
        let stride = size_of::<Params>().next_multiple_of(limits.min_uniform_buffer_offset_alignment as usize);
        let matrix_floats = frame.bone_matrices.len() * MATRIX_FLOATS;
        let mut jobs = Vec::with_capacity(valid.len());
        let mut params = vec![0_u8; valid.len() * stride];
        for (index, instance) in valid.into_iter().enumerate() {
            let (params_for, groups) = job_params(
                instance,
                frame,
                frame_number,
                matrix_floats,
                limits.max_compute_workgroups_per_dimension,
            );
            let at = index * stride;
            params[at..at + size_of::<Params>()].copy_from_slice(bytemuck::bytes_of(&params_for));
            jobs.push(Job {
                params_offset: (index * stride) as u64,
                positions: instance.source.positions,
                surface: instance.source.surface,
                skin: instance.source.skin.unwrap_or(gpu.placeholder),
                morph: instance.morph.map_or(gpu.placeholder, |morph| morph.targets.buffer),
                groups,
            });
        }
        let mut pose = Vec::with_capacity(matrix_floats + frame.morph_weights.len() + 4);
        pose.extend(frame.bone_matrices.iter().flat_map(Mat4::to_cols_array));
        pose.extend_from_slice(&frame.morph_weights);
        pose.resize(pose.len().next_multiple_of(4).max(4), 0.0);
        let pose = self.pose.write(device, bytemuck::cast_slice(&pose), retired)?;
        let params = self.params.write(device, &params, retired)?;
        Ok((Some(Prepared { jobs, pose, params }), problems))
    }
}

fn job_params(
    instance: &SkinInstance,
    frame: &SkinFrame,
    frame_number: u64,
    matrix_floats: usize,
    max_groups: u32,
) -> (Params, (u32, u32)) {
    let bank_capacity = frame.targets.bank_capacity;
    let current_bank = DeformedPositions::current_bank(frame_number);
    let previous_bank = DeformedPositions::previous_bank(frame_number);
    let first = instance.range.first_vertex;
    let groups = instance.source.vertex_count.div_ceil(WORKGROUP_SIZE);
    let across = groups.min(max_groups).max(1);
    let rows = groups.div_ceil(across);
    let mut flags = 0;
    if instance.source.skin.is_some() {
        flags |= FLAG_SKINNED;
    }
    if instance.history == MotionHistory::Fresh {
        flags |= FLAG_WRITE_PREVIOUS;
    }
    let params = Params {
        vertex_count: instance.source.vertex_count,
        source_base_vertex: instance.source.base_vertex,
        position_current: current_bank * bank_capacity + first,
        position_previous: previous_bank * bank_capacity + first,
        surface_first: first,
        bone_base: instance.bones.map_or(0, |bones| bones.first * 4),
        bone_count: instance.bones.map_or(0, |bones| bones.count),
        weight_base: instance.morph.map_or(0, |morph| {
            u32::try_from(matrix_floats)
                .unwrap_or(u32::MAX)
                .saturating_add(morph.first_weight)
        }),
        morph_count: instance.morph.map_or(0, |morph| morph.targets.target_count()),
        morph_vertices: instance.morph.map_or(0, |morph| morph.targets.vertex_count()),
        flags,
        row_threads: across * WORKGROUP_SIZE,
    };
    (params, (across, rows))
}

fn buffer_resource(buffer: BufferId) -> BindingResource {
    BindingResource::whole_buffer(buffer)
}

fn bind_entries(job: &Job, prepared: (BufferId, BufferId), targets: &SkinTargets) -> Vec<BindGroupEntry> {
    let (pose, params) = prepared;
    let entry = |binding, resource| BindGroupEntry { binding, resource };
    vec![
        entry(
            0,
            BindingResource::Buffer {
                buffer: params,
                offset: job.params_offset,
                size: Some(size_of::<Params>() as u64),
            },
        ),
        entry(1, buffer_resource(pose)),
        entry(2, buffer_resource(job.positions)),
        entry(3, buffer_resource(job.surface)),
        entry(4, buffer_resource(job.skin)),
        entry(5, buffer_resource(job.morph)),
        entry(6, buffer_resource(targets.positions)),
        entry(7, buffer_resource(targets.surface)),
    ]
}

impl RenderFeature for SkinPack {
    fn name(&self) -> &'static str {
        "skin"
    }

    // Before shadows and the prepass, which read the deformed positions.
    fn stage(&self) -> Stage {
        Stage::Setup
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        let limits = &caps.limits;
        if limits.max_compute_invocations_per_workgroup >= WORKGROUP_SIZE
            && limits.max_compute_workgroup_size[0] >= WORKGROUP_SIZE
        {
            Negotiation::Full
        } else {
            Negotiation::Disabled {
                reason: "skinning needs compute workgroups of 64 invocations".into(),
            }
        }
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        self.gpu = Some(SkinGpu::create(ctx.device())?);
        // Buffers of a lost device are gone; a first record creates new ones.
        self.pose = GrowBuffer::new("skin pose", BufferUsage::STORAGE);
        self.params = GrowBuffer::new("skin params", BufferUsage::UNIFORM);
        self.recorded = None;
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let gpu = self.gpu.ok_or(RhiError::Unsupported("skin used before setup"))?;
        let (frame_number, epoch) = (ctx.view().frame, ctx.view().record_epoch);
        let first_view = self.recorded.is_none_or(|(recorded, _)| recorded != epoch);

        let mut retired = Vec::new();
        let (targets, prepared, problems) = {
            let Some(frame) = ctx.shared().get::<SkinFrame>() else {
                self.recorded = Some((epoch, false));
                ctx.shared_mut().remove::<SkinOutput>();
                return Ok(());
            };
            let (prepared, problems) = if first_view {
                self.prepare(ctx.device(), &gpu, frame, frame_number, &mut retired)?
            } else {
                (None, Vec::new())
            };
            (frame.targets, prepared, problems)
        };
        for buffer in retired {
            ctx.release_later(buffer);
        }

        if first_view {
            self.recorded = Some((epoch, prepared.is_some()));
            self.report(ctx, &problems);
        }
        if !self.recorded.is_some_and(|(_, active)| active) {
            ctx.shared_mut().remove::<SkinOutput>();
            return Ok(());
        }

        let positions_size = 2 * u64::from(targets.bank_capacity) * DEFORMED_POSITION_STRIDE;
        let surface_size = u64::from(targets.bank_capacity) * DEFORMED_SURFACE_STRIDE;
        let positions = ctx.graph().import_buffer(
            "skin deformed positions",
            targets.positions,
            positions_size,
            RESTING,
            RESTING,
        );
        let surface =
            ctx.graph()
                .import_buffer("skin deformed surface", targets.surface, surface_size, RESTING, RESTING);
        let (positions, surface) = match prepared {
            Some(prepared) => record_pass(ctx, &gpu, prepared, targets, (positions, surface)),
            None => (positions, surface),
        };
        ctx.shared_mut().insert(SkinOutput {
            positions,
            surface,
            bank_capacity: targets.bank_capacity,
            frame: frame_number,
        });
        Ok(())
    }
}

fn record_pass(
    ctx: &mut RecordCtx<'_>,
    gpu: &SkinGpu,
    prepared: Prepared,
    targets: SkinTargets,
    (positions, surface): (BufferHandle, BufferHandle),
) -> (BufferHandle, BufferHandle) {
    let gpu = *gpu;
    let mut pass = ctx.graph().add_pass(
        PassDesc::new("skin", Stage::Setup)
            .priority(PASS_PRIORITY)
            .side_effect(),
    );
    let positions = pass.modify_buffer(positions, Access::STORAGE_WRITE);
    let surface = pass.modify_buffer(surface, Access::STORAGE_WRITE);
    pass.record(move |pass| {
        let groups = prepared
            .jobs
            .iter()
            .map(|job| {
                pass.create_bind_group(&BindGroupDesc {
                    label: "skin",
                    layout: gpu.layout,
                    entries: bind_entries(job, (prepared.pose, prepared.params), &targets),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut compute = pass.commands().compute_pass(ComputePassDesc {
            label: "skin",
            timestamps: None,
        });
        compute.set_pipeline(gpu.pipeline);
        for (job, group) in prepared.jobs.iter().zip(groups) {
            compute.set_bind_group(0, group);
            compute.dispatch(job.groups.0, job.groups.1, 1);
        }
        Ok(())
    });
    (positions, surface)
}

impl SkinPack {
    fn report(&mut self, ctx: &mut RecordCtx<'_>, problems: &[SkinError]) {
        let messages: Vec<String> = problems.iter().map(ToString::to_string).collect();
        if messages != self.reported {
            for problem in problems {
                ctx.report(problem.to_diagnostic());
            }
            self.reported = messages;
        }
    }
}
