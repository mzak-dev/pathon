#![cfg(feature = "gpu-tests")]
// The contract M5's TAA relies on, proven on a real GPU (`PATHON_TEST_BACKEND`, Vulkan by default): the current bank
// holds this frame's pose, the other bank last frame's, so `current - previous` is the object-space motion vector.

mod support;

use pathon_api::glam::{Mat4, Vec3};
use pathon_api::rhi::RhiError;
use pathon_core::RenderError;
use pathon_golden::{announce_adapter, gpu_or_skip};
use pathon_mesh::{GeometryPool, IndexData, MeshData, PoolDesc, VertexLayout};
use pathon_skin::reference::{self, DeformedVertex, Influences, RestVertex};
use pathon_skin::{MorphTarget, MotionHistory, SkinInstance, SkinSource};
use support::{Pose, Rig, assert_close, backend_from_environment};

fn rig(test: &str, instances: usize) -> Option<Rig> {
    let built = Rig::build(backend_from_environment(), instances, 1);
    let rig = gpu_or_skip(test, built, |error| {
        matches!(error, RenderError::Rhi(RhiError::NoAdapter(_)))
    })?;
    announce_adapter(test, rig.device().caps());
    Some(rig)
}

fn assert_vertices(rig: &Rig, instance: usize, frame: u64, expected: &[DeformedVertex], what: &str) {
    let positions = rig.positions_of(instance, frame, false);
    let surface = rig.surface_of(instance);
    assert_eq!(positions.len(), expected.len());
    for (index, expected) in expected.iter().enumerate() {
        let at = format!("{what}, vertex {index}");
        assert_close(positions[index], expected.position, &format!("{at} position"));
        let [nx, ny, nz, tx, ty, tz, handedness, u, v] = surface[index];
        assert_close([nx, ny, nz], expected.normal, &format!("{at} normal"));
        assert_close(
            [tx, ty, tz],
            [expected.tangent[0], expected.tangent[1], expected.tangent[2]],
            &format!("{at} tangent"),
        );
        assert_eq!(
            (handedness, [u, v]),
            (expected.tangent[3], expected.uv),
            "{at} handedness and uv"
        );
    }
}

// The vertex at the far end of the forearm: fully bone 1, `y = radius`, `z = 0`, so a bend about Z is closed-form.
fn wrist_after(angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    let (y, z) = (0.2, 0.0);
    [1.0 + cos - y * sin, sin + y * cos, z]
}

fn wrist_index(rig: &Rig) -> usize {
    (rig.arm.vertex_count() - support::ARM_SIDES) as usize
}

#[test]
fn a_bone_turning_between_frames_gives_the_cpu_pose_and_the_motion_vector() {
    let Some(mut rig) = rig("a_bone_turning_between_frames", 1) else {
        return;
    };
    let angles = [0.0_f32, 0.4, 0.9, 1.3, -0.5];
    let mut last: Option<(Pose, Vec<DeformedVertex>)> = None;
    let wrist = wrist_index(&rig);
    for (step, angle) in angles.into_iter().enumerate() {
        let history = if step == 0 {
            MotionHistory::Fresh
        } else {
            MotionHistory::Continuing
        };
        let pose = Pose::elbow(angle, history);
        rig.frame(std::slice::from_ref(&pose));
        let frame = rig.output().frame;
        let expected = rig.expected(&pose, &[]);
        assert_vertices(&rig, 0, frame, &expected, &format!("frame {frame}"));

        let current = rig.positions_of(0, frame, false);
        let previous = rig.positions_of(0, frame, true);
        match &last {
            None => assert_eq!(previous, current, "the first frame has no motion"),
            Some((_, before)) => {
                for (index, before) in before.iter().enumerate() {
                    assert_close(
                        previous[index],
                        before.position,
                        &format!("frame {frame} previous, vertex {index}"),
                    );
                }
                let motion = reference::motion(current[wrist], previous[wrist]);
                let analytic = reference::motion(wrist_after(angle), wrist_after(angles[step - 1]));
                assert_close(motion, analytic, &format!("frame {frame} wrist motion"));
                assert_close(
                    reference::motion(current[0], previous[0]),
                    [0.0; 3],
                    "the upper arm does not move",
                );
                assert!(Vec3::from(motion).length() > 0.05, "the wrist really moves");
            }
        }
        last = Some((pose, expected));
    }
}

