use std::sync::Arc;

use pathon_api::glam::Affine3A;

use crate::bounds::Aabb;
use crate::camera::{CameraData, CameraDesc, Projection};
use crate::error::SceneError;
use crate::handle::{CameraId, InstanceId, LightId, MaterialHandle, MeshHandle};
use crate::instance::{InstanceDesc, InstanceFlags};
use crate::layer::LayerMask;
use crate::light::Light;
use crate::world::{CatchUp, RenderWorld};

// A published world. Cheap to clone and send to the render side; it never changes.
pub type Snapshot = Arc<RenderWorld>;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct ExtractStats {
    pub epoch: u64,
    pub spawned: usize,
    pub despawned: usize,
    // Writes that changed a value.
    pub written: usize,
    // Writes of a value equal to the stored one: skipped, nothing marked.
    pub unchanged: usize,
    // Objects whose previous transform was brought up to date because they moved in the previous extract.
    pub settled: usize,
    // Instance slots the world marks as changed, the upload list of a consumer.
    pub changed_instances: usize,
    // The write buffer was still held by a reader (or a previous extract panicked), so it was rebuilt by a full copy
    // instead of an incremental catch-up. Allocates; a reader that drops snapshots in time never causes it.
    pub rebuilt_back_buffer: bool,
}

#[derive(Debug, Default)]
struct FreeLists {
    instances: Vec<u32>,
    cameras: Vec<u32>,
    lights: Vec<u32>,
}

// The render world with a write side and a read side. `extract` writes the next state into the back buffer while
// snapshots of the front buffer stay untouched, then swaps. Each change is applied to the back buffer once and to
// the other one on the next extract (catch-up), so unchanged data is never rewritten and the steady state allocates
// nothing.
#[derive(Debug)]
pub struct Scene {
    buffers: [Snapshot; 2],
    front: usize,
    // The back buffer lacks the changes of the extract that produced the front.
    back_is_behind: bool,
    extracting: bool,
    free: FreeLists,
    epoch: u64,
    last_extract: ExtractStats,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    pub fn new() -> Self {
        Self {
            buffers: [Snapshot::default(), Snapshot::default()],
            front: 0,
            back_is_behind: false,
            extracting: false,
            free: FreeLists::default(),
            epoch: 0,
            last_extract: ExtractStats::default(),
        }
    }

    // The latest published world, for the render side.
    pub fn snapshot(&self) -> Snapshot {
        Arc::clone(&self.buffers[self.front])
    }

    pub fn world(&self) -> &RenderWorld {
        &self.buffers[self.front]
    }

    pub fn last_extract(&self) -> &ExtractStats {
        &self.last_extract
    }

    // Writes the next state of the scene through `fill` and publishes it. The writer sees the state of the previous
    // extract and changes it; whatever `fill` does not touch stays as it was. If `fill` panics nothing is published
    // and the next extract starts from the last published state.
    pub fn extract<R>(&mut self, fill: impl FnOnce(&mut SceneWriter<'_>) -> R) -> R {
        let (front_index, back_index) = (self.front, 1 - self.front);
        let mut stats = ExtractStats::default();
        if self.extracting {
            self.back_is_behind = false;
            stats.rebuilt_back_buffer = true;
            self.buffers[back_index] = Arc::new(World::clone(&self.buffers[front_index]));
            self.free = free_lists_of(&self.buffers[front_index]);
        }
        if Arc::get_mut(&mut self.buffers[back_index]).is_none() {
            // A reader still holds the previous snapshot. The free lists describe the front state, so they stay valid.
            self.back_is_behind = false;
            stats.rebuilt_back_buffer = true;
            self.buffers[back_index] = Arc::new(World::clone(&self.buffers[front_index]));
        }

        let [first, second] = &mut self.buffers;
        let (front, back) = if front_index == 0 {
            (&*first, second)
        } else {
            (&*second, first)
        };
        let back = Arc::get_mut(back).expect("the back buffer has one owner here");
        let catch_up = if self.back_is_behind {
            CatchUp::Needed
        } else {
            CatchUp::NotNeeded
        };
        stats.settled = back.begin_extract(front, catch_up);

        self.extracting = true;
        let result = fill(&mut SceneWriter {
            world: back,
            free: &mut self.free,
            stats: &mut stats,
        });
        self.extracting = false;

        self.epoch += 1;
        back.epoch = self.epoch;
        stats.epoch = self.epoch;
        stats.changed_instances = back.instances.changes().len();
        self.last_extract = stats;
        self.front = back_index;
        self.back_is_behind = true;
        result
    }
}

type World = RenderWorld;

fn free_lists_of(world: &World) -> FreeLists {
    FreeLists {
        instances: world.instances.free_slots(),
        cameras: world.cameras.free_slots(),
        lights: world.lights.free_slots(),
    }
}

// The write side of an extract. Every method that takes a handle fails with `N0001` for a stale one; a value equal to
// the stored one is skipped.
#[derive(Debug)]
pub struct SceneWriter<'scene> {
    world: &'scene mut World,
    free: &'scene mut FreeLists,
    stats: &'scene mut ExtractStats,
}

