use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use pathon_api::glam::{Affine3A, Vec2, Vec3};
use pathon_api::rhi::{BufferId, Id, PhysicalSize, RawHandle};
use pathon_api::{Camera, ViewCtx, ViewId};
use pathon_scene::{
    CameraDesc, Dirty, InstanceDesc, InstanceId, LayerMask, Light, MaterialHandle, MeshHandle, Projection, Scene,
    SceneError,
};

fn mesh(index: u32) -> MeshHandle {
    MeshHandle::from_id(BufferId::from_raw(RawHandle::new(index, 0)))
}

fn at(x: f32) -> Affine3A {
    Affine3A::from_translation(Vec3::new(x, 0.0, 0.0))
}

fn desc(x: f32) -> InstanceDesc {
    InstanceDesc::new(mesh(0), MaterialHandle::DEFAULT, at(x))
}

fn spawn_many(scene: &mut Scene, count: usize) -> Vec<InstanceId> {
    scene.extract(|writer| {
        (0..count)
            .map(|index| writer.spawn(&desc(index as f32)).expect("finite"))
            .collect()
    })
}

#[test]
fn an_empty_scene_extracts_and_reads_without_panicking() {
    let mut scene = Scene::new();
    assert_eq!(scene.world().epoch(), 0);
    assert!(scene.world().instances().is_empty());

    for _ in 0..3 {
        scene.extract(|_| {});
    }

    let world = scene.snapshot();
    assert_eq!(world.epoch(), 3);
    assert_eq!(world.instances().slot_count(), 0);
    assert_eq!(world.instances().iter_alive().count(), 0);
    assert_eq!(world.instances().iter_visible(LayerMask::ALL).count(), 0);
    assert_eq!(world.instances().changes().len(), 0);
    assert!(world.cameras().is_empty() && world.lights().is_empty());
    assert_eq!(scene.last_extract().changed_instances, 0);
}

#[test]
fn a_snapshot_taken_before_an_extract_does_not_change_during_it() {
    let mut scene = Scene::new();
    let ids = spawn_many(&mut scene, 4);
    let before = scene.snapshot();
    let before_transforms = before.instances().transforms().to_vec();

    scene.extract(|writer| {
        for (index, id) in ids.iter().enumerate() {
            writer.set_transform(*id, at(100.0 + index as f32)).expect("live");
        }
        writer.despawn(ids[0]).expect("live");
        // Mid-extract, the reader still sees the previous state.
        assert_eq!(before.instances().transforms(), before_transforms);
        assert!(before.instances().contains(ids[0]));
    });

    assert_eq!(before.instances().transforms(), before_transforms);
    assert_eq!(before.epoch(), 1);
    let after = scene.snapshot();
    assert_eq!(after.epoch(), 2);
    assert!(!after.instances().contains(ids[0]));
    assert_eq!(after.instances().transforms()[1], at(101.0));
}

#[test]
fn the_writer_sees_the_previous_state_and_swapping_alternates_buffers() {
    let mut scene = Scene::new();
    let ids = spawn_many(&mut scene, 1);

    for step in 1..=6 {
        scene.extract(|writer| {
            let slot = writer.world().instances().slot_of(ids[0]).expect("live");
            let current = writer.world().instances().transforms()[slot];
            assert_eq!(current, at((step - 1) as f32 * 10.0));
            writer.set_transform(ids[0], at(step as f32 * 10.0)).expect("live");
        });
        assert_eq!(scene.world().instances().transforms()[0], at(step as f32 * 10.0));
    }
}

#[test]
fn a_reader_holding_the_back_buffer_forces_a_rebuild_but_never_corrupts_data() {
    let mut scene = Scene::new();
    let ids = spawn_many(&mut scene, 3);
    let held = scene.snapshot();
    scene.extract(|writer| writer.set_transform(ids[1], at(50.0)).expect("live"));
    assert!(!scene.last_extract().rebuilt_back_buffer);

    // `held` is the state the next write buffer is built on.
    scene.extract(|writer| writer.set_transform(ids[2], at(60.0)).expect("live"));

    assert!(scene.last_extract().rebuilt_back_buffer);
    let world = scene.snapshot();
    assert_eq!(world.instances().transforms()[1], at(50.0));
    assert_eq!(world.instances().transforms()[2], at(60.0));
    assert_eq!(held.instances().transforms()[1], at(1.0));
    drop((held, world));

    scene.extract(|writer| writer.set_transform(ids[0], at(70.0)).expect("live"));
    scene.extract(|writer| writer.set_transform(ids[0], at(71.0)).expect("live"));
    assert!(!scene.last_extract().rebuilt_back_buffer);
    assert_eq!(scene.world().instances().transforms()[2], at(60.0));
}

