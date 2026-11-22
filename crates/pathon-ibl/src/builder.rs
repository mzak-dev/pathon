use std::collections::HashMap;
use std::f32::consts::PI;

use bytemuck::{Pod, Zeroable};
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupId, BindingResource, BufferDesc, BufferUsage, CommandList,
    ComputePassDesc, DeviceApi, Extent3d, Format, FormatCaps, Resource, SubresourceRange, TexelLayout, TextureCopy,
    TextureDesc, TextureId, TextureUsage, TextureViewDesc, TextureViewDimension, TextureViewId,
};

use crate::error::IblError;
use crate::pipelines::{Pipelines, Step, WORKGROUP_SIZE};
use crate::{EquirectImage, IblEnvironment, IblSettings, IblTexture};

const FORMAT: Format = Format::Rgba16Float;
const FACES: u32 = TextureDesc::CUBE_FACES;
// COPY_SRC lets a host read the maps back (previews, export of a baked environment).
const MAP_USAGE: TextureUsage = TextureUsage::SAMPLED
    .union(TextureUsage::STORAGE)
    .union(TextureUsage::COPY_SRC);
const MAX_SAMPLES_PER_AXIS: u32 = 8;
// The irradiance pass visits every texel of one source mip per output texel: 32 x 32 per face is the smallest mip
// that still resolves a sun, and keeps the pass at a few tens of millions of loads.
const IRRADIANCE_SOURCE_FACE: u32 = 32;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EquirectParams {
    face_size: u32,
    samples_per_axis: u32,
    padding: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PrefilterParams {
    face_size: u32,
    sample_count: u32,
    roughness: f32,
    texel_solid_angle: f32,
    top_lod: f32,
    padding: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct IrradianceParams {
    face_size: u32,
    padding: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LutParams {
    size: u32,
    sample_count: u32,
    padding: [u32; 2],
}

#[derive(Default)]
struct Temporaries(Vec<Resource>);

impl Temporaries {
    fn keep<Id: Into<Resource> + Copy>(&mut self, id: Id) -> Id {
        self.0.push(id.into());
        id
    }

    fn release(self, device: &dyn DeviceApi) {
        self.0.into_iter().rev().for_each(|resource| device.destroy(resource));
    }

    fn uniform(&mut self, device: &dyn DeviceApi, params: &impl Pod) -> Result<BindingResource, IblError> {
        let bytes = bytemuck::bytes_of(params);
        let buffer = self.keep(device.create_buffer(&BufferDesc {
            label: "ibl params",
            size: bytes.len() as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        })?);
        device.write_buffer(buffer, 0, bytes)?;
        Ok(BindingResource::whole_buffer(buffer))
    }

    fn view(
        &mut self,
        device: &dyn DeviceApi,
        texture: TextureId,
        desc: &TextureViewDesc,
    ) -> Result<TextureViewId, IblError> {
        Ok(self.keep(device.create_texture_view(texture, desc)?))
    }

    fn bind_group(
        &mut self,
        device: &dyn DeviceApi,
        step: &Step,
        resources: Vec<BindingResource>,
    ) -> Result<BindGroupId, IblError> {
        let entries = resources
            .into_iter()
            .enumerate()
            .map(|(binding, resource)| BindGroupEntry {
                binding: u32::try_from(binding).unwrap_or(u32::MAX),
                resource,
            })
            .collect();
        Ok(self.keep(device.create_bind_group(&BindGroupDesc {
            label: "ibl",
            layout: step.layout,
            entries,
        })?))
    }
}

// Six layers of one mip, as the storage or sampled array the compute shaders address by face.
fn mip_layers(mip: u32) -> TextureViewDesc {
    TextureViewDesc {
        dimension: Some(TextureViewDimension::D2Array),
        base_mip: mip,
        mip_count: Some(1),
        base_layer: 0,
        layer_count: Some(FACES),
        ..TextureViewDesc::default()
    }
}

fn groups(size: u32) -> u32 {
    size.div_ceil(WORKGROUP_SIZE)
}

fn dispatch(list: &mut CommandList, label: &'static str, step: &Step, group: BindGroupId, size: (u32, u32, u32)) {
    let mut pass = list.compute_pass(ComputePassDesc {
        label,
        timestamps: None,
    });
    pass.set_pipeline(step.pipeline);
    pass.set_bind_group(0, group);
    pass.dispatch(groups(size.0), groups(size.1), size.2);
}

// A fresh mip is written by one pass and handed to the samplers right after.
fn write_mip(
    list: &mut CommandList,
    texture: TextureId,
    mip: u32,
    label: &'static str,
    step: &Step,
    group: BindGroupId,
    size: u32,
) {
    list.texture_access(
        texture,
        SubresourceRange::mip(mip),
        Access::empty(),
        Access::STORAGE_WRITE,
    );
    dispatch(list, label, step, group, (size, size, FACES));
    list.texture_access(
        texture,
        SubresourceRange::mip(mip),
        Access::STORAGE_WRITE,
        Access::SAMPLED,
    );
}

fn check_device(device: &dyn DeviceApi, image: Option<&EquirectImage>, settings: &IblSettings) -> Result<(), IblError> {
    let caps = device.caps();
    let needed = FormatCaps::SAMPLED | FormatCaps::FILTERABLE | FormatCaps::STORAGE;
    if !caps.format(FORMAT).contains(needed) {
        return Err(IblError::Unsupported {
            reason: format!("{FORMAT:?} lacks {:?}", needed - caps.format(FORMAT)),
        });
    }
    let limits = &caps.limits;
    let invocations = WORKGROUP_SIZE * WORKGROUP_SIZE;
    if limits.max_compute_invocations_per_workgroup < invocations || limits.max_texture_array_layers < FACES {
        return Err(IblError::Unsupported {
            reason: "compute workgroups of 64 invocations and six texture layers are required".into(),
        });
    }
    let max = limits.max_texture_dimension_2d;
    let largest = image.map_or(settings.cube_size, |image| {
        image.width().max(image.height()).max(settings.cube_size)
    });
    if largest > max {
        let (width, height) = image.map_or((settings.cube_size, settings.cube_size), |image| {
            (image.width(), image.height())
        });
        return Err(IblError::ImageTooLarge { width, height, max });
    }
    Ok(())
}

/// Builds environments and caches what does not depend on them: the pipelines and the BRDF tables. Call
/// [`release`](Self::release) before the device goes away.
#[derive(Default, Debug)]
pub struct IblBuilder {
    pipelines: Option<Pipelines>,
    luts: HashMap<(u32, u32), IblTexture>,
    generation: u64,
}

impl IblBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Converts `image` into the prefiltered and irradiance cubes and returns them with the BRDF table. The commands
    /// are submitted and the maps are in [`IblEnvironment::STATE`].
    pub fn build_environment(
        &mut self,
        device: &dyn DeviceApi,
        image: &EquirectImage,
        settings: &IblSettings,
    ) -> Result<IblEnvironment, IblError> {
        settings.validate()?;
        check_device(device, Some(image), settings)?;
        let brdf_lut = self.brdf_lut(device, settings)?;
        let pipelines = self.pipelines(device)?;

        let mut temporaries = Temporaries::default();
        let mut owned = Vec::new();
        let outcome = record_environment(device, &pipelines, image, settings, &mut temporaries, &mut owned);
        temporaries.release(device);
        let (irradiance, prefiltered) = outcome.inspect_err(|_| {
            owned
                .iter()
                .rev()
                .for_each(|resource: &Resource| device.destroy(*resource));
        })?;

        self.generation += 1;
        Ok(IblEnvironment {
            irradiance,
            prefiltered,
            brdf_lut,
            sampler: pipelines.sampler,
            prefiltered_levels: settings.prefiltered_levels(),
            generation: self.generation,
        })
    }

    /// The BRDF table for `settings`: generated by compute on the first request, cached afterwards.
    pub fn brdf_lut(&mut self, device: &dyn DeviceApi, settings: &IblSettings) -> Result<IblTexture, IblError> {
        settings.validate()?;
        let key = (settings.lut_size, settings.lut_samples);
        if let Some(lut) = self.luts.get(&key) {
            return Ok(*lut);
        }
        check_device(device, None, settings)?;
        let pipelines = self.pipelines(device)?;
        let mut temporaries = Temporaries::default();
        let outcome = record_brdf_lut(device, &pipelines, settings, &mut temporaries);
        temporaries.release(device);
        let lut = outcome?;
        self.luts.insert(key, lut);
        Ok(lut)
    }

    /// Destroys the pipelines, the sampler and every cached table; environments keep their own cubes.
    pub fn release(&mut self, device: &dyn DeviceApi) {
        if let Some(pipelines) = self.pipelines.take() {
            pipelines.release(device);
        }
        for (_, lut) in self.luts.drain() {
            device.destroy(Resource::TextureView(lut.view));
            device.destroy(Resource::Texture(lut.texture));
        }
    }

    fn pipelines(&mut self, device: &dyn DeviceApi) -> Result<Pipelines, IblError> {
        if let Some(pipelines) = self.pipelines {
            return Ok(pipelines);
        }
        let pipelines = Pipelines::create(device)?;
        self.pipelines = Some(pipelines);
        Ok(pipelines)
    }
}

fn record_brdf_lut(
    device: &dyn DeviceApi,
    pipelines: &Pipelines,
    settings: &IblSettings,
    temporaries: &mut Temporaries,
) -> Result<IblTexture, IblError> {
    let size = settings.lut_size;
    let texture = device.create_texture(&TextureDesc::new_2d("ibl brdf lut", size, size, FORMAT, MAP_USAGE))?;
    let view = device.create_texture_view(texture, &TextureViewDesc::default())?;
    let storage = temporaries.view(device, texture, &TextureViewDesc::default())?;
    let params = LutParams {
        size,
        sample_count: settings.lut_samples,
        padding: [0; 2],
    };
    let group = {
        let uniform = temporaries.uniform(device, &params)?;
        temporaries.bind_group(
            device,
            &pipelines.brdf_lut,
            vec![uniform, BindingResource::TextureView(storage)],
        )?
    };
    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::empty(), Access::STORAGE_WRITE);
    dispatch(&mut list, "ibl brdf lut", &pipelines.brdf_lut, group, (size, size, 1));
    list.texture_access(texture, SubresourceRange::ALL, Access::STORAGE_WRITE, Access::SAMPLED);
    device.submit(list)?;
    Ok(IblTexture { texture, view })
}

fn cube_texture(device: &dyn DeviceApi, label: &'static str, size: u32, levels: u32) -> Result<TextureId, IblError> {
    Ok(device.create_texture(&TextureDesc::new_cube(label, size, FORMAT, MAP_USAGE).with_mips(levels))?)
}

// Returns (irradiance, prefiltered). Every object that must outlive the call is pushed on `owned` as soon as it
// exists, so a failure half way frees it.
fn record_environment(
    device: &dyn DeviceApi,
    pipelines: &Pipelines,
    image: &EquirectImage,
    settings: &IblSettings,
    temporaries: &mut Temporaries,
    owned: &mut Vec<Resource>,
) -> Result<(IblTexture, IblTexture), IblError> {
    let levels = settings.prefiltered_levels();
    let source_levels = settings.source_levels();
    let size = settings.cube_size;
    let (width, height) = (image.width(), image.height());

    let equirect = temporaries.keep(device.create_texture(&TextureDesc::new_2d(
        "ibl equirect",
        width,
        height,
        FORMAT,
        TextureUsage::SAMPLED | TextureUsage::COPY_DST,
    ))?);
    device.write_texture(
        &TextureCopy::whole(equirect),
        bytemuck::cast_slice(image.texels()),
        TexelLayout {
            offset: 0,
            bytes_per_row: width * 8,
            rows_per_image: height,
        },
        Extent3d::new_2d(width, height),
    )?;
    let source = temporaries.keep(cube_texture(device, "ibl source", size, source_levels)?);
    let prefiltered = cube_texture(device, "ibl prefiltered", size, levels)?;
    owned.push(Resource::Texture(prefiltered));
    let irradiance = cube_texture(device, "ibl irradiance", settings.irradiance_size, 1)?;
    owned.push(Resource::Texture(irradiance));

    let mut list = CommandList::new();
    list.texture_access(equirect, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);

    // The scratch cube: the picture resampled per face, then a box-filtered mip chain for filtered importance sampling.
    let samples_per_axis = (width.div_ceil(4 * size)).clamp(1, MAX_SAMPLES_PER_AXIS);
    let equirect_view = temporaries.view(device, equirect, &TextureViewDesc::default())?;
    let first_mip = temporaries.view(device, source, &mip_layers(0))?;
    let uniform = temporaries.uniform(
        device,
        &EquirectParams {
            face_size: size,
            samples_per_axis,
            padding: [0; 2],
        },
    )?;
    let group = temporaries.bind_group(
        device,
        &pipelines.equirect_to_cube,
        vec![
            uniform,
            BindingResource::TextureView(equirect_view),
            BindingResource::TextureView(first_mip),
        ],
    )?;
    write_mip(
        &mut list,
        source,
        0,
        "ibl equirect to cube",
        &pipelines.equirect_to_cube,
        group,
        size,
    );
    for mip in 1..source_levels {
        let previous = temporaries.view(device, source, &mip_layers(mip - 1))?;
        let current = temporaries.view(device, source, &mip_layers(mip))?;
        let group = temporaries.bind_group(
            device,
            &pipelines.downsample_cube,
            vec![
                BindingResource::TextureView(previous),
                BindingResource::TextureView(current),
            ],
        )?;
        write_mip(
            &mut list,
            source,
            mip,
            "ibl downsample",
            &pipelines.downsample_cube,
            group,
            size >> mip,
        );
    }

    // Mip m of the prefiltered cube is the environment convolved for roughness m / (levels - 1).
    let source_cube = temporaries.view(device, source, &TextureViewDesc::cube())?;
    let texel_solid_angle = 4.0 * PI / (FACES as f32 * (size as f32) * (size as f32));
    for mip in 0..levels {
        let face_size = size >> mip;
        let uniform = temporaries.uniform(
            device,
            &PrefilterParams {
                face_size,
                sample_count: settings.prefilter_samples,
                roughness: mip as f32 / (levels - 1) as f32,
                texel_solid_angle,
                top_lod: (source_levels - 1) as f32,
                padding: [0; 3],
            },
        )?;
        let destination = temporaries.view(device, prefiltered, &mip_layers(mip))?;
        let group = temporaries.bind_group(
            device,
            &pipelines.prefilter,
            vec![
                uniform,
                BindingResource::TextureView(source_cube),
                BindingResource::Sampler(pipelines.sampler),
                BindingResource::TextureView(destination),
            ],
        )?;
        write_mip(
            &mut list,
            prefiltered,
            mip,
            "ibl prefilter",
            &pipelines.prefilter,
            group,
            face_size,
        );
    }

    // The irradiance cube reads the smallest source mip that still has IRRADIANCE_SOURCE_FACE texels per face.
    let source_mip = (source_levels - 1).min(size.ilog2().saturating_sub(IRRADIANCE_SOURCE_FACE.ilog2()));
    let source_face = temporaries.view(device, source, &mip_layers(source_mip))?;
    let destination = temporaries.view(device, irradiance, &mip_layers(0))?;
    let uniform = temporaries.uniform(
        device,
        &IrradianceParams {
            face_size: settings.irradiance_size,
            padding: [0; 3],
        },
    )?;
    let group = temporaries.bind_group(
        device,
        &pipelines.irradiance,
        vec![
            uniform,
            BindingResource::TextureView(source_face),
            BindingResource::TextureView(destination),
        ],
    )?;
    write_mip(
        &mut list,
        irradiance,
        0,
        "ibl irradiance",
        &pipelines.irradiance,
        group,
        settings.irradiance_size,
    );
    device.submit(list)?;

    let mut cube_view = |texture| -> Result<IblTexture, IblError> {
        let view = device.create_texture_view(texture, &TextureViewDesc::cube())?;
        owned.push(Resource::TextureView(view));
        Ok(IblTexture { texture, view })
    };
    Ok((cube_view(irradiance)?, cube_view(prefiltered)?))
}
