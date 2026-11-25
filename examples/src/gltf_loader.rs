// M4-10: the glTF 2.0 loader of the test scene. It lives in `examples/` on purpose: the engine crates never parse
// files, they take what a host hands them (docs/scenes.md). Everything here is CPU-side; `viewer::upload` turns it
// into pathon-mesh pools, pathon-texture textures, a pathon-material table and a pathon-scene world.

mod data_uri;
mod geometry;
mod images;
mod materials;
mod nodes;

use std::path::{Path, PathBuf};

use bytemuck::{Pod, Zeroable};
use pathon::api::glam::{Affine3A, Vec3};
use pathon_material::StandardMaterial;
use pathon_scene::{Light, Projection};

/// Second vertex stream of `VertexLayout::standard()`: normal, tangent (w is the bitangent sign) and the first UV set.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, Pod, Zeroable)]
pub struct SurfaceVertex {
    pub normal: [f32; 3],
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
}

/// One glTF primitive: the unit that has exactly one material.
#[derive(Clone, Debug)]
pub struct CpuMesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub surface: Vec<SurfaceVertex>,
    pub indices: Vec<u32>,
    /// Index into [`LoadedScene::materials`].
    pub material: usize,
}

impl CpuMesh {
    pub fn local_bounds(&self) -> (Vec3, Vec3) {
        self.positions
            .iter()
            .fold((Vec3::MAX, Vec3::MIN), |(min, max), position| {
                let position = Vec3::from(*position);
                (min.min(position), max.max(position))
            })
    }
}

/// Decoded texels, 8 bits per channel, row 0 at the top.
#[derive(Clone, Debug)]
pub struct CpuImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The five maps of the standard material, in the order of their bindings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextureRole {
    BaseColor,
    MetallicRoughness,
    Normal,
    Occlusion,
    Emissive,
}

impl TextureRole {
    pub const COUNT: usize = 5;
    pub const ALL: [Self; Self::COUNT] = [
        Self::BaseColor,
        Self::MetallicRoughness,
        Self::Normal,
        Self::Occlusion,
        Self::Emissive,
    ];
}

/// `KHR_texture_transform` as a 2x2 row-major matrix and an offset: `uv' = matrix * uv + offset`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct UvTransform {
    pub matrix: [f32; 4],
    pub offset: [f32; 2],
}

impl UvTransform {
    pub const IDENTITY: Self = Self {
        matrix: [1.0, 0.0, 0.0, 1.0],
        offset: [0.0, 0.0],
    };
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextureRef {
    /// Index into [`LoadedScene::images`].
    pub image: usize,
    pub transform: UvTransform,
}

#[derive(Clone, Debug)]
pub struct CpuMaterial {
    pub name: String,
    /// Factors only: `standard.textures` stays empty, the maps are in `textures`.
    pub standard: StandardMaterial,
    pub textures: [Option<TextureRef>; TextureRole::COUNT],
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CpuInstance {
    /// Index into [`LoadedScene::meshes`].
    pub mesh: usize,
    pub transform: Affine3A,
}

#[derive(Clone, Debug)]
pub struct SceneCamera {
    pub name: String,
    /// Camera to world.
    pub transform: Affine3A,
    pub projection: Projection,
}

#[derive(Debug, Default)]
pub struct LoadedScene {
    pub meshes: Vec<CpuMesh>,
    pub images: Vec<CpuImage>,
    pub materials: Vec<CpuMaterial>,
    pub instances: Vec<CpuInstance>,
    pub lights: Vec<Light>,
    pub cameras: Vec<SceneCamera>,
    /// The camera the scene manifest marks as `main`, when there is one.
    pub main_camera: Option<usize>,
}

impl LoadedScene {
    /// World-space axis-aligned bounds of every instance; `None` for a scene without geometry.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let local: Vec<(Vec3, Vec3)> = self.meshes.iter().map(CpuMesh::local_bounds).collect();
        self.instances
            .iter()
            .map(|instance| {
                let (min, max) = local[instance.mesh];
                (0..8).fold((Vec3::MAX, Vec3::MIN), |(low, high), corner| {
                    let pick = |bit: u32| if corner & bit == 0 { 0.0 } else { 1.0 };
                    let local = min + (max - min) * Vec3::new(pick(1), pick(2), pick(4));
                    let world = instance.transform.transform_point3(local);
                    (low.min(world), high.max(world))
                })
            })
            .reduce(|(min_a, max_a), (min_b, max_b)| (min_a.min(min_b), max_a.max(max_b)))
    }

