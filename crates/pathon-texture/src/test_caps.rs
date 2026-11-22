use pathon_api::rhi::{AdapterInfo, Backend, Caps, DeviceType, Features, Format, FormatCaps, Limits};

const RAW: [Format; 4] = [
    Format::Rgba8Unorm,
    Format::Rgba8UnormSrgb,
    Format::Rgba16Float,
    Format::Rgba32Float,
];

// A device with the given compressed formats plus the common raw ones; `raw_caps` is what the raw formats may do
// (32-bit float is never filterable).
pub(crate) fn mock_caps(
    features: Features,
    compressed: &[&[Format]],
    max_dimension: u32,
    raw_caps: FormatCaps,
) -> Caps {
    let formats = compressed
        .iter()
        .copied()
        .flatten()
        .map(|format| (*format, FormatCaps::SAMPLED | FormatCaps::FILTERABLE))
        .chain(RAW.iter().map(|format| {
            let caps = if *format == Format::Rgba32Float {
                FormatCaps::SAMPLED
            } else {
                raw_caps
            };
            (*format, caps)
        }))
        .collect();
    Caps {
        backend: Backend::Null,
        adapter: AdapterInfo {
            name: "mock".into(),
            driver: String::new(),
            device_type: DeviceType::Other,
        },
        features,
        limits: Limits {
            max_texture_dimension_2d: max_dimension,
            ..Limits::WEBGPU_DEFAULT
        },
        formats,
        timestamp_period_ns: 1.0,
    }
}
