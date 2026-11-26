// The persistent GPU state of the pack: the voxel grid, the rays of the frame and the two generations of the probe
// atlases (one is read while the other is written, and they swap every frame).

use pathon_api::rhi::{
    Access, BufferDesc, BufferId, BufferUsage, CommandList, DeviceApi, Extent3d, Format, RhiError, SamplerDesc,
    SubresourceRange, TexelLayout, TextureCopy, TextureDesc, TextureId, TextureUsage, TextureViewDesc,
};
use pathon_ibl::IblTexture;

use crate::settings::GiSettings;
use crate::volume::ProbeLayout;
use crate::voxel::{BYTES_PER_CELL, VoxelLayout};

pub(crate) const ATLAS_FORMAT: Format = Format::Rgba16Float;
/// Bytes of one ray in the ray buffer: two `vec4<f32>`.
pub(crate) const RAY_BYTES: u64 = 32;
const MIN_LIGHT_CAPACITY: u32 = 16;
const LIGHT_BYTES: u64 = 64;

/// The two atlases of one generation.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct AtlasPair {
    pub irradiance: TextureId,
    pub irradiance_desc: TextureDesc,
    pub visibility: TextureId,
    pub visibility_desc: TextureDesc,
}

fn atlas(
    device: &dyn DeviceApi,
    label: &'static str,
    (width, height): (u32, u32),
) -> Result<(TextureId, TextureDesc), RhiError> {
    let desc = TextureDesc::new_2d(
        label,
        width,
        height,
        ATLAS_FORMAT,
        TextureUsage::SAMPLED | TextureUsage::STORAGE | TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
    );
    Ok((device.create_texture(&desc)?, desc))
}

impl AtlasPair {
    fn new(device: &dyn DeviceApi, volume: &ProbeLayout, generation: usize) -> Result<Self, RhiError> {
        let (irradiance_label, visibility_label) = if generation == 0 {
            ("gi irradiance a", "gi visibility a")
        } else {
            ("gi irradiance b", "gi visibility b")
        };
        let (irradiance, irradiance_desc) = atlas(device, irradiance_label, volume.irradiance_size())?;
        let (visibility, visibility_desc) = atlas(device, visibility_label, volume.visibility_size())?;
        Ok(Self {
            irradiance,
            irradiance_desc,
            visibility,
            visibility_desc,
        })
    }
}

/// Everything that lives as long as the volume does.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct GiResources {
    pub volume: ProbeLayout,
    pub grid: VoxelLayout,
    pub settings: GiSettings,
    pub albedo: BufferId,
    pub emissive: BufferId,
    pub radiance: BufferId,
    pub rays: BufferId,
    pub lights: BufferId,
    pub light_capacity: u32,
    pub atlases: [AtlasPair; 2],
}

impl GiResources {
    pub(crate) fn create(
        device: &dyn DeviceApi,
        volume: ProbeLayout,
        grid: VoxelLayout,
        settings: GiSettings,
    ) -> Result<Self, RhiError> {
        let cells = u64::from(grid.cell_count());
        let storage = BufferUsage::STORAGE | BufferUsage::COPY_DST;
        let buffer = |label, size: u64, usage| device.create_buffer(&BufferDesc { label, size, usage });
        let rays = u64::from(settings.probes_per_frame.min(volume.probe_count())) * u64::from(settings.rays_per_probe);
        let resources = Self {
            volume,
            grid,
            settings,
            albedo: buffer("gi voxel albedo", cells * 4, storage | BufferUsage::COPY_SRC)?,
            emissive: buffer("gi voxel emission", cells * 4, storage)?,
            radiance: buffer(
                "gi voxel radiance",
                cells * (BYTES_PER_CELL - 8),
                storage | BufferUsage::COPY_SRC,
            )?,
            rays: buffer("gi rays", rays.max(1) * RAY_BYTES, storage | BufferUsage::COPY_SRC)?,
            lights: buffer("gi lights", u64::from(MIN_LIGHT_CAPACITY) * LIGHT_BYTES, storage)?,
            light_capacity: MIN_LIGHT_CAPACITY,
            atlases: [AtlasPair::new(device, &volume, 0)?, AtlasPair::new(device, &volume, 1)?],
        };
        resources.rest(device)?;
        Ok(resources)
    }

