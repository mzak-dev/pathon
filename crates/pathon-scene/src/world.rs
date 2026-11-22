use crate::camera::CameraData;
use crate::handle::{CameraId, LightId};
use crate::instance::Instances;
use crate::light::Light;
use crate::table::Table;

// What the render side reads: one consistent state of the scene, produced by exactly one extract. Immutable once
// published; see `Scene::snapshot`.
#[derive(Clone, Debug, Default)]
pub struct RenderWorld {
    pub(crate) epoch: u64,
    pub(crate) instances: Instances,
    pub(crate) cameras: Table<CameraId, CameraData>,
    pub(crate) lights: Table<LightId, Light>,
}

impl RenderWorld {
    // Number of the extract that produced this world (the first one is 1; an empty scene is 0). A consumer that
    // uploads incrementally remembers the last epoch it saw; when it is not the previous one, it uploads everything.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn instances(&self) -> &Instances {
        &self.instances
    }

    pub fn cameras(&self) -> &Table<CameraId, CameraData> {
        &self.cameras
    }

    pub fn lights(&self) -> &Table<LightId, Light> {
        &self.lights
    }

    pub fn camera(&self, id: CameraId) -> Option<&CameraData> {
        self.cameras.get(id)
    }

    pub fn light(&self, id: LightId) -> Option<&Light> {
        self.lights.get(id)
    }

    // Brings this (older) copy to the state of `front` and starts a new extract: the changes of the copy's own previous
    // life are forgotten, the previous extract's moves are settled. Returns how many objects were settled.
    pub(crate) fn begin_extract(&mut self, front: &Self, catch_up: CatchUp) -> usize {
        self.instances.clear_changes();
        self.cameras.clear_changes();
        self.lights.clear_changes();
        if matches!(catch_up, CatchUp::Needed) {
            self.instances.catch_up(&front.instances);
            self.cameras.catch_up(&front.cameras);
            self.lights.catch_up(&front.lights);
        }
        self.instances.settle_moved_in(&front.instances)
            + self.cameras.settle_changed_in(&front.cameras, CameraData::settle)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CatchUp {
    Needed,
    NotNeeded,
}
