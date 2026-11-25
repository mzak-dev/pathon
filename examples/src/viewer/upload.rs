// The host side of the viewer: a `LoadedScene` becomes what pathon's crates keep. Meshes go to a `GeometryPool`,
// images through `TextureLoader` (sRGB views, mips in compute), materials into a `MaterialTable`, and instances,
// lights and cameras into a `Scene` the render side reads as a snapshot.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

use pathon::api::glam::Vec3;
use pathon::api::resources::DeferredRelease;
use pathon::rhi::{
    AddressMode, BindGroupDesc, BindGroupEntry, BindGroupId, BindGroupLayoutDesc, BindGroupLayoutEntry,
    BindGroupLayoutId, BindingResource, BindingType, BufferDesc, BufferId, BufferUsage, DeviceApi, FilterMode, Id,
    RawHandle, SamplerBindingType, SamplerDesc, SamplerId, ShaderStages, TextureViewId,
};
use pathon_material::{MaterialId, MaterialTable, MaterialTextures, StandardMaterial, TextureSlot};
use pathon_mesh::{GeometryPool, IndexData, MeshData, MeshId, PoolDesc, VertexLayout};
use pathon_scene::{Aabb, CameraDesc, CameraId, InstanceDesc, MaterialHandle, MeshHandle, Scene, Snapshot};
use pathon_texture::{ColorSpace, Texture, TextureLoader, TextureSource, Variant};

use crate::gltf_loader::{CpuMaterial, LoadedScene, TextureRole, UvTransform};

const FRAMES_IN_FLIGHT: u32 = 2;
const ANISOTROPY: u16 = 8;
const IMAGE_LABEL: &str = "gltf image";
const WHITE_TEXEL: [u8; 4] = [255; 4];
/// Per role two `vec4`s: the 2x2 matrix of the UV transform, then its offset.
const UV_TRANSFORM_VECTORS: usize = TextureRole::COUNT * 2;

// `MaterialId` is a plain table index and does not implement `Id`; the scene only needs a carrier for its raw form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct MaterialIndex(RawHandle);

impl Id for MaterialIndex {
    const KIND: &'static str = "material";

    fn from_raw(raw: RawHandle) -> Self {
        Self(raw)
    }

    fn raw(self) -> RawHandle {
        self.0
    }
}

fn material_handle(id: MaterialId) -> MaterialHandle {
    MaterialHandle::from_id(MaterialIndex(RawHandle::new(id.index(), 0)))
}

#[derive(Debug, thiserror::Error)]
#[error("{what}: {reason}")]
pub struct UploadError {
    what: &'static str,
    reason: String,
}

fn fail(what: &'static str, reason: impl Display) -> UploadError {
    UploadError {
        what,
        reason: reason.to_string(),
    }
}

/// A camera of the file, spawned into the scene.
#[derive(Clone, Debug)]
pub struct SceneCameraEntry {
    pub name: String,
    pub id: CameraId,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UploadStats {
    pub meshes: usize,
    pub textures: usize,
    pub materials: usize,
    pub instances: usize,
    pub lights: usize,
    pub triangles: usize,
}

/// Everything the viewer pack draws from. Immutable after the upload, so the render side shares it through an `Arc`.
#[derive(Debug)]
pub struct ViewerAssets {
    pub pool: GeometryPool,
    pub materials: MaterialTable,
    /// `MaterialId`s in the order of `LoadedScene::materials`; an instance's `MaterialHandle` holds the index.
    pub material_ids: Vec<MaterialId>,
    pub material_buffer: BufferId,
    /// Group 1 of the forward pipelines (docs/crates/pathon-material.md, `texture_source`: the profile Web).
    pub material_layout: BindGroupLayoutId,
    /// One bind group per `MaterialId`.
    pub material_groups: Vec<BindGroupId>,
    pub textures: Vec<Texture>,
    pub snapshot: Snapshot,
    pub cameras: Vec<SceneCameraEntry>,
    pub main_camera: Option<usize>,
    pub bounds: (Vec3, Vec3),
    pub stats: UploadStats,
}

/// The `Shared` value the pack looks for.
#[derive(Clone, Debug)]
pub struct SharedAssets(pub Arc<ViewerAssets>);

fn material_layout_entries() -> Vec<BindGroupLayoutEntry> {
    let entry = |binding, ty| BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty,
        count: None,
    };
    let mut entries: Vec<BindGroupLayoutEntry> = (0..TextureRole::COUNT)
        .map(|binding| {
            entry(
                u32::try_from(binding).expect("five bindings"),
                BindingType::texture_2d(),
            )
        })
        .collect();
    entries.push(entry(5, BindingType::Sampler(SamplerBindingType::Filtering)));
    entries.push(entry(6, BindingType::UniformBuffer));
    entries
}

