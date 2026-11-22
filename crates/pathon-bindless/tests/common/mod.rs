#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

pub mod scene;

use pathon_api::rhi::{BindGroupLayoutDesc, Features, ShaderStages};
use pathon_bindless::{BINDLESS_FEATURES, BinderDesc, PathPreference, TextureBinder};
use pathon_material::MaterialHooks;
use pathon_rhi::NullDevice;
use pathon_shader::{ShaderKey, ShaderLibrary, reflect_bind_groups};

pub const ROOT_NAME: &str = "texture probe";

// Every texture slot of the material is read through `load_surface`, so the whole texture source is reachable.
const ROOT: &str = "
import package::material::standard::{MaterialData, load_surface};
import package::material::surface::SurfaceInput;

@group(0) @binding(0) var<storage, read> materials: array<MaterialData>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    var input: SurfaceInput;
    input.normal = vec3<f32>(0.0, 0.0, 1.0);
    input.tangent = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    input.uv = position.xy * 0.01;
    input.uv_dx = vec2<f32>(0.01, 0.0);
    input.uv_dy = vec2<f32>(0.0, 0.01);
    input.color = vec4<f32>(1.0);
    input.front_facing = true;
    let surface = load_surface(materials[0], input);
    return vec4<f32>(surface.diffuse_color * surface.occlusion + surface.emissive + surface.normal, surface.alpha);
}
";

pub fn fragment_key() -> ShaderKey {
    pathon_material::MaterialVariant::default().shader_key(ROOT_NAME)
}

/// A device that has the bindless features or not, like the Desktop and Web profiles.
pub fn null_device(bindless: bool) -> NullDevice {
    NullDevice::with_features(if bindless { BINDLESS_FEATURES } else { Features::empty() })
}

pub fn binder_on(device: &NullDevice, preference: PathPreference, capacity: u32) -> TextureBinder {
    TextureBinder::new(
        device,
        &BinderDesc {
            preference,
            capacity,
            visibility: ShaderStages::FRAGMENT,
            ..BinderDesc::default()
        },
    )
    .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
}

fn library_with(hooks: &MaterialHooks) -> ShaderLibrary {
    let mut library = ShaderLibrary::new();
    for (path, source) in hooks.modules() {
        library.add_module(path, source).expect("material module path");
    }
    library.add_root(ROOT_NAME, ROOT);
    library
}

pub fn default_library() -> ShaderLibrary {
    library_with(&MaterialHooks::default())
}

/// The material modules with the binder's texture source in `group`.
pub fn material_library(binder: &TextureBinder, group: u32) -> ShaderLibrary {
    let mut hooks = MaterialHooks::default();
    hooks.set_texture_source(binder.texture_source_wgsl(group));
    hooks
        .validate()
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    library_with(&hooks)
}

pub fn reflected_layout(wgsl: &str) -> Vec<BindGroupLayoutDesc> {
    reflect_bind_groups(wgsl, &["fs"]).expect("reflection")
}

/// `count` 1x1 textures with their views; the null device does not run anything, a texture only needs to exist.
pub fn texture_views(device: &dyn pathon_api::rhi::DeviceApi, count: usize) -> Vec<pathon_api::rhi::TextureViewId> {
    use pathon_api::rhi::{Format, TextureDesc, TextureUsage, TextureViewDesc};
    (0..count)
        .map(|_| {
            let texture = device
                .create_texture(&TextureDesc::new_2d(
                    "t",
                    1,
                    1,
                    Format::Rgba8Unorm,
                    TextureUsage::SAMPLED,
                ))
                .expect("texture");
            device
                .create_texture_view(texture, &TextureViewDesc::default())
                .expect("view")
        })
        .collect()
}

/// A device without the requested features is not a failure of the code under test: the test says so and stops.
#[expect(
    clippy::print_stderr,
    reason = "a skipped test has to be visible in the log of a CI run"
)]
pub fn skip(test: &str, missing: Features) {
    eprintln!("SKIPPED {test}: the adapter lacks {missing:?}");
}
