// The WESL of the pack composes with the material, IBL and bindless modules, validates in naga and makes pipelines on
// the null device (which checks layouts against what the shaders declare) for every material permutation.

use pathon_api::rhi::BindGroupLayoutId;
use pathon_bindless::{BINDLESS_FEATURES, BinderDesc, PathPreference, TextureBinder};
use pathon_material::{MaterialHooks, MaterialVariant};
use pathon_mesh::VertexLayout;
use pathon_pbr::{
    ClusterPipeline, ForwardKey, LTC_BINDING, PbrShaders, PrepassKey, SHADOW_FIRST_BINDING, TEXTURE_GROUP,
};
use pathon_rhi::{Features, NullDevice};
use pathon_shader::{ShaderFeatures, ShaderKey, reflect_bind_groups};
use pathon_shadow::ShadowBindings;

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
            for (split, shadows, area) in [
                (false, false, false),
                (true, false, false),
                (false, true, false),
                (true, true, false),
                (false, false, true),
                (true, true, true),
            ] {
                shaders
                    .forward_pipeline(
                        &device,
                        ForwardKey {
                            variant,
                            split,
                            shadows,
                            area,
                        },
                        &layouts,
                        textures,
                    )
                    .unwrap_or_else(|error| {
                        panic!("{variant:?} split {split} shadows {shadows} area {area}: {error}")
                    });
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
                shadows: false,
                area: false,
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
fn the_shadow_permutation_adds_the_three_shadow_bindings_and_nothing_else() {
    let mut shaders = PbrShaders::new(&MaterialHooks::default()).expect("modules register");
    let bindings = |shaders: &mut PbrShaders, shadows: bool| {
        let key = ForwardKey {
            variant: MaterialVariant::default(),
            split: false,
            shadows,
            area: false,
        }
        .shader_key();
        let wgsl = shaders
            .wgsl(&key)
            .unwrap_or_else(|error| panic!("shadows {shadows}: {error}"));
        let group = reflect_bind_groups(&wgsl, &["vs", "fs"]).expect("reflection").remove(0);
        group.entries.iter().map(|entry| entry.binding).collect::<Vec<_>>()
    };

    let without = bindings(&mut shaders, false);
    let with = bindings(&mut shaders, true);

    assert_eq!(without, (0..SHADOW_FIRST_BINDING).collect::<Vec<_>>());
    assert_eq!(
        with,
        (0..SHADOW_FIRST_BINDING + ShadowBindings::COUNT).collect::<Vec<_>>()
    );
}

#[test]
fn the_area_permutation_adds_the_ltc_table_after_the_shadow_bindings() {
    let mut shaders = PbrShaders::new(&MaterialHooks::default()).expect("modules register");
    let bindings = |shaders: &mut PbrShaders, shadows: bool, area: bool| {
        let key = ForwardKey {
            variant: MaterialVariant::default(),
            split: false,
            shadows,
            area,
        }
        .shader_key();
        let wgsl = shaders
            .wgsl(&key)
            .unwrap_or_else(|error| panic!("shadows {shadows} area {area}: {error}"));
        let group = reflect_bind_groups(&wgsl, &["vs", "fs"]).expect("reflection").remove(0);
        group.entries.iter().map(|entry| entry.binding).collect::<Vec<_>>()
    };

    let without = bindings(&mut shaders, false, false);
    let with = bindings(&mut shaders, false, true);
    let with_both = bindings(&mut shaders, true, true);

    assert_eq!(without, (0..SHADOW_FIRST_BINDING).collect::<Vec<_>>());
    assert_eq!(with, without.iter().copied().chain([LTC_BINDING]).collect::<Vec<_>>());
    assert_eq!(
        with_both,
        (0..SHADOW_FIRST_BINDING + ShadowBindings::COUNT)
            .chain([LTC_BINDING])
            .collect::<Vec<_>>()
    );
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
