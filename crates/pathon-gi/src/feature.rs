use std::collections::HashMap;

use pathon_api::glam::{Mat3, Quat, Vec3};
use pathon_api::graph::{BufferHandle, PassDesc, Stage, TextureHandle};
use pathon_api::resources::{UniformRing, UniformSlice};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupId, BindingResource, BufferDesc, BufferId, BufferUsage, Caps,
    ComputePassDesc, DeviceApi, Format, FormatCaps, RhiError, SamplerId, TextureViewId,
};
use pathon_api::{Diagnostic, Negotiation, RecordCtx, RenderFeature, SetupCtx};
use pathon_bindless::TextureBinder;
use pathon_ibl::{IblEnvironment, IblTexture};
use pathon_material::{MaterialHooks, MaterialTable};
use pathon_mesh::GeometryPool;
use pathon_scene::{Aabb, Snapshot};

use crate::frame::GiFrame;
use crate::gpu::{GiResources, atlas_sampler, black_cube};
use crate::input::{SceneInput, VoxelJob, check_pool, gather};
use crate::lights::{GiLight, pack_lights};
use crate::settings::{GiSettings, MAX_RAYS_PER_FRAME};
use crate::shaders::{GiShaders, Pass};
use crate::status::GiStatus;
use crate::volume::{GiVolumeUniforms, ProbeLayout};
use crate::voxel::{VoxelGridUniforms, VoxelLayout};

/// Frames over which the hysteresis of a new volume ramps from none to the setting: the first frames take the new rays
/// as they are, so the volume does not start dark.
const WARMUP_FRAMES: u32 = 8;
const JOB_STRIDE: u64 = 256;
const JOB_STRIDE_BYTES: usize = 256;
const RESTING: Access = Access::STORAGE_READ;
const READ_WRITE: Access = Access::STORAGE_READ.union(Access::STORAGE_WRITE);
// Ahead of the core's "clear output" (priority 0) in the same stage, so their order is not left to registration.
const PASS_PRIORITY: i32 = -10;
const WORKGROUP: u32 = 64;
const MAX_GROUPS: u32 = 65_535;
const ATLAS_WORKGROUP: u32 = 8;

/// The bytes of WGSL `GiPass`, 112 bytes in a uniform buffer.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PassUniforms {
    frame: [u32; 4],
    relight: [u32; 4],
    rotation_x: [f32; 4],
    rotation_y: [f32; 4],
    rotation_z: [f32; 4],
    blend: [f32; 4],
    reach: [f32; 4],
}

/// The bytes of WGSL `Job`, 112 bytes in a uniform buffer.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct JobUniforms {
    rows: [[f32; 4]; 3],
    mesh: [u32; 4],
    albedo: [f32; 4],
    emissive: [f32; 4],
    textures: [u32; 4],
}

impl JobUniforms {
    fn new(job: &VoxelJob) -> Self {
        Self {
            rows: job.rows,
            #[expect(clippy::cast_sign_loss, reason = "the shader reads the bits back as an i32")]
            mesh: [job.first_index, job.triangles, job.base_vertex as u32, 0],
            albedo: job.albedo,
            emissive: job.emissive,
            textures: [job.base_color_slot, job.emissive_slot, 0, 0],
        }
    }
}

#[derive(Debug, Default)]
#[expect(clippy::struct_excessive_bools, reason = "one flag per problem that is reported once")]
struct Reported {
    settings: bool,
    inputs: bool,
    pool: bool,
    empty: bool,
}

#[derive(Debug)]
struct Gpu {
    sampler: SamplerId,
    black_environment: IblTexture,
    uniforms: UniformRing,
    uniform_epoch: Option<u64>,
}

impl Gpu {
    fn write_uniform(&mut self, ctx: &RecordCtx<'_>, bytes: &[u8]) -> Result<UniformSlice, RhiError> {
        let epoch = ctx.view().record_epoch;
        if self.uniform_epoch != Some(epoch) {
            self.uniforms.begin_frame();
            self.uniform_epoch = Some(epoch);
        }
        self.uniforms.write(ctx.device(), bytes)
    }
}

// What one frame does to the volume, resolved while the graph is built: everything the pass closures own.
#[derive(Debug)]
struct Plan {
    voxelize: Option<VoxelizePlan>,
    cells: u32,
    slices: u32,
    batch_rays: u32,
    atlas: [(u32, u32); 2],
    grid_uniform: UniformSlice,
    volume_uniform: UniformSlice,
    pass_uniform: UniformSlice,
    environment: (IblTexture, SamplerId),
    previous: usize,
    next: usize,
}

#[derive(Debug)]
struct VoxelizePlan {
    buffer: BufferId,
    jobs: Vec<PlannedJob>,
}

