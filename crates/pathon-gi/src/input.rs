use std::hash::{DefaultHasher, Hash, Hasher};

use pathon_api::glam::Affine3A;
use pathon_api::rhi::BufferId;
use pathon_material::{AlphaKind, MaterialTable, flag_bits};
use pathon_mesh::{GeometryPool, MeshId, Semantic};
use pathon_scene::{Aabb, InstanceFlags, RenderWorld};

/// The most triangles one dispatch of the voxelizer covers; a larger mesh is split.
const MAX_TRIANGLES_PER_JOB: u32 = 2_000_000;
/// A material this transparent lets light through, so it does not stop probe rays.
const MAX_OPAQUE_TRANSMISSION: f32 = 0.5;

/// One draw of a mesh into the voxels: a run of triangles of a pool chunk, the transform that places it and the
/// surface values the voxels take from its material.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VoxelJob {
    pub rows: [[f32; 4]; 3],
    /// The position stream, the surface stream and the index buffer of the chunk.
    pub positions: BufferId,
    pub surface: BufferId,
    pub indices: BufferId,
    pub first_index: u32,
    pub triangles: u32,
    pub base_vertex: i32,
    pub material: u32,
    pub albedo: [f32; 4],
    pub emissive: [f32; 4],
    pub base_color_slot: u32,
    pub emissive_slot: u32,
    /// The five texture slots of the material, in the order the texture binder wants them.
    pub texture_values: [u32; 5],
}

/// What the scene contributes to the voxels: the jobs, the box they fill and a number that changes when any of them
/// does.
#[derive(Clone, Debug, Default)]
pub(crate) struct SceneInput {
    pub jobs: Vec<VoxelJob>,
    pub bounds: Option<Aabb>,
    pub signature: u64,
    /// Instances left out because their mesh or material is not known.
    pub skipped: usize,
}

/// Why the geometry pool cannot be voxelized.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct UnfitPool;

fn affine_rows(transform: &Affine3A) -> [[f32; 4]; 3] {
    let (matrix, translation) = (transform.matrix3, transform.translation);
    [
        [matrix.x_axis.x, matrix.y_axis.x, matrix.z_axis.x, translation.x],
        [matrix.x_axis.y, matrix.y_axis.y, matrix.z_axis.y, translation.y],
        [matrix.x_axis.z, matrix.y_axis.z, matrix.z_axis.z, translation.z],
    ]
}

/// Whether the pool has the streams the voxelizer reads: positions (three floats) in stream 0 and the standard surface
/// stream (normal, tangent, uv0) as stream 1.
pub(crate) fn check_pool(pool: &GeometryPool) -> Result<(), UnfitPool> {
    let layout = pool.layout();
    let position = layout.find(Semantic::Position);
    let uv = layout.find(Semantic::Uv0);
    let streams = layout.streams();
    let fits = matches!(position, Some((0, attribute)) if attribute.offset == 0)
        && matches!(uv, Some((1, attribute)) if attribute.offset == 28)
        && streams.first().is_some_and(|stream| stream.stride == 12)
        && streams.get(1).is_some_and(|stream| stream.stride == 36);
    if fits { Ok(()) } else { Err(UnfitPool) }
}

pub(crate) fn gather(world: &RenderWorld, pool: &GeometryPool, materials: &MaterialTable) -> SceneInput {
    let mut input = SceneInput::default();
    let mut hasher = DefaultHasher::new();
    let instances = world.instances();
    for slot in instances.iter_alive() {
        if !instances.flags()[slot].contains(InstanceFlags::VISIBLE) {
            continue;
        }
        let mesh = instances.meshes()[slot].to_id::<MeshId>();
        let material_raw = instances.materials()[slot].raw().index();
        let material = if material_raw == u32::MAX { 0 } else { material_raw };
        let (Some(info), Some(variant), Some(data)) = (
            pool.mesh(mesh),
            materials.variant_at(material),
            materials.data_at(material),
        ) else {
            input.skipped += 1;
            continue;
        };
        let lets_light_through = variant.alpha == AlphaKind::Transparent
            || (data.flags & flag_bits::TRANSMISSION != 0 && data.transmission > MAX_OPAQUE_TRANSMISSION);
        if lets_light_through {
            continue;
        }
        let location = info.location;
        let (Some(vertex), Some(indices)) = (
            pool.vertex_buffers(location.vertex_chunk),
            pool.index_buffer(location.index_chunk),
        ) else {
            input.skipped += 1;
            continue;
        };
        let (Some(&positions), Some(&surface)) = (vertex.first(), vertex.get(1)) else {
            input.skipped += 1;
            continue;
        };
        let transform = instances.transforms()[slot];
        let world_bounds = Aabb::new(info.bounds.min, info.bounds.max).transformed(&transform);
        input.bounds = Some(match input.bounds {
            None => world_bounds,
            Some(known) => Aabb::new(known.min.min(world_bounds.min), known.max.max(world_bounds.max)),
        });
        let rows = affine_rows(&transform);
        let triangles = location.index_count / 3;
        let mut done = 0;
        while done < triangles {
            let take = (triangles - done).min(MAX_TRIANGLES_PER_JOB);
            let job = VoxelJob {
                rows,
                positions,
                surface,
                indices,
                first_index: location.first_index + done * 3,
                triangles: take,
                base_vertex: location.base_vertex_signed(),
                material,
                albedo: data.base_color,
                emissive: data.emissive,
                base_color_slot: data.base_color_texture,
                emissive_slot: data.emissive_texture,
                texture_values: [
                    data.base_color_texture,
                    data.metallic_roughness_texture,
                    data.normal_texture,
                    data.occlusion_texture,
                    data.emissive_texture,
                ],
            };
            job.hash_into(&mut hasher);
            input.jobs.push(job);
            done += take;
        }
    }
    input.signature = hasher.finish();
    input
}

impl VoxelJob {
    fn hash_into(&self, hasher: &mut DefaultHasher) {
        for row in self.rows {
            for value in row {
                value.to_bits().hash(hasher);
            }
        }
        (self.first_index, self.triangles, self.base_vertex, self.material).hash(hasher);
        self.positions.hash(hasher);
        self.indices.hash(hasher);
        for value in self.albedo.into_iter().chain(self.emissive) {
            value.to_bits().hash(hasher);
        }
        self.texture_values.hash(hasher);
    }
}
