// Layout, capacity, slot reuse, stale handles and the rebuild strategy of `TextureBinder` on the null device, whose
// validation follows the wgpu rules for binding arrays (ADR-0017).

mod common;

use common::{binder_on, null_device, texture_views};
use pathon_api::rhi::Features;
use pathon_bindless::{
    BINDLESS_FEATURES, BinderDesc, MAX_MATERIAL_TEXTURES, PathPreference, SamplerPreset, TextureBinder, TexturePath,
    decode_shader_value,
};
use pathon_rhi::NullDevice;

#[test]
fn the_bindless_group_is_rebuilt_once_for_a_batch_of_changes() {
    let device = null_device(true);
    let mut binder = binder_on(&device, PathPreference::Auto, 256);
    let views = texture_views(&device, 100);
    for view in &views {
        binder.register(*view, SamplerPreset::default()).expect("room");
    }
    assert_eq!(binder.group_builds(), 0, "registering builds nothing");

    binder.prepare(&device).expect("group");
    binder.prepare(&device).expect("group");
    assert_eq!(
        binder.group_builds(),
        1,
        "one rebuild for the whole batch, none when nothing changed"
    );

    let extra = texture_views(&device, 1);
    binder.register(extra[0], SamplerPreset::default()).expect("room");
    binder.prepare(&device).expect("group");
    assert_eq!(binder.group_builds(), 2);
}

#[test]
fn the_array_is_filled_to_the_capacity_with_the_placeholder() {
    // Without PARTIALLY_BOUND_BINDING_ARRAY the null device wants exactly `count` views, as wgpu does.
    let device = NullDevice::with_features(BINDLESS_FEATURES);
    let mut binder = binder_on(&device, PathPreference::Bindless, 1024);
    assert_eq!(binder.path(), TexturePath::Bindless { capacity: 1024 });

    let empty = binder.bind_group(&device, 0, &[]);

    assert!(empty.is_ok(), "an empty table still binds: {empty:?}");
}

#[test]
fn bind_group_is_the_same_object_for_every_material_on_the_bindless_path() {
    let device = null_device(true);
    let mut binder = binder_on(&device, PathPreference::Auto, 64);
    let views = texture_views(&device, 2);
    let handles: Vec<_> = views
        .iter()
        .map(|view| binder.register(*view, SamplerPreset::LinearRepeat).expect("room"))
        .collect();

    let first = binder.bind_group(&device, 0, &handles[..1]).expect("group");
    let second = binder.bind_group(&device, 7, &handles[1..]).expect("group");

    assert_eq!(first, second);
    assert_eq!(binder.group_builds(), 1);
    assert_eq!(binder.material_group_count(), 0);
}

#[test]
fn the_old_array_group_is_destroyed_only_after_the_frames_in_flight() {
    let device = null_device(true);
    let mut binder = binder_on(&device, PathPreference::Auto, 64);
    let views = texture_views(&device, 2);
    binder.begin_frame(&device, 5);
    binder.register(views[0], SamplerPreset::default()).expect("room");
    binder.prepare(&device).expect("group");
    binder.register(views[1], SamplerPreset::default()).expect("room");
    binder.prepare(&device).expect("group");
    assert_eq!(binder.pending_releases(), 1, "the first group waits");

    binder.begin_frame(&device, 6);
    assert_eq!(binder.pending_releases(), 1);
    binder.begin_frame(&device, 7);
    assert_eq!(binder.pending_releases(), 0);
}

#[test]
fn a_removed_texture_is_a_stale_handle_at_once_and_its_slot_comes_back_later() {
    let device = null_device(true);
    let mut binder = binder_on(&device, PathPreference::Auto, 64);
    let views = texture_views(&device, 3);
    binder.begin_frame(&device, 1);
    let first = binder.register(views[0], SamplerPreset::default()).expect("room");
    binder.register(views[1], SamplerPreset::default()).expect("room");

    assert_eq!(binder.unregister(first), Ok(views[0]));
    let error = binder.bind_group(&device, 0, &[first]).expect_err("stale");
    assert_eq!(error.code(), "B0004");
    assert_eq!(binder.shader_value(first).expect_err("stale").code(), "B0004");
    binder.begin_frame(&device, 3);
    let reused = binder.register(views[2], SamplerPreset::default()).expect("room");
    assert_eq!(reused.index(), first.index());
    assert!(
        binder.bind_group(&device, 0, &[first]).is_err(),
        "the old handle stays dead"
    );
    assert!(binder.bind_group(&device, 0, &[reused]).is_ok());
}

#[test]
fn a_full_table_is_b0003() {
    let device = null_device(true);
    let mut binder = binder_on(&device, PathPreference::Auto, 64);
    let views = texture_views(&device, 65);
    for view in &views[..64] {
        binder.register(*view, SamplerPreset::default()).expect("room");
    }

    let error = binder.register(views[64], SamplerPreset::default()).expect_err("full");

    assert_eq!(error.code(), "B0003");
    assert_eq!(error.to_diagnostic().code, "B0003");
}

