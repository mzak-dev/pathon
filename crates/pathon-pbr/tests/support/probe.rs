#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test crate that includes this module uses a different part of it"
)]

// A feature that copies a slot (every mip of it) into a readable buffer, and asks for the slot when it is on demand.

use std::sync::{Arc, Mutex};

use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::{
    Access, BufferCopy, BufferDesc, BufferId, BufferUsage, DeviceApi, Extent3d, RhiError, TexelLayout, TextureAspect,
    TextureCopy,
};
use pathon_api::{PrepareCtx, RecordCtx, RenderFeature, SetupCtx, Slot};

const ROW_ALIGNMENT: u32 = 256;

#[derive(Clone, Debug, Default)]
pub struct SharedBuffer(Arc<Mutex<Option<BufferId>>>);

impl SharedBuffer {
    pub fn get(&self) -> BufferId {
        self.0.lock().expect("mutex").expect("the probe ran setup")
    }

    fn set(&self, buffer: BufferId) {
        *self.0.lock().expect("mutex") = Some(buffer);
    }
}

/// Where one mip sits in the readback buffer.
#[derive(Clone, Copy, Debug)]
pub struct MipLayout {
    pub width: u32,
    pub height: u32,
    pub padded_row: u32,
    pub offset: u64,
}

/// Reads what a `Probe` copied, after the frame ran.
#[derive(Clone, Debug)]
pub struct ProbeReader {
    buffer: SharedBuffer,
    mips: Vec<MipLayout>,
    texel_bytes: u32,
}

impl ProbeReader {
    pub fn mips(&self) -> &[MipLayout] {
        &self.mips
    }

    /// The bytes of mip `mip`, rows packed.
    pub fn texels(&self, device: &dyn DeviceApi, mip: usize) -> Vec<u8> {
        let bytes = device.read_buffer(self.buffer.get()).expect("read");
        let layout = self.mips[mip];
        let row = (layout.width * self.texel_bytes) as usize;
        (0..layout.height as usize)
            .flat_map(|y| {
                let offset = usize::try_from(layout.offset).expect("a readback buffer fits in memory");
                let start = offset + y * layout.padded_row as usize;
                bytes[start..start + row].to_vec()
            })
            .collect()
    }

    pub fn f32s(&self, device: &dyn DeviceApi, mip: usize) -> Vec<f32> {
        self.texels(device, mip)
            .chunks(4)
            .map(|word| f32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect()
    }

    pub fn u32s(&self, device: &dyn DeviceApi, mip: usize) -> Vec<u32> {
        self.texels(device, mip)
            .chunks(4)
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect()
    }

    /// Pairs of half floats (`Rg16Float`).
    pub fn half_pairs(&self, device: &dyn DeviceApi, mip: usize) -> Vec<[f32; 2]> {
        self.texels(device, mip)
            .chunks(4)
            .map(|word| {
                [
                    half_to_f32(u16::from_le_bytes([word[0], word[1]])),
                    half_to_f32(u16::from_le_bytes([word[2], word[3]])),
                ]
            })
            .collect()
    }
}

#[derive(Debug)]
pub struct Probe {
    slot: Slot,
    aspect: TextureAspect,
    texel_bytes: u32,
    mips: Vec<MipLayout>,
    total: u64,
    buffer: SharedBuffer,
    request: bool,
}

impl Probe {
    pub fn new(slot: Slot, size: (u32, u32), mip_count: u32, texel_bytes: u32) -> (Self, ProbeReader) {
        let buffer = SharedBuffer::default();
        let mut mips = Vec::new();
        let mut total = 0;
        for mip in 0..mip_count {
            let (width, height) = ((size.0 >> mip).max(1), (size.1 >> mip).max(1));
            let padded_row = (width * texel_bytes).next_multiple_of(ROW_ALIGNMENT);
            mips.push(MipLayout {
                width,
                height,
                padded_row,
                offset: total,
            });
            total += u64::from(padded_row * height);
        }
        let reader = ProbeReader {
            buffer: buffer.clone(),
            mips: mips.clone(),
            texel_bytes,
        };
        let probe = Self {
            slot,
            aspect: if slot == Slot::DEPTH {
                TextureAspect::DepthOnly
            } else {
                TextureAspect::All
            },
            texel_bytes,
            mips,
            total,
            buffer,
            request: true,
        };
        (probe, reader)
    }
}

impl RenderFeature for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }

    fn stage(&self) -> Stage {
        Stage::Overlay
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        let buffer = ctx.device().create_buffer(&BufferDesc {
            label: "probe",
            size: self.total,
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })?;
        self.buffer.set(buffer);
        Ok(())
    }

    fn prepare_view(&mut self, ctx: &mut PrepareCtx<'_>) {
        if self.request {
            ctx.request(self.slot);
        }
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(handle) = ctx.slot(self.slot) else {
            return Ok(());
        };
        let (buffer, mips, aspect) = (self.buffer.get(), self.mips.clone(), self.aspect);
        let mut pass = ctx
            .graph()
            .add_pass(PassDesc::new("probe", Stage::Overlay).side_effect());
        let handle = pass.read(handle, Access::COPY_SRC);
        pass.record(move |pass| {
            let texture = pass.texture(handle);
            for (mip, layout) in (0..).zip(&mips) {
                pass.commands().copy_texture_to_buffer(
                    TextureCopy {
                        texture,
                        mip,
                        origin: pathon_api::rhi::Origin3d::default(),
                        aspect,
                    },
                    BufferCopy {
                        buffer,
                        layout: TexelLayout {
                            offset: layout.offset,
                            bytes_per_row: layout.padded_row,
                            rows_per_image: layout.height,
                        },
                    },
                    Extent3d::new_2d(layout.width, layout.height),
                );
            }
            Ok(())
        });
        Ok(())
    }
}

pub fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * fraction * 2.0_f32.powi(-24),
        31 if fraction == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + fraction / 1024.0) * 2.0_f32.powi(exponent - 15),
    }
}