#[derive(Debug)]
struct PlannedJob {
    offset: u64,
    positions: BufferId,
    surface: BufferId,
    indices: BufferId,
    triangles: u32,
    textures: Option<BindGroupId>,
}

/// The probe volume as a render feature: voxelizes the scene, lights the voxels, traces rays from the probes and folds
/// them into the atlases, every frame, in `Stage::Setup` (before the passes that read the atlases). Publishes a
/// [`GiFrame`] and a [`GiStatus`] in `Shared`.
#[derive(Debug, Default)]
pub struct GiFeature {
    gpu: Option<Gpu>,
    shaders: Option<(String, GiShaders)>,
    resources: Option<GiResources>,
    scene_signature: Option<u64>,
    frames_in_volume: u32,
    frame_number: u32,
    cursor: u32,
    latest: usize,
    recorded: Option<(u64, bool)>,
    plan: Option<Plan>,
    reported: Reported,
}

impl GiFeature {
    pub fn new() -> Self {
        Self::default()
    }

    fn report_once(ctx: &mut RecordCtx<'_>, flag: &mut bool, diagnostic: Diagnostic) {
        if !std::mem::replace(flag, true) {
            ctx.report(diagnostic);
        }
    }

    fn settings(&mut self, ctx: &mut RecordCtx<'_>) -> GiSettings {
        let requested = ctx.shared().get::<GiSettings>().copied().unwrap_or_default();
        let (settings, problem) = requested.sanitized();
        match problem {
            Some(problem) => Self::report_once(
                ctx,
                &mut self.reported.settings,
                Diagnostic::warning("J0001", format!("GI settings replaced by the defaults: {problem}"))
                    .with_hint("keep GiSettings inside the documented ranges"),
            ),
            None => self.reported.settings = false,
        }
        settings
    }

    fn release_resources(&mut self, ctx: &mut RecordCtx<'_>) {
        if let Some(resources) = self.resources.take() {
            for resource in resources.resources() {
                ctx.release_later(resource);
            }
        }
        self.scene_signature = None;
    }

    // The volume for `scene`: made again when the settings or the scene's box changed what it has to cover.
    fn ensure_volume(
        &mut self,
        ctx: &mut RecordCtx<'_>,
        settings: &GiSettings,
        bounds: Aabb,
    ) -> Result<bool, RhiError> {
        let probes_per_frame = settings.probes_per_frame.min(settings.max_probes);
        let batch_cap = (MAX_RAYS_PER_FRAME / settings.rays_per_probe).max(1);
        let settings = GiSettings {
            probes_per_frame: probes_per_frame.min(batch_cap),
            ..*settings
        };
        let (Some(volume), Some(grid)) = (
            ProbeLayout::fit(bounds, settings.probe_spacing, settings.max_probes),
            VoxelLayout::fit(bounds, settings.voxel_size, settings.max_voxels),
        ) else {
            return Ok(false);
        };
        let unchanged = self.resources.as_ref().is_some_and(|resources| {
            resources.settings.same_volume(&settings) && resources.volume == volume && resources.grid == grid
        });
        if unchanged {
            if let Some(resources) = &mut self.resources {
                resources.settings = settings;
            }
            return Ok(false);
        }
        self.release_resources(ctx);
        self.resources = Some(GiResources::create(ctx.device(), volume, grid, settings)?);
        self.frames_in_volume = 0;
        self.cursor = 0;
        self.latest = 0;
        Ok(true)
    }

    fn ensure_shaders(
        &mut self,
        device: &dyn DeviceApi,
        hooks: &MaterialHooks,
        binder: Option<&TextureBinder>,
    ) -> Result<(), RhiError> {
        let mut hooks = hooks.clone();
        if let Some(binder) = binder {
            hooks.set_texture_source(binder.texture_source_wgsl(crate::shaders::TEXTURE_GROUP));
        }
        let fingerprint: String = hooks
            .modules()
            .iter()
            .flat_map(|(path, source)| [*path, ":", source])
            .collect();
        if self.shaders.as_ref().is_none_or(|(known, _)| *known != fingerprint) {
            let shaders = GiShaders::new(device, &hooks, binder.map(TextureBinder::layout))?;
            self.shaders = Some((fingerprint, shaders));
        }
        Ok(())
    }

    // Decides what this frame does and uploads its data; `None` (nothing published) when there is nothing to light.
    fn prepare(&mut self, ctx: &mut RecordCtx<'_>) -> Result<Option<Plan>, RhiError> {
        let settings = self.settings(ctx);
        if !settings.enabled {
            return Ok(None);
        }
        let mut binder = ctx.shared_mut().remove::<TextureBinder>();
        let result = self.prepare_with(ctx, &settings, binder.as_mut());
        if let Some(binder) = binder {
            ctx.shared_mut().insert(binder);
        }
        result
    }