impl SceneWriter<'_> {
    fn count(&mut self, changed: Result<bool, SceneError>) -> Result<(), SceneError> {
        if changed? {
            self.stats.written += 1;
        } else {
            self.stats.unchanged += 1;
        }
        Ok(())
    }

    // The state the writer starts from and edits; read it to decide what to write.
    pub fn world(&self) -> &RenderWorld {
        self.world
    }

    pub fn spawn(&mut self, desc: &InstanceDesc) -> Result<InstanceId, SceneError> {
        let id = self.world.instances.spawn(&mut self.free.instances, desc)?;
        self.stats.spawned += 1;
        Ok(id)
    }

    pub fn despawn(&mut self, id: InstanceId) -> Result<(), SceneError> {
        self.world.instances.despawn(&mut self.free.instances, id)?;
        self.stats.despawned += 1;
        Ok(())
    }

    pub fn set_transform(&mut self, id: InstanceId, transform: Affine3A) -> Result<(), SceneError> {
        let changed = self.world.instances.set_transform(id, transform);
        self.count(changed)
    }

    pub fn set_mesh(&mut self, id: InstanceId, mesh: MeshHandle) -> Result<(), SceneError> {
        let changed = self.world.instances.set_mesh(id, mesh);
        self.count(changed)
    }

    pub fn set_material(&mut self, id: InstanceId, material: MaterialHandle) -> Result<(), SceneError> {
        let changed = self.world.instances.set_material(id, material);
        self.count(changed)
    }

    pub fn set_layers(&mut self, id: InstanceId, layers: LayerMask) -> Result<(), SceneError> {
        let changed = self.world.instances.set_layers(id, layers);
        self.count(changed)
    }

    pub fn set_flags(&mut self, id: InstanceId, flags: InstanceFlags) -> Result<(), SceneError> {
        let changed = self.world.instances.set_flags(id, flags);
        self.count(changed)
    }

    pub fn set_local_bounds(&mut self, id: InstanceId, bounds: Aabb) -> Result<(), SceneError> {
        let changed = self.world.instances.set_local_bounds(id, bounds);
        self.count(changed)
    }

    pub fn spawn_camera(&mut self, desc: &CameraDesc) -> Result<CameraId, SceneError> {
        desc.validate()?;
        let id = self.world.cameras.insert(&mut self.free.cameras, CameraData::new(desc));
        self.stats.spawned += 1;
        Ok(id)
    }

    pub fn despawn_camera(&mut self, id: CameraId) -> Result<(), SceneError> {
        self.world.cameras.remove(&mut self.free.cameras, id)?;
        self.stats.despawned += 1;
        Ok(())
    }

    // Replaces the camera's description; its previous pose is kept for motion vectors.
    pub fn set_camera(&mut self, id: CameraId, desc: &CameraDesc) -> Result<(), SceneError> {
        desc.validate()?;
        let changed = self.world.cameras.update(id, |camera| camera.apply(desc));
        self.count(changed)
    }

    pub fn set_camera_transform(&mut self, id: CameraId, transform: Affine3A) -> Result<(), SceneError> {
        let Some(camera) = self.world.cameras.get(id) else {
            return Err(SceneError::StaleHandle { kind: "camera" });
        };
        let desc = CameraDesc::new(transform, camera.projection).with_layers(camera.layers);
        self.set_camera(id, &desc)
    }

    pub fn set_camera_projection(&mut self, id: CameraId, projection: Projection) -> Result<(), SceneError> {
        let Some(camera) = self.world.cameras.get(id) else {
            return Err(SceneError::StaleHandle { kind: "camera" });
        };
        let desc = CameraDesc::new(camera.transform, projection).with_layers(camera.layers);
        self.set_camera(id, &desc)
    }

    pub fn spawn_light(&mut self, light: &Light) -> Result<LightId, SceneError> {
        light.validate()?;
        let id = self.world.lights.insert(&mut self.free.lights, *light);
        self.stats.spawned += 1;
        Ok(id)
    }

    pub fn despawn_light(&mut self, id: LightId) -> Result<(), SceneError> {
        self.world.lights.remove(&mut self.free.lights, id)?;
        self.stats.despawned += 1;
        Ok(())
    }

    pub fn set_light(&mut self, id: LightId, light: &Light) -> Result<(), SceneError> {
        light.validate()?;
        let changed = self.world.lights.update(id, |stored| {
            let changed = stored != light;
            *stored = *light;
            changed
        });
        self.count(changed)
    }
}