#[test]
fn a_stale_handle_is_rejected_in_the_writer_and_in_the_world() {
    let mut scene = Scene::new();
    let old = spawn_many(&mut scene, 1)[0];
    scene.extract(|writer| writer.despawn(old).expect("live"));
    let new = scene.extract(|writer| writer.spawn(&desc(5.0)).expect("finite"));

    assert_eq!(old.raw().index(), new.raw().index());
    assert_ne!(old, new);
    assert!(!scene.world().instances().contains(old));
    assert!(scene.world().instances().contains(new));
    scene.extract(|writer| {
        assert_eq!(
            writer.set_transform(old, at(1.0)),
            Err(SceneError::StaleHandle { kind: "instance" })
        );
        assert_eq!(writer.despawn(old), Err(SceneError::StaleHandle { kind: "instance" }));
        assert_eq!(writer.set_transform(new, at(2.0)), Ok(()));
    });
    assert_eq!(scene.world().instances().transforms()[0], at(2.0));
}

#[test]
fn a_handle_despawned_and_respawned_inside_one_extract_is_new() {
    let mut scene = Scene::new();
    let old = spawn_many(&mut scene, 1)[0];

    let new = scene.extract(|writer| {
        writer.despawn(old).expect("live");
        writer.spawn(&desc(9.0)).expect("finite")
    });

    assert_ne!(old, new);
    assert!(scene.world().instances().contains(new));
    assert!(!scene.world().instances().contains(old));
    assert_eq!(scene.world().instances().dirty(0), Dirty::SPAWNED | Dirty::DESPAWNED);
}

#[test]
fn only_changed_instances_are_marked_and_equal_writes_are_skipped() {
    let mut scene = Scene::new();
    let ids = spawn_many(&mut scene, 1000);
    scene.extract(|_| {});

    scene.extract(|writer| {
        for (index, id) in ids.iter().enumerate() {
            let x = if index % 100 == 7 {
                index as f32 + 0.5
            } else {
                index as f32
            };
            writer.set_transform(*id, at(x)).expect("live");
        }
    });

    let stats = *scene.last_extract();
    assert_eq!((stats.written, stats.unchanged), (10, 990));
    assert_eq!(scene.world().instances().changes().len(), 10);
    for &slot in scene.world().instances().changes() {
        assert_eq!(scene.world().instances().dirty(slot as usize), Dirty::TRANSFORM);
        assert_eq!(slot % 100, 7);
    }
}

#[test]
fn an_unchanged_extract_rewrites_nothing() {
    let mut scene = Scene::new();
    let ids = spawn_many(&mut scene, 10_000);
    scene.extract(|_| {});

    scene.extract(|writer| {
        for (index, id) in ids.iter().enumerate() {
            writer.set_transform(*id, at(index as f32)).expect("live");
            writer.set_material(*id, MaterialHandle::DEFAULT).expect("live");
        }
    });

    let stats = *scene.last_extract();
    assert_eq!((stats.written, stats.unchanged, stats.settled), (0, 20_000, 0));
    assert_eq!(stats.changed_instances, 0);
}