fn uv_transform_vectors(
    textures: &[Option<(usize, UvTransform)>; TextureRole::COUNT],
) -> [[f32; 4]; UV_TRANSFORM_VECTORS] {
    let mut vectors = [[0.0; 4]; UV_TRANSFORM_VECTORS];
    for (role, texture) in textures.iter().enumerate() {
        let transform = texture.map_or(UvTransform::IDENTITY, |(_, transform)| transform);
        vectors[role * 2] = transform.matrix;
        vectors[role * 2 + 1] = [transform.offset[0], transform.offset[1], 0.0, 0.0];
    }
    vectors
}

fn color_space(role: TextureRole) -> ColorSpace {
    match role {
        TextureRole::BaseColor | TextureRole::Emissive => ColorSpace::Srgb,
        TextureRole::Normal => ColorSpace::Normal,
        TextureRole::MetallicRoughness | TextureRole::Occlusion => ColorSpace::Linear,
    }
}

// An image can serve as colour in one material and as data in another, so it is uploaded once per colour space.
struct TextureCache<'device> {
    device: &'device dyn DeviceApi,
    loader: TextureLoader,
    scene: &'device LoadedScene,
    index_of: HashMap<(usize, ColorSpace), usize>,
    textures: Vec<Texture>,
    views: Vec<TextureViewId>,
}

impl<'device> TextureCache<'device> {
    fn new(device: &'device dyn DeviceApi, scene: &'device LoadedScene) -> Self {
        Self {
            device,
            loader: TextureLoader::new(),
            scene,
            index_of: HashMap::new(),
            textures: Vec::new(),
            views: Vec::new(),
        }
    }

    fn view(&mut self, image: usize, space: ColorSpace) -> Result<TextureViewId, UploadError> {
        if let Some(&index) = self.index_of.get(&(image, space)) {
            return Ok(self.views[index]);
        }
        let cpu = &self.scene.images[image];
        let source =
            TextureSource::new(IMAGE_LABEL, cpu.width, cpu.height, space).with_variant(Variant::rgba8(vec![&cpu.rgba]));
        let texture = self
            .loader
            .load(self.device, &source)
            .map_err(|error| fail("texture upload", error.to_diagnostic()))?;
        let view = texture
            .create_view(self.device)
            .map_err(|error| fail("texture view", error))?;
        self.index_of.insert((image, space), self.views.len());
        self.textures.push(texture);
        self.views.push(view);
        Ok(view)
    }

    fn white(&mut self) -> Result<TextureViewId, UploadError> {
        let source =
            TextureSource::new("white", 1, 1, ColorSpace::Srgb).with_variant(Variant::rgba8(vec![&WHITE_TEXEL]));
        let texture = self
            .loader
            .load(self.device, &source)
            .map_err(|error| fail("white texture", error.to_diagnostic()))?;
        let view = texture
            .create_view(self.device)
            .map_err(|error| fail("white view", error))?;
        self.textures.push(texture);
        Ok(view)
    }
}

// What the standard material needs to read a texture here: a slot that is the role, because each material has its own
// bind group of five textures (M4-04 replaces this with global slots).
fn standard_with_slots(material: &CpuMaterial) -> StandardMaterial {
    let slot = |role: TextureRole| material.textures[role as usize].map(|_| TextureSlot(role as u32));
    let mut standard = material.standard.clone();
    standard.textures = MaterialTextures {
        base_color: slot(TextureRole::BaseColor),
        metallic_roughness: slot(TextureRole::MetallicRoughness),
        normal: slot(TextureRole::Normal),
        occlusion: slot(TextureRole::Occlusion),
        emissive: slot(TextureRole::Emissive),
    };
    // The viewer has no refraction: glass is drawn as blended, so transmission selects the transparent pass.
    if standard.transmission > 0.0 {
        standard.alpha_mode = pathon_material::AlphaMode::Blend;
    }
    standard
}

fn create_sampler(device: &dyn DeviceApi) -> Result<SamplerId, UploadError> {
    device
        .create_sampler(&SamplerDesc {
            label: "material sampler",
            address_mode: AddressMode::Repeat,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: FilterMode::Linear,
            anisotropy_clamp: ANISOTROPY,
            ..SamplerDesc::LINEAR_CLAMP
        })
        .map_err(|error| fail("sampler", error))
}

struct MaterialGroups {
    layout: BindGroupLayoutId,
    groups: Vec<BindGroupId>,
}

