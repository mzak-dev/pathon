use std::collections::HashMap;
use std::num::NonZeroU32;

use pathon_api::Diagnostic;
use pathon_api::resources::DeferredRelease;
use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupId, BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId,
    BindingResource, BindingType, BufferDesc, BufferId, BufferUsage, CommandList, DeviceApi, Extent3d, Format,
    SamplerBindingType, SamplerId, ShaderStages, SubresourceRange, TexelLayout, TextureCopy, TextureDesc, TextureId,
    TextureUsage, TextureViewDesc, TextureViewId,
};

use crate::error::{BindlessShortfall, fallback_diagnostic};
use crate::path::{DEFAULT_CAPACITY, choose_path};
use crate::wgsl::{
    BINDLESS_FIRST_SAMPLER_BINDING, BINDLESS_TEXTURES_BINDING, MATERIAL_FIRST_SAMPLER_BINDING,
    MATERIAL_FIRST_TEXTURE_BINDING, MATERIAL_SLOTS_BINDING, bindless_texture_source, per_material_texture_source,
};
use crate::{
    BindlessError, MAX_MATERIAL_TEXTURES, PathPreference, SamplerPreset, TextureHandle, TexturePath, TextureTable,
};

const NO_TEXTURE: u32 = u32::MAX;
const SLOTS_UNIFORM_SIZE: u64 = (MAX_MATERIAL_TEXTURES * 4) as u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BinderDesc {
    pub label: &'static str,
    pub preference: PathPreference,
    /// Slots of the texture table; on the bindless path also the length of the `binding_array`, clamped to the device
    /// limit.
    pub capacity: u32,
    pub frames_in_flight: u32,
    /// Stages that sample material textures.
    pub visibility: ShaderStages,
}

impl Default for BinderDesc {
    fn default() -> Self {
        Self {
            label: "material textures",
            preference: PathPreference::Auto,
            capacity: DEFAULT_CAPACITY,
            frames_in_flight: 2,
            visibility: ShaderStages::FRAGMENT | ShaderStages::COMPUTE,
        }
    }
}

#[derive(Debug)]
struct BindlessGroup {
    group: Option<BindGroupId>,
    built_at_revision: u64,
}

#[derive(Debug)]
struct MaterialSet {
    textures: Vec<TextureHandle>,
    group: BindGroupId,
    slots: BufferId,
}

/// Textures of materials as one bind group, whichever way the device can bind them (ADR-0036): the bindless path has
/// one group with a `binding_array` shared by every material, the per-material path one group per material with the
/// same logical slots. The renderer calls the same methods on both; only the WGSL of
/// `package::material::texture_source` ([`Self::texture_source_wgsl`]) differs.
#[derive(Debug)]
pub struct TextureBinder {
    label: &'static str,
    path: TexturePath,
    shortfall: Option<BindlessShortfall>,
    table: TextureTable,
    layout: BindGroupLayoutId,
    layout_desc: BindGroupLayoutDesc,
    samplers: [SamplerId; SamplerPreset::COUNT],
    placeholder_texture: TextureId,
    placeholder: TextureViewId,
    release: DeferredRelease,
    frame: u64,
    bindless: Option<BindlessGroup>,
    materials: HashMap<u32, MaterialSet>,
    group_builds: u64,
}

impl TextureBinder {
    pub fn new(device: &dyn DeviceApi, desc: &BinderDesc) -> Result<Self, BindlessError> {
        let choice = choose_path(device.caps(), desc.preference, desc.capacity)?;
        let table_capacity = match choice.path {
            TexturePath::Bindless { capacity } => capacity,
            TexturePath::PerMaterial => desc.capacity,
        };
        let table = TextureTable::new(table_capacity, desc.frames_in_flight)?;
        let samplers = create_samplers(device)?;
        let (placeholder_texture, placeholder) = create_placeholder(device)?;
        let layout_desc = layout_desc(desc, choice.path);
        let layout = device.create_bind_group_layout(&layout_desc)?;
        Ok(Self {
            label: desc.label,
            path: choice.path,
            shortfall: choice.shortfall,
            table,
            layout,
            layout_desc,
            samplers,
            placeholder_texture,
            placeholder,
            release: DeferredRelease::new(u64::from(desc.frames_in_flight)),
            frame: 0,
            bindless: matches!(choice.path, TexturePath::Bindless { .. }).then_some(BindlessGroup {
                group: None,
                built_at_revision: 0,
            }),
            materials: HashMap::new(),
            group_builds: 0,
        })
    }

