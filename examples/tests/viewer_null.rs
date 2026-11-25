// The packs the viewer assembles, validated on the null device (every barrier, binding and command is checked) for
// both lightings and both texture paths, without a GPU.

mod viewer_scene;

use pathon::api::Severity;
use pathon::api::glam::Vec3;
use pathon::rhi::{Backend, Format, PhysicalSize};
use pathon_bindless::PathPreference;
use pathon_examples::viewer::session::Session;
use pathon_examples::viewer::settings::LightingMode;
use pathon_examples::viewer::stack::ViewerConfig;
use viewer_scene::{SIZE, red_quad_scene};

fn render_one_frame(lighting: LightingMode, texture_path: PathPreference) -> Vec<String> {
    let config = ViewerConfig {
        lighting,
        texture_path,
        backend: Some(Backend::Null),
        ..ViewerConfig::default()
    };
    let renderer = config.builder().build().unwrap_or_else(|error| panic!("{error}"));
    let mut session = Session::new(renderer, &red_quad_scene(), &config).unwrap_or_else(|error| panic!("{error}"));
    session.camera.position = Vec3::new(0.0, 0.0, 3.0);
    let target = session
        .renderer
        .add_offscreen_target(PhysicalSize::new(SIZE, SIZE), Format::Rgba8UnormSrgb)
        .expect("target");
    let report = session.render(target, 1.0).unwrap_or_else(|error| panic!("{error}"));
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(ToString::to_string)
        .collect()
}

#[test]
fn the_pbr_stack_validates_with_bindless_textures() {
    assert_eq!(render_one_frame(LightingMode::Pbr, PathPreference::Auto), Vec::<String>::new());
}

#[test]
fn the_pbr_stack_validates_with_a_bind_group_per_material() {
    assert_eq!(
        render_one_frame(LightingMode::Pbr, PathPreference::PerMaterial),
        Vec::<String>::new()
    );
}

#[test]
fn the_simple_lighting_validates() {
    assert_eq!(
        render_one_frame(LightingMode::Simple, PathPreference::Auto),
        Vec::<String>::new()
    );
}
