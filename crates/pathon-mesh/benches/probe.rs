// Probe for `cargo xtask bench` (M4-17, ADR-0035): CPU cost of the geometry pool on the null device, plus the number of
// allocations a thousand uploads make. Run by hand: cargo bench -p pathon-mesh --bench probe

use std::time::{Duration, Instant};

use pathon_bench::{CountingAllocator, Metric, count_allocations, emit};
use pathon_mesh::{GeometryPool, IndexData, MeshData, MeshId, PoolDesc, VertexLayout};
use pathon_rhi::Device;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SAMPLES: usize = 31;
const WARMUP_RUNS: usize = 3;
const MESHES: usize = 1000;
const VERTICES: usize = 24;
const INDICES: usize = 36;
const FRAMES_IN_FLIGHT: u64 = 2;

struct Geometry {
    positions: Vec<[f32; 3]>,
    surface: Vec<u8>,
    indices: Vec<u32>,
}

impl Geometry {
    fn cube_like() -> Self {
        Self {
            positions: (0..VERTICES).map(|index| [index as f32, 0.0, 0.0]).collect(),
            surface: vec![0; VERTICES * 36],
            indices: (0..INDICES)
                .map(|index| u32::try_from(index % VERTICES).expect("small"))
                .collect(),
        }
    }

    fn data(&self) -> MeshData<'_> {
        MeshData::new(
            "probe mesh",
            vec![bytemuck::cast_slice(&self.positions), &self.surface],
            IndexData::U32(&self.indices),
        )
    }
}

fn new_pool(device: &Device) -> GeometryPool {
    GeometryPool::new(
        device,
        "probe pool",
        VertexLayout::standard(),
        PoolDesc::default().with_frames_in_flight(FRAMES_IN_FLIGHT),
    )
}

fn upload_all(pool: &mut GeometryPool, device: &Device, data: &MeshData<'_>) -> Vec<MeshId> {
    (0..MESHES)
        .map(|_| pool.upload(device, data).expect("upload"))
        .collect()
}

fn time(action: impl FnOnce()) -> Duration {
    let start = Instant::now();
    action();
    start.elapsed()
}

fn main() {
    let device = Device::null();
    let geometry = Geometry::cube_like();
    let data = geometry.data();

    let mut upload_samples = Vec::new();
    let mut recycle_samples = Vec::new();
    for run in 0..WARMUP_RUNS + SAMPLES {
        let mut pool = new_pool(&device);
        let mut ids = Vec::new();
        let upload = time(|| ids = upload_all(&mut pool, &device, &data));
        // Recycling: remove every mesh, let the frames in flight pass so the ranges come back, upload again.
        let recycle = time(|| {
            for id in &ids {
                pool.remove(*id, 0).expect("live");
            }
            pool.begin_frame(&device, FRAMES_IN_FLIGHT);
            upload_all(&mut pool, &device, &data);
        });
        pool.release_all(&device);
        if run >= WARMUP_RUNS {
            upload_samples.push(upload);
            recycle_samples.push(recycle);
        }
    }
    emit(&Metric::timing("mesh.pool.upload_1000", &upload_samples));
    emit(&Metric::timing("mesh.pool.recycle_1000", &recycle_samples));

    let mut pool = new_pool(&device);
    let (_, used) = count_allocations(|| upload_all(&mut pool, &device, &data));
    emit(&Metric::count("mesh.pool.upload_allocations_1000", used.allocations));
    pool.release_all(&device);
}