    pub fn path(&self) -> TexturePath {
        self.path
    }

    /// `B0001` when [`PathPreference::Auto`] wanted bindless and the device could not; show it once at startup.
    pub fn fallback_diagnostic(&self) -> Option<Diagnostic> {
        self.shortfall.as_ref().map(fallback_diagnostic)
    }

    pub fn table(&self) -> &TextureTable {
        &self.table
    }

    /// The layout of the group [`Self::bind_group`] returns; build the pipeline layout with it at the `group` number
    /// that [`Self::texture_source_wgsl`] was asked for.
    pub fn layout(&self) -> BindGroupLayoutId {
        self.layout
    }

    /// The description `layout` was created from, for comparing with what shader reflection reports.
    pub fn layout_desc(&self) -> &BindGroupLayoutDesc {
        &self.layout_desc
    }

    /// Source of `package::material::texture_source` for this path, with the bindings in `group`. Give it to
    /// `MaterialHooks::set_texture_source` (`pathon-material`) before registering the material modules.
    pub fn texture_source_wgsl(&self, group: u32) -> String {
        match self.path {
            TexturePath::Bindless { capacity } => bindless_texture_source(group, capacity),
            TexturePath::PerMaterial => per_material_texture_source(group),
        }
    }

    /// Registers a 2D, filterable, float-sampled view that is in the `SAMPLED` state when a pass reads it.
    pub fn register(&mut self, view: TextureViewId, sampler: SamplerPreset) -> Result<TextureHandle, BindlessError> {
        self.table.add(view, sampler)
    }

    /// Removes the texture; every per-material group that holds it is dropped (and rebuilt on the next
    /// `bind_group`, which then fails with `B0004` for this handle). Returns the view: destroy it no earlier than
    /// `frames_in_flight` frames later.
    pub fn unregister(&mut self, handle: TextureHandle) -> Result<TextureViewId, BindlessError> {
        let view = self.table.remove(handle)?;
        let affected: Vec<u32> = self
            .materials
            .iter()
            .filter(|(_, set)| set.textures.contains(&handle))
            .map(|(material, _)| *material)
            .collect();
        for material in affected {
            self.forget_material(material);
        }
        Ok(view)
    }

    /// The value for `MaterialTextures` (`TextureSlot(value)`): same on both paths.
    pub fn shader_value(&self, handle: TextureHandle) -> Result<u32, BindlessError> {
        self.table.shader_value(handle)
    }

    /// Once per frame, before anything else: frees slots and bind groups that no list in flight can read any more.
    pub fn begin_frame(&mut self, device: &dyn DeviceApi, frame: u64) {
        self.frame = frame;
        self.table.begin_frame(frame);
        self.release.collect(frame, device);
    }

    /// Bindless: rebuilds the array group when textures were added or removed since the last build. One call after a
    /// batch of `register` / `unregister` makes one rebuild; `bind_group` calls it when the group is out of date, so
    /// forgetting it costs a rebuild per interleaved change, not correctness. Per-material: nothing to do.
    pub fn prepare(&mut self, device: &dyn DeviceApi) -> Result<(), BindlessError> {
        let Some(state) = &self.bindless else {
            return Ok(());
        };
        let revision = self.table.revision();
        if state.group.is_some() && state.built_at_revision == revision {
            return Ok(());
        }
        let views: Vec<TextureViewId> = self
            .table
            .views()
            .map(|view| view.unwrap_or(self.placeholder))
            .collect();
        let mut entries = vec![BindGroupEntry {
            binding: BINDLESS_TEXTURES_BINDING,
            resource: BindingResource::TextureViewArray(views),
        }];
        entries.extend(self.sampler_entries(BINDLESS_FIRST_SAMPLER_BINDING));
        let group = device.create_bind_group(&BindGroupDesc {
            label: self.label,
            layout: self.layout,
            entries,
        })?;
        self.group_builds += 1;
        let frame = self.frame;
        if let Some(state) = &mut self.bindless {
            if let Some(old) = state.group.replace(group) {
                self.release.release(frame, old);
            }
            state.built_at_revision = revision;
        }
        Ok(())
    }

