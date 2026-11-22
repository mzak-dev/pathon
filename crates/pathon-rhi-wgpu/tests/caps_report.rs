#![cfg(feature = "gpu-tests")]
#![expect(clippy::print_stdout, reason = "the Caps report is printed for the test log")]

// M0-08 / M1-01: `Caps` must agree with what raw wgpu says about the same adapter and device, both for a device pathon
// creates and for one a host created and pathon adopts. The report goes to stdout and, when `PATHON_CAPS_REPORT` names
// a file (CI sets it to `$GITHUB_STEP_SUMMARY`), is appended to it per system.

use std::fmt::Write as _;
use std::io::Write as _;

use pathon_rhi_types::{Backend, DeviceApi, DeviceRequest, Features, Format, FormatCaps, RhiError};
use pathon_rhi_wgpu::{WgpuDevice, interop};

// Written out here on purpose: a mistake in the backend's own conversion table must not hide itself.
const FEATURE_PAIRS: &[(Features, wgpu::Features)] = &[
    (Features::TIMESTAMP_QUERY, wgpu::Features::TIMESTAMP_QUERY),
    (
        Features::TIMESTAMP_QUERY_INSIDE_PASSES,
        wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES,
    ),
    (Features::TEXTURE_BINDING_ARRAY, wgpu::Features::TEXTURE_BINDING_ARRAY),
    (Features::BUFFER_BINDING_ARRAY, wgpu::Features::BUFFER_BINDING_ARRAY),
    (
        Features::NON_UNIFORM_INDEXING,
        wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
    ),
    (
        Features::PARTIALLY_BOUND_BINDING_ARRAY,
        wgpu::Features::PARTIALLY_BOUND_BINDING_ARRAY,
    ),
    (
        Features::MULTI_DRAW_INDIRECT_COUNT,
        wgpu::Features::MULTI_DRAW_INDIRECT_COUNT,
    ),
    (
        Features::INDIRECT_FIRST_INSTANCE,
        wgpu::Features::INDIRECT_FIRST_INSTANCE,
    ),
    (Features::SHADER_INT64, wgpu::Features::SHADER_INT64),
    (
        Features::SHADER_INT64_ATOMIC_MIN_MAX,
        wgpu::Features::SHADER_INT64_ATOMIC_MIN_MAX,
    ),
    (Features::SHADER_F16, wgpu::Features::SHADER_F16),
    (Features::SUBGROUP, wgpu::Features::SUBGROUP),
    (Features::TEXTURE_ATOMIC, wgpu::Features::TEXTURE_ATOMIC),
    (
        Features::TEXTURE_FORMAT_16BIT_NORM,
        wgpu::Features::TEXTURE_FORMAT_16BIT_NORM,
    ),
    (Features::FLOAT32_FILTERABLE, wgpu::Features::FLOAT32_FILTERABLE),
    (Features::TEXTURE_COMPRESSION_BC, wgpu::Features::TEXTURE_COMPRESSION_BC),
    (
        Features::TEXTURE_COMPRESSION_ETC2,
        wgpu::Features::TEXTURE_COMPRESSION_ETC2,
    ),
    (
        Features::TEXTURE_COMPRESSION_ASTC,
        wgpu::Features::TEXTURE_COMPRESSION_ASTC,
    ),
    (Features::PIPELINE_CACHE, wgpu::Features::PIPELINE_CACHE),
    (Features::DEPTH_CLIP_CONTROL, wgpu::Features::DEPTH_CLIP_CONTROL),
    (Features::DEPTH32FLOAT_STENCIL8, wgpu::Features::DEPTH32FLOAT_STENCIL8),
    (Features::POLYGON_MODE_LINE, wgpu::Features::POLYGON_MODE_LINE),
    (Features::IMMEDIATES, wgpu::Features::IMMEDIATES),
];

// Experimental wgpu features are never enabled by pathon yet, so `Caps` must not claim them.
const EXPERIMENTAL_FEATURES: Features = Features::RAY_QUERY.union(Features::MESH_SHADER);

fn request_on(backend: Option<Backend>) -> DeviceRequest {
    DeviceRequest {
        backend,
        optional_features: Features::all(),
        ..DeviceRequest::default()
    }
}

fn pathon_device() -> WgpuDevice {
    WgpuDevice::new_blocking(&request_on(None)).expect("this machine has a wgpu adapter (CI: lavapipe or Metal)")
}

