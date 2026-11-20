use std::any::{Any, TypeId};
use std::collections::HashMap;

use pathon_rhi_types::MaybeSend;

#[cfg(not(target_arch = "wasm32"))]
type SharedValue = Box<dyn Any + Send>;
#[cfg(target_arch = "wasm32")]
type SharedValue = Box<dyn Any>;

// Data packs share beyond textures — scene buffers, light lists, material tables — keyed by type. One pack (or the
// application) inserts a value, others read it; neither depends on the other's crate, only on the shared type.
#[derive(Default)]
pub struct Shared {
    values: HashMap<TypeId, SharedValue>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared").field("values", &self.values.len()).finish()
    }
}

impl Shared {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<T: Any + MaybeSend>(&mut self, value: T) -> Option<T> {
        let previous = self.values.insert(TypeId::of::<T>(), Box::new(value))?;
        previous.downcast().ok().map(|boxed| *boxed)
    }

    pub fn get<T: Any>(&self) -> Option<&T> {
        self.values.get(&TypeId::of::<T>())?.downcast_ref()
    }

    pub fn get_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.values.get_mut(&TypeId::of::<T>())?.downcast_mut()
    }

    pub fn get_or_insert_with<T: Any + MaybeSend>(&mut self, create: impl FnOnce() -> T) -> &mut T {
        self.values
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(create()))
            .downcast_mut()
            .expect("the entry for TypeId::of::<T>() always holds a T")
    }

    pub fn remove<T: Any>(&mut self) -> Option<T> {
        self.values
            .remove(&TypeId::of::<T>())?
            .downcast()
            .ok()
            .map(|boxed| *boxed)
    }

    pub fn contains<T: Any>(&self) -> bool {
        self.values.contains_key(&TypeId::of::<T>())
    }

    pub(crate) fn absorb(&mut self, other: Self) {
        self.values.extend(other.values);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct LightCount(u32);

    #[test]
    fn values_are_found_by_type() {
        let mut shared = Shared::new();
        assert_eq!(shared.insert(LightCount(3)), None);
        shared.get_mut::<LightCount>().expect("inserted").0 += 1;

        assert_eq!(shared.get::<LightCount>(), Some(&LightCount(4)));
        assert_eq!(shared.insert(LightCount(9)), Some(LightCount(4)));
        assert_eq!(shared.remove::<LightCount>(), Some(LightCount(9)));
        assert!(!shared.contains::<LightCount>());
    }
}
