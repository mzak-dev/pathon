#![doc = include_str!("../../../docs/crates/pathon-debug.md")]

mod color;
mod draw;
mod gpu;
mod lines;
mod pack;
mod preview;
pub mod reference;
mod settings;
mod shapes;
mod vertices;
mod wireframe;

pub use color::DebugColor;
pub use draw::{DebugDraw, DebugLine, LineStyle};
pub use lines::LinesFeature;
pub use pack::DebugPack;
pub use preview::SlotPreviewFeature;
pub use settings::{Corner, DebugSettings, PreviewLayout, PreviewSlot, SlotPreview, WireframeMode};
pub use wireframe::WireframeFeature;
