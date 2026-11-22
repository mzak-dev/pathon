use pathon_api::rhi::{
    Access, Caps, CommandList, DeviceApi, Extent3d, Format, FormatCaps, Origin3d, SubresourceRange, TexelLayout,
    TextureAspect, TextureCopy, TextureDesc, TextureId, TextureUsage,
};

use crate::choice::{FormatChoice, choose_format};
use crate::error::TextureError;
use crate::mipmap::{MipMode, MipmapGenerator, can_generate_mips};
use crate::reference::generate_chain;
use crate::source::{TextureSource, Variant, blocks_in_level};
use crate::texture::{Texture, view_formats_for};

/// Uploads [`TextureSource`]s: picks the encoding the device samples, writes the mips and generates the missing ones.
#[derive(Default, Debug)]
pub struct TextureLoader {
    mipmaps: MipmapGenerator,
    cpu_mips: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MipPlan {
    /// The variant brings its whole chain.
    AsGiven,
    Compute,
    Cpu,
}

impl TextureLoader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Averages mips on the CPU even where compute could: the path devices without storage support take anyway, for
    /// comparing the two.
    #[must_use]
    pub fn with_cpu_mips(self) -> Self {
        Self { cpu_mips: true, ..self }
    }

    /// The variant and formats the device gets for `source`, decided from its capability matrix alone.
    pub fn choose_format(caps: &Caps, source: &TextureSource<'_>) -> Result<FormatChoice, TextureError> {
        choose_format(caps, source)
    }

    /// Creates the texture, writes every level and returns it in [`Texture::STATE`].
    ///
    /// Submits to the device itself (writes and barriers), so it may run during setup or on a loading thread: any
    /// `DeviceApi` method may be called from any thread (ADR-0020).
    pub fn load(&mut self, device: &dyn DeviceApi, source: &TextureSource<'_>) -> Result<Texture, TextureError> {
        source.validate()?;
        let choice = choose_format(device.caps(), source)?;
        let variant = &source.variants[choice.variant];
        let mut plan = mip_plan(device.caps(), source, variant, choice)?;
        if self.cpu_mips && plan == MipPlan::Compute {
            plan = MipPlan::Cpu;
        }
        let mip_count = match plan {
            MipPlan::AsGiven => u32::try_from(variant.levels().len()).unwrap_or(u32::MAX),
            MipPlan::Compute | MipPlan::Cpu => Extent3d::new_2d(source.width, source.height).max_mip_count(),
        };
        let mut usage = TextureUsage::SAMPLED | TextureUsage::COPY_DST | source.extra_usage;
        if plan == MipPlan::Compute {
            usage |= TextureUsage::STORAGE;
        }
        let desc = TextureDesc::new_2d(source.label, source.width, source.height, choice.storage, usage)
            .with_layers(source.layers)
            .with_mips(mip_count)
            .with_view_formats(view_formats_for(choice.storage, choice.sampling));
        let id = device.create_texture(&desc)?;
        let texture = Texture { id, desc, choice };
        self.fill(device, id, source, variant, plan).inspect_err(|_| {
            device.destroy(id.into());
        })?;
        Ok(texture)
    }

    pub fn release(&mut self, device: &dyn DeviceApi) {
        self.mipmaps.release(device);
    }

    fn fill(
        &mut self,
        device: &dyn DeviceApi,
        texture: TextureId,
        source: &TextureSource<'_>,
        variant: &Variant<'_>,
        plan: MipPlan,
    ) -> Result<(), TextureError> {
        let format = variant.format();
        for (level, bytes) in (0_u32..).zip(variant.levels()) {
            write_level(device, texture, source, format, level, bytes)?;
        }
        let mode = MipMode::from(source.color_space);
        match plan {
            MipPlan::AsGiven => settle(device, texture),
            MipPlan::Compute => self.mipmaps.generate(device, texture, mode, Access::COPY_DST),
            MipPlan::Cpu => {
                let chain = generate_chain(
                    format,
                    mode,
                    source.width,
                    source.height,
                    source.layers,
                    variant.levels()[0],
                )
                .ok_or_else(|| source.invalid("no CPU mip path for this format"))?;
                for (level, bytes) in (1_u32..).zip(&chain) {
                    write_level(device, texture, source, format, level, bytes)?;
                }
                settle(device, texture)
            }
        }
    }
}