// The backend each system must be able to create a device on (CI: lavapipe on Linux, Metal on macOS).
const REQUIRED_BACKEND: Option<Backend> = if cfg!(target_os = "windows") {
    Some(Backend::Dx12)
} else if cfg!(target_os = "macos") {
    Some(Backend::Metal)
} else if cfg!(target_os = "linux") {
    Some(Backend::Vulkan)
} else {
    None
};

fn assert_caps_match_wgpu(device: &WgpuDevice) {
    let caps = device.caps();
    let raw_device = device.raw_device();
    let raw_features = raw_device.features();
    for (ours, theirs) in FEATURE_PAIRS {
        assert_eq!(
            caps.has(*ours),
            raw_features.contains(*theirs),
            "feature {ours:?} differs from wgpu {theirs:?}"
        );
    }
    assert!(
        !caps.features.intersects(EXPERIMENTAL_FEATURES),
        "experimental features must stay off"
    );

    let limits = raw_device.limits();
    assert_eq!(caps.limits.max_texture_dimension_2d, limits.max_texture_dimension_2d);
    assert_eq!(caps.limits.max_texture_array_layers, limits.max_texture_array_layers);
    assert_eq!(caps.limits.max_bind_groups, limits.max_bind_groups);
    assert_eq!(
        caps.limits.max_uniform_buffer_binding_size,
        limits.max_uniform_buffer_binding_size
    );
    assert_eq!(
        caps.limits.max_storage_buffer_binding_size,
        limits.max_storage_buffer_binding_size
    );
    assert_eq!(caps.limits.max_buffer_size, limits.max_buffer_size);
    assert_eq!(caps.limits.max_color_attachments, limits.max_color_attachments);
    assert_eq!(
        caps.limits.max_compute_workgroup_size,
        [
            limits.max_compute_workgroup_size_x,
            limits.max_compute_workgroup_size_y,
            limits.max_compute_workgroup_size_z
        ]
    );
    assert_eq!(
        caps.limits.max_compute_invocations_per_workgroup,
        limits.max_compute_invocations_per_workgroup
    );
    assert_eq!(
        u64::from(caps.limits.min_uniform_buffer_offset_alignment),
        u64::from(limits.min_uniform_buffer_offset_alignment)
    );
    assert_eq!(
        caps.limits.max_immediate_size,
        if caps.has(Features::IMMEDIATES) {
            limits.max_immediate_size
        } else {
            0
        }
    );

    let info = device.raw_adapter().get_info();
    assert_eq!(caps.adapter.name, info.name);
    assert_eq!(caps.formats.len(), Format::ALL.len());
}

// Formats whose texture-binding, storage and render-attachment support wgpu reports must show up in `Caps`.
fn assert_format_matrix_matches_wgpu(device: &WgpuDevice) {
    let caps = device.caps();
    for (format, format_caps) in &caps.formats {
        let Some(wgpu_format) = pathon_wgpu_format(*format) else {
            continue;
        };
        let raw = device.raw_adapter().get_texture_format_features(wgpu_format);
        if format_caps.is_empty() {
            continue; // Gated behind a feature the device did not enable.
        }
        assert_eq!(
            format_caps.contains(FormatCaps::SAMPLED),
            raw.allowed_usages.contains(wgpu::TextureUsages::TEXTURE_BINDING),
            "{format:?} SAMPLED"
        );
        assert_eq!(
            format_caps.contains(FormatCaps::RENDER_TARGET),
            raw.allowed_usages.contains(wgpu::TextureUsages::RENDER_ATTACHMENT),
            "{format:?} RENDER_TARGET"
        );
        assert_eq!(
            format_caps.contains(FormatCaps::STORAGE),
            raw.allowed_usages.contains(wgpu::TextureUsages::STORAGE_BINDING),
            "{format:?} STORAGE"
        );
    }
}

// Only the uncompressed formats everyone has; compressed ones are gated by features and covered by the matrix above.
fn pathon_wgpu_format(format: Format) -> Option<wgpu::TextureFormat> {
    match format {
        Format::Rgba8Unorm => Some(wgpu::TextureFormat::Rgba8Unorm),
        Format::Rgba8UnormSrgb => Some(wgpu::TextureFormat::Rgba8UnormSrgb),
        Format::R8Unorm => Some(wgpu::TextureFormat::R8Unorm),
        Format::Depth32Float => Some(wgpu::TextureFormat::Depth32Float),
        _ => None,
    }
}