    /// The group to bind for a draw of `material` (the logical index of `MaterialTable`) that uses `textures`: the one
    /// shared group on the bindless path (`material` and `textures` are only checked), the material's own group on the
    /// per-material path (built on the first call and when `textures` change). List every texture the material
    /// stores, in any order, at most [`MAX_MATERIAL_TEXTURES`].
    pub fn bind_group(
        &mut self,
        device: &dyn DeviceApi,
        material: u32,
        textures: &[TextureHandle],
    ) -> Result<BindGroupId, BindlessError> {
        for handle in textures {
            self.table.view(*handle)?;
        }
        if textures.len() > MAX_MATERIAL_TEXTURES {
            return Err(BindlessError::TooManyMaterialTextures {
                material,
                requested: textures.len(),
                max: MAX_MATERIAL_TEXTURES,
            });
        }
        if self.bindless.is_some() {
            self.prepare(device)?;
            let group = self.bindless.as_ref().and_then(|state| state.group);
            return Ok(group.expect("prepare builds the group"));
        }
        if let Some(set) = self.materials.get(&material)
            && set.textures == textures
        {
            return Ok(set.group);
        }
        let set = self.build_material_set(device, textures)?;
        let group = set.group;
        if let Some(old) = self.materials.insert(material, set) {
            self.release_set(&old);
        }
        Ok(group)
    }

    /// Drops the per-material group of a removed material (its texture handles stay registered).
    pub fn forget_material(&mut self, material: u32) {
        if let Some(set) = self.materials.remove(&material) {
            self.release_set(&set);
        }
    }

    /// Bind groups built so far; a test sees that a batch of changes rebuilt the array group once.
    pub fn group_builds(&self) -> u64 {
        self.group_builds
    }

    pub fn material_group_count(&self) -> usize {
        self.materials.len()
    }

    /// Resources replaced or dropped that still wait for their frames in flight before they are destroyed.
    pub fn pending_releases(&self) -> usize {
        self.release.len()
    }

    /// Destroys everything the binder created, including bind groups still waiting for their frames. Call after the
    /// device is idle.
    pub fn release(&mut self, device: &dyn DeviceApi) {
        let materials: Vec<u32> = self.materials.keys().copied().collect();
        for material in materials {
            self.forget_material(material);
        }
        if let Some(group) = self.bindless.as_mut().and_then(|state| state.group.take()) {
            device.destroy(group.into());
        }
        self.release.collect(u64::MAX, device);
        for sampler in self.samplers {
            device.destroy(sampler.into());
        }
        device.destroy(self.placeholder.into());
        device.destroy(self.placeholder_texture.into());
        device.destroy(self.layout.into());
    }

    /// After device loss every handle is invalid: the next `prepare` / `bind_group` needs a new binder.
    pub fn forget_gpu(&mut self) {
        self.release.forget_all();
        self.materials.clear();
    }