#[test]
fn previous_model_is_last_extracts_model_and_zero_once_motion_stops() {
    let mut scene = Scene::new();
    let id = spawn_many(&mut scene, 1)[0];
    let model_and_previous = |scene: &Scene| {
        let instances = scene.world().instances();
        (instances.transforms()[0], instances.previous_transforms()[0])
    };
    assert_eq!(model_and_previous(&scene), (at(0.0), at(0.0)));

    scene.extract(|writer| writer.set_transform(id, at(1.0)).expect("live"));
    assert_eq!(model_and_previous(&scene), (at(1.0), at(0.0)));

    scene.extract(|writer| writer.set_transform(id, at(3.0)).expect("live"));
    assert_eq!(model_and_previous(&scene), (at(3.0), at(1.0)));

    // Two writes in one extract: previous is the value before the extract.
    scene.extract(|writer| {
        writer.set_transform(id, at(4.0)).expect("live");
        writer.set_transform(id, at(5.0)).expect("live");
    });
    assert_eq!(model_and_previous(&scene), (at(5.0), at(3.0)));

    scene.extract(|_| {});
    assert_eq!(model_and_previous(&scene), (at(5.0), at(5.0)));
    assert_eq!(scene.last_extract().settled, 1);
    assert_eq!(scene.world().instances().dirty(0), Dirty::TRANSFORM);

    scene.extract(|_| {});
    assert_eq!(scene.last_extract().settled, 0);
    assert_eq!(scene.world().instances().changes().len(), 0);
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % bound
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Expected {
    current: Affine3A,
    previous: Affine3A,
    mesh: MeshHandle,
}

// A random history of extracts compared with a plain map that states the contract directly: after an extract the world
// holds every live instance with its latest value and the value it had before that extract, and `changes` lists every
// slot whose data differs from the previous snapshot. Both buffers take turns, so a missed catch-up shows up.
#[test]
fn random_extracts_match_a_plain_model_and_report_every_difference() {
    let mut scene = Scene::new();
    let mut random = Lcg(42);
    let mut model: HashMap<InstanceId, Expected> = HashMap::new();
    let mut previous_state: HashMap<u32, (u32, bool, Affine3A, Affine3A, MeshHandle)> = HashMap::new();

    for extract in 0..300 {
        for expected in model.values_mut() {
            expected.previous = expected.current;
        }
        scene.extract(|writer| {
            for _ in 0..random.next(40) {
                let ids: Vec<InstanceId> = model.keys().copied().collect();
                match random.next(5) {
                    0 => {
                        let x = random.next(1000) as f32;
                        let id = writer.spawn(&desc(x)).expect("finite");
                        model.insert(
                            id,
                            Expected {
                                current: at(x),
                                previous: at(x),
                                mesh: mesh(0),
                            },
                        );
                    }
                    1 if !ids.is_empty() => {
                        let id = ids[random.next(ids.len())];
                        writer.despawn(id).expect("live");
                        model.remove(&id);
                    }
                    2 | 3 if !ids.is_empty() => {
                        let id = ids[random.next(ids.len())];
                        let x = random.next(1000) as f32;
                        writer.set_transform(id, at(x)).expect("live");
                        model.get_mut(&id).expect("modelled").current = at(x);
                    }
                    4 if !ids.is_empty() => {
                        let id = ids[random.next(ids.len())];
                        let new_mesh = mesh(u32::try_from(random.next(4)).expect("small"));
                        writer.set_mesh(id, new_mesh).expect("live");
                        model.get_mut(&id).expect("modelled").mesh = new_mesh;
                    }
                    _ => {}
                }
            }
        });

        let world = scene.snapshot();
        let instances = world.instances();
        assert_eq!(instances.len(), model.len(), "extract {extract}");
        for (id, expected) in &model {
            let slot = instances.slot_of(*id).expect("modelled instance is live");
            let actual = Expected {
                current: instances.transforms()[slot],
                previous: instances.previous_transforms()[slot],
                mesh: instances.meshes()[slot],
            };
            assert_eq!(actual, *expected, "extract {extract}, slot {slot}");
        }
        assert_eq!(instances.iter_alive().count(), model.len());

        let changed: std::collections::HashSet<u32> = instances.changes().iter().copied().collect();
        assert_eq!(changed.len(), instances.changes().len(), "a slot is listed once");
        let mut current_state = HashMap::new();
        for slot in 0..instances.slot_count() {
            let state = (
                instances.id_at(slot).map_or(u32::MAX, |id| id.raw().generation()),
                instances.is_alive(slot),
                instances.transforms()[slot],
                instances.previous_transforms()[slot],
                instances.meshes()[slot],
            );
            let slot = u32::try_from(slot).expect("small");
            if instances.is_alive(slot as usize) && previous_state.get(&slot) != Some(&state) {
                assert!(
                    changed.contains(&slot),
                    "extract {extract}: slot {slot} differs but is not in changes"
                );
            }
            current_state.insert(slot, state);
        }
        previous_state = current_state;
    }
}

#[test]
fn a_panicking_extract_publishes_nothing_and_the_next_one_recovers() {
    let mut scene = Scene::new();
    let ids = spawn_many(&mut scene, 3);
    scene.extract(|writer| writer.set_transform(ids[0], at(10.0)).expect("live"));

    let result = catch_unwind(AssertUnwindSafe(|| {
        scene.extract(|writer| {
            writer.set_transform(ids[1], at(99.0)).expect("live");
            writer.despawn(ids[2]).expect("live");
            panic!("host failed halfway");
        });
    }));

    assert!(result.is_err());
    assert_eq!(scene.world().epoch(), 2);
    assert_eq!(scene.world().instances().transforms()[1], at(1.0));
    assert!(scene.world().instances().contains(ids[2]));

    let respawned = scene.extract(|writer| writer.spawn(&desc(7.0)).expect("finite"));
    assert!(scene.last_extract().rebuilt_back_buffer);
    scene.extract(|writer| writer.set_transform(ids[1], at(2.0)).expect("live"));
    let instances = scene.world().instances();
    assert_eq!(instances.len(), 4);
    assert!(instances.contains(respawned) && instances.contains(ids[2]));
    assert_eq!(instances.transforms()[0], at(10.0));
    assert_eq!(instances.transforms()[1], at(2.0));
    assert_eq!(
        respawned.raw().index(),
        3,
        "the aborted despawn must not have freed a slot"
    );
}

#[test]
fn cameras_keep_the_previous_pose_and_apply_the_view_jitter() {
    let mut scene = Scene::new();
    let projection = Projection::perspective(std::f32::consts::FRAC_PI_3, 0.1);
    let camera = scene.extract(|writer| {
        writer
            .spawn_camera(&CameraDesc::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, projection))
            .expect("valid")
    });
    scene.extract(|writer| writer.set_camera_transform(camera, at(2.0)).expect("live"));

    let data = *scene.world().camera(camera).expect("live");
    assert_eq!(data.transform, at(2.0));
    assert_ne!(data.previous_transform, data.transform);
    let mut view = ViewCtx::new(ViewId(1), PhysicalSize::new(1920, 1080), Camera::default());
    view.jitter = Vec2::new(0.25, 0.25);
    let jittered = data.view_camera(&view).view_projection();
    let plain = data.view_projection(1920.0 / 1080.0);
    assert!(!jittered.abs_diff_eq(plain, 1e-7));

    scene.extract(|_| {});
    let settled = scene.world().camera(camera).expect("live");
    assert_eq!(settled.previous_transform, settled.transform);
    scene.extract(|writer| {
        assert!(matches!(
            writer.spawn_camera(&CameraDesc::new(Affine3A::IDENTITY, Projection::perspective(1.0, 0.0))),
            Err(SceneError::InvalidCamera { .. })
        ));
        writer.despawn_camera(camera).expect("live");
        assert_eq!(
            writer.set_camera_transform(camera, at(1.0)),
            Err(SceneError::StaleHandle { kind: "camera" })
        );
    });
    assert!(scene.world().camera(camera).is_none());
}