fn create_material_groups(
    device: &dyn DeviceApi,
    scene: &LoadedScene,
    cache: &mut TextureCache<'_>,
) -> Result<MaterialGroups, UploadError> {
    let layout = device
        .create_bind_group_layout(&BindGroupLayoutDesc {
            label: "material textures",
            entries: material_layout_entries(),
        })
        .map_err(|error| fail("material layout", error))?;
    let sampler = create_sampler(device)?;
    let white = cache.white()?;
    let mut groups = Vec::with_capacity(scene.materials.len());
    for material in &scene.materials {
        let mut views = [white; TextureRole::COUNT];
        let mut used: [Option<(usize, UvTransform)>; TextureRole::COUNT] = Default::default();
        for role in TextureRole::ALL {
            if let Some(texture) = material.textures[role as usize] {
                views[role as usize] = cache.view(texture.image, color_space(role))?;
                used[role as usize] = Some((texture.image, texture.transform));
            }
        }
        let transforms = device
            .create_buffer(&BufferDesc {
                label: "material uv transforms",
                size: std::mem::size_of::<[[f32; 4]; UV_TRANSFORM_VECTORS]>() as u64,
                usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
            })
            .map_err(|error| fail("uv transform buffer", error))?;
        device
            .write_buffer(transforms, 0, bytemuck::bytes_of(&uv_transform_vectors(&used)))
            .map_err(|error| fail("uv transform upload", error))?;
        let mut entries: Vec<BindGroupEntry> = views
            .iter()
            .enumerate()
            .map(|(binding, view)| BindGroupEntry {
                binding: u32::try_from(binding).expect("five textures"),
                resource: BindingResource::TextureView(*view),
            })
            .collect();
        entries.push(BindGroupEntry {
            binding: 5,
            resource: BindingResource::Sampler(sampler),
        });
        entries.push(BindGroupEntry {
            binding: 6,
            resource: BindingResource::whole_buffer(transforms),
        });
        groups.push(
            device
                .create_bind_group(&BindGroupDesc {
                    label: "material",
                    layout,
                    entries,
                })
                .map_err(|error| fail("material bind group", error))?,
        );
    }
    Ok(MaterialGroups { layout, groups })
}

fn upload_meshes(
    device: &dyn DeviceApi,
    scene: &LoadedScene,
    pool: &mut GeometryPool,
) -> Result<Vec<MeshId>, UploadError> {
    scene
        .meshes
        .iter()
        .map(|mesh| {
            let data = MeshData::new(
                "gltf primitive",
                vec![
                    bytemuck::cast_slice(&mesh.positions),
                    bytemuck::cast_slice(&mesh.surface),
                ],
                IndexData::U32(&mesh.indices),
            );
            pool.upload(device, &data)
                .map_err(|error| fail("mesh upload", error.to_diagnostic()))
        })
        .collect()
}

/// Uploads everything the viewer draws. Textures are uploaded here, one `load` each: the loader submits its own
/// commands, so this runs before the first frame.
pub fn upload_scene(device: &dyn DeviceApi, scene: &LoadedScene) -> Result<ViewerAssets, UploadError> {
    let mut cache = TextureCache::new(device, scene);
    let groups = create_material_groups(device, scene, &mut cache)?;
    let TextureCache {
        mut loader, textures, ..
    } = cache;
    loader.release(device);

    let mut materials = MaterialTable::new("materials", FRAMES_IN_FLIGHT);
    let material_ids: Vec<MaterialId> = scene
        .materials
        .iter()
        .map(|material| {
            materials
                .add(&standard_with_slots(material))
                .map_err(|error| fail("material", error.to_diagnostic()))
        })
        .collect::<Result<_, _>>()?;
    let material_buffer = materials
        .sync(device, &mut DeferredRelease::new(u64::from(FRAMES_IN_FLIGHT)), 0)
        .map_err(|error| fail("material upload", error))?;

    let mut pool = GeometryPool::new(device, "scene", VertexLayout::standard(), PoolDesc::default());
    let mesh_ids = upload_meshes(device, scene, &mut pool)?;

    let local_bounds: Vec<Aabb> = scene
        .meshes
        .iter()
        .map(|mesh| {
            let (min, max) = mesh.local_bounds();
            Aabb::new(min, max)
        })
        .collect();
    let mut world = Scene::new();
    let cameras = world
        .extract(|writer| {
            for instance in &scene.instances {
                let mesh = &scene.meshes[instance.mesh];
                let desc = InstanceDesc::new(
                    MeshHandle::from_id(mesh_ids[instance.mesh]),
                    material_handle(material_ids[mesh.material]),
                    instance.transform,
                )
                .with_local_bounds(local_bounds[instance.mesh]);
                writer.spawn(&desc)?;
            }
            for light in &scene.lights {
                writer.spawn_light(light)?;
            }
            scene
                .cameras
                .iter()
                .map(|camera| {
                    Ok(SceneCameraEntry {
                        name: camera.name.clone(),
                        id: writer.spawn_camera(&CameraDesc::new(camera.transform, camera.projection))?,
                    })
                })
                .collect::<Result<Vec<_>, pathon_scene::SceneError>>()
        })
        .map_err(|error| fail("scene", error.to_diagnostic()))?;

    Ok(ViewerAssets {
        pool,
        materials,
        material_ids,
        material_buffer,
        material_layout: groups.layout,
        material_groups: groups.groups,
        stats: UploadStats {
            meshes: scene.meshes.len(),
            textures: textures.len(),
            materials: scene.materials.len(),
            instances: scene.instances.len(),
            lights: scene.lights.len(),
            triangles: scene.triangle_count(),
        },
        textures,
        snapshot: world.snapshot(),
        cameras,
        main_camera: scene.main_camera,
        bounds: scene.bounds().unwrap_or((Vec3::splat(-1.0), Vec3::splat(1.0))),
    })
}
