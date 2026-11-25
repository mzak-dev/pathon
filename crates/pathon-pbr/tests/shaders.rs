// The WESL of the pack composes with the material, IBL and bindless modules, validates in naga and makes pipelines on
// the null device (which checks layouts against what the shaders declare) for every material permutation.

use pathon_api::rhi::BindGroupLayoutId;
use pathon_bindless::{BINDLESS_FEATURES, BinderDesc, PathPreference, TextureBinder};
use pathon_material::{MaterialHooks, MaterialVariant};
use pathon_mesh::VertexLayout;
use pathon_pbr::{ClusterPipeline, ForwardKey, PbrShaders, PrepassKey, TEXTURE_GROUP};
use pathon_rhi::{Features, NullDevice};
use pathon_shader::{ShaderFeatures, ShaderKey};

fn binder(device: &NullDevice, preference: PathPreference) -> TextureBinder {
    TextureBinder::new(
        device,
        &BinderDesc {
            preference,
            ..BinderDesc::default()
        },
    )
    .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()))
}

fn hooks_for(binder: &TextureBinder) -> MaterialHooks {
    let mut hooks = MaterialHooks::default();
    hooks.set_texture_source(binder.texture_source_wgsl(TEXTURE_GROUP));
    hooks
}

#[test]
fn every_roots_composes_and_validates_without_textures() {
    let mut shaders = PbrShaders::new(&MaterialHooks::default()).expect("modules register");
    for root in ["prepass", "forward", "cluster", "depth_pyramid"] {
        let key = ShaderKey {
            root: root.to_owned(),
            features: ShaderFeatures::default(),
        };
        shaders.wgsl(&key).unwrap_or_else(|error| panic!("{root}: {error}"));
    }
}

#[test]
fn forward_and_prepass_pipelines_exist_for_all_forty_eight_permutations_on_both_texture_paths() {
    let layouts = VertexLayout::standard().vertex_buffer_layouts();
    for (features, preference) in [
        (BINDLESS_FEATURES, PathPreference::Auto),
        (Features::empty(), PathPreference::Auto),
    ] {
        let device = NullDevice::with_features(features);
        let binder = binder(&device, preference);
        let mut shaders = PbrShaders::new(&hooks_for(&binder)).expect("modules register");
        let textures: Option<BindGroupLayoutId> = Some(binder.layout());

        for variant in MaterialVariant::all() {
            for split in [false, true] {
                shaders
                    .forward_pipeline(&device, ForwardKey { variant, split }, &layouts, textures)
                    .unwrap_or_else(|error| panic!("{variant:?} split {split}: {error}"));
            }
        }
        for (masked, full) in [(false, false), (true, false), (false, true), (true, true)] {
            shaders
                .prepass_pipeline(&device, PrepassKey { masked, full }, &layouts, textures)
                .unwrap_or_else(|error| panic!("masked {masked} full {full}: {error}"));
        }
    }
}

#[test]
fn the_pack_works_without_a_texture_binder() {
    let device = NullDevice::new();
    let layouts = VertexLayout::standard().vertex_buffer_layouts();
    let mut shaders = PbrShaders::new(&MaterialHooks::default()).expect("modules register");

    shaders
        .forward_pipeline(
            &device,
            ForwardKey {
                variant: MaterialVariant::default(),
                split: true,
            },
            &layouts,
            None,
        )
        .expect("opaque forward without textures");
    shaders
        .prepass_pipeline(
            &device,
            PrepassKey {
                masked: false,
                full: true,
            },
            &layouts,
            None,
        )
        .expect("prepass without textures");
}

#[test]
fn cluster_and_pyramid_pipelines_build() {
    let device = NullDevice::new();
    let mut shaders = PbrShaders::new(&MaterialHooks::default()).expect("modules register");

    ClusterPipeline::new(&device, &mut shaders).expect("cluster pipelines");
    shaders
        .compute_pipeline(&device, "depth_pyramid", "copy_depth", "copy", |_| {})
        .expect("copy depth");
    shaders
        .compute_pipeline(&device, "depth_pyramid", "downsample", "downsample", |desc| {
            for entry in &mut desc.entries {
                if let pathon_api::rhi::BindingType::SampledTexture { sample_type, .. } = &mut entry.ty {
                    *sample_type = pathon_api::rhi::TextureSampleType::UnfilterableFloat;
                }
            }
        })
        .expect("downsample");
}
