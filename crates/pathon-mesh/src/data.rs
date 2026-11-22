use std::borrow::Cow;

use pathon_api::glam::Vec3;

use crate::error::MeshError;
use crate::layout::{Semantic, VertexLayout};

#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub enum IndexData<'src> {
    U16(&'src [u16]),
    U32(&'src [u32]),
}

impl IndexData<'_> {
    pub fn len(&self) -> usize {
        match self {
            Self::U16(indices) => indices.len(),
            Self::U32(indices) => indices.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }
}

// One stream of bytes per stream of the pool's layout, `vertex_count * stride` bytes each.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MeshData<'src> {
    pub label: &'static str,
    pub streams: Vec<&'src [u8]>,
    pub indices: IndexData<'src>,
}

impl<'src> MeshData<'src> {
    pub fn new(label: &'static str, streams: Vec<&'src [u8]>, indices: IndexData<'src>) -> Self {
        Self {
            label,
            streams,
            indices,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ValidatedMesh<'src> {
    pub(crate) vertex_count: u64,
    pub(crate) indices: Cow<'src, [u32]>,
    pub(crate) bounds: Bounds,
}

fn invalid(data: &MeshData<'_>, reason: String) -> MeshError {
    MeshError::InvalidData {
        label: data.label,
        reason,
    }
}

pub(crate) fn validate<'src>(layout: &VertexLayout, data: &MeshData<'src>) -> Result<ValidatedMesh<'src>, MeshError> {
    if data.streams.len() != layout.streams().len() {
        return Err(invalid(
            data,
            format!(
                "{} streams given, the layout has {}",
                data.streams.len(),
                layout.streams().len()
            ),
        ));
    }
    let mut vertex_count = None;
    for (index, (bytes, stream)) in data.streams.iter().zip(layout.streams()).enumerate() {
        let len = bytes.len() as u64;
        if !len.is_multiple_of(stream.stride) {
            return Err(invalid(
                data,
                format!(
                    "stream {index} has {len} bytes, not a multiple of its stride {}",
                    stream.stride
                ),
            ));
        }
        let count = len / stream.stride;
        if *vertex_count.get_or_insert(count) != count {
            return Err(invalid(
                data,
                format!("stream {index} has {count} vertices, the first has a different count"),
            ));
        }
    }
    let vertex_count = vertex_count.unwrap_or(0);
    if vertex_count == 0 || data.indices.is_empty() {
        return Err(invalid(
            data,
            "a mesh needs at least one vertex and one index".to_owned(),
        ));
    }
    if vertex_count > u64::from(u32::MAX) {
        return Err(invalid(data, format!("{vertex_count} vertices exceed u32 indexing")));
    }
    let indices = match data.indices {
        IndexData::U16(indices) => Cow::Owned(indices.iter().map(|index| u32::from(*index)).collect()),
        IndexData::U32(indices) => Cow::Borrowed(indices),
    };
    if let Some(index) = indices.iter().find(|index| u64::from(**index) >= vertex_count) {
        return Err(invalid(
            data,
            format!("index {index} is outside {vertex_count} vertices"),
        ));
    }
    Ok(ValidatedMesh {
        vertex_count,
        indices,
        bounds: bounds_of(layout, data),
    })
}

fn bounds_of(layout: &VertexLayout, data: &MeshData<'_>) -> Bounds {
    let (stream, position) = layout
        .find(Semantic::Position)
        .expect("a validated layout always has Position");
    let stride = layout.streams()[stream].stride;
    let offset = usize::try_from(position.offset).expect("attribute offsets are below the stride");
    let stride = usize::try_from(stride).expect("strides fit usize");
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for vertex in data.streams[stream].chunks_exact(stride) {
        let position: [f32; 3] = bytemuck::pod_read_unaligned(&vertex[offset..offset + 12]);
        let position = Vec3::from_array(position);
        min = min.min(position);
        max = max.max(position);
    }
    Bounds { min, max }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(values: &[[f32; 3]]) -> Vec<u8> {
        bytemuck::cast_slice(values).to_vec()
    }

    #[test]
    fn bounds_cover_every_vertex() {
        let bytes = positions(&[[1.0, -2.0, 3.0], [-1.0, 5.0, 0.0], [0.0, 0.0, 9.0]]);
        let data = MeshData::new("tri", vec![&bytes], IndexData::U16(&[0, 1, 2]));
        let mesh = validate(&VertexLayout::position_only(), &data).expect("valid");

        assert_eq!(
            mesh.bounds,
            Bounds::new(Vec3::new(-1.0, -2.0, 0.0), Vec3::new(1.0, 5.0, 9.0))
        );
        assert_eq!(mesh.indices.as_ref(), [0, 1, 2]);
    }

    #[test]
    fn malformed_data_is_rejected_with_m0002() {
        let three = positions(&[[0.0; 3]; 3]);
        let ragged = vec![0u8; 13];
        let two_surface_vertices = vec![0u8; 72];
        let three_surface_vertices = vec![0u8; 108];
        let layout = VertexLayout::standard();
        let cases = [
            ("missing stream", MeshData::new("a", vec![&three], IndexData::U32(&[0]))),
            (
                "ragged stream",
                MeshData::new("b", vec![&ragged, &ragged], IndexData::U32(&[0])),
            ),
            (
                "stream counts differ",
                MeshData::new("c", vec![&three, &two_surface_vertices], IndexData::U32(&[0])),
            ),
            ("no vertices", MeshData::new("d", vec![&[], &[]], IndexData::U32(&[0]))),
            (
                "no indices",
                MeshData::new("e", vec![&three, &three_surface_vertices], IndexData::U32(&[])),
            ),
            (
                "index out of range",
                MeshData::new("f", vec![&three, &three_surface_vertices], IndexData::U16(&[0, 1, 3])),
            ),
        ];
        for (name, data) in cases {
            let error = validate(&layout, &data).expect_err(name);
            assert_eq!(error.code(), "M0002", "{name}: {error}");
        }
    }
}