    pub fn triangle_count(&self) -> usize {
        self.instances
            .iter()
            .map(|instance| self.meshes[instance.mesh].indices.len() / 3)
            .sum()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("cannot read {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("not a glTF 2.0 file: {0}")]
    Gltf(#[from] gltf::Error),
    #[error("buffer {index}: {reason}")]
    Buffer { index: usize, reason: String },
    #[error("image {index}: {message}")]
    Image { index: usize, message: String },
    #[error("the scene has no drawable triangles")]
    NoGeometry,
}

fn read_external(base_dir: Option<&Path>, uri: &str) -> Result<Vec<u8>, String> {
    let base = base_dir.ok_or("a relative URI needs a file to be relative to")?;
    // `%20` and friends: glTF URIs are percent-encoded, file names with spaces are the common case.
    let decoded = percent_decode(uri);
    std::fs::read(base.join(decoded)).map_err(|error| error.to_string())
}

fn percent_decode(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%' && index + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[index + 1..index + 3]).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(byte) = escaped {
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn read_uri(uri: &str, base_dir: Option<&Path>) -> Result<Vec<u8>, String> {
    if uri.starts_with("data:") {
        data_uri::decode_data_uri(uri).ok_or_else(|| "malformed data URI".to_owned())
    } else {
        read_external(base_dir, uri)
    }
}

fn load_buffers(
    document: &gltf::Document,
    blob: Option<Vec<u8>>,
    base_dir: Option<&Path>,
) -> Result<Vec<Vec<u8>>, LoadError> {
    let mut blob = blob;
    document
        .buffers()
        .map(|buffer| {
            let index = buffer.index();
            let reason = |reason: String| LoadError::Buffer { index, reason };
            match buffer.source() {
                gltf::buffer::Source::Bin => blob.take().ok_or_else(|| reason("the file has no binary chunk".into())),
                gltf::buffer::Source::Uri(uri) => read_uri(uri, base_dir).map_err(reason),
            }
        })
        .collect()
}

fn image_sources(
    document: &gltf::Document,
    buffers: &[Vec<u8>],
    base_dir: Option<&Path>,
) -> Result<Vec<Vec<u8>>, LoadError> {
    document
        .images()
        .map(|image| {
            let index = image.index();
            let failed = |message: String| LoadError::Image { index, message };
            match image.source() {
                gltf::image::Source::View { view, .. } => {
                    let buffer = buffers
                        .get(view.buffer().index())
                        .ok_or_else(|| failed("its buffer view names a missing buffer".into()))?;
                    buffer
                        .get(view.offset()..view.offset() + view.length())
                        .map(<[u8]>::to_vec)
                        .ok_or_else(|| failed("its buffer view is out of range".into()))
                }
                gltf::image::Source::Uri { uri, .. } => read_uri(uri, base_dir).map_err(failed),
            }
        })
        .collect()
}

// `<scene>.scene.json` (docs/scenes.md) says which camera is the main one, so the loader need not guess.
fn main_camera_name(manifest: &str) -> Option<String> {
    let manifest: serde_json::Value = serde_json::from_str(manifest).ok()?;
    manifest
        .get("cameras")?
        .as_array()?
        .iter()
        .find(|camera| camera.get("main").and_then(serde_json::Value::as_bool) == Some(true))?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

/// Loads a `.glb` or `.gltf`. External files are resolved against `base_dir`.
pub fn load_slice(bytes: &[u8], base_dir: Option<&Path>, manifest: Option<&str>) -> Result<LoadedScene, LoadError> {
    let gltf::Gltf { document, blob } = gltf::Gltf::from_slice(bytes)?;
    let buffers = load_buffers(&document, blob, base_dir)?;
    let images = images::decode_all(&image_sources(&document, &buffers, base_dir)?)?;

    let mut materials: Vec<CpuMaterial> = document
        .materials()
        .map(|material| materials::read_material(&material))
        .collect();
    // Primitives without a material share one default at the end.
    let default_material = materials.len();
    materials.push(CpuMaterial {
        name: "default".into(),
        standard: StandardMaterial::default(),
        textures: Default::default(),
    });

    let mut meshes = Vec::new();
    let mut primitive_meshes = nodes::PrimitiveMeshes::new();
    for mesh in document.meshes() {
        let mut built = Vec::new();
        for primitive in mesh.primitives() {
            let material = primitive.material().index().unwrap_or(default_material);
            let name = format!("{}/{}", mesh.name().unwrap_or("mesh"), primitive.index());
            if let Some(cpu) = geometry::read_primitive(&primitive, &buffers, name, material) {
                built.push(meshes.len());
                meshes.push(cpu);
            }
        }
        primitive_meshes.insert(mesh.index(), built);
    }

    let flattened = nodes::flatten(&document, &primitive_meshes);
    if flattened.instances.is_empty() {
        return Err(LoadError::NoGeometry);
    }
    let main_camera = manifest
        .and_then(main_camera_name)
        .and_then(|name| flattened.cameras.iter().position(|camera| camera.name == name))
        .or((!flattened.cameras.is_empty()).then_some(0));
    Ok(LoadedScene {
        meshes,
        images,
        materials,
        instances: flattened.instances,
        lights: flattened.lights,
        cameras: flattened.cameras,
        main_camera,
    })
}

pub fn load_path(path: &Path) -> Result<LoadedScene, LoadError> {
    let io = |source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    };
    let bytes = std::fs::read(path).map_err(io)?;
    let manifest = std::fs::read_to_string(path.with_extension("scene.json")).ok();
    load_slice(&bytes, path.parent(), manifest.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIANGLE_POSITIONS: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

    fn encode_base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut text = String::new();
        for chunk in bytes.chunks(3) {
            let word = chunk
                .iter()
                .enumerate()
                .fold(0_u32, |word, (index, byte)| word | u32::from(*byte) << (16 - 8 * index));
            for sextet in 0..=chunk.len() {
                text.push(ALPHABET[(word >> (18 - 6 * sextet) & 63) as usize] as char);
            }
            text.extend(std::iter::repeat_n('=', 3 - chunk.len()));
        }
        text
    }

    fn white_pixel_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        writer.write_image_data(&[255; 4]).expect("pixels");
        writer.finish().expect("finish");
        bytes
    }

    // One triangle, one material with a base color texture, one directional light and one camera, all inline.
    fn synthetic_gltf() -> String {
        let mut buffer: Vec<u8> = bytemuck::cast_slice(&TRIANGLE_POSITIONS).to_vec();
        buffer.extend_from_slice(bytemuck::cast_slice(&[0_u16, 1, 2, 0]));
        let buffer_uri = format!("data:application/octet-stream;base64,{}", encode_base64(&buffer));
        let image_uri = format!("data:image/png;base64,{}", encode_base64(&white_pixel_png()));
        format!(
            r#"{{
            "asset": {{"version": "2.0"}},
            "extensionsUsed": ["KHR_lights_punctual", "KHR_materials_clearcoat"],
            "extensions": {{"KHR_lights_punctual": {{"lights": [{{"type": "directional", "intensity": 5.0}}]}}}},
            "scene": 0,
            "scenes": [{{"nodes": [0, 1, 2]}}],
            "nodes": [
                {{"mesh": 0, "translation": [1, 2, 3]}},
                {{"camera": 0, "name": "eye"}},
                {{"extensions": {{"KHR_lights_punctual": {{"light": 0}}}}}}
            ],
            "cameras": [{{"type": "perspective", "perspective": {{"yfov": 0.8, "znear": 0.1}}}}],
            "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0}}, "indices": 1, "material": 0}}]}}],
            "materials": [{{
                "pbrMetallicRoughness": {{"baseColorFactor": [0.5, 0.5, 0.5, 1], "metallicFactor": 0, "baseColorTexture": {{"index": 0}}}},
                "extensions": {{"KHR_materials_clearcoat": {{"clearcoatFactor": 0.7}}}}
            }}],
            "textures": [{{"source": 0}}],
            "images": [{{"uri": "{image_uri}"}}],
            "buffers": [{{"byteLength": {length}, "uri": "{buffer_uri}"}}],
            "bufferViews": [
                {{"buffer": 0, "byteOffset": 0, "byteLength": 36}},
                {{"buffer": 0, "byteOffset": 36, "byteLength": 6}}
            ],
            "accessors": [
                {{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]}},
                {{"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"}}
            ]
        }}"#,
            length = buffer.len()
        )
    }

