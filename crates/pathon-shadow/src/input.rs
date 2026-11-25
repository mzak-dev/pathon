use std::ops::Range;

use pathon_api::glam::{Affine3A, Vec3};
use pathon_api::rhi::BufferId;
use pathon_mesh::{GeometryPool, MeshError, MeshId};
use pathon_scene::{Aabb, InstanceFlags, Light, LightKind, RenderWorld};

/// The directional light that casts the cascaded shadows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DirectionalShadowLight {
    /// The direction the light shines along (from the light towards the scene), not necessarily normalised.
    pub direction: Vec3,
    /// What `shadow_factor_directional` is asked about: the index the lighting shader knows this light by. A light
    /// with another index is not shadowed by this map.
    pub light_index: u32,
}

impl DirectionalShadowLight {
    pub fn new(direction: Vec3, light_index: u32) -> Self {
        Self { direction, light_index }
    }

    /// `None` unless the light is directional.
    pub fn from_light(light: &Light, light_index: u32) -> Option<Self> {
        matches!(light.kind, LightKind::Directional).then(|| Self::new(light.direction(), light_index))
    }
}

/// One draw of a mesh into the depth maps. Buffers are the ones of a `pathon-mesh` chunk: positions are the first
/// vertex stream (three `f32`, `vertex_stride` bytes apart), indices are `u32` and relative to `base_vertex`.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct ShadowCaster {
    pub vertex_buffer: BufferId,
    pub vertex_stride: u32,
    pub index_buffer: BufferId,
    pub first_index: u32,
    pub index_count: u32,
    pub base_vertex: i32,
    /// Object to world.
    pub model: Affine3A,
    /// World box, for culling per cascade; `Aabb::UNBOUNDED` when unknown (it then reaches every cascade).
    pub bounds: Aabb,
}

impl ShadowCaster {
    /// A draw of `indices` (relative to `base_vertex`) of buffers the host owns, with unknown bounds: it is drawn
    /// into every cascade until [`ShadowCaster::with_bounds`] says where it stands.
    pub fn new(
        vertex_buffer: BufferId,
        vertex_stride: u32,
        index_buffer: BufferId,
        indices: Range<u32>,
        base_vertex: i32,
        model: Affine3A,
    ) -> Self {
        Self {
            vertex_buffer,
            vertex_stride,
            index_buffer,
            first_index: indices.start,
            index_count: indices.end.saturating_sub(indices.start),
            base_vertex,
            model,
            bounds: Aabb::UNBOUNDED,
        }
    }

    /// A mesh of `pool` placed by `model`; the world box comes from the mesh's bounds.
    pub fn from_mesh(pool: &GeometryPool, mesh: MeshId, model: Affine3A) -> Result<Self, MeshError> {
        let info = pool.mesh(mesh).ok_or(MeshError::UnknownMesh)?;
        let location = info.location;
        let vertex_buffer = pool
            .vertex_buffers(location.vertex_chunk)
            .and_then(|buffers| buffers.first().copied())
            .ok_or(MeshError::UnknownMesh)?;
        let index_buffer = pool.index_buffer(location.index_chunk).ok_or(MeshError::UnknownMesh)?;
        let stride = pool.layout().streams().first().map_or(12, |stream| stream.stride);
        Ok(Self {
            vertex_buffer,
            vertex_stride: u32::try_from(stride).unwrap_or(12),
            index_buffer,
            first_index: location.first_index,
            index_count: location.index_count,
            base_vertex: location.base_vertex_signed(),
            model,
            bounds: Aabb::new(info.bounds.min, info.bounds.max).transformed(&model),
        })
    }

    #[must_use]
    pub fn with_bounds(self, bounds: Aabb) -> Self {
        Self { bounds, ..self }
    }
}

/// What the shadow pack renders each frame, kept in `Shared`: the light and the casters. The host (or the lighting
/// pack) refills it before every frame; with no `light` there are no shadows and no [`crate::ShadowFrame`].
#[derive(Clone, Debug, Default)]
pub struct ShadowScene {
    pub light: Option<DirectionalShadowLight>,
    casters: Vec<ShadowCaster>,
}

