use pathon_api::rhi::{VertexAttribute, VertexBufferLayout, VertexFormat, VertexStepMode};

pub(crate) const LINE_FLAG_DEPTH_TEST: u32 = 1;

/// One line segment as the GPU reads it: a per-instance vertex of the quad that `lines.wgsl` expands.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct LineInstance {
    pub start: [f32; 3],
    pub thickness: f32,
    pub end: [f32; 3],
    pub flags: u32,
    pub color: [f32; 4],
}

const _: () = assert!(size_of::<LineInstance>() == 48);

impl LineInstance {
    pub(crate) fn layout() -> VertexBufferLayout {
        VertexBufferLayout {
            stride: size_of::<Self>() as u64,
            step_mode: VertexStepMode::Instance,
            attributes: vec![
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: 0,
                    location: 0,
                },
                VertexAttribute {
                    format: VertexFormat::Float32x3,
                    offset: 16,
                    location: 1,
                },
                VertexAttribute {
                    format: VertexFormat::Uint32,
                    offset: 28,
                    location: 2,
                },
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: 32,
                    location: 3,
                },
            ],
        }
    }
}

/// A corner of a wireframe triangle; every triangle owns three of them (no index buffer).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct WireVertex {
    pub position: [f32; 3],
    pub color: [u8; 4],
    pub thickness: f32,
}

const _: () = assert!(size_of::<WireVertex>() == 20);

impl WireVertex {
    pub(crate) fn layout() -> VertexBufferLayout {
        VertexBufferLayout {
            stride: size_of::<Self>() as u64,
            step_mode: VertexStepMode::Vertex,
            attributes: vec![
                VertexAttribute {
                    format: VertexFormat::Float32x3,
                    offset: 0,
                    location: 0,
                },
                VertexAttribute {
                    format: VertexFormat::Unorm8x4,
                    offset: 12,
                    location: 1,
                },
                VertexAttribute {
                    format: VertexFormat::Float32,
                    offset: 16,
                    location: 2,
                },
            ],
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=255 before the cast"
)]
pub(crate) fn pack_color(color: [f32; 4]) -> [u8; 4] {
    color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8)
}
