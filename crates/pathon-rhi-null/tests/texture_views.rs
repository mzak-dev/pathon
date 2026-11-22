use pathon_rhi_null::NullDevice;
use pathon_rhi_types::{
    Access, Color, ColorAttachment, CommandList, DeviceApi, Format, RenderPassDesc, RhiError, SubresourceRange,
    TextureDesc, TextureDimension, TextureId, TextureUsage, TextureViewDesc, TextureViewDimension,
};

const USAGE: TextureUsage = TextureUsage::SAMPLED.union(TextureUsage::RENDER_TARGET);

fn code<T>(result: Result<T, RhiError>) -> &'static str {
    match result {
        Ok(_) => panic!("validation should fail"),
        Err(error) => error.code(),
    }
}

fn texture(device: &NullDevice, desc: &TextureDesc) -> TextureId {
    device.create_texture(desc).expect("texture")
}

fn cube_texture(device: &NullDevice, layers: u32) -> TextureId {
    texture(
        device,
        &TextureDesc::new_2d("cube", 8, 8, Format::Rgba8Unorm, USAGE).with_layers(layers),
    )
}

fn view_of(dimension: TextureViewDimension) -> TextureViewDesc {
    TextureViewDesc {
        dimension: Some(dimension),
        ..TextureViewDesc::default()
    }
}

#[test]
fn six_square_layers_make_a_cube_view() {
    let device = NullDevice::new();
    let cube = texture(&device, &TextureDesc::new_cube("cube", 8, Format::Rgba8Unorm, USAGE));

    device
        .create_texture_view(cube, &TextureViewDesc::cube())
        .expect("cube");
    device
        .create_texture_view(cube, &view_of(TextureViewDimension::D2Array))
        .expect("array of faces");
    device
        .create_texture_view(cube, &TextureViewDesc::layer(3))
        .expect("single face");
}

#[test]
fn a_cube_view_needs_exactly_six_layers() {
    let device = NullDevice::new();
    let twelve = cube_texture(&device, 12);

    device
        .create_texture_view(twelve, &TextureViewDesc::cube())
        .expect("a cube view defaults to the first six layers");
    let all_twelve = TextureViewDesc {
        layer_count: Some(12),
        ..TextureViewDesc::cube()
    };
    assert_eq!(code(device.create_texture_view(twelve, &all_twelve)), "R0109");
    device
        .create_texture_view(
            twelve,
            &TextureViewDesc {
                layer_count: Some(6),
                base_layer: 6,
                ..TextureViewDesc::cube()
            },
        )
        .expect("second cube of the array");
    assert_eq!(
        code(device.create_texture_view(cube_texture(&device, 5), &TextureViewDesc::cube())),
        "R0109"
    );
}

#[test]
fn a_cube_array_view_needs_a_multiple_of_six_layers() {
    let device = NullDevice::new();
    let two_cubes = cube_texture(&device, 12);
    let seven = cube_texture(&device, 7);

    device
        .create_texture_view(two_cubes, &view_of(TextureViewDimension::CubeArray))
        .expect("two cubes");
    assert_eq!(
        code(device.create_texture_view(seven, &view_of(TextureViewDimension::CubeArray))),
        "R0109"
    );
}

#[test]
fn a_cube_view_of_a_non_square_texture_is_rejected() {
    let device = NullDevice::new();
    let wide = texture(
        &device,
        &TextureDesc::new_2d("wide", 8, 4, Format::Rgba8Unorm, USAGE).with_layers(6),
    );

    assert_eq!(
        code(device.create_texture_view(wide, &TextureViewDesc::cube())),
        "R0109"
    );
}

#[test]
fn a_plain_2d_view_covers_one_layer() {
    let device = NullDevice::new();
    let cube = cube_texture(&device, 6);

    device
        .create_texture_view(cube, &view_of(TextureViewDimension::D2))
        .expect("first face by default");
    assert_eq!(
        code(device.create_texture_view(
            cube,
            &TextureViewDesc {
                layer_count: Some(2),
                ..view_of(TextureViewDimension::D2)
            }
        )),
        "R0109"
    );
}

