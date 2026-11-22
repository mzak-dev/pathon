// M4-02: a host keeps its own objects in a plain `Vec` and extracts them into the render world every frame. The scene
// stores only handles, so meshes live in the geometry pool of pathon-mesh and the host maps its own ids onto both.
// Runs without a GPU or a window; the result is in the log.

use pathon::api::glam::{Affine3A, Vec3};
use pathon::rhi::Device;
use pathon_mesh::{GeometryPool, IndexData, MeshData, MeshId, ObjectTransforms, PoolDesc, VertexLayout};
use pathon_scene::{
    CameraDesc, InstanceDesc, InstanceId, Light, MaterialHandle, MeshHandle, Projection, Scene, SceneError,
};

struct Body {
    mesh: MeshId,
    position: Vec3,
    velocity: Vec3,
    // The host's link to the scene: set by the first extract that sees the body.
    instance: Option<InstanceId>,
    removed: bool,
}

fn upload_triangle(pool: &mut GeometryPool, device: &Device, label: &'static str) -> MeshId {
    let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let data = MeshData::new(
        label,
        vec![bytemuck::cast_slice(&positions)],
        IndexData::U16(&[0, 1, 2]),
    );
    pool.upload(device, &data)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
}

// The adapter: one pass over the host's data, writing only what exists or changed.
fn extract_bodies(scene: &mut Scene, bodies: &mut [Body]) -> Result<(), SceneError> {
    scene.extract(|writer| {
        for body in bodies {
            let transform = Affine3A::from_translation(body.position);
            match (body.instance, body.removed) {
                (None, false) => {
                    let desc = InstanceDesc::new(MeshHandle::from_id(body.mesh), MaterialHandle::DEFAULT, transform);
                    body.instance = Some(writer.spawn(&desc)?);
                }
                (Some(instance), false) => writer.set_transform(instance, transform)?,
                (Some(instance), true) => {
                    writer.despawn(instance)?;
                    body.instance = None;
                }
                (None, true) => {}
            }
        }
        Ok(())
    })
}

fn main() {
    pathon_examples::init_logging();
    let device = Device::null();
    let mut pool = GeometryPool::new(&device, "scene", VertexLayout::position_only(), PoolDesc::default());
    let crate_mesh = upload_triangle(&mut pool, &device, "crate");
    let ball_mesh = upload_triangle(&mut pool, &device, "ball");

    let mut bodies: Vec<Body> = (0..8)
        .map(|index| Body {
            mesh: if index % 2 == 0 { crate_mesh } else { ball_mesh },
            position: Vec3::new(index as f32, 0.0, 0.0),
            velocity: if index < 2 {
                Vec3::new(0.0, 1.0, 0.0)
            } else {
                Vec3::ZERO
            },
            instance: None,
            removed: false,
        })
        .collect();

    let mut scene = Scene::new();
    let (camera, sun) = scene.extract(|writer| {
        let camera = writer
            .spawn_camera(&CameraDesc::look_at(
                Vec3::new(4.0, 3.0, 12.0),
                Vec3::new(4.0, 0.0, 0.0),
                Projection::perspective(std::f32::consts::FRAC_PI_3, 0.1),
            ))
            .expect("valid camera");
        let sun = writer
            .spawn_light(&Light::directional(Affine3A::IDENTITY, 100_000.0).with_shadows())
            .expect("valid light");
        writer
            .spawn_light(&Light::point(
                Affine3A::from_translation(Vec3::new(4.0, 2.0, 1.0)),
                800.0,
            ))
            .expect("valid light");
        (camera, sun)
    });

    let mut doomed = None;
    for frame in 0..4 {
        if frame == 3 {
            bodies[7].removed = true;
            doomed = bodies[7].instance;
        }
        for body in &mut bodies {
            body.position += body.velocity * 0.1;
        }
        extract_bodies(&mut scene, &mut bodies).expect("the host keeps its handles current");
        let stats = scene.last_extract();
        log::info!(
            "frame {frame}: epoch {} spawned {} despawned {} written {} unchanged {} settled {} -> upload list of {} \
             instances",
            stats.epoch,
            stats.spawned,
            stats.despawned,
            stats.written,
            stats.unchanged,
            stats.settled,
            stats.changed_instances,
        );
    }

    // The render side reads a snapshot: what a pack uploads is the transform pair of ADR-0027.
    let world = scene.snapshot();
    let instances = world.instances();
    let first = bodies[0]
        .instance
        .and_then(|id| instances.slot_of(id))
        .expect("body 0 is live");
    let pair = ObjectTransforms {
        model: instances.transforms()[first].into(),
        previous_model: instances.previous_transforms()[first].into(),
    };
    log::info!(
        "body 0 model row 1 {:?}, previous {:?}",
        pair.model.0[1],
        pair.previous_model.0[1]
    );

    let mesh_back: MeshId = instances.meshes()[first].to_id();
    let location = pool.mesh(mesh_back).map(|info| info.location);
    log::info!("body 0 draws {mesh_back:?} at {location:?}");

    let camera_data = world.camera(camera).expect("camera");
    let clip = camera_data.view_projection(16.0 / 9.0) * Vec3::new(4.0, 0.0, 0.0).extend(1.0);
    log::info!(
        "camera sees the scene centre at ndc depth {:.4} (reverse-Z: 1 is near)",
        clip.z / clip.w
    );
    log::info!("sun: {:?} lux", world.light(sun).and_then(Light::illuminance_lux));

    // A handle that outlived its object is rejected, not reused.
    let stale = scene.extract(|writer| writer.set_transform(doomed.expect("spawned in frame 0"), Affine3A::IDENTITY));
    if let Err(error) = stale {
        log::info!("{}", error.to_diagnostic());
    }
}
