// M4-04: the same scene through the bindless path (one `binding_array`) and through a bind group per material (the Web
// profile), compared pixel by pixel and against one golden image. The null half runs everywhere and validates every
// command, binding and layout; the GPU half (`--features gpu-tests`) reads pixels back.
// `PATHON_TEST_BACKEND=vulkan|dx12|metal` picks the backend.

mod common;

#[cfg(feature = "gpu-tests")]
use common::scene::SIZE;
use common::scene::render;
use pathon_bindless::{BINDLESS_FEATURES, PathPreference, TexturePath};
use pathon_rhi::{Device, Features, NullDevice};

fn null_device(features: Features) -> Device {
    Device::Null(NullDevice::with_features(features))
}

#[test]
fn the_scene_validates_on_both_paths_and_the_material_data_is_the_same() {
    let desktop = render(&null_device(BINDLESS_FEATURES), PathPreference::Auto);
    let web = render(&null_device(Features::empty()), PathPreference::Auto);
    let forced = render(&null_device(BINDLESS_FEATURES), PathPreference::PerMaterial);

    assert!(matches!(desktop.path, TexturePath::Bindless { capacity: 1024 }));
    assert_eq!(web.path, TexturePath::PerMaterial);
    assert_eq!(forced.path, TexturePath::PerMaterial);
    assert_eq!(
        desktop.material_data, web.material_data,
        "logical slots: no path-specific data"
    );
    assert_eq!(desktop.material_data, forced.material_data);
    assert_eq!(
        (desktop.group_builds, desktop.material_groups),
        (1, 0),
        "six textures registered, one group built"
    );
    assert_eq!(
        (web.group_builds, web.material_groups),
        (3, 3),
        "one group per material, four objects"
    );
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use pathon_golden::{GoldenImage, Tolerance, announce_adapter, assert_golden, compare, gpu_or_skip};
    use pathon_rhi::{Backend, DeviceApi, DeviceRequest, RhiError};

    use super::*;

    fn backend_from_environment() -> Option<Backend> {
        match std::env::var("PATHON_TEST_BACKEND").ok()?.to_ascii_lowercase().as_str() {
            "vulkan" => Some(Backend::Vulkan),
            "dx12" => Some(Backend::Dx12),
            "metal" => Some(Backend::Metal),
            other => panic!("PATHON_TEST_BACKEND `{other}` is not vulkan, dx12 or metal"),
        }
    }

    // The same adapter twice: once asked for the binding-array features, once for nothing, which is what a Web
    // device reports in `Caps`. The path is chosen from `Caps` alone.
    fn device(test: &str, label: &'static str, required_features: Features) -> Option<Device> {
        let built = Device::build(&DeviceRequest {
            label,
            backend: backend_from_environment(),
            required_features,
            ..Default::default()
        });
        if let Err(RhiError::MissingFeatures(missing)) = &built {
            common::skip(test, *missing);
            return None;
        }
        let device = gpu_or_skip(test, built, |error| matches!(error, RhiError::NoAdapter(_)))?;
        announce_adapter(test, device.caps());
        Some(device)
    }

    fn golden(rendered: &common::scene::Rendered) -> GoldenImage {
        GoldenImage::from_rgba8(SIZE, SIZE, rendered.image.clone()).expect("a full image")
    }

    #[test]
    fn the_same_scene_renders_the_same_on_both_paths() {
        let test = "the_same_scene_renders_the_same_on_both_paths";
        let Some(bindless_device) = device(test, "bindless", BINDLESS_FEATURES) else {
            return;
        };
        let Some(web_device) = device(test, "per material", Features::empty()) else {
            return;
        };

        let bindless = render(&bindless_device, PathPreference::Auto);
        let per_material = render(&web_device, PathPreference::Auto);

        assert!(
            matches!(bindless.path, TexturePath::Bindless { .. }),
            "{:?}",
            bindless.path
        );
        assert_eq!(per_material.path, TexturePath::PerMaterial);
        assert_eq!(bindless.material_data, per_material.material_data);
        let (a, b) = (golden(&bindless), golden(&per_material));
        let difference = compare(&a, &b, Tolerance::new(1, 0.0)).expect("same size");
        assert_eq!(
            difference.differing_pixels, 0,
            "the paths differ by more than 1 in {} pixels, first at {:?}",
            difference.differing_pixels, difference.first_difference
        );
        let lit = a.as_rgba8().chunks(4).filter(|pixel| pixel[..3] != [0, 0, 0]).count();
        assert!(
            lit > SIZE as usize * SIZE as usize / 4,
            "the scene is mostly black: {lit} lit pixels"
        );
        assert_golden("bindless_scene", &a, Tolerance::cross_backend());
        assert_golden("bindless_scene", &b, Tolerance::cross_backend());
    }

    #[test]
    fn per_material_can_be_forced_on_a_device_that_could_do_bindless() {
        let test = "per_material_can_be_forced_on_a_device_that_could_do_bindless";
        let Some(device) = device(test, "forced per material", BINDLESS_FEATURES) else {
            return;
        };

        let automatic = render(&device, PathPreference::Auto);
        let forced = render(&device, PathPreference::PerMaterial);

        assert!(matches!(automatic.path, TexturePath::Bindless { .. }));
        assert_eq!(forced.path, TexturePath::PerMaterial);
        let difference = compare(&golden(&automatic), &golden(&forced), Tolerance::new(1, 0.0)).expect("same size");
        assert_eq!(
            difference.differing_pixels, 0,
            "first at {:?}",
            difference.first_difference
        );
    }
}
