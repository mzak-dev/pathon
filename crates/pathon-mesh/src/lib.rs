#![doc = include_str!("../../../docs/crates/pathon-mesh.md")]

mod chunked_allocator;
mod data;
mod deformed;
mod error;
mod history;
mod layout;
mod pool;

pub use data::{Bounds, IndexData, MeshData};
pub use deformed::{DEFORMED_POSITION_STRIDE, DeformedPositions, DeformedRange};
pub use error::{ArenaKind, MeshError};
pub use history::{AffineRows, ObjectTransforms, TransformHistory};
pub use layout::{Semantic, StreamAttribute, VertexLayout, VertexStream};
pub use pool::{ArenaStats, GeometryPool, MeshId, MeshInfo, MeshLocation, PoolDesc, PoolStats};
