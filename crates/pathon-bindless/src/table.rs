use pathon_api::rhi::TextureViewId;

use crate::{BindlessError, SamplerPreset};

/// Largest capacity: the slot is the low 24 bits of the value a material stores, the sampler preset sits above them.
pub const MAX_CAPACITY: u32 = 1 << 24;
const INDEX_BITS: u32 = 24;

/// Generational handle to a texture of a [`TextureTable`]. A handle outlives its texture harmlessly: the generation
/// changes on `remove`, so a stale handle is rejected even when the slot already holds another texture.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TextureHandle {
    index: u32,
    generation: u32,
}

impl TextureHandle {
    /// The slot, the index of the element in the `binding_array` on the bindless path.
    pub fn index(self) -> u32 {
        self.index
    }
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    generation: u32,
    texture: Option<(TextureViewId, SamplerPreset)>,
}

/// The registry of textures materials refer to by a logical slot, independent of how a shader reaches them.
///
/// Capacity is fixed at creation: the length of the shader's `binding_array` is part of the compiled shader. A removed
/// slot becomes free for reuse after `frames_in_flight` frames (call [`Self::begin_frame`] once per frame), because a
/// recorded list may still read it ([ADR-0022](../../../docs/adr/0022-zegar-klatki-paczek.md)).
#[derive(Clone, Debug)]
pub struct TextureTable {
    slots: Vec<Slot>,
    free: Vec<u32>,
    waiting: Vec<(u64, u32)>,
    capacity: u32,
    frames_in_flight: u64,
    frame: u64,
    live: u32,
    revision: u64,
}

impl TextureTable {
    pub fn new(capacity: u32, frames_in_flight: u32) -> Result<Self, BindlessError> {
        if capacity == 0 || capacity > MAX_CAPACITY || frames_in_flight == 0 {
            return Err(BindlessError::InvalidConfiguration {
                reason: format!(
                    "capacity {capacity} (1..={MAX_CAPACITY}) with {frames_in_flight} frames in flight (at least 1)"
                ),
            });
        }
        Ok(Self {
            slots: Vec::new(),
            free: Vec::new(),
            waiting: Vec::new(),
            capacity,
            frames_in_flight: u64::from(frames_in_flight),
            frame: 0,
            live: 0,
            revision: 0,
        })
    }

    pub fn add(&mut self, view: TextureViewId, sampler: SamplerPreset) -> Result<TextureHandle, BindlessError> {
        let index = match self.free.pop() {
            Some(index) => index,
            None if self.slots.len() < self.capacity as usize => {
                self.slots.push(Slot {
                    generation: 0,
                    texture: None,
                });
                u32::try_from(self.slots.len() - 1).expect("the capacity is at most 2^24")
            }
            None => {
                return Err(BindlessError::TableFull {
                    capacity: self.capacity,
                    removed_waiting: u32::try_from(self.waiting.len()).expect("at most the capacity"),
                });
            }
        };
        let slot = &mut self.slots[index as usize];
        slot.texture = Some((view, sampler));
        self.live += 1;
        self.revision += 1;
        Ok(TextureHandle {
            index,
            generation: slot.generation,
        })
    }

    /// Frees the slot: the handle is dead at once, the slot is reused after `frames_in_flight` frames. Returns the view
    /// so the caller can destroy it, but not earlier than the same number of frames later: bind groups still in flight
    /// refer to it.
    pub fn remove(&mut self, handle: TextureHandle) -> Result<TextureViewId, BindlessError> {
        let (view, _) = self.entry(handle)?;
        let slot = &mut self.slots[handle.index as usize];
        slot.texture = None;
        slot.generation = slot.generation.wrapping_add(1);
        self.waiting.push((self.frame, handle.index));
        self.live -= 1;
        self.revision += 1;
        Ok(view)
    }

