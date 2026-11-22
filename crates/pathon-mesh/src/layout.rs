use pathon_api::rhi::{VertexAttribute, VertexBufferLayout, VertexFormat, VertexStepMode};

use crate::error::MeshError;

// Locations are fixed per semantic, so shaders do not depend on how attributes are split into streams.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Semantic {
    Position,
    Normal,
    Tangent,
    Uv0,
    Uv1,
    Color,
    Joints,
    Weights,
}

impl Semantic {
    pub const fn location(self) -> u32 {
        match self {
            Self::Position => 0,
            Self::Normal => 1,
            Self::Tangent => 2,
            Self::Uv0 => 3,
            Self::Uv1 => 4,
            Self::Color => 5,
            Self::Joints => 6,
            Self::Weights => 7,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct StreamAttribute {
    pub semantic: Semantic,
    pub format: VertexFormat,
    pub offset: u64,
}

impl StreamAttribute {
    pub const fn new(semantic: Semantic, format: VertexFormat, offset: u64) -> Self {
        Self {
            semantic,
            format,
            offset,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct VertexStream {
    pub stride: u64,
    pub attributes: Vec<StreamAttribute>,
}

impl VertexStream {
    pub fn new(stride: u64, attributes: Vec<StreamAttribute>) -> Self {
        Self { stride, attributes }
    }
}

// Stream `i` is vertex buffer slot `i` in a pipeline built from `vertex_buffer_layouts`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct VertexLayout {
    streams: Vec<VertexStream>,
}

fn invalid(reason: impl Into<String>) -> MeshError {
    MeshError::InvalidLayout { reason: reason.into() }
}

impl VertexLayout {
    pub fn new(streams: Vec<VertexStream>) -> Result<Self, MeshError> {
        if streams.is_empty() {
            return Err(invalid("a layout needs at least one stream"));
        }
        let mut seen = Vec::new();
        for (index, stream) in streams.iter().enumerate() {
            Self::validate_stream(index, stream, &mut seen)?;
        }
        let position = streams
            .iter()
            .flat_map(|stream| &stream.attributes)
            .find(|attribute| attribute.semantic == Semantic::Position);
        match position {
            Some(attribute) if attribute.format == VertexFormat::Float32x3 => Ok(Self { streams }),
            Some(attribute) => Err(invalid(format!(
                "Position must be Float32x3, got {:?}",
                attribute.format
            ))),
            None => Err(invalid("Position is required")),
        }
    }

    fn validate_stream(index: usize, stream: &VertexStream, seen: &mut Vec<Semantic>) -> Result<(), MeshError> {
        if stream.stride == 0 || !stream.stride.is_multiple_of(4) {
            return Err(invalid(format!(
                "stream {index} stride {} is not a positive multiple of 4",
                stream.stride
            )));
        }
        if stream.attributes.is_empty() {
            return Err(invalid(format!("stream {index} has no attributes")));
        }
        for (position, attribute) in stream.attributes.iter().enumerate() {
            if seen.contains(&attribute.semantic) {
                return Err(invalid(format!("{:?} appears more than once", attribute.semantic)));
            }
            seen.push(attribute.semantic);
            let end = attribute.offset.saturating_add(attribute.format.size_bytes());
            if !attribute.offset.is_multiple_of(4) || end > stream.stride {
                return Err(invalid(format!(
                    "{:?} at offset {} does not fit a stride of {} in stream {index}",
                    attribute.semantic, attribute.offset, stream.stride
                )));
            }
            let overlaps = stream.attributes[..position].iter().any(|earlier| {
                attribute.offset < earlier.offset.saturating_add(earlier.format.size_bytes()) && earlier.offset < end
            });
            if overlaps {
                return Err(invalid(format!(
                    "{:?} overlaps another attribute in stream {index}",
                    attribute.semantic
                )));
            }
        }
        Ok(())
    }

    fn built_in(streams: Vec<VertexStream>) -> Self {
        Self::new(streams).expect("built-in layouts are valid (covered by a test)")
    }

    pub fn position_only() -> Self {
        Self::built_in(vec![Self::position_stream()])
    }

    pub fn standard() -> Self {
        Self::built_in(vec![Self::position_stream(), Self::surface_stream()])
    }

    pub fn skinned() -> Self {
        Self::built_in(vec![
            Self::position_stream(),
            Self::surface_stream(),
            Self::skin_stream(),
        ])
    }

    fn position_stream() -> VertexStream {
        VertexStream::new(
            12,
            vec![StreamAttribute::new(Semantic::Position, VertexFormat::Float32x3, 0)],
        )
    }

    fn surface_stream() -> VertexStream {
        VertexStream::new(
            36,
            vec![
                StreamAttribute::new(Semantic::Normal, VertexFormat::Float32x3, 0),
                StreamAttribute::new(Semantic::Tangent, VertexFormat::Float32x4, 12),
                StreamAttribute::new(Semantic::Uv0, VertexFormat::Float32x2, 28),
            ],
        )
    }

    // Four u16 joint indices packed little-endian into two u32, then four normalized u8 weights.
    fn skin_stream() -> VertexStream {
        VertexStream::new(
            12,
            vec![
                StreamAttribute::new(Semantic::Joints, VertexFormat::Uint32x2, 0),
                StreamAttribute::new(Semantic::Weights, VertexFormat::Unorm8x4, 8),
            ],
        )
    }

    pub fn streams(&self) -> &[VertexStream] {
        &self.streams
    }

    pub fn max_stride(&self) -> u64 {
        self.streams.iter().map(|stream| stream.stride).max().unwrap_or(4)
    }

    pub fn find(&self, semantic: Semantic) -> Option<(usize, &StreamAttribute)> {
        self.streams.iter().enumerate().find_map(|(index, stream)| {
            let attribute = stream
                .attributes
                .iter()
                .find(|attribute| attribute.semantic == semantic)?;
            Some((index, attribute))
        })
    }

    pub fn vertex_buffer_layouts(&self) -> Vec<VertexBufferLayout> {
        self.streams
            .iter()
            .map(|stream| VertexBufferLayout {
                stride: stream.stride,
                step_mode: VertexStepMode::Vertex,
                attributes: stream
                    .attributes
                    .iter()
                    .map(|attribute| VertexAttribute {
                        format: attribute.format,
                        offset: attribute.offset,
                        location: attribute.semantic.location(),
                    })
                    .collect(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn stream(stride: u64, attributes: &[(Semantic, VertexFormat, u64)]) -> VertexStream {
        VertexStream::new(
            stride,
            attributes
                .iter()
                .map(|(semantic, format, offset)| StreamAttribute::new(*semantic, *format, *offset))
                .collect(),
        )
    }

    const POSITION: (Semantic, VertexFormat, u64) = (Semantic::Position, VertexFormat::Float32x3, 0);

    #[test]
    fn built_in_layouts_are_valid_and_distinct() {
        let layouts: HashSet<VertexLayout> = [
            VertexLayout::position_only(),
            VertexLayout::standard(),
            VertexLayout::skinned(),
        ]
        .into_iter()
        .collect();
        assert_eq!(layouts.len(), 3);
    }

    #[test]
    fn layout_maps_to_rhi_buffers_with_fixed_locations() {
        let buffers = VertexLayout::skinned().vertex_buffer_layouts();

        assert_eq!(buffers.len(), 3);
        assert_eq!(buffers[0].stride, 12);
        assert_eq!(buffers[1].stride, 36);
        let locations: Vec<u32> = buffers[1]
            .attributes
            .iter()
            .map(|attribute| attribute.location)
            .collect();
        assert_eq!(locations, [1, 2, 3]);
        assert_eq!(buffers[2].attributes[0].format, VertexFormat::Uint32x2);
        assert_eq!(buffers[2].attributes[1].location, Semantic::Weights.location());
    }

    #[test]
    fn find_reports_stream_and_offset() {
        let layout = VertexLayout::standard();
        let (stream, uv) = layout.find(Semantic::Uv0).expect("standard has uv0");

        assert_eq!((stream, uv.offset), (1, 28));
        assert!(layout.find(Semantic::Joints).is_none());
    }

    #[test]
    fn invalid_layouts_are_rejected_with_m0001() {
        let cases = [
            ("no streams", vec![]),
            ("empty stream", vec![stream(12, &[])]),
            (
                "no position",
                vec![stream(8, &[(Semantic::Uv0, VertexFormat::Float32x2, 0)])],
            ),
            (
                "position not f32x3",
                vec![stream(16, &[(Semantic::Position, VertexFormat::Float32x4, 0)])],
            ),
            ("stride not multiple of 4", vec![stream(14, &[POSITION])]),
            ("zero stride", vec![stream(0, &[POSITION])]),
            ("attribute past stride", vec![stream(8, &[POSITION])]),
            (
                "unaligned offset",
                vec![stream(20, &[POSITION, (Semantic::Uv0, VertexFormat::Float32x2, 14)])],
            ),
            (
                "overlap",
                vec![stream(16, &[POSITION, (Semantic::Normal, VertexFormat::Float32x3, 8)])],
            ),
            (
                "duplicate semantic across streams",
                vec![stream(12, &[POSITION]), stream(12, &[POSITION])],
            ),
        ];
        for (name, streams) in cases {
            let error = VertexLayout::new(streams).expect_err(name);
            assert_eq!(error.code(), "M0001", "{name}: {error}");
        }
    }

    #[test]
    fn position_may_live_in_a_later_stream() {
        let layout = VertexLayout::new(vec![
            stream(8, &[(Semantic::Uv0, VertexFormat::Float32x2, 0)]),
            stream(12, &[POSITION]),
        ])
        .expect("valid");

        assert_eq!(layout.find(Semantic::Position).map(|(stream, _)| stream), Some(1));
    }
}
