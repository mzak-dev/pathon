use std::fmt;
use std::marker::PhantomData;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RawHandle {
    index: u32,
    generation: u32,
}

impl RawHandle {
    pub const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    pub const fn index(self) -> u32 {
        self.index
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }
}

pub trait Id: Copy + Eq + fmt::Debug {
    const KIND: &'static str;

    fn from_raw(raw: RawHandle) -> Self;

    fn raw(self) -> RawHandle;
}

macro_rules! define_ids {
    ($($name:ident => $kind:literal),* $(,)?) => {$(
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub struct $name(RawHandle);

        impl Id for $name {
            const KIND: &'static str = $kind;

            fn from_raw(raw: RawHandle) -> Self {
                Self(raw)
            }

            fn raw(self) -> RawHandle {
                self.0
            }
        }
    )*};
}

define_ids! {
    BufferId => "buffer",
    TextureId => "texture",
    TextureViewId => "texture view",
    SamplerId => "sampler",
    ShaderModuleId => "shader module",
    BindGroupLayoutId => "bind group layout",
    BindGroupId => "bind group",
    RenderPipelineId => "render pipeline",
    ComputePipelineId => "compute pipeline",
    QuerySetId => "query set",
    SurfaceId => "surface",
}

struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

pub struct Pool<I, T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    _id: PhantomData<fn() -> I>,
}

impl<I: Id, T> Pool<I, T> {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            _id: PhantomData,
        }
    }

    pub fn insert(&mut self, value: T) -> I {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.value = Some(value);
            return I::from_raw(RawHandle::new(index, slot.generation));
        }
        let index = u32::try_from(self.slots.len()).expect("pool holds fewer than u32::MAX objects");
        self.slots.push(Slot {
            generation: 0,
            value: Some(value),
        });
        I::from_raw(RawHandle::new(index, 0))
    }

    pub fn get(&self, id: I) -> Option<&T> {
        let raw = id.raw();
        let slot = self.slots.get(raw.index() as usize)?;
        if slot.generation != raw.generation() {
            return None;
        }
        slot.value.as_ref()
    }

    pub fn get_mut(&mut self, id: I) -> Option<&mut T> {
        let raw = id.raw();
        let slot = self.slots.get_mut(raw.index() as usize)?;
        if slot.generation != raw.generation() {
            return None;
        }
        slot.value.as_mut()
    }

    pub fn remove(&mut self, id: I) -> Option<T> {
        let raw = id.raw();
        let slot = self.slots.get_mut(raw.index() as usize)?;
        if slot.generation != raw.generation() {
            return None;
        }
        let value = slot.value.take()?;
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(raw.index());
        Some(value)
    }

    pub fn contains(&self, id: I) -> bool {
        self.get(id).is_some()
    }

    pub fn len(&self) -> usize {
        self.slots.len() - self.free.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = (I, &T)> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            let index = u32::try_from(index).ok()?;
            let value = slot.value.as_ref()?;
            Some((I::from_raw(RawHandle::new(index, slot.generation)), value))
        })
    }

    pub fn clear(&mut self) {
        let ids: Vec<I> = self.iter().map(|(id, _)| id).collect();
        for id in ids {
            self.remove(id);
        }
    }
}

impl<I: Id, T> Default for Pool<I, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: Id, T> fmt::Debug for Pool<I, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("kind", &I::KIND)
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_handle_does_not_reach_reused_slot() {
        let mut pool: Pool<BufferId, &str> = Pool::new();
        let first = pool.insert("first");
        pool.remove(first);
        let second = pool.insert("second");

        assert_eq!(first.raw().index(), second.raw().index());
        assert_eq!(pool.get(first), None);
        assert_eq!(pool.get(second), Some(&"second"));
    }

    #[test]
    fn len_counts_only_live_objects() {
        let mut pool: Pool<TextureId, u32> = Pool::new();
        let a = pool.insert(1);
        pool.insert(2);
        pool.remove(a);

        assert_eq!(pool.len(), 1);
        assert_eq!(pool.iter().count(), 1);
    }
}
