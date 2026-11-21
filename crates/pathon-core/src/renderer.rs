use std::collections::HashSet;
use std::sync::Arc;

use pathon_api::{RecordCtx, Shared, Slot, Slots, TaskPool, ViewCtx, ViewId};
use pathon_diag::Diagnostic;
use pathon_graph::{FrameGraph, FrameSizes, GraphCapture, GraphStats, TextureHandle};
use pathon_resources::DeferredRelease;
use pathon_rhi::{
    Access, Color, CommandList, Device, DeviceApi, DeviceRequest, Format, Id, PhysicalSize, Pool, PresentMode,
    RhiError, SurfaceConfig, SurfaceId, SurfaceTarget, TextureId,
};

use crate::builder::RendererBuilder;
use crate::error::RenderError;
use crate::frame::{FrameInput, FrameReport, ViewInput};
use crate::registry::{FeatureInfo, FeatureStatus, Registry};
use crate::runner::{FrameEnv, GraphRunner, OutputTarget};
use crate::target::{self, OFFSCREEN_FINAL, Target, TargetId, TargetKind};

pub(crate) enum DeviceOwnership {
    Owned(DeviceRequest),
    Borrowed,
}

#[cfg(not(target_arch = "wasm32"))]
type ReuploadFn = Box<dyn FnMut(&Device) + Send>;
#[cfg(target_arch = "wasm32")]
type ReuploadFn = Box<dyn FnMut(&Device)>;