fn markdown_report(label: &str, device: &WgpuDevice) -> String {
    let caps = device.caps();
    let mut report = String::new();
    let _ = writeln!(report, "### Caps: {label}");
    let _ = writeln!(
        report,
        "- adapter: {} ({:?}, {:?}), driver: {}",
        caps.adapter.name, caps.backend, caps.adapter.device_type, caps.adapter.driver
    );
    let _ = writeln!(report, "- best profile: {:?}", caps.best_profile());
    let _ = writeln!(report, "- features: `{:?}`", caps.features);
    let _ = writeln!(
        report,
        "- limits: texture2d {}, bind groups {}, storage binding {}, buffer {}, workgroup {:?}, immediates {}",
        caps.limits.max_texture_dimension_2d,
        caps.limits.max_bind_groups,
        caps.limits.max_storage_buffer_binding_size,
        caps.limits.max_buffer_size,
        caps.limits.max_compute_workgroup_size,
        caps.limits.max_immediate_size,
    );
    let usable = caps
        .formats
        .iter()
        .filter(|(_, format_caps)| !format_caps.is_empty())
        .count();
    let _ = writeln!(
        report,
        "- formats with any capability: {usable} / {}",
        caps.formats.len()
    );
    report
}

fn publish_report(report: &str) {
    println!("{report}");
    let Some(path) = std::env::var_os("PATHON_CAPS_REPORT") else {
        return;
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("PATHON_CAPS_REPORT is a writable file");
    writeln!(file, "{report}").expect("the report is written");
}

#[test]
fn caps_of_a_pathon_device_match_raw_wgpu() {
    let device = pathon_device();
    assert_caps_match_wgpu(&device);
    assert_format_matrix_matches_wgpu(&device);
    publish_report(&markdown_report("pathon device", &device));
}

#[test]
fn a_device_exists_on_every_native_backend_of_this_system() {
    let mut created = Vec::new();
    for backend in [Backend::Vulkan, Backend::Dx12, Backend::Metal] {
        match WgpuDevice::new_blocking(&request_on(Some(backend))) {
            Ok(device) => {
                assert_eq!(device.caps().backend, backend);
                assert_caps_match_wgpu(&device);
                assert_format_matrix_matches_wgpu(&device);
                publish_report(&markdown_report(&format!("{backend:?} device"), &device));
                created.push(backend);
            }
            Err(RhiError::NoAdapter(reason)) => println!("{backend:?}: no adapter ({reason})"),
            Err(error) => panic!("{backend:?}: {error}"),
        }
    }
    if let Some(required) = REQUIRED_BACKEND {
        assert!(created.contains(&required), "{required:?} is missing, got {created:?}");
    }
}

#[test]
fn caps_of_an_adopted_device_match_raw_wgpu() {
    let host = pathon_device();
    let adapter = host.raw_adapter().clone();
    let (raw_device, raw_queue) = request_device_now(
        &adapter,
        &wgpu::DeviceDescriptor {
            label: Some("host device"),
            required_features: raw_features_of(&host),
            required_limits: adapter.limits(),
            ..wgpu::DeviceDescriptor::default()
        },
    );
    let adopted = interop::adopt(host.raw_instance().clone(), adapter, raw_device, raw_queue);
    assert_caps_match_wgpu(&adopted);
    assert_format_matrix_matches_wgpu(&adopted);
    assert_eq!(
        adopted.caps(),
        host.caps(),
        "an adopted device with the same descriptor reports the same Caps"
    );
    publish_report(&markdown_report("adopted device", &adopted));
}

#[test]
fn a_minimal_host_device_reports_the_defaults_it_got() {
    let host = pathon_device();
    let adapter = host.raw_adapter().clone();
    let (raw_device, raw_queue) = request_device_now(&adapter, &wgpu::DeviceDescriptor::default());
    let adopted = interop::adopt(host.raw_instance().clone(), adapter, raw_device, raw_queue);
    assert_caps_match_wgpu(&adopted);
    assert!(adopted.caps().features.is_empty(), "the host asked for no features");
    assert_eq!(
        adopted.caps().limits.max_bind_groups,
        wgpu::Limits::default().max_bind_groups
    );
}

fn raw_features_of(device: &WgpuDevice) -> wgpu::Features {
    device.raw_device().features()
}

// Native wgpu futures are ready on the first poll.
fn request_device_now(adapter: &wgpu::Adapter, descriptor: &wgpu::DeviceDescriptor<'_>) -> (wgpu::Device, wgpu::Queue) {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    let mut future = pin!(adapter.request_device(descriptor));
    match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => result.expect("the adapter creates a device"),
        Poll::Pending => panic!("native wgpu futures complete immediately"),
    }
}
