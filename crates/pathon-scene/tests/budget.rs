// Extract budget (ADR-0029, ADR-0035): the limit is read from docs/bench/budgets.toml, the same file `cargo xtask
// bench` and CI use. Timing, so it is ignored in the default run; measured on demand:
//   cargo test -p pathon-scene --release --test budget -- --ignored --nocapture

use std::path::Path;
use std::time::{Duration, Instant};

use pathon_api::glam::{Affine3A, Vec3};
use pathon_api::rhi::{BufferId, Id, RawHandle};
use pathon_bench::Budgets;
use pathon_scene::{InstanceDesc, InstanceId, MaterialHandle, MeshHandle, Scene};

const INSTANCES: usize = 10_000;
const RUNS: usize = 41;

fn at(x: f32) -> Affine3A {
    Affine3A::from_translation(Vec3::new(x, 0.0, 0.0))
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn budget(metric: &str) -> Duration {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/bench/budgets.toml");
    let limit = Budgets::load(&path).expect("budgets.toml").max(metric);
    Duration::from_secs_f64(limit.value / 1e9)
}

#[test]
fn the_budget_file_has_the_extract_limits() {
    assert!(budget("scene.extract.spawn_10k") > Duration::ZERO);
    assert!(budget("scene.extract.move_10k") > Duration::ZERO);
}

#[test]
#[ignore = "timing: run in release, see the header"]
fn extracting_ten_thousand_instances_fits_the_budget() {
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
    let (spawn_budget, update_budget) = (budget("scene.extract.spawn_10k"), budget("scene.extract.move_10k"));
    eprintln!(
        "extract of {INSTANCES} instances, median of {RUNS}: spawn all {spawn:?} (budget {spawn_budget:?}), \
         move all {update:?} (budget {update_budget:?})"
    );
    assert!(
        spawn < spawn_budget,
        "spawn {spawn:?} exceeds the budget {spawn_budget:?}"
    );
    assert!(
        update < update_budget,
        "move {update:?} exceeds the budget {update_budget:?}"
    );
}