    fn scene_input(&mut self, ctx: &mut RecordCtx<'_>) -> Option<(SceneInput, Vec<GiLight>)> {
        let shared = ctx.shared();
        let world = shared.get::<Snapshot>().cloned();
        let present = world.is_some() && shared.contains::<GeometryPool>() && shared.contains::<MaterialTable>();
        if !present {
            Self::report_once(
                ctx,
                &mut self.reported.inputs,
                Diagnostic::warning(
                    "J0002",
                    "GI needs a scene Snapshot, a GeometryPool and a MaterialTable in Shared",
                )
                .with_hint("insert them with Renderer::shared_mut(); the snapshot every frame"),
            );
            return None;
        }
        self.reported.inputs = false;
        let world = world?;
        let shared = ctx.shared();
        let pool = shared.get::<GeometryPool>()?;
        if check_pool(pool).is_err() {
            let diagnostic = Diagnostic::error(
                "J0003",
                "the geometry pool has no standard position and surface streams to voxelize",
            )
            .with_hint("use VertexLayout::standard() (or skinned()) for the pool GiPack voxelizes");
            Self::report_once(ctx, &mut self.reported.pool, diagnostic);
            return None;
        }
        self.reported.pool = false;
        let (pool, materials) = (
            ctx.shared().get::<GeometryPool>()?,
            ctx.shared().get::<MaterialTable>()?,
        );
        Some((gather(&world, pool, materials), pack_lights(&world)))
    }

    #[expect(clippy::too_many_lines, reason = "one straight-line preparation of a frame")]
    fn prepare_with(
        &mut self,
        ctx: &mut RecordCtx<'_>,
        settings: &GiSettings,
        mut binder: Option<&mut TextureBinder>,
    ) -> Result<Option<Plan>, RhiError> {
        let Some((input, lights)) = self.scene_input(ctx) else {
            return Ok(None);
        };
        let Some(bounds) = settings.bounds.or(input.bounds) else {
            Self::report_once(
                ctx,
                &mut self.reported.empty,
                Diagnostic::warning(
                    "J0004",
                    "GI found nothing to voxelize: no visible opaque instance has a mesh",
                )
                .with_hint("give GiSettings::bounds to light a scene that is filled later"),
            );
            return Ok(None);
        };
        self.reported.empty = false;
        let Some(gpu) = self.gpu.as_mut() else {
            return Err(RhiError::Unsupported("GI used before setup"));
        };
        let (sampler, black_environment) = (gpu.sampler, gpu.black_environment);

        let rebuilt = self.ensure_volume(ctx, settings, bounds)?;
        let Some(mut resources) = self.resources else {
            return Ok(None);
        };
        resources.settings = GiSettings {
            probes_per_frame: resources.settings.probes_per_frame,
            ..*settings
        };
        self.resources = Some(resources);

        let hooks = ctx.shared().get::<MaterialHooks>().cloned().unwrap_or_default();
        self.ensure_shaders(ctx.device(), &hooks, binder.as_deref())?;
        if let Some(binder) = binder.as_deref_mut() {
            binder
                .prepare(ctx.device())
                .map_err(|error| RhiError::validation("J0006", error.to_string()))?;
        }

        let changed = rebuilt || self.scene_signature != Some(input.signature);
        let voxelize = if changed {
            self.scene_signature = Some(input.signature);
            let plan = plan_voxelization(ctx, &input.jobs, binder)?;
            if self.frames_in_volume > WARMUP_FRAMES {
                self.frames_in_volume = WARMUP_FRAMES / 2;
            }
            Some(plan)
        } else {
            None
        };

        // Lights: written to the buffer the passes read.
        if let Some(old) = self.resources.as_mut().map_or(Ok(None), |resources| {
            resources.grow_lights(ctx.device(), u32::try_from(lights.len()).unwrap_or(u32::MAX))
        })? {
            ctx.release_later(old);
        }
        let Some(resources) = self.resources else {
            return Ok(None);
        };
        if !lights.is_empty() {
            ctx.device()
                .write_buffer(resources.lights, 0, bytemuck::cast_slice(&lights))?;
        }

        let environment = {
            let published = ctx.shared().get::<IblEnvironment>().copied();
            match published {
                Some(environment) => (environment.prefiltered, environment.sampler),
                None => (black_environment, sampler),
            }
        };

        let ramp = (self.frames_in_volume as f32 / WARMUP_FRAMES as f32).min(1.0);
        let hysteresis = resources.settings.hysteresis * if self.frames_in_volume == 0 { 0.0 } else { ramp };
        let slices = if self.frames_in_volume < WARMUP_FRAMES || changed {
            1
        } else {
            resources.settings.voxel_relight_period
        };
        let probes = resources.volume.probe_count();
        let batch = resources.settings.probes_per_frame.min(probes);
        let rotation = frame_rotation(self.frame_number);
        let pass = PassUniforms {
            frame: [self.frame_number, self.cursor, batch, resources.settings.rays_per_probe],
            relight: [
                self.frame_number % slices,
                slices,
                u32::try_from(lights.len()).unwrap_or(u32::MAX),
                0,
            ],
            rotation_x: [rotation.x_axis.x, rotation.y_axis.x, rotation.z_axis.x, 0.0],
            rotation_y: [rotation.x_axis.y, rotation.y_axis.y, rotation.z_axis.y, 0.0],
            rotation_z: [rotation.x_axis.z, rotation.y_axis.z, rotation.z_axis.z, 0.0],
            blend: [
                hysteresis,
                resources.settings.environment_intensity,
                resources.settings.bounce_intensity,
                resources.settings.depth_sharpness,
            ],
            reach: [
                resources.grid.bounds().max.distance(resources.grid.bounds().min),
                0.0,
                0.0,
                0.0,
            ],
        };
        let Some(gpu) = self.gpu.as_mut() else {
            return Err(RhiError::Unsupported("GI used before setup"));
        };
        let grid_uniform = gpu.write_uniform(ctx, bytemuck::bytes_of(&VoxelGridUniforms::new(&resources.grid)))?;
        let volume_uniform = gpu.write_uniform(
            ctx,
            bytemuck::bytes_of(&GiVolumeUniforms::new(&resources.volume, &resources.settings)),
        )?;
        let pass_uniform = gpu.write_uniform(ctx, bytemuck::bytes_of(&pass))?;

        let status = GiStatus {
            probes,
            probe_spacing: resources.volume.spacing,
            voxel_cells: resources.grid.cell_count(),
            voxel_size: resources.grid.size,
            rays_per_frame: batch * resources.settings.rays_per_probe,
            voxel_jobs: u32::try_from(input.jobs.len()).unwrap_or(u32::MAX),
            revoxelized: changed,
            gpu_bytes: resources.gpu_bytes(),
        };
        ctx.shared_mut().insert(status);

        let previous = self.latest;
        let next = 1 - previous;
        self.cursor = (self.cursor + batch) % probes;
        self.frame_number = self.frame_number.wrapping_add(1);
        self.frames_in_volume = self.frames_in_volume.saturating_add(1);
        Ok(Some(Plan {
            voxelize,
            cells: resources.grid.cell_count(),
            slices,
            batch_rays: batch * resources.settings.rays_per_probe,
            atlas: [resources.volume.irradiance_size(), resources.volume.visibility_size()],
            grid_uniform,
            volume_uniform,
            pass_uniform,
            environment,
            previous,
            next,
        }))
    }
}

