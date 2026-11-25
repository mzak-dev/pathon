use std::collections::HashMap;

use pathon_api::Diagnostic;
use pathon_api::glam::{Affine3A, Mat3, Vec3};
use pathon_api::rhi::{BindGroupId, BufferId, DeviceApi, Id, VertexBufferLayout};
use pathon_bindless::TextureBinder;
use pathon_material::{AlphaKind, MaterialData, MaterialTable, MaterialVariant};
use pathon_mesh::{GeometryPool, MeshId, Semantic};
use pathon_scene::{InstanceFlags, RenderWorld};

/// `pbr::common::Instance`: the rows of the model and previous model matrices, the normal matrix and the material
/// index. 160 bytes.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuInstance {
    pub model: [[f32; 4]; 3],
    pub previous_model: [[f32; 4]; 3],
    pub normal_matrix: [[f32; 4]; 3],
    pub material: [u32; 4],
}

// Three `vec4` columns of a `mat3x4<f32>` hold the three rows of the affine matrix.
fn affine_rows(transform: &Affine3A) -> [[f32; 4]; 3] {
    let (matrix, translation) = (transform.matrix3, transform.translation);
    [
        [matrix.x_axis.x, matrix.y_axis.x, matrix.z_axis.x, translation.x],
        [matrix.x_axis.y, matrix.y_axis.y, matrix.z_axis.y, translation.y],
        [matrix.x_axis.z, matrix.y_axis.z, matrix.z_axis.z, translation.z],
    ]
}

fn normal_columns(transform: &Affine3A) -> [[f32; 4]; 3] {
    let normal = Mat3::from(transform.matrix3).inverse().transpose();
    let column = |axis: Vec3| [axis.x, axis.y, axis.z, 0.0];
    [column(normal.x_axis), column(normal.y_axis), column(normal.z_axis)]
}

impl GpuInstance {
    pub fn new(transform: &Affine3A, previous: &Affine3A, material: u32) -> Self {
        Self {
            model: affine_rows(transform),
            previous_model: affine_rows(previous),
            normal_matrix: normal_columns(transform),
            material: [material, 0, 0, 0],
        }
    }
}

/// The buffers one chunk of the pool binds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChunkBuffers {
    pub vertex: Vec<BufferId>,
    pub index: BufferId,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DrawItem {
    pub chunk: usize,
    pub first_index: u32,
    pub index_count: u32,
    pub base_vertex: i32,
    pub instance: u32,
    pub variant: MaterialVariant,
    pub texture_group: Option<BindGroupId>,
    /// Distance from the camera to the instance origin: the sort key of blended draws.
    pub distance: f32,
}

/// What the scene draws this frame, ready for the passes: the instance array, the chunks it uses and the draw lists.
#[derive(Clone, Debug, Default)]
pub struct SceneDraws {
    pub instances: Vec<GpuInstance>,
    pub chunks: Vec<ChunkBuffers>,
    /// Opaque and masked, grouped by permutation and chunk.
    pub opaque: Vec<DrawItem>,
    /// Blended, farthest first.
    pub transparent: Vec<DrawItem>,
    pub vertex_buffers: Vec<VertexBufferLayout>,
    pub skipped: usize,
}

fn variant_order(variant: MaterialVariant) -> (u8, bool, bool, bool, bool) {
    let alpha = match variant.alpha {
        AlphaKind::Opaque => 0,
        AlphaKind::Masked => 1,
        AlphaKind::Transparent => 2,
    };
    (
        alpha,
        variant.subsurface,
        variant.transmission,
        variant.clearcoat,
        variant.burley_diffuse,
    )
}

/// The slots of `MaterialData` that hold textures.
pub(crate) fn texture_values(data: &MaterialData) -> [u32; 5] {
    [
        data.base_color_texture,
        data.metallic_roughness_texture,
        data.normal_texture,
        data.occlusion_texture,
        data.emissive_texture,
    ]
}

pub(crate) struct SceneSources<'sources> {
    pub world: &'sources RenderWorld,
    pub pool: &'sources GeometryPool,
    pub materials: &'sources MaterialTable,
    pub camera_position: Vec3,
}

