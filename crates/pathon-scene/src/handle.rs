use pathon_api::rhi::{Id, RawHandle};

macro_rules! scene_ids {
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

scene_ids! {
    InstanceId => "instance",
    CameraId => "camera",
    LightId => "light",
}

// Handles to resources the scene does not own. The owner of the resource (pathon-mesh, pathon-material, a host
// adapter) keeps its own generational id; the scene stores only its raw form and gives it back unchanged.
macro_rules! external_handles {
    ($($name:ident),* $(,)?) => {$(
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub struct $name(RawHandle);

        impl $name {
            pub fn from_id(id: impl Id) -> Self {
                Self(id.raw())
            }

            pub fn to_id<I: Id>(self) -> I {
                I::from_raw(self.0)
            }

            pub fn raw(self) -> RawHandle {
                self.0
            }
        }
    )*};
}

external_handles!(MeshHandle, MaterialHandle);

impl MeshHandle {
    // Placeholder in free slots: it names no mesh.
    pub const NONE: Self = Self(RawHandle::new(u32::MAX, 0));
}

impl MaterialHandle {
    // The pack's fallback material: for instances that name none.
    pub const DEFAULT: Self = Self(RawHandle::new(u32::MAX, 0));
}

impl Default for MaterialHandle {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use pathon_api::rhi::BufferId;

    use super::*;

    #[test]
    fn external_handle_round_trips_the_owners_id() {
        let id = BufferId::from_raw(RawHandle::new(7, 3));

        let handle = MeshHandle::from_id(id);

        assert_eq!(handle.to_id::<BufferId>(), id);
        assert_eq!(handle.raw(), RawHandle::new(7, 3));
    }
}