#[test]
fn instances_that_join_late_have_no_motion_and_do_not_disturb_others() {
    let Some(mut rig) = rig("instances_that_join_late", 2) else {
        return;
    };
    let steady = |angle| Pose::elbow(angle, MotionHistory::Continuing);
    rig.frame(&[Pose::elbow(0.2, MotionHistory::Fresh)]);
    rig.frame(&[steady(0.5)]);
    // Instance 1 is skinned for the first time, so only its history is restarted.
    rig.frame(&[steady(0.8), Pose::elbow(-0.6, MotionHistory::Fresh)]);
    let frame = rig.output().frame;

    assert_eq!(rig.positions_of(1, frame, true), rig.positions_of(1, frame, false));
    let moved = rig.positions_of(0, frame, false);
    let before = rig.positions_of(0, frame, true);
    assert_ne!(moved, before);
    let [single_previous, single_current] =
        [Pose::elbow(0.5, MotionHistory::Fresh), steady(0.8)].map(|pose| rig.expected(&pose, &[]));
    let wrist = wrist_index(&rig);
    assert_close(
        before[wrist],
        single_previous[wrist].position,
        "instance 0 keeps its previous pose",
    );
    assert_close(moved[wrist], single_current[wrist].position, "instance 0 current pose");
    assert_vertices(
        &rig,
        1,
        frame,
        &rig.expected(&Pose::elbow(-0.6, MotionHistory::Fresh), &[]),
        "instance 1",
    );
}

#[test]
fn morph_weights_interpolate_like_the_cpu() {
    let Some(mut rig) = rig("morph_weights_interpolate", 1) else {
        return;
    };
    let vertices = rig.arm.vertex_count() as usize;
    let wave = |scale: f32| -> Vec<[f32; 3]> {
        (0..vertices)
            .map(|index| {
                let phase = index as f32 * 0.37;
                [phase.sin() * scale, phase.cos() * scale, 0.1 * scale]
            })
            .collect()
    };
    let targets = [
        MorphTarget {
            positions: wave(0.3),
            normals: wave(0.2),
            tangents: Vec::new(),
        },
        MorphTarget {
            positions: wave(-0.15),
            normals: Vec::new(),
            tangents: wave(0.25),
        },
    ];
    rig.add_morph_targets(&targets);

    for weights in [[0.0, 0.0], [0.5, 0.0], [0.25, 0.75], [1.0, 1.0], [-0.5, 0.2]] {
        let pose = Pose {
            weights: weights.to_vec(),
            ..Pose::elbow(0.6, MotionHistory::Fresh)
        };
        rig.frame(std::slice::from_ref(&pose));
        let frame = rig.output().frame;
        assert_vertices(
            &rig,
            0,
            frame,
            &rig.expected(&pose, &targets),
            &format!("weights {weights:?}"),
        );
    }

    // Linear in the weight: half of target 0 is the midpoint between none and all of it.
    let mut positions = Vec::new();
    for weight in [0.0, 0.5, 1.0] {
        let pose = Pose {
            weights: vec![weight, 0.0],
            ..Pose::elbow(0.0, MotionHistory::Fresh)
        };
        rig.frame(&[pose]);
        positions.push(rig.positions_of(0, rig.output().frame, false));
    }
    for index in 0..vertices {
        let midpoint = (Vec3::from(positions[0][index]) + Vec3::from(positions[2][index])) * 0.5;
        assert_close(
            positions[1][index],
            midpoint.to_array(),
            "half the weight, half the way",
        );
    }
}

