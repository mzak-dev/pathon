#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

// A feature that runs `gi_sample` of the WGSL module at the points a test asks for and copies the answers into a
// buffer: the sampling function as the lighting shader calls it, without the lighting shader.

use std::sync::{Arc, Mutex};

use pathon_api::glam::Vec3;
use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource, BindingType,
    BufferDesc, BufferId, BufferUsage, ComputePassDesc, ComputePipelineDesc, ComputePipelineId, DeviceApi,
    RhiError, ShaderStage, ShaderStages, StorageAccess,
};
use pathon_api::{RecordCtx, RenderFeature};
use pathon_gi::GiBindings;
use pathon_shader::{ShaderFeatures, ShaderKey, ShaderLibrary};

const SOURCE: &str = "
import package::gi::common::GiVolume;
import package::gi::sample::gi_sample;
// GI_BINDINGS
@group(0) @binding(4) var<storage, read> points: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> results: array<vec4<f32>>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= arrayLength(&points) / 2u) {
        return;
    }
    let position = points[id.x * 2u].xyz;
    let normal = points[id.x * 2u + 1u].xyz;
    let sampled = gi_sample(gi_volume, gi_irradiance_atlas, gi_visibility_atlas, gi_atlas_sampler, position, normal, normal, normal);
    results[id.x * 2u] = vec4<f32>(sampled.irradiance, sampled.confidence);
    results[id.x * 2u + 1u] = vec4<f32>(sampled.sky_visibility, 0.0, 0.0, 0.0);
}
";

/// What the shader answered for a point.
#[derive(Clone, Copy, Debug)]
pub struct Sampled {
    pub irradiance: Vec3,
    pub confidence: f32,
    pub sky_visibility: f32,
}

#[derive(Debug, Default)]
pub struct Requests {
    pub points: Vec<(Vec3, Vec3)>,
    pub answer: Option<(BufferId, usize)>,
}

pub type SharedRequests = Arc<Mutex<Requests>>;

#[derive(Debug)]
struct Gpu {
    pipeline: ComputePipelineId,
    layout: pathon_api::rhi::BindGroupLayoutId,
}

pub struct Sampler {
    pub requests: SharedRequests,
    gpu: Option<Gpu>,
    capacity: usize,
    buffers: Option<(BufferId, BufferId, BufferId)>,
}

impl Sampler {
    pub fn new(requests: SharedRequests) -> Self {
        Self {
            requests,
            gpu: None,
            capacity: 0,
            buffers: None,
        }
    }

    fn gpu(&mut self, device: &dyn DeviceApi) -> Result<&Gpu, RhiError> {
        if self.gpu.is_none() {
            let bindings = GiBindings::starting_at(0);
            let mut library = ShaderLibrary::new();
            for (path, source) in pathon_gi::modules() {
                library.add_module(path, source).expect("gi modules");
            }
            let source = SOURCE.replace("// GI_BINDINGS", &bindings.wgsl_declarations(0));
            library.add_root("sample_test", &source);
            let module = library
                .module(
                    device,
                    &ShaderKey {
                        root: "sample_test".to_owned(),
                        features: ShaderFeatures::default(),
                    },
                )
                .unwrap_or_else(|error| panic!("{error}"));
            let mut entries = bindings.layout_entries(ShaderStages::COMPUTE);
            for (binding, access) in [(4, StorageAccess::ReadOnly), (5, StorageAccess::ReadWrite)] {
                entries.push(BindGroupLayoutEntry {
                    binding,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageBuffer { access },
                    count: None,
                });
            }
            let layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
                label: "gi sample test",
                entries,
            })?;
            let pipeline = device.create_compute_pipeline(&ComputePipelineDesc {
                label: "gi sample test",
                bind_group_layouts: vec![layout],
                shader: ShaderStage {
                    module,
                    entry_point: "main",
                    constants: vec![],
                },
                immediate_size: 0,
            })?;
            self.gpu = Some(Gpu { pipeline, layout });
        }
        Ok(self.gpu.as_ref().expect("built above"))
    }
}