fn mip_plan(
    caps: &Caps,
    source: &TextureSource<'_>,
    variant: &Variant<'_>,
    choice: FormatChoice,
) -> Result<MipPlan, TextureError> {
    let single_level = variant.levels().len() == 1;
    let has_more_mips = Extent3d::new_2d(source.width, source.height).max_mip_count() > 1;
    // Compressed levels cannot be averaged without decoding, so a lone level stays a lone level.
    let generate = source.generate_mips && single_level && has_more_mips && variant.format().compression().is_none();
    if !generate {
        return Ok(MipPlan::AsGiven);
    }
    if !can_generate_mips(variant.format()) {
        return Err(TextureError::MipGeneration {
            label: source.label,
            reason: format!("{:?} is not averaged; supply the whole chain", variant.format()),
        });
    }
    let storage_writable = caps.format(choice.storage).contains(FormatCaps::STORAGE);
    Ok(if storage_writable {
        MipPlan::Compute
    } else {
        MipPlan::Cpu
    })
}

fn write_level(
    device: &dyn DeviceApi,
    texture: TextureId,
    source: &TextureSource<'_>,
    format: Format,
    level: u32,
    bytes: &[u8],
) -> Result<(), TextureError> {
    let (blocks_x, blocks_y) = blocks_in_level(format, source.width, source.height, level);
    let (block_width, block_height) = format.block_dimensions();
    let block_bytes = format.block_size_bytes().unwrap_or(0);
    // Compressed copies cover whole blocks even where the mip is smaller than one.
    let extent = Extent3d {
        width: blocks_x * block_width,
        height: blocks_y * block_height,
        depth_or_layers: source.layers,
    };
    device.write_texture(
        &TextureCopy {
            texture,
            mip: level,
            origin: Origin3d::default(),
            aspect: TextureAspect::All,
        },
        bytes,
        TexelLayout {
            offset: 0,
            bytes_per_row: blocks_x * block_bytes,
            rows_per_image: blocks_y,
        },
        extent,
    )?;
    Ok(())
}

// `write_texture` leaves its subresources in COPY_DST (ADR-0020); one barrier hands them to the shaders.
fn settle(device: &dyn DeviceApi, texture: TextureId) -> Result<(), TextureError> {
    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);
    Ok(device.submit(list)?)
}

#[cfg(test)]
mod tests {
    use pathon_api::rhi::Features;

    use super::*;
    use crate::source::ColorSpace;
    use crate::test_caps::mock_caps;

    const SAMPLEABLE: FormatCaps = FormatCaps::SAMPLED.union(FormatCaps::FILTERABLE);

    fn plan(raw_caps: FormatCaps, source: &TextureSource<'_>) -> MipPlan {
        let caps = mock_caps(Features::empty(), &[], 8192, raw_caps);
        let choice = choose_format(&caps, source).expect("raw is sampleable");
        mip_plan(&caps, source, &source.variants[choice.variant], choice).expect("plan")
    }

    fn single_level(format: Format, generate: bool) -> TextureSource<'static> {
        // The plan looks at sizes only, so the level bytes can stay empty.
        let source = TextureSource::new("t", 8, 8, ColorSpace::Linear).with_variant(Variant::new(format, vec![&[]]));
        if generate {
            source
        } else {
            source.without_mip_generation()
        }
    }

    #[test]
    fn compute_writes_the_chain_where_the_format_is_storage_writable() {
        let plan = plan(
            SAMPLEABLE | FormatCaps::STORAGE,
            &single_level(Format::Rgba8Unorm, true),
        );
        assert_eq!(plan, MipPlan::Compute);
    }

    #[test]
    fn the_cpu_writes_it_where_compute_cannot() {
        assert_eq!(plan(SAMPLEABLE, &single_level(Format::Rgba8Unorm, true)), MipPlan::Cpu);
    }

    #[test]
    fn nothing_is_generated_when_not_asked_for_or_not_needed() {
        let all = FormatCaps::all();
        assert_eq!(plan(all, &single_level(Format::Rgba8Unorm, false)), MipPlan::AsGiven);
        let one_by_one = TextureSource::new("t", 1, 1, ColorSpace::Linear).with_variant(Variant::rgba8(vec![&[]]));
        assert_eq!(plan(all, &one_by_one), MipPlan::AsGiven);
    }

    #[test]
    fn formats_the_shader_does_not_average_are_x0005() {
        let caps = mock_caps(Features::empty(), &[], 8192, FormatCaps::all());
        let source = single_level(Format::R8Unorm, true);
        let choice = FormatChoice {
            variant: 0,
            storage: Format::R8Unorm,
            sampling: Format::R8Unorm,
            fallback: false,
        };
        let error = mip_plan(&caps, &source, &source.variants[0], choice).expect_err("unsupported");
        assert_eq!(error.code(), "X0005");
    }
}