#[test]
fn per_material_groups_are_cached_per_material_and_rebuilt_when_the_textures_change() {
    let device = null_device(false);
    let mut binder = binder_on(&device, PathPreference::Auto, 64);
    assert_eq!(binder.path(), TexturePath::PerMaterial);
    let views = texture_views(&device, 3);
    let handles: Vec<_> = views
        .iter()
        .map(|view| binder.register(*view, SamplerPreset::default()).expect("room"))
        .collect();

    let a = binder.bind_group(&device, 0, &handles[..2]).expect("group");
    let a_again = binder.bind_group(&device, 0, &handles[..2]).expect("group");
    let b = binder.bind_group(&device, 1, &handles[1..]).expect("group");
    assert_eq!(a, a_again, "cached");
    assert_ne!(a, b);
    assert_eq!((binder.material_group_count(), binder.group_builds()), (2, 2));

    let changed = binder.bind_group(&device, 0, &handles).expect("group");
    assert_ne!(changed, a, "the textures of material 0 changed");
    assert_eq!(binder.group_builds(), 3);
    assert_eq!(binder.pending_releases(), 2, "the old group and its slot buffer wait");
}

#[test]
fn unregistering_a_texture_drops_only_the_groups_that_hold_it() {
    let device = null_device(false);
    let mut binder = binder_on(&device, PathPreference::Auto, 64);
    let views = texture_views(&device, 3);
    let handles: Vec<_> = views
        .iter()
        .map(|view| binder.register(*view, SamplerPreset::default()).expect("room"))
        .collect();
    binder.bind_group(&device, 0, &handles[..2]).expect("group");
    binder.bind_group(&device, 1, &handles[2..]).expect("group");

    binder.unregister(handles[0]).expect("live");

    assert_eq!(binder.material_group_count(), 1);
    assert_eq!(
        binder.bind_group(&device, 0, &handles[..2]).expect_err("stale").code(),
        "B0004"
    );
}

#[test]
fn too_many_textures_for_one_material_is_b0005() {
    let device = null_device(false);
    let mut binder = binder_on(&device, PathPreference::Auto, 64);
    let views = texture_views(&device, MAX_MATERIAL_TEXTURES + 1);
    let handles: Vec<_> = views
        .iter()
        .map(|view| binder.register(*view, SamplerPreset::default()).expect("room"))
        .collect();

    assert!(binder.bind_group(&device, 0, &handles[..MAX_MATERIAL_TEXTURES]).is_ok());
    let error = binder.bind_group(&device, 0, &handles).expect_err("too many");
    assert_eq!(error.code(), "B0005");
}

#[test]
fn shader_values_are_the_same_on_both_paths() {
    let bindless_device = null_device(true);
    let web_device = null_device(false);
    let mut bindless = binder_on(&bindless_device, PathPreference::Auto, 64);
    let mut web = binder_on(&web_device, PathPreference::Auto, 64);
    let views = texture_views(&bindless_device, 2);
    let web_views = texture_views(&web_device, 2);

    let values = |binder: &mut TextureBinder, views: &[_]| -> Vec<u32> {
        views
            .iter()
            .zip([SamplerPreset::LinearRepeat, SamplerPreset::NearestClamp])
            .map(|(view, sampler)| {
                let handle = binder.register(*view, sampler).expect("room");
                binder.shader_value(handle).expect("live")
            })
            .collect()
    };

    let (a, b) = (values(&mut bindless, &views), values(&mut web, &web_views));
    assert_eq!(a, b);
    assert_eq!(decode_shader_value(a[1]), Some((1, SamplerPreset::NearestClamp)));
}

#[test]
fn forcing_bindless_on_a_web_device_is_b0002_and_auto_reports_b0001() {
    let device = null_device(false);

    let forced = TextureBinder::new(
        &device,
        &BinderDesc {
            preference: PathPreference::Bindless,
            ..BinderDesc::default()
        },
    )
    .expect_err("no binding arrays");
    let auto = binder_on(&device, PathPreference::Auto, 64);

    assert_eq!(forced.code(), "B0002");
    assert!(forced.to_diagnostic().hint.is_some());
    let warning = auto.fallback_diagnostic().expect("fell back");
    assert_eq!(warning.code, "B0001");
    assert!(
        binder_on(&null_device(true), PathPreference::Auto, 64)
            .fallback_diagnostic()
            .is_none()
    );
}

#[test]
fn release_destroys_everything_the_binder_created() {
    let device = null_device(false);
    let mut binder = binder_on(&device, PathPreference::Auto, 64);
    let views = texture_views(&device, 2);
    let handles: Vec<_> = views
        .iter()
        .map(|view| binder.register(*view, SamplerPreset::default()).expect("room"))
        .collect();
    binder.bind_group(&device, 0, &handles).expect("group");

    binder.release(&device);

    assert_eq!((binder.material_group_count(), binder.pending_releases()), (0, 0));
}

#[test]
fn bindless_is_unavailable_without_the_non_uniform_feature_alone() {
    let device = NullDevice::with_features(Features::TEXTURE_BINDING_ARRAY);

    let binder = binder_on(&device, PathPreference::Auto, 64);

    assert_eq!(binder.path(), TexturePath::PerMaterial);
}
