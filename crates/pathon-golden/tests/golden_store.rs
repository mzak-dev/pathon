use std::fs;
use std::path::{Path, PathBuf};

use pathon_golden::{GoldenError, GoldenImage, GoldenStore, Outcome, Tolerance, compare, diff_image, read_texture};
use pathon_rhi::{Access, Device, DeviceApi, Extent3d, Format, TexelLayout, TextureCopy, TextureDesc, TextureUsage};

fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("golden").join(test);
    drop(fs::remove_dir_all(&dir));
    dir
}

fn store(test: &str) -> GoldenStore {
    let dir = scratch(test);
    GoldenStore::new(dir.join("references"), dir.join("artifacts"))
}

fn solid(width: u32, height: u32, rgba: [u8; 4]) -> GoldenImage {
    let bytes = rgba.repeat((width * height) as usize);
    GoldenImage::from_rgba8(width, height, bytes).expect("image")
}

fn with_pixel(image: &GoldenImage, x: u32, y: u32, rgba: [u8; 4]) -> GoldenImage {
    let mut bytes = image.as_rgba8().to_vec();
    let start = ((y * image.width() + x) * 4) as usize;
    bytes[start..start + 4].copy_from_slice(&rgba);
    GoldenImage::from_rgba8(image.width(), image.height(), bytes).expect("image")
}

#[test]
fn update_mode_creates_the_reference_and_the_next_run_matches_exactly() {
    let image = with_pixel(&solid(8, 8, [10, 20, 30, 255]), 3, 4, [200, 0, 0, 255]);
    let updating = store("create").with_update(true);
    let created = updating.check("scene", &image, Tolerance::exact()).expect("created");
    assert!(matches!(created, Outcome::Created(_)), "{created:?}");

    let checking = updating.with_update(false);
    let outcome = checking.check("scene", &image, Tolerance::exact()).expect("matches");
    assert!(
        matches!(outcome, Outcome::Matched(c) if c.is_identical()),
        "{outcome:?}"
    );
}

#[test]
fn a_missing_reference_is_an_error_that_names_the_update_variable() {
    let error = store("missing")
        .check("scene", &solid(4, 4, [0; 4]), Tolerance::exact())
        .expect_err("no reference");
    assert!(matches!(error, GoldenError::MissingReference { .. }));
    assert!(error.to_string().contains("PATHON_UPDATE_GOLDEN=1"), "{error}");
}

#[test]
fn a_deliberately_changed_image_is_rejected_with_a_readable_report_and_artifacts() {
    let reference = solid(16, 16, [0, 0, 255, 255]);
    let store = store("negative");
    store
        .clone()
        .with_update(true)
        .check("cube", &reference, Tolerance::exact())
        .expect("reference");
    let tampered = with_pixel(&reference, 5, 9, [255, 0, 0, 255]);

    let error = store
        .check("cube", &tampered, Tolerance::exact())
        .expect_err("a changed pixel must fail");

    let GoldenError::Mismatch(mismatch) = &error else {
        panic!("expected a mismatch, got {error:?}");
    };
    assert_eq!(mismatch.comparison.differing_pixels, 1);
    assert_eq!(mismatch.comparison.max_channel_delta, 255);
    let report = error.to_string();
    for expected in [
        "\"cube\" does not match",
        "1 of 256 pixels differ",
        "first differing pixel (5, 9)",
        "cube.actual.png",
        "cube.diff.png",
        "PATHON_UPDATE_GOLDEN=1",
    ] {
        assert!(report.contains(expected), "missing {expected:?} in:\n{report}");
    }
    assert!(mismatch.artifacts.actual.is_file() && mismatch.artifacts.expected.is_file());
    let diff = decode(&mismatch.artifacts.diff);
    assert_eq!(
        diff.pixel(5, 9),
        [255, 0, 0, 255],
        "the differing pixel is red in the diff image"
    );
}

fn decode(path: &Path) -> GoldenImage {
    let decoder = png::Decoder::new(std::io::BufReader::new(fs::File::open(path).expect("open")));
    let mut reader = decoder.read_info().expect("png header");
    let mut buffer = vec![0; reader.output_buffer_size().expect("size")];
    let info = reader.next_frame(&mut buffer).expect("png data");
    buffer.truncate(info.buffer_size());
    GoldenImage::from_rgba8(info.width, info.height, buffer).expect("rgba")
}

#[test]
fn the_tolerance_bounds_the_channel_delta_and_the_fraction_of_differing_pixels() {
    let reference = solid(10, 10, [100, 100, 100, 255]);
    let slightly_off = with_pixel(&reference, 0, 0, [105, 100, 100, 255]);
    let far_off = with_pixel(&reference, 0, 0, [140, 100, 100, 255]);
    let store = store("tolerance");
    store
        .clone()
        .with_update(true)
        .check("grey", &reference, Tolerance::exact())
        .expect("reference");

    assert!(
        store.check("grey", &slightly_off, Tolerance::new(5, 0.0)).is_ok(),
        "delta within the bound"
    );
    assert!(
        store.check("grey", &slightly_off, Tolerance::exact()).is_err(),
        "exact rejects any delta"
    );
    assert!(
        store.check("grey", &far_off, Tolerance::new(5, 0.0)).is_err(),
        "delta beyond the bound"
    );
    assert!(
        store.check("grey", &far_off, Tolerance::new(5, 0.01)).is_ok(),
        "1 of 100 pixels is within 1%"
    );
    assert!(
        store.check("grey", &far_off, Tolerance::new(5, 0.005)).is_err(),
        "1 of 100 pixels is beyond 0.5%"
    );
}

