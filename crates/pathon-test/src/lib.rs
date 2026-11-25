#![doc = include_str!("../../../docs/crates/pathon-test.md")]

mod adversarial;
mod synthetic_slots;
mod unlit_mesh;

pub use adversarial::{GeometryPack, OnDemandConsumer, UpscalerPack, VendorDummy};
pub use synthetic_slots::{SYNTHETIC_PYRAMID_LEVELS, SyntheticSlots};
pub use unlit_mesh::UnlitMeshPack;