#[test]
fn a_mesh_without_a_skinning_stream_morphs_alone() {
    let Some(mut rig) = rig("a_mesh_without_a_skinning_stream", 0) else {
        return;
    };
    let device = std::sync::Arc::clone(rig.renderer.device());
    let mut pool = GeometryPool::new(&*device, "morph only", VertexLayout::standard(), PoolDesc::default());
    let positions = [[0.0_f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let surface = [[0.0_f32, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.5, 0.5]; 3];
    let mesh = pool
        .upload(
            &*device,
            &MeshData::new(
                "triangle",
                vec![bytemuck::cast_slice(&positions), bytemuck::cast_slice(&surface)],
                IndexData::U32(&[0, 1, 2]),
            ),
        )
        .expect("mesh");
    let source = SkinSource::from_pool(&pool, mesh).expect("source");
    assert!(source.skin.is_none());
    let target = MorphTarget {
        positions: vec![[0.0, 0.0, 2.0]; 3],
        normals: vec![[0.0, 1.0, 0.0]; 3],
        tangents: Vec::new(),
    };
    rig.morph = Some(
        pathon_skin::MorphTargets::upload(&*device, "triangle morph", 3, std::slice::from_ref(&target)).expect("morph"),
    );
    let range = rig.positions.allocate(3).expect("range");
    let morph = rig.morph;
    rig.frame_with(|frame| {
        frame.morph_weights.push(0.5);
        frame
            .instances
            .push(SkinInstance::new(source, range).with_morph(morph.expect("morph"), 0));
    })
    .expect("frame");
    let frame = rig.output().frame;

    for (index, position) in rig.positions_in(range, frame, false).into_iter().enumerate() {
        assert_close(
            position,
            [positions[index][0], positions[index][1], 1.0],
            "half of a 2.0 offset",
        );
    }
    let normal = Vec3::new(0.0, 0.5, 1.0).normalize();
    for surface in rig.surface_in(range) {
        assert_close(
            [surface[0], surface[1], surface[2]],
            normal.to_array(),
            "morphed, renormalized normal",
        );
        assert_eq!(&surface[6..], &[1.0, 0.5, 0.5], "handedness and uv are untouched");
    }
}

#[test]
fn unnormalized_and_broken_weights_never_make_nan() {
    let Some(mut rig) = rig("unnormalized_and_broken_weights", 0) else {
        return;
    };
    let device = std::sync::Arc::clone(rig.renderer.device());
    let mut pool = GeometryPool::new(&*device, "broken weights", VertexLayout::skinned(), PoolDesc::default());
    let influences = [
        // The rounded sum is not one.
        Influences {
            joints: [0, 0, 0, 0],
            weights: [100, 100, 0, 0],
        },
        // No influence at all.
        Influences {
            joints: [0; 4],
            weights: [0; 4],
        },
        // A joint past the palette, which must not drag the vertex towards zero.
        Influences {
            joints: [9, 0, 0, 0],
            weights: [200, 55, 0, 0],
        },
        // Four full weights.
        Influences {
            joints: [1, 1, 1, 1],
            weights: [255; 4],
        },
        Influences {
            joints: [0, 1, 0, 0],
            weights: [128, 127, 0, 0],
        },
    ];
    let rest: Vec<RestVertex> = (0..influences.len())
        .map(|index| RestVertex {
            position: [index as f32, 0.5, -0.25],
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0, -1.0],
            uv: [0.0, 0.0],
        })
        .collect();
    let positions: Vec<[f32; 3]> = rest.iter().map(|vertex| vertex.position).collect();
    let surface: Vec<[f32; 9]> = rest
        .iter()
        .map(|vertex| {
            let [nx, ny, nz] = vertex.normal;
            let [tx, ty, tz, w] = vertex.tangent;
            [nx, ny, nz, tx, ty, tz, w, vertex.uv[0], vertex.uv[1]]
        })
        .collect();
    let skin: Vec<[u32; 3]> = influences
        .iter()
        .map(|influences| {
            let [low, high] = influences.packed_joints();
            [low, high, influences.packed_weights()]
        })
        .collect();
    let mesh = pool
        .upload(
            &*device,
            &MeshData::new(
                "broken",
                vec![
                    bytemuck::cast_slice(&positions),
                    bytemuck::cast_slice(&surface),
                    bytemuck::cast_slice(&skin),
                ],
                IndexData::U32(&[0, 1, 2]),
            ),
        )
        .expect("mesh");
    let source = SkinSource::from_pool(&pool, mesh).expect("source");
    let range = rig.positions.allocate(5).expect("range");
    let bones = [
        Mat4::from_translation(Vec3::new(0.0, 0.0, 3.0)),
        Mat4::from_scale(Vec3::splat(2.0)),
    ];
    rig.frame_with(|frame| {
        frame.bone_matrices.extend_from_slice(&bones);
        frame.instances.push(SkinInstance::new(source, range).with_bones(0, 2));
    })
    .expect("frame");
    let frame = rig.output().frame;

    let actual = rig.positions_in(range, frame, false);
    let surfaces = rig.surface_in(range);
    for (index, vertex) in rest.iter().enumerate() {
        let expected = reference::deform_vertex(vertex, Some((influences[index], &bones)), &[]);
        assert!(
            actual[index]
                .iter()
                .chain(&surfaces[index][..7])
                .all(|value| value.is_finite()),
            "vertex {index}"
        );
        assert_close(actual[index], expected.position, &format!("vertex {index} position"));
        assert_close(
            [surfaces[index][0], surfaces[index][1], surfaces[index][2]],
            expected.normal,
            &format!("vertex {index} normal"),
        );
    }
    assert_close(actual[1], rest[1].position, "no influence keeps the rest pose");
    assert_close(actual[0], [0.0, 0.5, 2.75], "100 + 100 still lands exactly on bone 0");
}

#[test]
fn the_result_does_not_depend_on_where_the_palette_sits_or_how_long_it_is() {
    let Some(mut rig) = rig("the_result_does_not_depend_on_the_palette", 1) else {
        return;
    };
    let angle = 0.7;
    let mut runs = Vec::new();
    for (padding_before, extra_bones) in [(0, 0), (0, 3), (7, 0), (5, 9), (0, 0)] {
        let mut pose = Pose::elbow(angle, MotionHistory::Fresh);
        pose.padding_before = padding_before;
        pose.bones
            .extend((0..extra_bones).map(|_| Mat4::from_scale(Vec3::splat(f32::MAX.sqrt()))));
        rig.frame(&[pose]);
        let frame = rig.output().frame;
        runs.push((rig.positions_of(0, frame, false), rig.surface_of(0)));
    }
    for (index, run) in runs.iter().enumerate().skip(1) {
        assert_eq!(run, &runs[0], "run {index} differs bit for bit from the first");
    }
    assert!(runs[0].0.iter().flatten().all(|value| value.is_finite()));
}