/// Walks the visible instances of the snapshot. Instances whose mesh or material is unknown are skipped and counted
/// (`L0003`); a pool without the standard vertex attributes draws nothing (`L0002`).
pub(crate) fn gather(
    sources: &SceneSources<'_>,
    device: &dyn DeviceApi,
    mut binder: Option<&mut TextureBinder>,
    diagnostics: &mut Vec<Diagnostic>,
) -> SceneDraws {
    let pool_layout = sources.pool.layout();
    let has = |semantic| pool_layout.find(semantic).is_some();
    if !(has(Semantic::Normal) && has(Semantic::Tangent) && has(Semantic::Uv0)) {
        diagnostics.push(
            Diagnostic::error("L0002", "the geometry pool has no normal, tangent and uv0 attributes")
                .with_hint("use VertexLayout::standard() for the pool PbrPack draws"),
        );
        return SceneDraws::default();
    }
    let mut draws = SceneDraws {
        vertex_buffers: pool_layout.vertex_buffer_layouts(),
        ..SceneDraws::default()
    };
    let mut chunk_of: HashMap<(u32, u32), usize> = HashMap::new();
    let mut groups: HashMap<u32, Option<BindGroupId>> = HashMap::new();
    let instances = sources.world.instances();

    for slot in instances.iter_alive() {
        if !instances.flags()[slot].contains(InstanceFlags::VISIBLE) {
            continue;
        }
        let mesh = instances.meshes()[slot].to_id::<MeshId>();
        let material_raw = instances.materials()[slot].raw().index();
        // `MaterialHandle::DEFAULT` names the pack's fallback material: the first of the table.
        let material = if material_raw == u32::MAX { 0 } else { material_raw };
        let (Some(info), Some(variant), Some(data)) = (
            sources.pool.mesh(mesh),
            sources.materials.variant_at(material),
            sources.materials.data_at(material),
        ) else {
            draws.skipped += 1;
            continue;
        };
        let location = info.location;
        let key = (location.vertex_chunk, location.index_chunk);
        let chunk = if let Some(chunk) = chunk_of.get(&key) {
            *chunk
        } else {
            let (Some(vertex), Some(index)) = (
                sources.pool.vertex_buffers(location.vertex_chunk),
                sources.pool.index_buffer(location.index_chunk),
            ) else {
                draws.skipped += 1;
                continue;
            };
            draws.chunks.push(ChunkBuffers {
                vertex: vertex.to_vec(),
                index,
            });
            chunk_of.insert(key, draws.chunks.len() - 1);
            draws.chunks.len() - 1
        };
        let texture_group = match binder.as_deref_mut() {
            Some(binder) => *groups.entry(material).or_insert_with(|| {
                match binder.bind_group_for_values(device, material, &texture_values(data)) {
                    Ok(group) => Some(group),
                    Err(error) => {
                        diagnostics.push(error.to_diagnostic());
                        None
                    }
                }
            }),
            None => None,
        };
        let transform = instances.transforms()[slot];
        let instance = u32::try_from(draws.instances.len()).expect("fewer than 2^32 instances");
        draws.instances.push(GpuInstance::new(
            &transform,
            &instances.previous_transforms()[slot],
            material,
        ));
        let item = DrawItem {
            chunk,
            first_index: location.first_index,
            index_count: location.index_count,
            base_vertex: location.base_vertex_signed(),
            instance,
            variant,
            texture_group,
            distance: (Vec3::from(transform.translation) - sources.camera_position).length(),
        };
        if variant.alpha == AlphaKind::Transparent {
            draws.transparent.push(item);
        } else {
            draws.opaque.push(item);
        }
    }
    draws.opaque.sort_by_key(|item| {
        (
            variant_order(item.variant),
            item.chunk,
            item.texture_group.map_or(0, |group| group.raw().index()),
        )
    });
    draws.transparent.sort_by(|a, b| b.distance.total_cmp(&a.distance));
    if draws.skipped > 0 {
        diagnostics.push(
            Diagnostic::warning(
                "L0003",
                format!("{} instances skipped: unknown mesh or material", draws.skipped),
            )
            .with_hint("MaterialHandle::DEFAULT is the material with index 0; meshes come from the pool in Shared"),
        );
    }
    draws
}

#[cfg(test)]
mod tests {
    use pathon_api::glam::Quat;

    use super::*;

    #[test]
    fn rows_apply_the_transform_to_a_point() {
        let transform = Affine3A::from_scale_rotation_translation(
            Vec3::new(2.0, 3.0, 4.0),
            Quat::from_rotation_y(0.7),
            Vec3::new(1.0, -2.0, 5.0),
        );
        let rows = affine_rows(&transform);
        let point = Vec3::new(0.3, -0.4, 0.9);

        let by_rows = Vec3::new(
            rows[0][0] * point.x + rows[0][1] * point.y + rows[0][2] * point.z + rows[0][3],
            rows[1][0] * point.x + rows[1][1] * point.y + rows[1][2] * point.z + rows[1][3],
            rows[2][0] * point.x + rows[2][1] * point.y + rows[2][2] * point.z + rows[2][3],
        );

        assert!((by_rows - transform.transform_point3(point)).length() < 1e-5);
    }

    #[test]
    fn normal_matrix_keeps_normals_perpendicular_under_non_uniform_scale() {
        let transform = Affine3A::from_scale(Vec3::new(1.0, 4.0, 1.0));
        let columns = normal_columns(&transform);
        let matrix = Mat3::from_cols(
            Vec3::from_slice(&columns[0][..3]),
            Vec3::from_slice(&columns[1][..3]),
            Vec3::from_slice(&columns[2][..3]),
        );
        let (tangent, normal) = (Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, -1.0, 0.0));

        let moved_tangent = transform.transform_vector3(tangent);
        let moved_normal = matrix * normal;

        assert!(moved_tangent.dot(moved_normal).abs() < 1e-5);
    }

    #[test]
    fn instance_is_one_hundred_sixty_bytes() {
        assert_eq!(std::mem::size_of::<GpuInstance>(), 160);
    }
}