    // The graph imports the atlases as already written and sampleable: an upload of zeros brings them to that state.
    fn rest(&self, device: &dyn DeviceApi) -> Result<(), RhiError> {
        let mut list = CommandList::new();
        for pair in &self.atlases {
            for (texture, desc) in [
                (pair.irradiance, &pair.irradiance_desc),
                (pair.visibility, &pair.visibility_desc),
            ] {
                let (width, height) = (desc.size.width, desc.size.height);
                let zeros = vec![0_u8; width as usize * height as usize * 8];
                device.write_texture(
                    &TextureCopy::whole(texture),
                    &zeros,
                    TexelLayout {
                        offset: 0,
                        bytes_per_row: width * 8,
                        rows_per_image: height,
                    },
                    Extent3d::new_2d(width, height),
                )?;
                list.texture_access(texture, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);
            }
        }
        device.submit(list)
    }

    /// A light buffer for at least `count` lights; the old one goes to the caller to be released.
    pub(crate) fn grow_lights(&mut self, device: &dyn DeviceApi, count: u32) -> Result<Option<BufferId>, RhiError> {
        if count <= self.light_capacity {
            return Ok(None);
        }
        let capacity = count.next_power_of_two();
        let buffer = device.create_buffer(&BufferDesc {
            label: "gi lights",
            size: u64::from(capacity) * LIGHT_BYTES,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        })?;
        self.light_capacity = capacity;
        Ok(Some(std::mem::replace(&mut self.lights, buffer)))
    }

    pub(crate) fn rays_per_frame(&self) -> u32 {
        self.settings.probes_per_frame.min(self.volume.probe_count()) * self.settings.rays_per_probe
    }

    /// Bytes of GPU memory the volume holds.
    pub(crate) fn gpu_bytes(&self) -> u64 {
        let texel = 8;
        let atlas = |pair: &AtlasPair| {
            let size = |desc: &TextureDesc| u64::from(desc.size.width) * u64::from(desc.size.height) * texel;
            size(&pair.irradiance_desc) + size(&pair.visibility_desc)
        };
        u64::from(self.grid.cell_count()) * BYTES_PER_CELL
            + u64::from(self.rays_per_frame()) * RAY_BYTES
            + atlas(&self.atlases[0])
            + atlas(&self.atlases[1])
    }

    pub(crate) fn resources(&self) -> Vec<pathon_api::rhi::Resource> {
        let mut all: Vec<pathon_api::rhi::Resource> = vec![
            self.albedo.into(),
            self.emissive.into(),
            self.radiance.into(),
            self.rays.into(),
            self.lights.into(),
        ];
        for pair in &self.atlases {
            all.push(pair.irradiance.into());
            all.push(pair.visibility.into());
        }
        all
    }
}

/// A one-texel black cube for a scene with no environment.
pub(crate) fn black_cube(device: &dyn DeviceApi) -> Result<IblTexture, RhiError> {
    let texture = device.create_texture(&TextureDesc::new_cube(
        "gi black environment",
        1,
        Format::Rgba16Float,
        TextureUsage::SAMPLED | TextureUsage::COPY_DST,
    ))?;
    device.write_texture(
        &TextureCopy::whole(texture),
        &[0; 8 * 6],
        TexelLayout {
            offset: 0,
            bytes_per_row: 8,
            rows_per_image: 1,
        },
        Extent3d {
            width: 1,
            height: 1,
            depth_or_layers: 6,
        },
    )?;
    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);
    device.submit(list)?;
    Ok(IblTexture {
        texture,
        view: device.create_texture_view(texture, &TextureViewDesc::cube())?,
    })
}

/// The sampler of the atlases: linear, clamped.
pub(crate) fn atlas_sampler(device: &dyn DeviceApi) -> Result<pathon_api::rhi::SamplerId, RhiError> {
    device.create_sampler(&SamplerDesc::LINEAR_CLAMP)
}