    /// Call once per frame: slots removed `frames_in_flight` or more frames ago become reusable.
    pub fn begin_frame(&mut self, frame: u64) {
        self.frame = frame;
        let (frames_in_flight, free) = (self.frames_in_flight, &mut self.free);
        self.waiting.retain(|&(removed_at, index)| {
            let reusable = removed_at + frames_in_flight <= frame;
            if reusable {
                free.push(index);
            }
            !reusable
        });
    }

    pub fn view(&self, handle: TextureHandle) -> Result<TextureViewId, BindlessError> {
        self.entry(handle).map(|(view, _)| view)
    }

    pub fn is_live(&self, handle: TextureHandle) -> bool {
        self.entry(handle).is_ok()
    }

    /// The value a material stores for this texture (`MaterialTextures::base_color = Some(TextureSlot(value))` in
    /// `pathon-material`): the slot in the low 24 bits and the sampler preset above them. It never equals the "no
    /// texture" marker `u32::MAX`, because a slot below [`MAX_CAPACITY`] never has all 24 bits set.
    pub fn shader_value(&self, handle: TextureHandle) -> Result<u32, BindlessError> {
        let (_, sampler) = self.entry(handle)?;
        Ok(encode_shader_value(handle.index, sampler))
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    pub fn live_count(&self) -> u32 {
        self.live
    }

    /// Slots of the array a shader can address, including those waiting for reuse.
    pub fn used_slots(&self) -> u32 {
        u32::try_from(self.slots.len()).expect("the capacity is at most 2^24")
    }

    /// Changes with every `add` and `remove`; a bind group built at an older revision is out of date.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// One entry per slot up to [`Self::capacity`]: the view of a live texture, `None` for empty and waiting slots.
    pub(crate) fn views(&self) -> impl Iterator<Item = Option<TextureViewId>> + '_ {
        (0..self.capacity as usize).map(|index| {
            self.slots
                .get(index)
                .and_then(|slot| slot.texture)
                .map(|(view, _)| view)
        })
    }

    fn entry(&self, handle: TextureHandle) -> Result<(TextureViewId, SamplerPreset), BindlessError> {
        self.slots
            .get(handle.index as usize)
            .filter(|slot| slot.generation == handle.generation)
            .and_then(|slot| slot.texture)
            .ok_or(BindlessError::StaleHandle { index: handle.index })
    }
}

pub(crate) fn encode_shader_value(index: u32, sampler: SamplerPreset) -> u32 {
    debug_assert!(index < MAX_CAPACITY - 1);
    index | (sampler.index() << INDEX_BITS)
}

/// Splits a value from [`TextureTable::shader_value`] into the slot and the sampler preset; `None` for the "no
/// texture" marker and for values with an unknown preset.
pub fn decode_shader_value(value: u32) -> Option<(u32, SamplerPreset)> {
    let preset = SamplerPreset::from_index(value >> INDEX_BITS)?;
    Some((value & (MAX_CAPACITY - 1), preset))
}

#[cfg(test)]
mod tests {
    use pathon_api::rhi::{DeviceApi, TextureDesc, TextureUsage, TextureViewDesc};

    use super::*;

    // Real ids: the table never looks inside a view, but the generational ids must differ.
    fn views(count: usize) -> Vec<TextureViewId> {
        let device = pathon_rhi::NullDevice::new();
        (0..count)
            .map(|_| {
                let texture = device
                    .create_texture(&TextureDesc::new_2d(
                        "t",
                        1,
                        1,
                        pathon_api::rhi::Format::Rgba8Unorm,
                        TextureUsage::SAMPLED,
                    ))
                    .expect("texture");
                device
                    .create_texture_view(texture, &TextureViewDesc::default())
                    .expect("view")
            })
            .collect()
    }