#[test]
fn a_view_dimension_must_match_the_texture_dimension() {
    let device = NullDevice::new();
    let flat = texture(&device, &TextureDesc::new_2d("flat", 8, 8, Format::Rgba8Unorm, USAGE));

    assert_eq!(
        code(device.create_texture_view(flat, &view_of(TextureViewDimension::D3))),
        "R0109"
    );
    assert_eq!(
        code(device.create_texture_view(flat, &view_of(TextureViewDimension::D1))),
        "R0109"
    );
    let volume = texture(
        &device,
        &TextureDesc {
            dimension: TextureDimension::D3,
            ..TextureDesc::new_2d("volume", 4, 4, Format::Rgba8Unorm, TextureUsage::SAMPLED).with_layers(4)
        },
    );
    device
        .create_texture_view(volume, &view_of(TextureViewDimension::D3))
        .expect("volume view");
    assert_eq!(
        code(device.create_texture_view(volume, &view_of(TextureViewDimension::D2Array))),
        "R0109"
    );
}

#[test]
fn a_view_range_reaching_past_the_texture_is_r0112() {
    let device = NullDevice::new();
    let cube = texture(
        &device,
        &TextureDesc::new_cube("cube", 8, Format::Rgba8Unorm, USAGE).with_mips(3),
    );

    let past_layers = TextureViewDesc {
        base_layer: 4,
        layer_count: Some(3),
        ..view_of(TextureViewDimension::D2Array)
    };
    assert_eq!(code(device.create_texture_view(cube, &past_layers)), "R0112");
    let past_mips = TextureViewDesc {
        base_mip: 2,
        mip_count: Some(2),
        ..TextureViewDesc::cube()
    };
    assert_eq!(code(device.create_texture_view(cube, &past_mips)), "R0112");
    let first_face_after_the_end = TextureViewDesc::layer(6);
    assert_eq!(
        code(device.create_texture_view(cube, &first_face_after_the_end)),
        "R0112"
    );
}

#[test]
fn view_formats_allow_the_srgb_twin_only() {
    let device = NullDevice::new();
    let reinterpretable = texture(
        &device,
        &TextureDesc::new_2d("albedo", 4, 4, Format::Rgba8Unorm, USAGE).with_view_formats(&[Format::Rgba8UnormSrgb]),
    );

    device
        .create_texture_view(reinterpretable, &TextureViewDesc::with_format(Format::Rgba8UnormSrgb))
        .expect("declared twin");
    device
        .create_texture_view(reinterpretable, &TextureViewDesc::with_format(Format::Rgba8Unorm))
        .expect("the texture's own format needs no declaration");
    assert_eq!(
        code(device.create_texture_view(reinterpretable, &TextureViewDesc::with_format(Format::Bgra8Unorm))),
        "R0109"
    );

    let plain = texture(&device, &TextureDesc::new_2d("plain", 4, 4, Format::Rgba8Unorm, USAGE));
    assert_eq!(
        code(device.create_texture_view(plain, &TextureViewDesc::with_format(Format::Rgba8UnormSrgb))),
        "R0109",
        "an undeclared view format must be rejected even when it is a legal twin"
    );
}

#[test]
fn a_view_format_of_another_family_is_rejected_at_texture_creation() {
    let device = NullDevice::new();
    let incompatible =
        TextureDesc::new_2d("bad", 4, 4, Format::Rgba8Unorm, USAGE).with_view_formats(&[Format::R32Float]);

    assert_eq!(code(device.create_texture(&incompatible)), "R0109");
}

#[test]
fn a_cube_face_view_can_be_rendered_to_after_its_barrier() {
    let device = NullDevice::new();
    let cube = texture(&device, &TextureDesc::new_cube("cube", 8, Format::Rgba8Unorm, USAGE));
    let mut list = CommandList::new();
    list.texture_access(cube, SubresourceRange::ALL, Access::empty(), Access::COLOR_ATTACHMENT);
    for face in 0..6 {
        let view = device
            .create_texture_view(cube, &TextureViewDesc::layer(face))
            .expect("face view");
        let _pass = list.render_pass(RenderPassDesc {
            label: "face",
            color: vec![ColorAttachment::clear(view, Color::BLACK)],
            depth: None,
            timestamps: None,
        });
    }
    list.texture_access(cube, SubresourceRange::ALL, Access::COLOR_ATTACHMENT, Access::SAMPLED);
    device.submit(list).expect("every face was cleared before sampling");
}