    fn sampler_entries(&self, first_binding: u32) -> impl Iterator<Item = BindGroupEntry> + '_ {
        self.samplers
            .iter()
            .zip(first_binding..)
            .map(|(sampler, binding)| BindGroupEntry {
                binding,
                resource: BindingResource::Sampler(*sampler),
            })
    }

    fn build_material_set(
        &mut self,
        device: &dyn DeviceApi,
        textures: &[TextureHandle],
    ) -> Result<MaterialSet, BindlessError> {
        let mut slot_values = [NO_TEXTURE; MAX_MATERIAL_TEXTURES];
        let mut entries = Vec::with_capacity(1 + MAX_MATERIAL_TEXTURES + SamplerPreset::COUNT);
        let slots = device.create_buffer(&BufferDesc {
            label: "material texture slots",
            size: SLOTS_UNIFORM_SIZE,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        })?;
        for (binding_slot, handle) in textures.iter().enumerate() {
            slot_values[binding_slot] = self.table.shader_value(*handle)?;
        }
        device.write_buffer(slots, 0, bytemuck::cast_slice(&slot_values))?;
        entries.push(BindGroupEntry {
            binding: MATERIAL_SLOTS_BINDING,
            resource: BindingResource::whole_buffer(slots),
        });
        for position in 0..MAX_MATERIAL_TEXTURES {
            let view = match textures.get(position) {
                Some(handle) => self.table.view(*handle)?,
                None => self.placeholder,
            };
            entries.push(BindGroupEntry {
                binding: MATERIAL_FIRST_TEXTURE_BINDING + u32::try_from(position).expect("a few textures"),
                resource: BindingResource::TextureView(view),
            });
        }
        entries.extend(self.sampler_entries(MATERIAL_FIRST_SAMPLER_BINDING));
        let group = device
            .create_bind_group(&BindGroupDesc {
                label: self.label,
                layout: self.layout,
                entries,
            })
            .inspect_err(|_| device.destroy(slots.into()))?;
        self.group_builds += 1;
        Ok(MaterialSet {
            textures: textures.to_vec(),
            group,
            slots,
        })
    }

    fn release_set(&mut self, set: &MaterialSet) {
        self.release.release(self.frame, set.group);
        self.release.release(self.frame, set.slots);
    }
}

fn create_samplers(device: &dyn DeviceApi) -> Result<[SamplerId; SamplerPreset::COUNT], BindlessError> {
    let mut ids = [None; SamplerPreset::COUNT];
    for (id, preset) in ids.iter_mut().zip(SamplerPreset::ALL) {
        *id = Some(device.create_sampler(&preset.desc())?);
    }
    Ok(ids.map(|id| id.expect("every preset got a sampler")))
}

// A white texel for every empty slot, so a stale or unused element reads as "no texture" and not as undefined data.
fn create_placeholder(device: &dyn DeviceApi) -> Result<(TextureId, TextureViewId), BindlessError> {
    let texture = device.create_texture(&TextureDesc::new_2d(
        "material texture placeholder",
        1,
        1,
        Format::Rgba8Unorm,
        TextureUsage::SAMPLED | TextureUsage::COPY_DST,
    ))?;
    device.write_texture(
        &TextureCopy::whole(texture),
        &[255; 4],
        TexelLayout {
            offset: 0,
            bytes_per_row: 4,
            rows_per_image: 1,
        },
        Extent3d::new_2d(1, 1),
    )?;
    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_DST, Access::SAMPLED);
    device.submit(list)?;
    let view = device.create_texture_view(texture, &TextureViewDesc::default())?;
    Ok((texture, view))
}

fn layout_desc(desc: &BinderDesc, path: TexturePath) -> BindGroupLayoutDesc {
    let entry = |binding: u32, ty: BindingType, count: Option<NonZeroU32>| BindGroupLayoutEntry {
        binding,
        visibility: desc.visibility,
        ty,
        count,
    };
    let sampler = || BindingType::Sampler(SamplerBindingType::Filtering);
    let mut entries = Vec::new();
    match path {
        TexturePath::Bindless { capacity } => {
            entries.push(entry(
                BINDLESS_TEXTURES_BINDING,
                BindingType::texture_2d(),
                NonZeroU32::new(capacity),
            ));
            for preset in 0..SamplerPreset::COUNT {
                entries.push(entry(
                    BINDLESS_FIRST_SAMPLER_BINDING + u32::try_from(preset).expect("four presets"),
                    sampler(),
                    None,
                ));
            }
        }
        TexturePath::PerMaterial => {
            entries.push(entry(MATERIAL_SLOTS_BINDING, BindingType::UniformBuffer, None));
            for texture in 0..MAX_MATERIAL_TEXTURES {
                entries.push(entry(
                    MATERIAL_FIRST_TEXTURE_BINDING + u32::try_from(texture).expect("a few textures"),
                    BindingType::texture_2d(),
                    None,
                ));
            }
            for preset in 0..SamplerPreset::COUNT {
                entries.push(entry(
                    MATERIAL_FIRST_SAMPLER_BINDING + u32::try_from(preset).expect("four presets"),
                    sampler(),
                    None,
                ));
            }
        }
    }
    BindGroupLayoutDesc {
        label: desc.label,
        entries,
    }
}