impl RenderFeature for Sampler {
    fn name(&self) -> &'static str {
        "gi sampler test"
    }

    fn stage(&self) -> Stage {
        Stage::Overlay
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(frame) = ctx.shared().get::<pathon_gi::GiFrame>().copied() else {
            return Ok(());
        };
        let points = self.requests.lock().expect("mutex").points.clone();
        if points.is_empty() {
            return Ok(());
        }
        let (pipeline, layout) = {
            let gpu = self.gpu(ctx.device())?;
            (gpu.pipeline, gpu.layout)
        };
        let count = points.len();
        if self.buffers.is_none() || self.capacity < count {
            let device = ctx.device();
            let make = |label, usage| {
                device.create_buffer(&BufferDesc {
                    label,
                    size: (count as u64 * 32).max(256),
                    usage,
                })
            };
            self.buffers = Some((
                make("gi sample points", BufferUsage::STORAGE | BufferUsage::COPY_DST)?,
                make("gi sample results", BufferUsage::STORAGE | BufferUsage::COPY_SRC)?,
                make("gi sample readback", BufferUsage::MAP_READ | BufferUsage::COPY_DST)?,
            ));
            self.capacity = count;
        }
        let (points_buffer, results_buffer, readback) = self.buffers.expect("made above");
        let bytes: Vec<f32> = points
            .iter()
            .flat_map(|(position, normal)| [position.x, position.y, position.z, 0.0, normal.x, normal.y, normal.z, 0.0])
            .collect();
        ctx.device().write_buffer(points_buffer, 0, bytemuck::cast_slice(&bytes))?;
        self.requests.lock().expect("mutex").answer = Some((readback, count));

        let size = (count as u64 * 32).max(256);
        let resting = Access::STORAGE_READ;
        let points_handle = ctx.graph().import_buffer("gi sample points", points_buffer, size, resting, resting);
        let results_handle = ctx.graph().import_buffer("gi sample results", results_buffer, size, resting, resting);
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("gi sample test", Stage::Overlay).priority(-1).side_effect());
        let points_handle = pass.modify_buffer(points_handle, Access::STORAGE_READ | Access::STORAGE_WRITE);
        let results_handle = pass.modify_buffer(results_handle, Access::STORAGE_READ | Access::STORAGE_WRITE);
        let irradiance = pass.read(frame.irradiance, Access::SAMPLED);
        let visibility = pass.read(frame.visibility, Access::SAMPLED);
        pass.record(move |pass| {
            let (irradiance_view, visibility_view) = (pass.view(irradiance)?, pass.view(visibility)?);
            let mut entries = GiBindings::starting_at(0).bind_group_entries(&frame, irradiance_view, visibility_view);
            entries.push(BindGroupEntry {
                binding: 4,
                resource: BindingResource::whole_buffer(pass.buffer(points_handle)),
            });
            entries.push(BindGroupEntry {
                binding: 5,
                resource: BindingResource::whole_buffer(pass.buffer(results_handle)),
            });
            let group = pass.create_bind_group(&BindGroupDesc {
                label: "gi sample test",
                layout,
                entries,
            })?;
            let mut compute = pass.commands().compute_pass(ComputePassDesc {
                label: "gi sample test",
                timestamps: None,
            });
            compute.set_pipeline(pipeline);
            compute.set_bind_group(0, group);
            compute.dispatch(u32::try_from(count).expect("few points").div_ceil(64), 1, 1);
            Ok(())
        });
        let mut copy = ctx
            .graph()
            .add_pass(PassDesc::new("gi sample copy", Stage::Overlay).priority(-1).side_effect());
        let results_handle = copy.read_buffer(results_handle, Access::COPY_SRC);
        copy.record(move |pass| {
            let source = pass.buffer(results_handle);
            pass.commands().copy_buffer_to_buffer(source, 0, readback, 0, size);
            Ok(())
        });
        Ok(())
    }
}

pub fn decode(device: &dyn DeviceApi, requests: &SharedRequests) -> Vec<Sampled> {
    let (buffer, count) = requests.lock().expect("mutex").answer.expect("a frame ran the sampler");
    let bytes = device.read_buffer(buffer).expect("read the answers");
    let floats: &[f32] = bytemuck::cast_slice(&bytes[..count * 32]);
    (0..count)
        .map(|index| Sampled {
            irradiance: Vec3::new(floats[index * 8], floats[index * 8 + 1], floats[index * 8 + 2]),
            confidence: floats[index * 8 + 3],
            sky_visibility: floats[index * 8 + 4],
        })
        .collect()
}
