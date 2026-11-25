#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

mod draw;

use pathon_api::glam::{Mat4, Vec3};
use pathon_api::rhi::{
    Access, Backend, BufferDesc, BufferId, BufferUsage, CommandList, DeviceApi, DeviceRequest, Format, PhysicalSize,
};
use pathon_api::{Camera, Diagnostic};
use pathon_core::{FrameInput, FrameReport, RenderError, Renderer, RendererBuilder, ViewInput};
use pathon_mesh::{DeformedPositions, DeformedRange, GeometryPool, MeshId, PoolDesc, VertexLayout};
use pathon_skin::reference::{self, DeformedVertex, Influences, MorphDelta, RestVertex};
use pathon_skin::{
    DeformedSurface, MorphTarget, MorphTargets, MotionHistory, SampleArm, SkinFrame, SkinInstance, SkinOutput,
    SkinPack, SkinSource, SkinTargets,
};

pub const ARM_RINGS: u32 = 16;
pub const ARM_SIDES: u32 = 8;
const CAPACITY: u32 = 1024;
// What the tests treat as the pack's output: a GPU deformation within float rounding of the CPU.
pub const TOLERANCE: f32 = 1e-4;

/// `PATHON_TEST_BACKEND` picks the backend; without it the device request decides, like in the other golden tests.
pub fn backend_from_environment() -> Option<Backend> {
    match std::env::var("PATHON_TEST_BACKEND").ok().as_deref() {
        None => None,
        Some("vulkan") => Some(Backend::Vulkan),
        Some("dx12") => Some(Backend::Dx12),
        Some("metal") => Some(Backend::Metal),
        Some(other) => panic!("PATHON_TEST_BACKEND `{other}` is not vulkan, dx12 or metal"),
    }
}

/// What one instance does in one frame.
#[derive(Clone, Debug)]
pub struct Pose {
    pub bones: Vec<Mat4>,
    pub weights: Vec<f32>,
    pub history: MotionHistory,
    /// Unrelated matrices put before this instance's slice, so its `first` is not zero.
    pub padding_before: usize,
}

impl Pose {
    pub fn elbow(angle: f32, history: MotionHistory) -> Self {
        Self {
            bones: SampleArm::pose(angle).to_vec(),
            weights: Vec::new(),
            history,
            padding_before: 0,
        }
    }
}

/// The renderer with `SkinPack`, an arm in a skinned pool and room for several deformed instances of it.
pub struct Rig {
    pub renderer: Renderer,
    pub arm: SampleArm,
    pub pool: GeometryPool,
    pub mesh: MeshId,
    pub positions: DeformedPositions,
    pub surface: DeformedSurface,
    pub ranges: Vec<DeformedRange>,
    pub morph: Option<MorphTargets>,
    views: Vec<ViewInput>,
}

impl Rig {
    pub fn build(backend: Option<Backend>, instances: usize, views: usize) -> Result<Self, RenderError> {
        Self::build_with(backend, instances, views, |builder| builder)
    }

    /// `configure` adds what the test needs besides the skin pack, such as a pass that reads its output.
    pub fn build_with(
        backend: Option<Backend>,
        instances: usize,
        views: usize,
        configure: impl FnOnce(RendererBuilder) -> RendererBuilder,
    ) -> Result<Self, RenderError> {
        let builder = Renderer::builder()
            .with_device_request(DeviceRequest {
                label: "skin test",
                backend,
                ..DeviceRequest::default()
            })
            .with_pack(SkinPack::new());
        let mut renderer = configure(builder).build()?;
        let views = (0..views)
            .map(|_| {
                let target = renderer
                    .add_offscreen_target(PhysicalSize::new(16, 16), Format::Rgba8Unorm)
                    .expect("target");
                ViewInput {
                    target,
                    camera: Camera::default(),
                }
            })
            .collect();
        let arm = SampleArm::new(ARM_RINGS, ARM_SIDES);
        let device = std::sync::Arc::clone(renderer.device());
        let mut pool = GeometryPool::new(&*device, "skin test", VertexLayout::skinned(), PoolDesc::default());
        let mesh = pool.upload(&*device, &arm.mesh_data("arm")).expect("arm");
        let mut positions = DeformedPositions::new(&*device, "deformed positions", CAPACITY, 2).expect("positions");
        let surface = DeformedSurface::new(&*device, "deformed surface", CAPACITY).expect("surface");
        let ranges = (0..instances)
            .map(|_| positions.allocate(arm.vertex_count()).expect("range"))
            .collect();
        Ok(Self {
            renderer,
            arm,
            pool,
            mesh,
            positions,
            surface,
            ranges,
            morph: None,
            views,
        })
    }

    pub fn device(&self) -> &dyn DeviceApi {
        &**self.renderer.device()
    }

    pub fn add_morph_targets(&mut self, targets: &[MorphTarget]) {
        self.morph =
            Some(MorphTargets::upload(self.device(), "arm morphs", self.arm.vertex_count(), targets).expect("morphs"));
    }

    pub fn frame_with(&mut self, build: impl FnOnce(&mut SkinFrame)) -> Result<FrameReport, RenderError> {
        let targets = SkinTargets::new(&self.positions, &self.surface).expect("targets");
        let mut frame = SkinFrame::new(targets);
        build(&mut frame);
        self.renderer.shared_mut().insert(frame);
        let input = FrameInput {
            views: self.views.clone(),
            ..FrameInput::default()
        };
        self.renderer.render_frame(&input)
    }