#[test]
fn another_resolution_is_reported_with_both_sizes() {
    let store = store("size");
    store
        .clone()
        .with_update(true)
        .check("small", &solid(4, 4, [1; 4]), Tolerance::exact())
        .expect("reference");

    let error = store
        .check("small", &solid(8, 4, [1; 4]), Tolerance::exact())
        .expect_err("size differs");

    let report = error.to_string();
    assert!(report.contains("reference is 4x4, actual is 8x4"), "{report}");
    assert!(report.contains("small.actual.png"), "{report}");
}

#[test]
fn a_variant_reference_takes_precedence_and_updates_write_to_the_variant_directory() {
    let base = store("variant");
    let shared = solid(4, 4, [10, 10, 10, 255]);
    let special = solid(4, 4, [200, 10, 10, 255]);
    base.clone()
        .with_update(true)
        .check("img", &shared, Tolerance::exact())
        .expect("shared");

    let lavapipe = base.clone().with_variant("lavapipe");
    assert!(
        lavapipe.check("img", &shared, Tolerance::exact()).is_ok(),
        "falls back to the shared reference"
    );
    let updated = lavapipe
        .clone()
        .with_update(true)
        .check("img", &special, Tolerance::exact())
        .expect("variant");
    assert!(matches!(updated, Outcome::Created(path) if path.ends_with("lavapipe/img.png")));

    assert!(
        lavapipe.check("img", &special, Tolerance::exact()).is_ok(),
        "the variant wins"
    );
    assert!(
        base.check("img", &shared, Tolerance::exact()).is_ok(),
        "other backends keep the shared one"
    );
}

#[test]
fn update_mode_leaves_an_identical_reference_alone_and_rewrites_a_different_one() {
    let updating = store("rewrite").with_update(true);
    let first = solid(4, 4, [1, 2, 3, 255]);
    updating.check("img", &first, Tolerance::exact()).expect("created");

    let again = updating.check("img", &first, Tolerance::exact()).expect("unchanged");
    assert!(matches!(again, Outcome::Matched(_)), "{again:?}");
    let rewritten = updating
        .check("img", &solid(4, 4, [9, 9, 9, 255]), Tolerance::exact())
        .expect("rewritten");
    assert!(matches!(rewritten, Outcome::Updated(_)), "{rewritten:?}");
}

#[test]
fn names_cannot_leave_the_golden_directory() {
    for name in ["", "../escape", "a/b", "a.b", "a b"] {
        let error = store("names")
            .check(name, &solid(1, 1, [0; 4]), Tolerance::exact())
            .expect_err("invalid name");
        assert!(matches!(error, GoldenError::InvalidName(_)), "{name:?}: {error:?}");
    }
}

#[test]
fn an_image_needs_matching_bytes() {
    assert!(GoldenImage::from_rgba8(2, 2, vec![0; 15]).is_err());
    assert!(GoldenImage::from_rgba8(0, 2, Vec::new()).is_err());
    assert!(GoldenImage::from_rgba8(2, 2, vec![0; 16]).is_ok());
}

#[test]
fn the_diff_image_marks_pixels_within_and_beyond_the_tolerance() {
    let reference = solid(3, 1, [100, 100, 100, 255]);
    let mut changed = with_pixel(&reference, 1, 0, [103, 100, 100, 255]);
    changed = with_pixel(&changed, 2, 0, [200, 100, 100, 255]);

    let diff = diff_image(&reference, &changed, Tolerance::new(5, 0.0)).expect("diff");

    assert_eq!(diff.pixel(1, 0), [0, 128, 255, 255], "within tolerance");
    assert_eq!(diff.pixel(2, 0), [255, 0, 0, 255], "beyond tolerance");
    assert_eq!(diff.pixel(0, 0)[3], 255);
    let comparison = compare(&reference, &changed, Tolerance::new(5, 0.0)).expect("comparison");
    assert_eq!((comparison.differing_pixels, comparison.max_channel_delta), (1, 100));
}

#[test]
fn a_texture_read_back_has_the_texture_size_and_four_bytes_per_pixel() {
    let device = Device::null();
    let texture = device
        .create_texture(&TextureDesc::new_2d(
            "readback source",
            10,
            3,
            Format::Bgra8Unorm,
            TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
        ))
        .expect("texture");
    device
        .write_texture(
            &TextureCopy::whole(texture),
            &[0; 10 * 3 * 4],
            TexelLayout {
                offset: 0,
                bytes_per_row: 40,
                rows_per_image: 3,
            },
            Extent3d::new_2d(10, 3),
        )
        .expect("write");

    let image = read_texture(&device, texture, Access::COPY_DST).expect("read back");

    assert_eq!((image.width(), image.height()), (10, 3));
    assert_eq!(image.as_rgba8().len(), 10 * 3 * 4);
}

#[test]
fn only_eight_bit_color_textures_can_be_read_back() {
    let device = Device::null();
    let texture = device
        .create_texture(&TextureDesc::new_2d(
            "depth",
            4,
            4,
            Format::Depth32Float,
            TextureUsage::COPY_SRC | TextureUsage::RENDER_TARGET,
        ))
        .expect("texture");

    let error = read_texture(&device, texture, Access::COPY_SRC).expect_err("unsupported");

    assert!(
        matches!(error, GoldenError::UnsupportedReadback(Format::Depth32Float)),
        "{error:?}"
    );
}

#[test]
fn a_real_setup_error_is_never_a_skip() {
    let outcome = std::panic::catch_unwind(|| pathon_golden::gpu_or_skip::<(), _>("test", Err("boom"), |_| false));
    assert!(outcome.is_err());
    assert_eq!(pathon_golden::gpu_or_skip::<_, &str>("test", Ok(7), |_| false), Some(7));
}