#[test]
fn lights_are_stored_validated_and_replaced_by_handle() {
    let mut scene = Scene::new();
    let sun = scene.extract(|writer| {
        assert!(matches!(
            writer.spawn_light(&Light::point(Affine3A::IDENTITY, -5.0)),
            Err(SceneError::InvalidLight { .. })
        ));
        writer
            .spawn_light(&Light::directional(Affine3A::IDENTITY, 100_000.0).with_shadows())
            .expect("valid")
    });
    scene.extract(|writer| {
        writer
            .set_light(sun, &Light::directional(Affine3A::IDENTITY, 50_000.0))
            .expect("live");
    });

    assert_eq!(
        scene.world().light(sun).and_then(Light::illuminance_lux),
        Some(50_000.0)
    );
    assert_eq!(scene.world().lights().changes().len(), 1);

    scene.extract(|writer| writer.despawn_light(sun).expect("live"));
    assert!(scene.world().light(sun).is_none());
    assert_eq!(scene.world().lights().len(), 0);
}

#[test]
fn scales_to_a_hundred_thousand_instances_and_only_touches_what_changed() {
    const COUNT: usize = 100_000;
    let mut scene = Scene::new();
    let ids = spawn_many(&mut scene, COUNT);
    assert_eq!(scene.last_extract().changed_instances, COUNT);
    scene.extract(|_| {});

    scene.extract(|writer| {
        for id in ids.iter().step_by(1000) {
            writer.set_transform(*id, at(-1.0)).expect("live");
        }
        for id in &ids[..COUNT / 2] {
            writer.set_layers(*id, LayerMask::layer(1)).expect("live");
        }
    });

    let world = scene.snapshot();
    assert_eq!(world.instances().len(), COUNT);
    // Half got new layers; of the 100 moved ones, 50 are in the other half.
    assert_eq!(world.instances().changes().len(), COUNT / 2 + COUNT / 2000);
    assert_eq!(world.instances().iter_visible(LayerMask::layer(1)).count(), COUNT / 2);
    assert_eq!(world.instances().iter_visible(LayerMask::DEFAULT).count(), COUNT / 2);

    scene.extract(|writer| {
        for id in ids.iter().skip(1).step_by(2000) {
            writer.despawn(*id).expect("live");
        }
    });
    assert_eq!(scene.world().instances().len(), COUNT - 50);
    // The settle of the previous extract's moves and the 50 despawns.
    assert_eq!(scene.last_extract().changed_instances, COUNT / 1000 + 50);
}
