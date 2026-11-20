use std::sync::Arc;

use pathon_api::{PrepareCtx, RecordCtx, Shared, Slot, Slots, TaskPool, ViewCtx, ViewId};
use pathon_diag::Diagnostic;
use pathon_graph::{FrameGraph, FrameSizes, GraphStats, PassDesc, Stage, TextureHandle, TexturePool};
use pathon_resources::DeferredRelease;
use pathon_rhi::{
    Access, Color, ColorAttachment, CommandList, Device, DeviceApi, DeviceRequest, Format, PhysicalSize, Pool,
    PresentMode, RenderPassDesc, RhiError, SurfaceConfig, SurfaceId, SurfaceTarget, TextureDesc, TextureId,
};

use crate::builder::RendererBuilder;
use crate::error::RenderError;
use crate::frame::{FrameInput, FrameReport, ViewInput};
use crate::registry::{FeatureStatus, Registry};
use crate::target::{self, OFFSCREEN_FINAL, Target, TargetId, TargetKind};

pub(crate) enum DeviceOwnership {
    Owned(DeviceRequest),
    Borrowed,
}

#[cfg(not(target_arch = "wasm32"))]
type ReuploadFn = Box<dyn FnMut(&Device) + Send>;
#[cfg(target_arch = "wasm32")]
type ReuploadFn = Box<dyn FnMut(&Device)>;

#[derive(Clone, Copy, Debug)]
struct OutputTarget {
    texture: TextureId,
    desc: TextureDesc,
    initial: Access,
    final_access: Access,
    // Host textures keep their contents (mode A); targets pathon owns start cleared.
    keeps_contents: bool,
}

// The renderer owns no thread, loop, window or ECS (ADR-0004, ADR-0005): the host calls it, supplies time and views,
// and may own the device and the render targets.
pub struct Renderer {
    device: Arc<Device>,
    ownership: DeviceOwnership,
    registry: Registry,
    task_pool: Arc<dyn TaskPool>,
    targets: Pool<TargetId, Target>,
    pool: TexturePool,
    release: DeferredRelease,
    frame: u64,
    clear_color: Color,
    on_reupload: Option<ReuploadFn>,
    shared: Shared,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer")
            .field("features", &self.registry.statuses())
            .field("targets", &self.targets)
            .field("frame", &self.frame)
            .finish_non_exhaustive()
    }
}

impl Renderer {
    pub fn builder() -> RendererBuilder {
        RendererBuilder::new()
    }

    pub(crate) fn new(
        device: Arc<Device>,
        ownership: DeviceOwnership,
        registry: Registry,
        task_pool: Arc<dyn TaskPool>,
        frames_in_flight: u64,
        clear_color: Color,
        shared: Shared,
    ) -> Self {
        Self {
            device,
            ownership,
            registry,
            task_pool,
            targets: Pool::new(),
            pool: TexturePool::new(),
            release: DeferredRelease::new(frames_in_flight),
            frame: 0,
            clear_color,
            on_reupload: None,
            shared,
        }
    }

    // Data packs share with each other and with the application (scene, lights, settings), keyed by type.
    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    pub fn shared_mut(&mut self) -> &mut Shared {
        &mut self.shared
    }

    pub fn device(&self) -> &Arc<Device> {
        &self.device
    }

    pub fn features(&self) -> Vec<(&'static str, FeatureStatus)> {
        self.registry.statuses()
    }

