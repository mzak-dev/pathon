// M4-02: CPU cost of extracting instances into the render world. Provisional budget (ADR-0029): 10 000 instances in
// under 2 ms. Run: cargo bench -p pathon-scene

use divan::Bencher;
use pathon_api::glam::{Affine3A, Vec3};
use pathon_api::rhi::{BufferId, Id, RawHandle};
use pathon_scene::{InstanceDesc, InstanceId, MaterialHandle, MeshHandle, Scene};

fn main() {
    divan::main();
}

fn at(x: f32) -> Affine3A {
    Affine3A::from_translation(Vec3::new(x, 0.0, 0.0))
}

fn populated(count: usize) -> (Scene, Vec<InstanceId>) {
    let mesh = MeshHandle::from_id(BufferId::from_raw(RawHandle::new(0, 0)));
    let mut scene = Scene::new();
    let ids = scene.extract(|writer| {
        (0..count)
            .map(|index| {
                let desc = InstanceDesc::new(mesh, MaterialHandle::DEFAULT, at(index as f32));
                writer.spawn(&desc).expect("finite")
            })
            .collect()
    });
    (scene, ids)
}

#[divan::bench(args = [10_000, 100_000])]
fn spawn_all(bencher: Bencher<'_, '_>, count: usize) {
    let mesh = MeshHandle::from_id(BufferId::from_raw(RawHandle::new(0, 0)));
    bencher.with_inputs(Scene::new).bench_local_values(|mut scene| {
        scene.extract(|writer| {
            for index in 0..count {
                let desc = InstanceDesc::new(mesh, MaterialHandle::DEFAULT, at(index as f32));
                writer.spawn(&desc).expect("finite");
            }
        });
        scene
    });
}

#[divan::bench(args = [10_000, 100_000])]
fn move_all(bencher: Bencher<'_, '_>, count: usize) {
    let (mut scene, ids) = populated(count);
    let mut offset = 0.0_f32;
    bencher.bench_local(|| {
        offset += 1.0;
        scene.extract(|writer| {
            for (index, id) in ids.iter().enumerate() {
                writer.set_transform(*id, at(index as f32 + offset)).expect("live");
            }
        });
    });
}

#[divan::bench(args = [10_000, 100_000])]
fn move_one_percent(bencher: Bencher<'_, '_>, count: usize) {
    let (mut scene, ids) = populated(count);
    let mut offset = 0.0_f32;
    bencher.bench_local(|| {
        offset += 1.0;
        scene.extract(|writer| {
            for (index, id) in ids.iter().enumerate().step_by(100) {
                writer.set_transform(*id, at(index as f32 + offset)).expect("live");
            }
        });
    });
}

#[divan::bench(args = [10_000, 100_000])]
fn unchanged(bencher: Bencher<'_, '_>, count: usize) {
    let (mut scene, ids) = populated(count);
    bencher.bench_local(|| {
        scene.extract(|writer| {
            for (index, id) in ids.iter().enumerate() {
                writer.set_transform(*id, at(index as f32)).expect("live");
            }
        });
    });
}
