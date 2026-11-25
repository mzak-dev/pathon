#![doc = include_str!("../../../docs/crates/pathon-sky.md")]

mod atmosphere;
mod environment;
mod feature;
mod frame;
mod gpu;
mod pack;
mod quality;
pub mod reference;
mod settings;
mod uniforms;

pub use atmosphere::{Atmosphere, OZONE_CENTER_KM, OZONE_HALF_WIDTH_KM};
pub use environment::SkyEnvironmentFeature;
pub use feature::{MULTISCATTER_LUT, SKY_VIEW_LUT, SkyFeature, TRANSMITTANCE_LUT};
pub use frame::SunLight;
pub use pack::SkyPack;
pub use quality::SkyQuality;
pub use settings::{CameraHeight, SkyIblSettings, SkySettings};
