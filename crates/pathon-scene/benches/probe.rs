// Probe for `cargo xtask bench` (M4-17, ADR-0035): CPU cost of extracting instances into the render world. The budget
// for 10 000 instances lives in docs/bench/budgets.toml. Run by hand: cargo bench -p pathon-scene --bench probe

use std::time::{Duration, Instant};

use pathon_api::glam::{Affine3A, Vec3};
use pathon_api::rhi::{BufferId, Id, RawHandle};
use pathon_bench::{CountingAllocator, Metric, count_allocations, emit};
use pathon_scene::{InstanceDesc, InstanceId, MaterialHandle, MeshHandle, Scene};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SAMPLES: usize = 31;
const WARMUP_RUNS: usize = 3;

fn at(x: f32) -> Affine3A {
    Affine3A::from_translation(Vec3::new(x, 0.0, 0.0))
}

fn spawn_all(scene: &mut Scene, count: usize) -> Vec<InstanceId> {
    let mesh = MeshHandle::from_id(BufferId::from_raw(RawHandle::new(0, 0)));
    scene.extract(|writer| {
        (0..count)
            .map(|index| {
                let desc = InstanceDesc::new(mesh, MaterialHandle::DEFAULT, at(index as f32));
                writer.spawn(&desc).expect("finite")
            })
            .collect()
    })
}

fn move_all(scene: &mut Scene, ids: &[InstanceId], offset: f32) {
    scene.extract(|writer| {
        for (index, id) in ids.iter().enumerate() {
            writer.set_transform(*id, at(index as f32 + offset)).expect("live");
        }
    });
}

fn time(action: impl FnOnce()) -> Duration {
    let start = Instant::now();
    action();
    start.elapsed()
}

fn measure(count: usize, label: &str) {
    let mut spawn_samples = Vec::new();
    let mut move_samples = Vec::new();
    for run in 0..WARMUP_RUNS + SAMPLES {
        let mut scene = Scene::new();
        let mut ids = Vec::new();
        let spawn = time(|| ids = spawn_all(&mut scene, count));
        // Three extracts settle both buffers, so the timed one is the steady state a running host sees.
        for step in 1..=3 {
            move_all(&mut scene, &ids, step as f32);
        }
        let moved = time(|| move_all(&mut scene, &ids, 9.0));
        if run >= WARMUP_RUNS {
            spawn_samples.push(spawn);
            move_samples.push(moved);
        }
    }
    emit(&Metric::timing(&format!("scene.extract.spawn_{label}"), &spawn_samples));
    emit(&Metric::timing(&format!("scene.extract.move_{label}"), &move_samples));
}

fn measure_steady_state_allocations(count: usize, label: &str) {
    let mut scene = Scene::new();
    let ids = spawn_all(&mut scene, count);
    for step in 1..=3 {
        move_all(&mut scene, &ids, step as f32);
    }
    let ((), used) = count_allocations(|| move_all(&mut scene, &ids, 9.0));
    emit(&Metric::count(
        &format!("scene.extract.steady_allocations_{label}"),
        used.allocations,
    ));
}

fn main() {
    measure(10_000, "10k");
    measure(100_000, "100k");
    measure_steady_state_allocations(10_000, "10k");
}
