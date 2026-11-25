#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

use pathon_golden::{announce_adapter, gpu_or_skip};
use pathon_ibl::SyntheticSky;
use pathon_rhi::{
    Access, Backend, BufferCopy, BufferDesc, BufferUsage, CommandList, Device, DeviceApi, DeviceRequest, Extent3d,
    Origin3d, RhiError, SubresourceRange, TexelLayout, TextureAspect, TextureCopy, TextureId,
};

const ROW_ALIGNMENT: u32 = 256;

fn backend_from_environment() -> Option<Backend> {
    match std::env::var("PATHON_TEST_BACKEND").ok()?.to_ascii_lowercase().as_str() {
        "vulkan" => Some(Backend::Vulkan),
        "dx12" => Some(Backend::Dx12),
        "metal" => Some(Backend::Metal),
        other => panic!("PATHON_TEST_BACKEND `{other}` is not vulkan, dx12 or metal"),
    }
}

/// A real device, or `None` (the test is skipped) when the machine has no adapter.
pub fn gpu(test: &str) -> Option<Device> {
    let built = Device::build(&DeviceRequest {
        label: "ibl test",
        backend: backend_from_environment(),
        ..Default::default()
    });
    let device = gpu_or_skip(test, built, |error| matches!(error, RhiError::NoAdapter(_)))?;
    announce_adapter(test, device.caps());
    Some(device)
}

/// The sky as the `f64` function the CPU references integrate.
pub fn sky_radiance(sky: SyntheticSky) -> impl Fn([f64; 3]) -> [f64; 3] {
    #[expect(clippy::cast_possible_truncation, reason = "unit directions fit f32")]
    move |direction| sky.radiance(direction.map(|axis| axis as f32)).map(f64::from)
}

pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) << 31;
    let exponent = u32::from((bits >> 10) & 0x1f);
    let mantissa = u32::from(bits & 0x3ff);
    let pattern = match (exponent, mantissa) {
        (0, 0) => sign,
        (0, _) => {
            let mut exponent = 113;
            let mut mantissa = mantissa;
            while mantissa & 0x400 == 0 {
                mantissa <<= 1;
                exponent -= 1;
            }
            sign | (exponent << 23) | ((mantissa & 0x3ff) << 13)
        }
        (31, _) => sign | 0x7f80_0000 | (mantissa << 13),
        _ => sign | ((exponent + 112) << 23) | (mantissa << 13),
    };
    f32::from_bits(pattern)
}

/// One mip of one layer of an `Rgba16Float` texture as `size * size` RGBA texels; the subresource ends in the state it
/// started in.
pub struct Mip {
    pub width: u32,
    pub height: u32,
    pub texels: Vec<[f32; 4]>,
}

impl Mip {
    pub fn at(&self, x: u32, y: u32) -> [f32; 4] {
        self.texels[(y * self.width + x) as usize]
    }
}

pub fn read_rgba16f(device: &dyn DeviceApi, texture: TextureId, state: Access, mip: u32, layer: u32) -> Mip {
    let desc = device.texture_desc(texture).expect("texture");
    let size = desc.size.mip_level(mip);
    let packed_row = size.width * 8;
    let padded_row = packed_row.next_multiple_of(ROW_ALIGNMENT);
    let buffer = device
        .create_buffer(&BufferDesc {
            label: "mip readback",
            size: u64::from(padded_row * size.height),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("buffer");
    let range = SubresourceRange {
        base_mip: mip,
        mip_count: 1,
        base_layer: layer,
        layer_count: 1,
    };
    let mut list = CommandList::new();
    list.texture_access(texture, range, state, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy {
            texture,
            mip,
            origin: Origin3d { x: 0, y: 0, z: layer },
            aspect: TextureAspect::All,
        },
        BufferCopy {
            buffer,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: padded_row,
                rows_per_image: size.height,
            },
        },
        Extent3d::new_2d(size.width, size.height),
    );
    list.texture_access(texture, range, Access::COPY_SRC, state);
    device.submit(list).expect("submit");
    let padded = device.read_buffer(buffer).expect("read");
    let mut texels = Vec::new();
    for row in padded.chunks(padded_row as usize).take(size.height as usize) {
        for texel in row[..packed_row as usize].as_chunks::<8>().0 {
            let channel = |index: usize| f16_to_f32(u16::from_le_bytes([texel[index * 2], texel[index * 2 + 1]]));
            texels.push([channel(0), channel(1), channel(2), channel(3)]);
        }
    }
    Mip {
        width: size.width,
        height: size.height,
        texels,
    }
}
