// Provisional extract budget (ADR-0029), measured on demand; M4-17 moves it into the shared budget table:
//   cargo test -p pathon-scene --release --test budget -- --ignored --nocapture

use std::time::{Duration, Instant};

use pathon_api::glam::{Affine3A, Vec3};
use pathon_api::rhi::{BufferId, Id, RawHandle};
use pathon_scene::{InstanceDesc, InstanceId, MaterialHandle, MeshHandle, Scene};

const INSTANCES: usize = 10_000;
const BUDGET: Duration = Duration::from_millis(2);
const RUNS: usize = 41;

fn at(x: f32) -> Affine3A {
    Affine3A::from_translation(Vec3::new(x, 0.0, 0.0))
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

#[test]
#[ignore = "timing: run in release, see the header"]
fn extracting_ten_thousand_instances_fits_the_provisional_budget() {
    let mesh = MeshHandle::from_id(BufferId::from_raw(RawHandle::new(0, 0)));
    let mut spawn_samples = Vec::new();
    let mut update_samples = Vec::new();

    for _ in 0..RUNS {
        let mut scene = Scene::new();
        let start = Instant::now();
        let ids: Vec<InstanceId> = scene.extract(|writer| {
            (0..INSTANCES)
                .map(|index| {
                    let desc = InstanceDesc::new(mesh, MaterialHandle::DEFAULT, at(index as f32));
                    writer.spawn(&desc).expect("finite")
                })
                .collect()
        });
        spawn_samples.push(start.elapsed());

        for step in 1..=3 {
            scene.extract(|writer| {
                for (index, id) in ids.iter().enumerate() {
                    writer.set_transform(*id, at(index as f32 + step as f32)).expect("live");
                }
            });
        }
        let start = Instant::now();
        scene.extract(|writer| {
            for (index, id) in ids.iter().enumerate() {
                writer.set_transform(*id, at(index as f32 + 9.0)).expect("live");
            }
        });
        update_samples.push(start.elapsed());
    }

    let (spawn, update) = (median(spawn_samples), median(update_samples));
    eprintln!("extract of {INSTANCES} instances, median of {RUNS}: spawn all {spawn:?}, move all {update:?}");
    assert!(spawn < BUDGET && update < BUDGET, "budget {BUDGET:?} exceeded");
}