/// A rotation that changes every frame: the rays of consecutive frames fall between each other, so the probes see more
/// directions over time (Majercik et al. 2019).
fn frame_rotation(frame: u32) -> Mat3 {
    let mut state = u64::from(frame)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(0x1234_5678);
    let mut next = || {
        state ^= state >> 30;
        state = state.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        state ^= state >> 27;
        state = state.wrapping_mul(0x94d0_49bb_1331_11eb);
        state ^= state >> 31;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let axis = Vec3::new(next() - 0.5, next() - 0.5, next() - 0.5);
    let axis = if axis.length_squared() > 1.0e-6 {
        axis.normalize()
    } else {
        Vec3::Y
    };
    Mat3::from_quat(Quat::from_axis_angle(axis, next() * std::f32::consts::TAU))
}

fn plan_voxelization(
    ctx: &mut RecordCtx<'_>,
    jobs: &[VoxelJob],
    mut binder: Option<&mut TextureBinder>,
) -> Result<VoxelizePlan, RhiError> {
    let device = ctx.device();
    let size = (jobs.len() as u64).max(1) * JOB_STRIDE;
    let buffer = device.create_buffer(&BufferDesc {
        label: "gi voxelize jobs",
        size,
        usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
    })?;
    let mut bytes = vec![0_u8; usize::try_from(size).expect("a job table fits in memory")];
    let mut groups: HashMap<u32, Option<BindGroupId>> = HashMap::new();
    let mut planned = Vec::with_capacity(jobs.len());
    for (index, job) in jobs.iter().enumerate() {
        let start = index * JOB_STRIDE_BYTES;
        let uniforms = JobUniforms::new(job);
        bytes[start..start + size_of::<JobUniforms>()].copy_from_slice(bytemuck::bytes_of(&uniforms));
        let textures = match binder.as_deref_mut() {
            Some(binder) => *groups.entry(job.material).or_insert_with(|| {
                binder
                    .bind_group_for_values(device, job.material, &job.texture_values)
                    .ok()
            }),
            None => None,
        };
        planned.push(PlannedJob {
            offset: index as u64 * JOB_STRIDE,
            positions: job.positions,
            surface: job.surface,
            indices: job.indices,
            triangles: job.triangles,
            textures,
        });
    }
    device.write_buffer(buffer, 0, &bytes)?;
    ctx.release_later(buffer);
    Ok(VoxelizePlan { buffer, jobs: planned })
}

fn buffer_entry(binding: u32, buffer: BufferId) -> BindGroupEntry {
    BindGroupEntry {
        binding,
        resource: BindingResource::whole_buffer(buffer),
    }
}

fn uniform_entry(binding: u32, slice: UniformSlice) -> BindGroupEntry {
    BindGroupEntry {
        binding,
        resource: slice.binding(),
    }
}

fn texture_entry(binding: u32, view: TextureViewId) -> BindGroupEntry {
    BindGroupEntry {
        binding,
        resource: BindingResource::TextureView(view),
    }
}

impl RenderFeature for GiFeature {
    fn name(&self) -> &'static str {
        "gi probes"
    }

    // Before the passes that read the atlases.
    fn stage(&self) -> Stage {
        Stage::Setup
    }

    // The environment the rays see when they leave the scene is the one `IblPack` built this frame.
    fn after(&self) -> &[&'static str] {
        &["ibl"]
    }

    fn negotiate(&mut self, caps: &Caps) -> Negotiation {
        let atlas = caps.format(crate::gpu::ATLAS_FORMAT);
        let usable = FormatCaps::SAMPLED | FormatCaps::FILTERABLE | FormatCaps::STORAGE;
        let limits = &caps.limits;
        if !atlas.contains(usable) {
            return Negotiation::Disabled {
                reason: "J0005: Rgba16Float must be a filterable texture that compute can write".to_owned(),
            };
        }
        if limits.max_compute_invocations_per_workgroup < WORKGROUP {
            return Negotiation::Disabled {
                reason: "J0005: the probe passes need compute workgroups of 64 invocations".to_owned(),
            };
        }
        Negotiation::Full
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let device = ctx.device();
        self.gpu = Some(Gpu {
            sampler: atlas_sampler(device)?,
            black_environment: black_cube(device)?,
            uniforms: UniformRing::new(device, "gi uniforms", 16 << 10)?,
            uniform_epoch: None,
        });
        // Whatever lived on the old device is gone: the volume starts again.
        self.shaders = None;
        self.resources = None;
        self.scene_signature = None;
        self.recorded = None;
        self.plan = None;
        self.reported = Reported::default();
        ctx.shared().remove::<GiFrame>();
        ctx.shared().remove::<GiStatus>();
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let epoch = ctx.view().record_epoch;
        if self.recorded.is_none_or(|(recorded, _)| recorded != epoch) {
            self.plan = self.prepare(ctx)?;
            self.recorded = Some((epoch, self.plan.is_some()));
            if self.plan.is_none() {
                ctx.shared_mut().remove::<GiStatus>();
            }
        }
        let first_view = self.plan.is_some();
        let Some(resources) = self
            .resources
            .filter(|_| self.recorded.is_some_and(|(_, active)| active))
        else {
            ctx.shared_mut().remove::<GiFrame>();
            return Ok(());
        };
        let latest = self.plan.as_ref().map_or(self.latest, |plan| plan.next);
        let frame = if let Some(plan) = self.plan.take() {
            let frame = self.record_passes(ctx, &resources, plan)?;
            self.latest = latest;
            frame
        } else {
            let atlases = resources.atlases[self.latest];
            let uniform = self.last_uniform(ctx)?;
            let irradiance = ctx.graph().import_texture(
                "gi irradiance",
                atlases.irradiance,
                atlases.irradiance_desc,
                Access::SAMPLED,
                Access::SAMPLED,
            );
            let visibility = ctx.graph().import_texture(
                "gi visibility",
                atlases.visibility,
                atlases.visibility_desc,
                Access::SAMPLED,
                Access::SAMPLED,
            );
            GiFrame {
                uniforms: GiVolumeUniforms::new(&resources.volume, &resources.settings),
                uniform_buffer: uniform,
                sampler: self
                    .gpu
                    .as_ref()
                    .map(|gpu| gpu.sampler)
                    .ok_or(RhiError::Unsupported("GI used before setup"))?,
                irradiance,
                visibility,
                probe_count: resources.volume.probe_count(),
            }
        };
        let _ = first_view;
        ctx.shared_mut().insert(frame);
        Ok(())
    }

    fn on_device_reset(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        self.setup(ctx)
    }
}

