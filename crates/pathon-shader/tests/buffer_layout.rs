use pathon_shader::{MemberLayout, ShaderError, reflect_buffer_layout};

const SHADER: &str = "
struct Light {
    position: vec3<f32>,
    range: f32,
    color: vec3<f32>,
    kind: u32,
    direction: vec3<f32>,
}
struct Camera { view_projection: mat4x4<f32>, eye: vec3<f32> }

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> lights: array<Light>;
@group(1) @binding(0) var<storage, read_write> counters: array<vec4<u32>>;
@group(1) @binding(1) var albedo: texture_2d<f32>;

@compute @workgroup_size(1)
fn main() {
    counters[0] = vec4<u32>(lights[0].kind, u32(camera.eye.x), 0u, 0u);
    _ = albedo;
}
";

fn member(name: &str, offset: u32, size: u32) -> MemberLayout {
    MemberLayout {
        name: name.to_owned(),
        offset,
        size,
    }
}

#[test]
fn a_uniform_struct_reports_offsets_and_size() {
    let layout = reflect_buffer_layout(SHADER, 0, 0).expect("camera");

    assert_eq!(layout.element_stride, None);
    assert_eq!(layout.struct_size, Some(80));
    assert_eq!(
        layout.members,
        [member("view_projection", 0, 64), member("eye", 64, 12)]
    );
}

#[test]
fn a_storage_array_reports_the_element_struct_and_stride() {
    let layout = reflect_buffer_layout(SHADER, 0, 1).expect("lights");

    // A vec3 followed by a scalar shares one 16-byte slot; the struct pads to a multiple of 16.
    assert_eq!(layout.element_stride, Some(48));
    assert_eq!(layout.struct_size, Some(48));
    assert_eq!(
        layout.members,
        [
            member("position", 0, 12),
            member("range", 12, 4),
            member("color", 16, 12),
            member("kind", 28, 4),
            member("direction", 32, 12),
        ]
    );
}

#[test]
fn an_array_of_vectors_has_a_stride_and_no_members() {
    let layout = reflect_buffer_layout(SHADER, 1, 0).expect("counters");

    assert_eq!(layout.element_stride, Some(16));
    assert_eq!(layout.struct_size, None);
    assert_eq!(layout.members.len(), 0);
}

#[test]
fn a_texture_has_no_buffer_layout() {
    let error = reflect_buffer_layout(SHADER, 1, 1).expect_err("not a buffer");

    assert!(matches!(
        error,
        ShaderError::UnsupportedBinding {
            group: 1,
            binding: 1,
            ..
        }
    ));
}

#[test]
fn an_undeclared_binding_and_invalid_wgsl_are_errors() {
    assert!(matches!(
        reflect_buffer_layout(SHADER, 5, 0),
        Err(ShaderError::UnsupportedBinding { .. })
    ));
    assert!(matches!(
        reflect_buffer_layout("fn broken(", 0, 0),
        Err(ShaderError::InvalidWgsl { .. })
    ));
}
