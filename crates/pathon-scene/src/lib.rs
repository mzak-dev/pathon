#![doc = include_str!("../../../docs/crates/pathon-scene.md")]

mod bounds;
mod camera;
mod error;
mod handle;
mod instance;
mod layer;
mod light;
mod scene;
mod table;
mod world;

pub use bounds::Aabb;
pub use camera::{CameraData, CameraDesc, Projection};
pub use error::SceneError;
pub use handle::{CameraId, InstanceId, LightId, MaterialHandle, MeshHandle};
pub use instance::{Dirty, InstanceDesc, InstanceFlags, Instances};
pub use layer::LayerMask;
pub use light::{AUTO_RANGE_CUTOFF_LUX, Light, LightKind};
pub use scene::{ExtractStats, Scene, SceneWriter, Snapshot};
pub use table::Table;
pub use world::RenderWorld;