// The renderer owns no thread, loop, window or ECS (ADR-0004, ADR-0005): the host calls it, supplies time and views,
// and may own the device and the render targets.
pub struct Renderer {
    device: Arc<Device>,
    ownership: DeviceOwnership,
    registry: Registry,
    task_pool: Arc<dyn TaskPool>,
    targets: Pool<TargetId, Target>,
    runner: GraphRunner,
    release: DeferredRelease,
    // Frames committed so far (ADR-0022): the clock of `release`.
    frame: u64,
    // Record attempts started so far, abandoned ones included.
    record_epoch: u64,
    on_reupload: Option<ReuploadFn>,
    shared: Shared,
    frame_captures: Vec<GraphCapture>,
    // Offscreen targets recorded this frame: they only reach `OFFSCREEN_FINAL` once the frame is committed.
    pending_offscreen: Vec<TargetId>,
    frame_diagnostics: Vec<Diagnostic>,
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
            runner: GraphRunner::new(clear_color),
            release: DeferredRelease::new(frames_in_flight),
            frame: 0,
            record_epoch: 0,
            on_reupload: None,
            shared,
            frame_captures: Vec::new(),
            pending_offscreen: Vec::new(),
            frame_diagnostics: Vec::new(),
        }
    }

    // Tools (pathon-devtools): every `FrameReport` then carries a `GraphCapture` per view.
    pub fn set_capture(&mut self, enabled: bool) {
        self.runner.set_capture(enabled);
    }

    pub fn feature_details(&self) -> Vec<FeatureInfo> {
        self.registry.details()
    }

    fn report(&mut self, diagnostic: &Diagnostic) {
        pathon_diag::emit(diagnostic);
        self.frame_diagnostics.push(diagnostic.clone());
    }

    fn report_duplicate_target(&mut self, index: usize, target: TargetId) {
        self.report(&Diagnostic::warning(
            "F0106",
            format!("view {index} uses target {target:?}, already used by an earlier view in this frame; skipped"),
        ));
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

    // Frames committed so far: a frame counts once its list was submitted (or, in mode A, returned).
    pub fn frame_index(&self) -> u64 {
        self.frame
    }

    pub fn features(&self) -> Vec<(&'static str, FeatureStatus)> {
        self.registry.statuses()
    }

    pub fn active_features(&self) -> Vec<&'static str> {
        self.registry.active_names()
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
        self.runner.forget_target(id, &mut self.release, self.frame);
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
        self.record_epoch += 1;
        let mut commands = CommandList::new();
        let mut presents = Vec::new();
        let mut report = FrameReport::default();
        let recorded = self.record_views(input, &mut commands, &mut presents, &mut report);
        let submitted = recorded.and_then(|()| self.device.submit(commands).map_err(RenderError::from));
        if let Err(error) = submitted {
            for surface in presents {
                self.device.discard_surface(surface);
            }
            self.abandon_frame();
            return Err(error);
        }
        // The list has run, so its states and history are real even if presenting fails below.
        self.commit_frame_state();
        let mut presented = Ok(());
        for surface in presents {
            presented = presented.and(self.device.present_surface(surface));
        }
        if let Err(error) = presented {
            self.drop_frame_results();
            return Err(error.into());
        }
        report.captures = std::mem::take(&mut self.frame_captures);
        report.diagnostics = std::mem::take(&mut self.frame_diagnostics);
        Ok(report)
    }

    fn record_views(
        &mut self,
        input: &FrameInput,
        commands: &mut CommandList,
        presents: &mut Vec<SurfaceId>,
        report: &mut FrameReport,
    ) -> Result<(), RenderError> {
        let mut used_targets = HashSet::new();
        for (index, view) in input.views.iter().enumerate() {
            if !used_targets.insert(view.target) {
                self.report_duplicate_target(index, view.target);
                report.skipped_views += 1;
                continue;
            }
            let Some(output) = self.acquire_output(view.target, presents)? else {
                report.skipped_views += 1;
                continue;
            };
            let (stats, compiled) = self.record_view(view, input, output, commands)?;
            report.views.push(stats);
            report.compiled_views += usize::from(compiled);
        }
        Ok(())
    }

    // Mode A: the host submits the returned list itself (e.g. through `interop::encode_into`).
    pub fn record_frame(&mut self, input: &FrameInput) -> Result<CommandList, RenderError> {
        self.ensure_device()?;
        self.record_epoch += 1;
        let mut commands = CommandList::new();
        if let Err(error) = self.record_offscreen_views(input, &mut commands) {
            self.abandon_frame();
            return Err(error);
        }
        self.commit_frame_state();
        self.drop_frame_results();
        Ok(commands)
    }

    fn record_offscreen_views(&mut self, input: &FrameInput, commands: &mut CommandList) -> Result<(), RenderError> {
        let mut used_targets = HashSet::new();
        for (index, view) in input.views.iter().enumerate() {
            let target = self.targets.get(view.target).ok_or(RenderError::UnknownTarget)?;
            if let TargetKind::Surface { .. } = target.kind {
                return Err(RenderError::SurfaceInRecordFrame);
            }
            if !used_targets.insert(view.target) {
                self.report_duplicate_target(index, view.target);
                continue;
            }
            let Some(output) = self.acquire_output(view.target, &mut Vec::new())? else {
                continue;
            };
            self.record_view(view, input, output, commands)?;
        }
        Ok(())
    }

    // What the dropped frame's views wrote to history never reached the GPU.
    fn abandon_frame(&mut self) {
        self.pending_offscreen.clear();
        self.runner.abandon_frame(&mut self.release, self.frame);
        self.drop_frame_results();
    }

    // A failed frame must not leak its captures and diagnostics (already logged) into the next report.
    fn drop_frame_results(&mut self) {
        self.frame_captures.clear();
        self.frame_diagnostics.clear();
    }

    fn ensure_device(&self) -> Result<(), RenderError> {
        if self.device.is_lost() {
            Err(RenderError::DeviceLost)
        } else {
            Ok(())
        }
    }

    fn commit_frame_state(&mut self) {
        for id in self.pending_offscreen.drain(..) {
            if let Some(TargetKind::Offscreen { state, .. }) = self.targets.get_mut(id).map(|target| &mut target.kind) {
                *state = OFFSCREEN_FINAL;
            }
        }
        self.runner.commit_frame();
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
                // A minimised window keeps its last configuration on the device; it waits for a real size.
                if config.format.is_none() || config.size.is_empty() {
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
                self.pending_offscreen.push(id);
                OutputTarget {
                    texture: *texture,
                    desc: *desc,
                    initial: *state,
                    final_access: OFFSCREEN_FINAL,
                    keeps_contents: false,
                }
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
        view: &ViewInput,
        input: &FrameInput,
        output: OutputTarget,
        commands: &mut CommandList,
    ) -> Result<(GraphStats, bool), RenderError> {
        let output_size = PhysicalSize::new(output.desc.size.width, output.desc.size.height);
        let mut view_ctx = ViewCtx::new(view_id(view.target), output_size, view.camera);
        view_ctx.frame = self.frame;
        view_ctx.record_epoch = self.record_epoch;
        view_ctx.time_seconds = input.time_seconds;
        view_ctx.delta_seconds = input.delta_seconds;
        self.registry.prepare_view(&mut view_ctx, &self.shared);

        let sizes = FrameSizes {
            render: view_ctx.render_size,
            output: output_size,
        };
        let device = self.device.as_ref();
        let registry = &mut self.registry;
        let shared = &mut self.shared;
        // `self.release` is lent to the runner while `fill` runs, so packs queue here and the queue is drained below.
        let mut released = Vec::new();
        let fill = |graph: &mut FrameGraph, output_handle: TextureHandle| {
            let mut slots = Slots::new();
            let mut diagnostics = Vec::new();
            let mut ctx = RecordCtx::new(
                graph,
                &mut slots,
                &view_ctx,
                device,
                &mut diagnostics,
                &mut released,
                shared,
            );
            ctx.set_slot(Slot::OUTPUT, output_handle);
            let record_failures = registry.record_view(&mut ctx);
            diagnostics.extend(record_failures);
            diagnostics
        };
        let env = FrameEnv {
            device,
            release: &mut self.release,
            frame: self.frame,
        };
        let mut diagnostics = Vec::new();
        let run = self
            .runner
            .run_view(view.target, output, sizes, fill, env, &mut diagnostics);
        for resource in released {
            self.release.release(self.frame, resource);
        }
        for diagnostic in &diagnostics {
            self.report(diagnostic);
        }
        let run = run?;
        commands.append(run.commands);
        self.frame_captures.extend(run.capture);
        Ok((run.stats, run.compiled))
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
        #[cfg_attr(
            target_arch = "wasm32",
            expect(clippy::arc_with_non_send_sync, reason = "one thread on the web (ADR-0004)")
        )]
        let device = Arc::new(Device::build_async(request).await?);
        self.reset(device)
    }

    // Moves the renderer onto a new device. Surfaces pathon created and offscreen targets are rebuilt; surfaces and
    // textures the host handed in are dropped and must be added again.
    pub fn reset(&mut self, device: Arc<Device>) -> Result<(), RenderError> {
        self.device = device;
        self.runner.forget_device();
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

// Generation in the high half: a re-created target reuses the slot index but gets a new id.
fn view_id(target: TargetId) -> ViewId {
    let raw = target.raw();
    ViewId((u64::from(raw.generation()) << 32) | u64::from(raw.index()))
}

#[cfg(test)]
mod tests {
    use pathon_api::graph::{PassDesc, Stage};
    use pathon_api::{Camera, RenderFeature};
    use pathon_rhi::Backend;

    use super::*;

    // Fails to record and leaves behind a pass that reports the device lost, so the same frame ends in an error.
    struct FailsThenLosesDevice;

    impl RenderFeature for FailsThenLosesDevice {
        fn name(&self) -> &'static str {
            "fails then loses device"
        }

        fn stage(&self) -> Stage {
            Stage::PostProcess
        }

        fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
            let pass = ctx
                .graph()
                .add_pass(PassDesc::new("loses device", Stage::PostProcess).side_effect());
            pass.record(|_| Err(RhiError::DeviceLost));
            Err(RhiError::InvalidHandle("broken pack"))
        }
    }

    #[test]
    fn view_diagnostics_are_reported_when_the_frame_then_fails() {
        let mut renderer = Renderer::builder()
            .with_device_request(DeviceRequest {
                backend: Some(Backend::Null),
                ..DeviceRequest::default()
            })
            .with_feature(FailsThenLosesDevice)
            .build()
            .expect("renderer");
        let target = renderer
            .add_offscreen_target(PhysicalSize::new(64, 32), Format::Rgba8Unorm)
            .expect("offscreen target");
        let input = FrameInput {
            views: vec![ViewInput {
                target,
                camera: Camera::default(),
            }],
            ..FrameInput::default()
        };
        let mut commands = CommandList::new();

        let result = renderer.record_views(&input, &mut commands, &mut Vec::new(), &mut FrameReport::default());

        assert!(matches!(result, Err(RenderError::DeviceLost)));
        let reported: Vec<&str> = renderer.frame_diagnostics.iter().map(|d| d.code).collect();
        assert_eq!(reported, ["F0105"]);
    }
}