    /// One instance per pose, each in its own deformed range, in one frame.
    pub fn frame(&mut self, poses: &[Pose]) -> FrameReport {
        let source = SkinSource::from_pool(&self.pool, self.mesh).expect("source");
        let (ranges, morph) = (self.ranges.clone(), self.morph);
        let report = self
            .frame_with(|frame| {
                for (pose, range) in poses.iter().zip(ranges) {
                    frame
                        .bone_matrices
                        .extend((0..pose.padding_before).map(|_| Mat4::from_scale(Vec3::splat(1000.0))));
                    let first = u32::try_from(frame.bone_matrices.len()).expect("small");
                    frame.bone_matrices.extend_from_slice(&pose.bones);
                    let mut instance = SkinInstance::new(source, range)
                        .with_bones(first, u32::try_from(pose.bones.len()).expect("small"));
                    if let Some(morph) = morph {
                        let first_weight = u32::try_from(frame.morph_weights.len()).expect("small");
                        frame.morph_weights.extend_from_slice(&pose.weights);
                        instance = instance.with_morph(morph, first_weight);
                    }
                    if pose.history == MotionHistory::Continuing {
                        instance = instance.continuing();
                    }
                    frame.instances.push(instance);
                }
            })
            .expect("frame");
        let errors: Vec<&Diagnostic> = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == pathon_api::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
        report
    }

    pub fn output(&self) -> SkinOutput {
        *self
            .renderer
            .shared()
            .get::<SkinOutput>()
            .expect("the pack published its output")
    }

    pub fn read(&self, buffer: BufferId, offset: u64, size: u64, resting: Access) -> Vec<u8> {
        let device = self.device();
        let readback = device
            .create_buffer(&BufferDesc {
                label: "skin readback",
                size,
                usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
            })
            .expect("readback");
        let mut list = CommandList::new();
        list.buffer_access(buffer, Access::COPY_SRC);
        list.copy_buffer_to_buffer(buffer, offset, readback, 0, size);
        list.buffer_access(buffer, resting);
        device.submit(list).expect("submit");
        let bytes = device.read_buffer(readback).expect("read");
        device.destroy(pathon_api::rhi::Resource::Buffer(readback));
        bytes
    }

    /// Positions of the instance's range in the bank of `frame & 1` (`0` current, `1` previous of that frame).
    pub fn positions_of(&self, instance: usize, frame: u64, previous: bool) -> Vec<[f32; 3]> {
        self.positions_in(self.ranges[instance], frame, previous)
    }

    pub fn positions_in(&self, range: DeformedRange, frame: u64, previous: bool) -> Vec<[f32; 3]> {
        let offset = if previous {
            self.positions.previous_offset(range, frame)
        } else {
            self.positions.current_offset(range, frame)
        };
        let size = u64::from(range.vertex_count) * pathon_mesh::DEFORMED_POSITION_STRIDE;
        let bytes = self.read(
            self.positions.buffer(),
            offset,
            size,
            Access::VERTEX | Access::STORAGE_READ,
        );
        bytemuck::cast_slice::<u8, [f32; 4]>(&bytes)
            .iter()
            .map(|position| {
                assert_eq!(position[3], 1.0, "w is one");
                [position[0], position[1], position[2]]
            })
            .collect()
    }

    pub fn surface_of(&self, instance: usize) -> Vec<[f32; 9]> {
        self.surface_in(self.ranges[instance])
    }

    pub fn surface_in(&self, range: DeformedRange) -> Vec<[f32; 9]> {
        let size = u64::from(range.vertex_count) * pathon_skin::DEFORMED_SURFACE_STRIDE;
        let bytes = self.read(
            self.surface.buffer(),
            DeformedSurface::offset(range.first_vertex),
            size,
            Access::VERTEX | Access::STORAGE_READ,
        );
        bytemuck::cast_slice::<u8, [f32; 9]>(&bytes).to_vec()
    }

    /// The CPU result for every vertex of the arm.
    pub fn expected(&self, pose: &Pose, morphs: &[MorphTarget]) -> Vec<DeformedVertex> {
        self.arm
            .rest()
            .iter()
            .zip(self.arm.influences())
            .enumerate()
            .map(|(vertex, (rest, influences))| expected_vertex(rest, *influences, pose, morphs, vertex))
            .collect()
    }
}

pub fn expected_vertex(
    rest: &RestVertex,
    influences: Influences,
    pose: &Pose,
    morphs: &[MorphTarget],
    vertex: usize,
) -> DeformedVertex {
    let deltas: Vec<MorphDelta> = morphs
        .iter()
        .zip(&pose.weights)
        .map(|(target, weight)| (*weight, target.delta(vertex)))
        .collect();
    reference::deform_vertex(rest, Some((influences, &pose.bones)), &deltas)
}

pub fn assert_close(actual: [f32; 3], expected: [f32; 3], what: &str) {
    for axis in 0..3 {
        assert!(
            (actual[axis] - expected[axis]).abs() <= TOLERANCE,
            "{what}: {actual:?} differs from {expected:?}"
        );
    }
}