impl GiFeature {
    // The volume uniform again for a second view of the frame.
    fn last_uniform(&mut self, ctx: &RecordCtx<'_>) -> Result<UniformSlice, RhiError> {
        let resources = self.resources.ok_or(RhiError::Unsupported("GI used before setup"))?;
        let gpu = self.gpu.as_mut().ok_or(RhiError::Unsupported("GI used before setup"))?;
        gpu.write_uniform(
            ctx,
            bytemuck::bytes_of(&GiVolumeUniforms::new(&resources.volume, &resources.settings)),
        )
    }

    #[expect(clippy::too_many_lines, reason = "the four passes of one frame, built in order")]
    fn record_passes(
        &mut self,
        ctx: &mut RecordCtx<'_>,
        resources: &GiResources,
        plan: Plan,
    ) -> Result<GiFrame, RhiError> {
        let (shaders_ptr, sampler) = {
            let shaders = &mut self
                .shaders
                .as_mut()
                .ok_or(RhiError::Unsupported("GI used before setup"))?
                .1;
            let device = ctx.device();
            let mut pipelines = HashMap::new();
            for pass in [
                Pass::Clear,
                Pass::Voxelize,
                Pass::Compact,
                Pass::Light,
                Pass::Trace,
                Pass::UpdateIrradiance,
                Pass::UpdateVisibility,
            ] {
                pipelines.insert(pass, (shaders.pipeline(device, pass)?, shaders.group_zero_layout(pass)));
            }
            (
                (pipelines, shaders.job_layout()),
                self.gpu.as_ref().map(|gpu| gpu.sampler),
            )
        };
        let (pipelines, job_layout) = shaders_ptr;
        let sampler = sampler.ok_or(RhiError::Unsupported("GI used before setup"))?;
        let (previous_pair, next_pair) = (resources.atlases[plan.previous], resources.atlases[plan.next]);

        let graph = ctx.graph();
        let cells_bytes = u64::from(resources.grid.cell_count()) * 4;
        let albedo = graph.import_buffer("gi voxel albedo", resources.albedo, cells_bytes, RESTING, RESTING);
        let emissive = graph.import_buffer("gi voxel emission", resources.emissive, cells_bytes, RESTING, RESTING);
        let radiance = graph.import_buffer(
            "gi voxel radiance",
            resources.radiance,
            cells_bytes * 6,
            RESTING,
            RESTING,
        );
        let solid = graph.import_buffer("gi solid cells", resources.solid, cells_bytes + 4, RESTING, RESTING);
        let rays_bytes = u64::from(plan.batch_rays.max(1)) * crate::gpu::RAY_BYTES;
        let rays = graph.import_buffer("gi rays", resources.rays, rays_bytes, RESTING, RESTING);
        let lights = graph.import_buffer(
            "gi lights",
            resources.lights,
            u64::from(resources.light_capacity) * 64,
            RESTING,
            RESTING,
        );
        let import = |graph: &mut pathon_api::graph::GraphBuilder<'_>, name, texture, desc| {
            graph.import_texture(name, texture, desc, Access::SAMPLED, Access::SAMPLED)
        };
        let previous_irradiance = import(
            graph,
            "gi irradiance previous",
            previous_pair.irradiance,
            previous_pair.irradiance_desc,
        );
        let previous_visibility = import(
            graph,
            "gi visibility previous",
            previous_pair.visibility,
            previous_pair.visibility_desc,
        );
        let next_irradiance = import(graph, "gi irradiance", next_pair.irradiance, next_pair.irradiance_desc);
        let next_visibility = import(graph, "gi visibility", next_pair.visibility, next_pair.visibility_desc);