    pub fn active_features(&self) -> Vec<&'static str> {
        self.registry
            .order
            .iter()
            .map(|index| self.registry.entries[*index].feature.name())
            .collect()
    }

    // Called after the device was rebuilt and every pack re-created its GPU objects: re-upload scene data here.
    pub fn on_device_reset(&mut self, callback: ReuploadFn) {
        self.on_reupload = Some(callback);
    }

    pub fn add_window(&mut self, window: SurfaceTarget) -> Result<TargetId, RenderError> {
        let surface = self.device.create_surface(&window)?;
        self.attach_surface(surface, window.size, Some(window))
    }

    // For surfaces the host created with `Device::create_surface` (on the main thread, ADR-0004).
    pub fn add_surface(&mut self, surface: SurfaceId, size: PhysicalSize) -> Result<TargetId, RenderError> {
        self.attach_surface(surface, size, None)
    }

    fn attach_surface(
        &mut self,
        surface: SurfaceId,
        size: PhysicalSize,
        window: Option<SurfaceTarget>,
    ) -> Result<TargetId, RenderError> {
        let mut config = SurfaceConfig {
            size,
            present_mode: PresentMode::Vsync,
            format: None,
        };
        target::configure(self.device.as_ref(), surface, &mut config)?;
        Ok(self.targets.insert(Target {
            kind: TargetKind::Surface {
                surface,
                window,
                config,
            },
            size,
        }))
    }

    pub fn add_offscreen_target(&mut self, size: PhysicalSize, format: Format) -> Result<TargetId, RenderError> {
        let desc = target::offscreen_desc(size, format);
        let texture = self.device.create_texture(&desc)?;
        let kind = TargetKind::Offscreen {
            texture,
            desc,
            state: Access::empty(),
        };
        Ok(self.targets.insert(Target { kind, size }))
    }

    // A texture the host owns (modes A and B): pathon takes it in `before` and leaves it in `after`.
    pub fn import_texture(
        &mut self,
        texture: TextureId,
        before: Access,
        after: Access,
    ) -> Result<TargetId, RenderError> {
        let desc = self.device.texture_desc(texture)?;
        let size = PhysicalSize::new(desc.size.width, desc.size.height);
        Ok(self.targets.insert(Target {
            kind: TargetKind::Imported {
                texture,
                desc,
                before,
                after,
            },
            size,
        }))
    }

    pub fn remove_target(&mut self, id: TargetId) -> Result<(), RenderError> {
        let removed = self.targets.remove(id).ok_or(RenderError::UnknownTarget)?;
        match removed.kind {
            TargetKind::Surface { surface, .. } => self.device.destroy_surface(surface),
            TargetKind::Offscreen { texture, .. } => self.release.release(self.frame, texture),
            TargetKind::Imported { .. } => {}
        }
        Ok(())
    }

    pub fn resize_target(&mut self, id: TargetId, size: PhysicalSize) -> Result<(), RenderError> {
        let device = Arc::clone(&self.device);
        let frame = self.frame;
        let target = self.targets.get_mut(id).ok_or(RenderError::UnknownTarget)?;
        match &mut target.kind {
            TargetKind::Surface { surface, config, .. } => {
                config.size = size;
                target::configure(device.as_ref(), *surface, config)?;
            }
            TargetKind::Offscreen { texture, desc, state } => {
                let resized = target::offscreen_desc(size, desc.format);
                let old = std::mem::replace(texture, device.create_texture(&resized)?);
                self.release.release(frame, old);
                *desc = resized;
                *state = Access::empty();
            }
            TargetKind::Imported { .. } => {
                return Err(RenderError::Rhi(RhiError::Unsupported(
                    "imported targets are resized by re-importing",
                )));
            }
        }
        target.size = size;
        Ok(())
    }

    pub fn target_texture(&self, id: TargetId) -> Option<TextureId> {
        match self.targets.get(id)?.kind {
            TargetKind::Offscreen { texture, .. } | TargetKind::Imported { texture, .. } => Some(texture),
            TargetKind::Surface { .. } => None,
        }
    }

    pub fn target_format(&self, id: TargetId) -> Option<Format> {
        match &self.targets.get(id)?.kind {
            TargetKind::Surface { config, .. } => config.format,
            TargetKind::Offscreen { desc, .. } | TargetKind::Imported { desc, .. } => Some(desc.format),
        }
    }

    // Records every view, submits on the device's queue and presents window surfaces.
    pub fn render_frame(&mut self, input: &FrameInput) -> Result<FrameReport, RenderError> {
        self.ensure_device()?;
        let mut commands = CommandList::new();
        let mut presents = Vec::new();
        let mut report = FrameReport::default();
        for (index, view) in input.views.iter().enumerate() {
            let Some(output) = self.acquire_output(view.target, &mut presents)? else {
                report.skipped_views += 1;
                continue;
            };
            report
                .views
                .push(self.record_view(index, view, input, output, &mut commands)?);
        }
        let submitted = self.device.submit(commands);
        for surface in presents {
            self.device.present_surface(surface)?;
        }
        submitted?;
        self.end_frame();
        Ok(report)
    }

    // Mode A: the host submits the returned list itself (e.g. through `interop::encode_into`).
    pub fn record_frame(&mut self, input: &FrameInput) -> Result<CommandList, RenderError> {
        self.ensure_device()?;
        let mut commands = CommandList::new();
        for (index, view) in input.views.iter().enumerate() {
            let target = self.targets.get(view.target).ok_or(RenderError::UnknownTarget)?;
            if let TargetKind::Surface { .. } = target.kind {
                return Err(RenderError::SurfaceInRecordFrame);
            }
            let Some(output) = self.acquire_output(view.target, &mut Vec::new())? else {
                continue;
            };
            self.record_view(index, view, input, output, &mut commands)?;
        }
        self.end_frame();
        Ok(commands)
    }

    fn ensure_device(&self) -> Result<(), RenderError> {
        if self.device.is_lost() {
            Err(RenderError::DeviceLost)
        } else {
            Ok(())
        }
    }

    fn end_frame(&mut self) {
        self.release.collect(self.frame, self.device.as_ref());
        self.frame += 1;
    }

    fn acquire_output(
        &mut self,
        id: TargetId,
        presents: &mut Vec<SurfaceId>,
    ) -> Result<Option<OutputTarget>, RenderError> {
        let device = Arc::clone(&self.device);
        let target = self.targets.get_mut(id).ok_or(RenderError::UnknownTarget)?;
        let output = match &mut target.kind {
            TargetKind::Surface { surface, config, .. } => {
                if config.format.is_none() {
                    return Ok(None);
                }
                let frame = match device.acquire_surface(*surface) {
                    Ok(frame) => frame,
                    Err(RhiError::SurfaceUnavailable) => return Ok(None),
                    Err(error) => return Err(error.into()),
                };
                presents.push(*surface);
                OutputTarget {
                    texture: frame.texture,
                    desc: device.texture_desc(frame.texture)?,
                    initial: Access::empty(),
                    final_access: Access::PRESENT,
                    keeps_contents: false,
                }
            }
            TargetKind::Offscreen { texture, desc, state } => {
                let output = OutputTarget {
                    texture: *texture,
                    desc: *desc,
                    initial: *state,
                    final_access: OFFSCREEN_FINAL,
                    keeps_contents: false,
                };
                *state = OFFSCREEN_FINAL;
                output
            }
            TargetKind::Imported {
                texture,
                desc,
                before,
                after,
            } => OutputTarget {
                texture: *texture,
                desc: *desc,
                initial: *before,
                final_access: *after,
                keeps_contents: true,
            },
        };
        Ok(Some(output))
    }

    fn record_view(
        &mut self,
        index: usize,
        view: &ViewInput,
        input: &FrameInput,
        output: OutputTarget,
        commands: &mut CommandList,
    ) -> Result<GraphStats, RenderError> {
        let output_size = PhysicalSize::new(output.desc.size.width, output.desc.size.height);
        let view_id = ViewId(u32::try_from(index).expect("fewer than u32::MAX views"));
        let mut view_ctx = ViewCtx::new(view_id, output_size, view.camera);
        view_ctx.frame_index = input.frame_index;
        view_ctx.time_seconds = input.time_seconds;
        view_ctx.delta_seconds = input.delta_seconds;
        for feature_index in &self.registry.order {
            let owns_temporal = self.registry.temporal_owner == Some(*feature_index);
            let mut ctx = PrepareCtx::new(&mut view_ctx, owns_temporal, &self.shared);
            self.registry.entries[*feature_index].feature.prepare_view(&mut ctx);
        }

        let mut graph = FrameGraph::new();
        let cleared = if output.keeps_contents {
            graph.import_texture(
                "output",
                output.texture,
                output.desc,
                output.initial,
                output.final_access,
            )
        } else {
            self.add_output_clear(&mut graph, output)
        };
        let mut slots = Slots::new();
        let mut diagnostics = Vec::new();
        let mut failed = Vec::new();
        {
            let mut ctx = RecordCtx::new(
                &mut graph,
                &mut slots,
                &view_ctx,
                self.device.as_ref(),
                &mut diagnostics,
                &mut self.shared,
            );
            ctx.set_slot(Slot::OUTPUT, cleared);
            for feature_index in &self.registry.order {
                if let Err(error) = self.registry.entries[*feature_index].feature.record(&mut ctx) {
                    failed.push((*feature_index, error));
                }
            }
        }
        for diagnostic in &diagnostics {
            pathon_diag::emit(diagnostic);
        }
        for (feature_index, error) in failed {
            let name = self.registry.entries[feature_index].feature.name();
            self.registry.disable(
                feature_index,
                Diagnostic::error("F0105", format!("'{name}' failed to record: {error}")),
            );
        }

        let sizes = FrameSizes {
            render: view_ctx.render_size,
            output: output_size,
        };
        let compiled = match graph.compile(sizes) {
            Ok(compiled) => compiled,
            Err(errors) => {
                for error in &errors {
                    pathon_diag::emit(error);
                }
                return self.record_clear_only(output, commands);
            }
        };
        for warning in compiled.warnings() {
            pathon_diag::emit(warning);
        }
        match graph.execute(
            compiled,
            self.device.as_ref(),
            &mut self.pool,
            &mut self.release,
            self.frame,
        ) {
            Ok(execution) => {
                commands.append(execution.commands);
                Ok(execution.stats)
            }
            Err(RhiError::DeviceLost) => Err(RenderError::DeviceLost),
            Err(error) => {
                pathon_diag::emit(&Diagnostic::error("F0105", format!("a pass failed to record: {error}")));
                self.record_clear_only(output, commands)
            }
        }
    }

    fn add_output_clear(&self, graph: &mut FrameGraph, output: OutputTarget) -> TextureHandle {
        let handle = graph.import_texture(
            "output",
            output.texture,
            output.desc,
            output.initial,
            output.final_access,
        );
        let clear_color = self.clear_color;
        let mut pass = graph.add_pass(PassDesc::new("clear output", Stage::Setup));
        let cleared = pass.write(handle, Access::COLOR_ATTACHMENT);
        pass.record(move |ctx| {
            let view = ctx.view(cleared)?;
            drop(ctx.commands().render_pass(RenderPassDesc {
                label: "clear output",
                color: vec![ColorAttachment::clear(view, clear_color)],
                depth: None,
                timestamps: None,
            }));
            Ok(())
        });
        cleared
    }

    // A broken graph still leaves the target presentable.
    fn record_clear_only(
        &mut self,
        output: OutputTarget,
        commands: &mut CommandList,
    ) -> Result<GraphStats, RenderError> {
        let mut graph = FrameGraph::new();
        self.add_output_clear(&mut graph, output);
        let compiled = graph
            .compile(FrameSizes::same(PhysicalSize::new(
                output.desc.size.width,
                output.desc.size.height,
            )))
            .map_err(|mut errors| RenderError::Diagnostic(errors.remove(0)))?;
        let execution = graph.execute(
            compiled,
            self.device.as_ref(),
            &mut self.pool,
            &mut self.release,
            self.frame,
        )?;
        commands.append(execution.commands);
        Ok(execution.stats)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn recover(&mut self) -> Result<(), RenderError> {
        let DeviceOwnership::Owned(request) = &self.ownership else {
            return Err(RenderError::BorrowedDevice);
        };
        let device = Arc::new(Device::build(request)?);
        self.reset(device)
    }

    pub async fn recover_async(&mut self) -> Result<(), RenderError> {
        let DeviceOwnership::Owned(request) = &self.ownership else {
            return Err(RenderError::BorrowedDevice);
        };
        let device = Arc::new(Device::build_async(request).await?);
        self.reset(device)
    }

    // Moves the renderer onto a new device. Surfaces pathon created and offscreen targets are rebuilt; surfaces and
    // textures the host handed in are dropped and must be added again.
    pub fn reset(&mut self, device: Arc<Device>) -> Result<(), RenderError> {
        self.device = device;
        self.pool.forget_all();
        self.release.forget_all();
        let ids: Vec<TargetId> = self.targets.iter().map(|(id, _)| id).collect();
        for id in ids {
            let keep = match self.targets.get_mut(id).map(|target| &mut target.kind) {
                Some(TargetKind::Surface {
                    surface,
                    window: Some(window),
                    config,
                }) => {
                    *surface = self.device.create_surface(window)?;
                    config.format = None;
                    target::configure(self.device.as_ref(), *surface, config)?;
                    true
                }
                Some(TargetKind::Offscreen { texture, desc, state }) => {
                    *texture = self.device.create_texture(desc)?;
                    *state = Access::empty();
                    true
                }
                _ => false,
            };
            if !keep {
                self.targets.remove(id);
            }
        }
        self.registry
            .reset(self.device.as_ref(), self.task_pool.as_ref(), &mut self.shared);
        if let Some(callback) = &mut self.on_reupload {
            callback(&self.device);
        }
        Ok(())
    }
}
