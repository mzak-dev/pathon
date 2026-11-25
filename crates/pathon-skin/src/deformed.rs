use pathon_api::rhi::{
    BufferDesc, BufferId, BufferUsage, DeviceApi, Resource, VertexAttribute, VertexBufferLayout, VertexFormat,
    VertexStepMode,
};
use pathon_mesh::{DeformedPositions, Semantic, VertexLayout};

use crate::error::SkinError;

/// One vertex of the surface stream: `Normal` 3 floats, `Tangent` 4, `Uv0` 2, the layout of stream 1 in
/// `VertexLayout::standard()`.
pub const DEFORMED_SURFACE_STRIDE: u64 = 36;

/// Normals and tangents after deformation, one bank, indexed by the same `DeformedRange` as `DeformedPositions`
/// (ADR-0027). UVs are copied so the buffer can replace stream 1 of the mesh in a rasterization pass.
#[derive(Debug)]
pub struct DeformedSurface {
    buffer: BufferId,
    capacity: u32,
}

impl DeformedSurface {
    pub fn new(device: &dyn DeviceApi, label: &'static str, capacity: u32) -> Result<Self, SkinError> {
        let limits = &device.caps().limits;
        let largest = limits.max_buffer_size.min(limits.max_storage_buffer_binding_size);
        let size = u64::from(capacity) * DEFORMED_SURFACE_STRIDE;
        if capacity == 0 || size > largest {
            return Err(SkinError::InvalidTargets {
                label,
                reason: format!("{capacity} vertices need {size} bytes, at most {largest} fit and none is empty"),
            });
        }
        let buffer = device.create_buffer(&BufferDesc {
            label,
            size,
            usage: BufferUsage::STORAGE | BufferUsage::VERTEX | BufferUsage::COPY_SRC | BufferUsage::COPY_DST,
        })?;
        Ok(Self { buffer, capacity })
    }

    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Byte offset of a range's first vertex, for `set_vertex_buffer`.
    pub fn offset(first_vertex: u32) -> u64 {
        u64::from(first_vertex) * DEFORMED_SURFACE_STRIDE
    }

    pub fn release(self, device: &dyn DeviceApi) {
        device.destroy(Resource::Buffer(self.buffer));
    }
}

/// What the deformed buffers of one frame are, as the pack publishes it and as the application can reach it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SkinTargets {
    pub positions: BufferId,
    pub surface: BufferId,
    pub bank_capacity: u32,
}

impl SkinTargets {
    pub fn new(positions: &DeformedPositions, surface: &DeformedSurface) -> Result<Self, SkinError> {
        if positions.bank_capacity() != surface.capacity() {
            return Err(SkinError::InvalidTargets {
                label: "skin targets",
                reason: format!(
                    "positions hold {} vertices per bank, the surface {}",
                    positions.bank_capacity(),
                    surface.capacity()
                ),
            });
        }
        Ok(Self {
            positions: positions.buffer(),
            surface: surface.buffer(),
            bank_capacity: surface.capacity(),
        })
    }
}

/// Vertex buffers of a deformed mesh: slot 0 is a bank of `DeformedPositions` (`Float32x4`, location 0), slot 1 is
/// `DeformedSurface` (normal 1, tangent 2, uv 3). The locations are the ones `VertexLayout::standard()` uses.
pub fn deformed_vertex_buffers() -> Vec<VertexBufferLayout> {
    let mut layouts = VertexLayout::standard().vertex_buffer_layouts();
    layouts[0] = VertexBufferLayout {
        stride: pathon_mesh::DEFORMED_POSITION_STRIDE,
        step_mode: VertexStepMode::Vertex,
        attributes: vec![VertexAttribute {
            format: VertexFormat::Float32x4,
            offset: 0,
            location: Semantic::Position.location(),
        }],
    };
    layouts
}

#[cfg(test)]
mod tests {
    use pathon_rhi::NullDevice;

    use super::*;

    #[test]
    fn surface_stride_is_the_standard_surface_stream() {
        assert_eq!(VertexLayout::standard().streams()[1].stride, DEFORMED_SURFACE_STRIDE);
        let layouts = deformed_vertex_buffers();
        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[0].stride, 16);
        assert_eq!(layouts[1].stride, 36);
    }

    #[test]
    fn capacity_must_match_and_fit() {
        let device = NullDevice::new();
        let positions = DeformedPositions::new(&device, "positions", 8, 2).expect("positions");
        let surface = DeformedSurface::new(&device, "surface", 8).expect("surface");
        let targets = SkinTargets::new(&positions, &surface).expect("same capacity");
        assert_eq!(targets.bank_capacity, 8);

        let other = DeformedSurface::new(&device, "other", 9).expect("surface");
        assert!(matches!(
            SkinTargets::new(&positions, &other),
            Err(SkinError::InvalidTargets { .. })
        ));
        assert!(DeformedSurface::new(&device, "empty", 0).is_err());
        assert!(DeformedSurface::new(&device, "huge", u32::MAX).is_err());
    }
}