        let (grid_uniform, volume_uniform, pass_uniform) = (plan.grid_uniform, plan.volume_uniform, plan.pass_uniform);
        let (albedo, emissive, radiance, solid) = if let Some(voxelize) = plan.voxelize {
            let (clear, clear_layout) = pipelines[&Pass::Clear];
            let (compact, compact_layout) = pipelines[&Pass::Compact];
            let (voxelize_pipeline, voxelize_layout) = pipelines[&Pass::Voxelize];
            let cells = plan.cells;
            let mut pass = graph.add_pass(
                PassDesc::new("gi voxelize", Stage::Setup)
                    .priority(PASS_PRIORITY)
                    .side_effect(),
            );
            let albedo = pass.modify_buffer(albedo, READ_WRITE);
            let emissive = pass.modify_buffer(emissive, READ_WRITE);
            let radiance = pass.modify_buffer(radiance, READ_WRITE);
            let solid = pass.modify_buffer(solid, READ_WRITE);
            pass.record(move |pass| {
                let (albedo_buffer, emissive_buffer, radiance_buffer, solid_buffer) = (
                    pass.buffer(albedo),
                    pass.buffer(emissive),
                    pass.buffer(radiance),
                    pass.buffer(solid),
                );
                let compact_group = pass.create_bind_group(&BindGroupDesc {
                    label: "gi compact",
                    layout: compact_layout,
                    entries: vec![
                        uniform_entry(1, grid_uniform),
                        buffer_entry(3, albedo_buffer),
                        buffer_entry(15, solid_buffer),
                    ],
                })?;
                let clear_group = pass.create_bind_group(&BindGroupDesc {
                    label: "gi clear",
                    layout: clear_layout,
                    entries: vec![
                        uniform_entry(1, grid_uniform),
                        buffer_entry(3, albedo_buffer),
                        buffer_entry(4, emissive_buffer),
                        buffer_entry(5, radiance_buffer),
                        buffer_entry(15, solid_buffer),
                    ],
                })?;
                let voxel_group = pass.create_bind_group(&BindGroupDesc {
                    label: "gi voxelize",
                    layout: voxelize_layout,
                    entries: vec![
                        uniform_entry(1, grid_uniform),
                        buffer_entry(3, albedo_buffer),
                        buffer_entry(4, emissive_buffer),
                    ],
                })?;
                let mut job_groups = Vec::with_capacity(voxelize.jobs.len());
                for job in &voxelize.jobs {
                    job_groups.push(pass.create_bind_group(&BindGroupDesc {
                        label: "gi voxelize job",
                        layout: job_layout,
                        entries: vec![
                            BindGroupEntry {
                                binding: 0,
                                resource: BindingResource::Buffer {
                                    buffer: voxelize.buffer,
                                    offset: job.offset,
                                    size: Some(size_of::<JobUniforms>() as u64),
                                },
                            },
                            buffer_entry(1, job.positions),
                            buffer_entry(2, job.surface),
                            buffer_entry(3, job.indices),
                        ],
                    })?);
                }
                let mut compute = pass.commands().compute_pass(ComputePassDesc {
                    label: "gi clear voxels",
                    timestamps: None,
                });
                compute.set_pipeline(clear);
                compute.set_bind_group(0, clear_group);
                compute.dispatch(cells.div_ceil(WORKGROUP), 1, 1);
                compute.set_pipeline(voxelize_pipeline);
                compute.set_bind_group(0, voxel_group);
                for (job, group) in voxelize.jobs.iter().zip(job_groups) {
                    compute.set_bind_group(1, group);
                    if let Some(textures) = job.textures {
                        compute.set_bind_group(2, textures);
                    }
                    compute.dispatch(job.triangles.div_ceil(WORKGROUP), 1, 1);
                }
                compute.set_pipeline(compact);
                compute.set_bind_group(0, compact_group);
                compute.dispatch(cells.div_ceil(WORKGROUP), 1, 1);
                Ok(())
            });
            (albedo, emissive, radiance, solid)
        } else {
            (albedo, emissive, radiance, solid)
        };