    #[test]
    fn a_synthetic_scene_loads_and_counts_its_resources() {
        let scene = load_slice(synthetic_gltf().as_bytes(), None, None).expect("loads");

        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.meshes[0].indices, [0, 1, 2]);
        assert_eq!(scene.meshes[0].surface.len(), 3);
        assert_eq!(scene.images.len(), 1);
        assert_eq!(scene.instances.len(), 1);
        assert_eq!(scene.lights.len(), 1);
        assert_eq!(scene.cameras.len(), 1);
        assert_eq!(scene.cameras[0].name, "eye");
        assert_eq!(scene.main_camera, Some(0));
        assert_eq!(scene.triangle_count(), 1);
        // The material of the primitive plus the shared default.
        assert_eq!(scene.materials.len(), 2);
        let material = &scene.materials[0];
        assert_eq!(material.standard.metallic, 0.0);
        assert!((material.standard.clearcoat - 0.7).abs() < 1e-6);
        assert_eq!(
            material.textures[TextureRole::BaseColor as usize].map(|texture| texture.image),
            Some(0)
        );
        assert!(material.textures[TextureRole::Normal as usize].is_none());
        assert_eq!(scene.meshes[0].material, 0);
    }

    #[test]
    fn the_node_translation_reaches_the_instance_and_the_bounds() {
        let scene = load_slice(synthetic_gltf().as_bytes(), None, None).expect("loads");

        let (min, max) = scene.bounds().expect("one instance");

        assert_eq!(min, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(max, Vec3::new(2.0, 3.0, 3.0));
    }

    #[test]
    fn light_intensity_becomes_lux() {
        let scene = load_slice(synthetic_gltf().as_bytes(), None, None).expect("loads");

        assert_eq!(scene.lights[0].illuminance_lux(), Some(5.0));
    }

    #[test]
    fn the_manifest_picks_the_main_camera() {
        let manifest = r#"{"cameras": [{"name": "other", "main": false}, {"name": "eye", "main": true}]}"#;
        assert_eq!(main_camera_name(manifest).as_deref(), Some("eye"));
        assert_eq!(main_camera_name("not json"), None);
    }

    #[test]
    fn percent_escapes_in_uris_are_decoded() {
        assert_eq!(percent_decode("my%20file%2Ebin"), "my file.bin");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn a_file_without_triangles_is_an_error() {
        let json = r#"{"asset": {"version": "2.0"}, "scenes": [{"nodes": []}], "scene": 0}"#;
        assert!(matches!(
            load_slice(json.as_bytes(), None, None),
            Err(LoadError::NoGeometry)
        ));
    }

    // Opt-in with `PATHON_TEST_SCENE` (docs/scenes.md): the big scenes are local files, not part of the repository.
    #[test]
    fn the_test_scene_loads_when_it_is_available() {
        let Some(path) = std::env::var_os("PATHON_TEST_SCENE") else {
            eprintln!("PATHON_TEST_SCENE is not set: skipping the test scene");
            return;
        };
        let scene = load_path(Path::new(&path)).expect("the test scene loads");
        assert!(scene.triangle_count() > 100_000, "{} triangles", scene.triangle_count());
        assert!(!scene.lights.is_empty() && !scene.cameras.is_empty());
        assert!(scene.main_camera.is_some());
        assert!(
            scene
                .images
                .iter()
                .all(|image| image.rgba.len() == image.width as usize * image.height as usize * 4)
        );
    }
}