    #[test]
    fn slots_are_consecutive_and_the_value_carries_the_sampler() {
        let views = views(2);
        let mut table = TextureTable::new(8, 2).expect("valid");

        let first = table.add(views[0], SamplerPreset::LinearRepeat).expect("room");
        let second = table.add(views[1], SamplerPreset::NearestClamp).expect("room");

        assert_eq!((first.index(), second.index()), (0, 1));
        assert_eq!(table.shader_value(first), Ok(0));
        let value = table.shader_value(second).expect("live");
        assert_eq!(decode_shader_value(value), Some((1, SamplerPreset::NearestClamp)));
        assert_ne!(value, u32::MAX);
    }

    #[test]
    fn a_removed_slot_is_reused_only_after_the_frames_in_flight_and_the_old_handle_stays_dead() {
        let views = views(4);
        let mut table = TextureTable::new(8, 2).expect("valid");
        table.begin_frame(10);
        let old = table.add(views[0], SamplerPreset::default()).expect("room");
        table.add(views[1], SamplerPreset::default()).expect("room");

        assert_eq!(table.remove(old), Ok(views[0]));
        assert!(!table.is_live(old));
        table.begin_frame(11);
        let early = table.add(views[2], SamplerPreset::default()).expect("room");
        assert_eq!(early.index(), 2, "still in flight");
        table.begin_frame(12);
        let reused = table.add(views[3], SamplerPreset::default()).expect("room");
        assert_eq!(reused.index(), old.index(), "recycled");
        assert_ne!(reused, old, "the generation changed");
        assert_eq!(table.view(old).expect_err("stale").code(), "B0004");
        assert_eq!(table.view(reused), Ok(views[3]));
    }

    #[test]
    fn a_full_table_is_b0003_and_removal_makes_room_later() {
        let views = views(3);
        let mut table = TextureTable::new(2, 1).expect("valid");
        table.begin_frame(0);
        let first = table.add(views[0], SamplerPreset::default()).expect("room");
        table.add(views[1], SamplerPreset::default()).expect("room");

        assert_eq!(
            table.add(views[2], SamplerPreset::default()).expect_err("full").code(),
            "B0003"
        );
        table.remove(first).expect("live");
        assert!(
            table.add(views[2], SamplerPreset::default()).is_err(),
            "not reusable yet"
        );
        table.begin_frame(1);
        assert!(table.add(views[2], SamplerPreset::default()).is_ok());
    }

    #[test]
    fn removing_twice_or_a_foreign_handle_is_b0004() {
        let views = views(1);
        let mut table = TextureTable::new(4, 2).expect("valid");
        let handle = table.add(views[0], SamplerPreset::default()).expect("room");
        table.remove(handle).expect("live");

        assert_eq!(table.remove(handle).expect_err("stale").code(), "B0004");
        let foreign = TextureTable::new(4, 2).expect("valid");
        assert_eq!(foreign.view(handle).expect_err("unknown").code(), "B0004");
    }

    #[test]
    fn the_revision_moves_with_every_change() {
        let views = views(1);
        let mut table = TextureTable::new(4, 2).expect("valid");
        let start = table.revision();
        let handle = table.add(views[0], SamplerPreset::default()).expect("room");
        let added = table.revision();
        table.remove(handle).expect("live");

        assert!(start < added && added < table.revision());
    }

    #[test]
    fn invalid_configurations_are_b0006() {
        for (capacity, frames) in [(0, 2), (MAX_CAPACITY + 1, 2), (4, 0)] {
            assert_eq!(
                TextureTable::new(capacity, frames).expect_err("invalid").code(),
                "B0006"
            );
        }
    }

    #[test]
    fn views_lists_live_textures_and_gaps_up_to_the_capacity() {
        let views = views(2);
        let mut table = TextureTable::new(4, 2).expect("valid");
        let first = table.add(views[0], SamplerPreset::default()).expect("room");
        table.add(views[1], SamplerPreset::default()).expect("room");
        table.remove(first).expect("live");

        assert_eq!(table.views().collect::<Vec<_>>(), [None, Some(views[1]), None, None]);
    }
}