        // Light the voxels (direct light, the previous frame's probes as the later bounces).
        let (light, light_layout) = pipelines[&Pass::Light];
        let cells = plan.cells;
        let slices = plan.slices;
        let mut pass = graph.add_pass(
            PassDesc::new("gi light voxels", Stage::Setup)
                .priority(PASS_PRIORITY)
                .side_effect(),
        );
        let albedo = pass.modify_buffer(albedo, READ_WRITE);
        let emissive = pass.modify_buffer(emissive, READ_WRITE);
        let radiance = pass.modify_buffer(radiance, READ_WRITE);
        let lights = pass.modify_buffer(lights, READ_WRITE);
        let solid = pass.modify_buffer(solid, READ_WRITE);
        let light_previous_irradiance = pass.read(previous_irradiance, Access::SAMPLED);
        let light_previous_visibility = pass.read(previous_visibility, Access::SAMPLED);
        pass.record(move |pass| {
            let (previous_irradiance_view, previous_visibility_view) = (
                pass.view(light_previous_irradiance)?,
                pass.view(light_previous_visibility)?,
            );
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "gi light voxels",
                layout: light_layout,
                entries: vec![
                    uniform_entry(0, volume_uniform),
                    uniform_entry(1, grid_uniform),
                    uniform_entry(2, pass_uniform),
                    buffer_entry(3, pass.buffer(albedo)),
                    buffer_entry(4, pass.buffer(emissive)),
                    buffer_entry(5, pass.buffer(radiance)),
                    buffer_entry(6, pass.buffer(lights)),
                    buffer_entry(15, pass.buffer(solid)),
                    texture_entry(8, previous_irradiance_view),
                    texture_entry(9, previous_visibility_view),
                    BindGroupEntry {
                        binding: 10,
                        resource: BindingResource::Sampler(sampler),
                    },
                ],
            })?;
            let mut compute = pass.commands().compute_pass(ComputePassDesc {
                label: "gi light voxels",
                timestamps: None,
            });
            compute.set_pipeline(light);
            compute.set_bind_group(0, group);
            // One thread per face of a solid cell of the slice, at most `cells / slices` cells, in rows of 65535 groups.
            let groups = (cells.div_ceil(slices) * 6).div_ceil(WORKGROUP);
            let columns = groups.clamp(1, MAX_GROUPS);
            compute.dispatch(columns, groups.div_ceil(columns).max(1), 1);
            Ok(())
        });

        // Trace the rays of this frame's probes.
        let (trace, trace_layout) = pipelines[&Pass::Trace];
        let (environment, environment_sampler) = plan.environment;
        let batch_rays = plan.batch_rays;
        let mut pass = graph.add_pass(
            PassDesc::new("gi trace probes", Stage::Setup)
                .priority(PASS_PRIORITY)
                .side_effect(),
        );
        let albedo = pass.modify_buffer(albedo, READ_WRITE);
        let radiance = pass.modify_buffer(radiance, READ_WRITE);
        let rays = pass.modify_buffer(rays, READ_WRITE);
        pass.record(move |pass| {
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "gi trace probes",
                layout: trace_layout,
                entries: vec![
                    uniform_entry(0, volume_uniform),
                    uniform_entry(1, grid_uniform),
                    uniform_entry(2, pass_uniform),
                    buffer_entry(3, pass.buffer(albedo)),
                    buffer_entry(5, pass.buffer(radiance)),
                    buffer_entry(7, pass.buffer(rays)),
                    texture_entry(13, environment.view),
                    BindGroupEntry {
                        binding: 14,
                        resource: BindingResource::Sampler(environment_sampler),
                    },
                ],
            })?;
            let mut compute = pass.commands().compute_pass(ComputePassDesc {
                label: "gi trace probes",
                timestamps: None,
            });
            compute.set_pipeline(trace);
            compute.set_bind_group(0, group);
            compute.dispatch(batch_rays.div_ceil(WORKGROUP), 1, 1);
            Ok(())
        });

        // Fold the rays into the next generation of the atlases.
        let (update_irradiance, irradiance_layout) = pipelines[&Pass::UpdateIrradiance];
        let (update_visibility, visibility_layout) = pipelines[&Pass::UpdateVisibility];
        let (irradiance_size, visibility_size) = (plan.atlas[0], plan.atlas[1]);
        let mut pass = graph.add_pass(
            PassDesc::new("gi update probes", Stage::Setup)
                .priority(PASS_PRIORITY)
                .side_effect(),
        );
        let rays = pass.modify_buffer(rays, READ_WRITE);
        let read_irradiance = pass.read(previous_irradiance, Access::SAMPLED);
        let read_visibility = pass.read(previous_visibility, Access::SAMPLED);
        let written_irradiance = pass.write(next_irradiance, Access::STORAGE_WRITE);
        let written_visibility = pass.write(next_visibility, Access::STORAGE_WRITE);
        pass.record(move |pass| {
            let rays_buffer = pass.buffer(rays);
            let (read_irradiance_view, written_irradiance_view) =
                (pass.view(read_irradiance)?, pass.view(written_irradiance)?);
            let (read_visibility_view, written_visibility_view) =
                (pass.view(read_visibility)?, pass.view(written_visibility)?);
            let irradiance_group = pass.create_bind_group(&BindGroupDesc {
                label: "gi update irradiance",
                layout: irradiance_layout,
                entries: vec![
                    uniform_entry(0, volume_uniform),
                    uniform_entry(2, pass_uniform),
                    buffer_entry(7, rays_buffer),
                    texture_entry(8, read_irradiance_view),
                    texture_entry(11, written_irradiance_view),
                ],
            })?;
            let visibility_group = pass.create_bind_group(&BindGroupDesc {
                label: "gi update visibility",
                layout: visibility_layout,
                entries: vec![
                    uniform_entry(0, volume_uniform),
                    uniform_entry(2, pass_uniform),
                    buffer_entry(7, rays_buffer),
                    texture_entry(9, read_visibility_view),
                    texture_entry(12, written_visibility_view),
                ],
            })?;
            let mut compute = pass.commands().compute_pass(ComputePassDesc {
                label: "gi update probes",
                timestamps: None,
            });
            compute.set_pipeline(update_irradiance);
            compute.set_bind_group(0, irradiance_group);
            compute.dispatch(
                irradiance_size.0.div_ceil(ATLAS_WORKGROUP),
                irradiance_size.1.div_ceil(ATLAS_WORKGROUP),
                1,
            );
            compute.set_pipeline(update_visibility);
            compute.set_bind_group(0, visibility_group);
            compute.dispatch(
                visibility_size.0.div_ceil(ATLAS_WORKGROUP),
                visibility_size.1.div_ceil(ATLAS_WORKGROUP),
                1,
            );
            Ok(())
        });
        let (irradiance, visibility) = (written_irradiance, written_visibility);
        let _: (BufferHandle, TextureHandle, Format) = (rays, irradiance, Format::Rgba16Float);
        Ok(GiFrame {
            uniforms: GiVolumeUniforms::new(&resources.volume, &resources.settings),
            uniform_buffer: volume_uniform,
            sampler,
            irradiance,
            visibility,
            probe_count: resources.volume.probe_count(),
        })
    }
}