impl ShadowScene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn casters(&self) -> &[ShadowCaster] {
        &self.casters
    }

    /// Forgets the casters (and keeps the allocation) for the next frame.
    pub fn clear_casters(&mut self) {
        self.casters.clear();
    }

    pub fn add_caster(&mut self, caster: ShadowCaster) {
        self.casters.push(caster);
    }

    pub fn add_mesh(&mut self, pool: &GeometryPool, mesh: MeshId, model: Affine3A) -> Result<(), MeshError> {
        self.casters.push(ShadowCaster::from_mesh(pool, mesh, model)?);
        Ok(())
    }

    /// Replaces the casters with every visible instance of `world` flagged `CASTS_SHADOWS`. `MeshHandle` is read as
    /// the `MeshId` of `pool` (`to_id`), the mapping `pathon-scene` documents; an instance whose mesh the pool does
    /// not know is skipped. Returns how many casters there are.
    pub fn fill_from_world(&mut self, world: &RenderWorld, pool: &GeometryPool) -> usize {
        self.casters.clear();
        let instances = world.instances();
        for slot in instances.iter_alive() {
            let flags = instances.flags()[slot];
            if !flags.contains(InstanceFlags::VISIBLE | InstanceFlags::CASTS_SHADOWS) {
                continue;
            }
            let mesh = instances.meshes()[slot].to_id::<MeshId>();
            let Ok(caster) = ShadowCaster::from_mesh(pool, mesh, instances.transforms()[slot]) else {
                continue;
            };
            let bounds = instances.world_bounds(slot).filter(Aabb::is_bounded);
            self.casters
                .push(bounds.map_or(caster, |bounds| caster.with_bounds(bounds)));
        }
        self.casters.len()
    }

    /// Takes the first directional light of `world` that casts shadows. `light_index` counts lights in the order
    /// `RenderWorld::lights` iterates them, the order a lighting pack uploads them in.
    pub fn use_first_shadow_light(&mut self, world: &RenderWorld) {
        self.light = world
            .lights()
            .iter()
            .enumerate()
            .filter(|(_, (_, light))| light.casts_shadows)
            .find_map(|(index, (_, light))| DirectionalShadowLight::from_light(light, u32::try_from(index).ok()?));
    }
}

#[cfg(test)]
mod tests {
    use pathon_api::rhi::{Id, RawHandle};
    use pathon_mesh::{IndexData, MeshData, PoolDesc, VertexLayout};
    use pathon_rhi::NullDevice;
    use pathon_scene::{InstanceDesc, MaterialHandle, MeshHandle, Scene};

    use super::*;

    fn triangle(pool: &mut GeometryPool, device: &NullDevice) -> MeshId {
        let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 3.0, 0.0]];
        pool.upload(
            device,
            &MeshData::new(
                "triangle",
                vec![bytemuck::cast_slice(&positions)],
                IndexData::U16(&[0, 1, 2]),
            ),
        )
        .expect("upload")
    }

    #[test]
    fn a_mesh_becomes_a_caster_with_world_bounds() {
        let device = NullDevice::new();
        let mut pool = GeometryPool::new(&device, "pool", VertexLayout::position_only(), PoolDesc::default());
        let mesh = triangle(&mut pool, &device);
        let mut scene = ShadowScene::new();

        scene
            .add_mesh(&pool, mesh, Affine3A::from_translation(Vec3::new(10.0, 0.0, 0.0)))
            .expect("known mesh");

        let caster = scene.casters()[0];
        assert_eq!((caster.index_count, caster.vertex_stride), (3, 12));
        assert_eq!(
            caster.bounds,
            Aabb::new(Vec3::new(10.0, 0.0, 0.0), Vec3::new(12.0, 3.0, 0.0))
        );
    }

    #[test]
    fn an_unknown_mesh_is_an_error() {
        let device = NullDevice::new();
        let mut pool = GeometryPool::new(&device, "pool", VertexLayout::position_only(), PoolDesc::default());
        let mesh = triangle(&mut pool, &device);
        pool.remove(mesh, 0).expect("remove");

        assert!(ShadowScene::new().add_mesh(&pool, mesh, Affine3A::IDENTITY).is_err());
    }

    #[test]
    fn only_visible_shadow_casting_instances_of_known_meshes_are_taken() {
        let device = NullDevice::new();
        let mut pool = GeometryPool::new(&device, "pool", VertexLayout::position_only(), PoolDesc::default());
        let mesh = triangle(&mut pool, &device);
        let handle = MeshHandle::from_id(mesh);
        let mut scene = Scene::new();
        scene.extract(|writer| {
            let desc = InstanceDesc::new(handle, MaterialHandle::DEFAULT, Affine3A::IDENTITY);
            writer.spawn(&desc).expect("caster");
            writer
                .spawn(&desc.with_flags(InstanceFlags::VISIBLE))
                .expect("does not cast");
            writer
                .spawn(&desc.with_flags(InstanceFlags::CASTS_SHADOWS))
                .expect("invisible");
            writer
                .spawn(&InstanceDesc::new(
                    MeshHandle::from_id(MeshId::from_raw(RawHandle::new(77, 0))),
                    MaterialHandle::DEFAULT,
                    Affine3A::IDENTITY,
                ))
                .expect("unknown mesh");
        });
        let mut shadows = ShadowScene::new();

        assert_eq!(shadows.fill_from_world(scene.world(), &pool), 1);
    }

    #[test]
    fn the_first_shadow_casting_directional_light_is_chosen_with_its_index() {
        let mut scene = Scene::new();
        scene.extract(|writer| {
            writer
                .spawn_light(&Light::point(Affine3A::IDENTITY, 800.0).with_shadows())
                .expect("point");
            writer
                .spawn_light(&Light::directional(Affine3A::IDENTITY, 1.0e5))
                .expect("without shadows");
            writer
                .spawn_light(&Light::directional(Affine3A::IDENTITY, 1.0e5).with_shadows())
                .expect("sun");
        });
        let mut shadows = ShadowScene::new();

        shadows.use_first_shadow_light(scene.world());

        assert_eq!(
            shadows.light.map(|light| light.light_index),
            Some(2),
            "a point light does not count"
        );
    }
}
