use crate::id::BufferId;

// Argument layouts are byte-identical to wgpu's `DrawIndirectArgs`, `DrawIndexedIndirectArgs` and
// `DispatchIndirectArgs`, which in turn match the Vulkan, D3D12 and Metal indirect structures. All fields are
// little-endian `u32` (`i32` for `base_vertex`), tightly packed.
macro_rules! indirect_args {
    ($(#[$meta:meta])* $name:ident, $size:literal, { $($field:ident: $ty:ty => $index:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[repr(C)]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
        pub struct $name {
            $(pub $field: $ty,)+
        }

        impl $name {
            pub const SIZE: u64 = $size;

            pub fn to_bytes(self) -> [u8; $size] {
                let mut bytes = [0; $size];
                $(bytes[$index * 4..$index * 4 + 4].copy_from_slice(&self.$field.to_le_bytes());)+
                bytes
            }

            pub fn from_bytes(bytes: [u8; $size]) -> Self {
                Self {
                    $($field: <$ty>::from_le_bytes([
                        bytes[$index * 4],
                        bytes[$index * 4 + 1],
                        bytes[$index * 4 + 2],
                        bytes[$index * 4 + 3],
                    ]),)+
                }
            }
        }
    };
}

indirect_args!(
    // One element of the buffer read by `draw_indirect` and `multi_draw_indirect`.
    DrawIndirectArgs, 16, {
        vertex_count: u32 => 0,
        instance_count: u32 => 1,
        first_vertex: u32 => 2,
        // Non-zero only with `Features::INDIRECT_FIRST_INSTANCE`; otherwise the draw is skipped.
        first_instance: u32 => 3,
    }
);

indirect_args!(
    // One element of the buffer read by `draw_indexed_indirect` and `multi_draw_indexed_indirect`.
    DrawIndexedIndirectArgs, 20, {
        index_count: u32 => 0,
        instance_count: u32 => 1,
        first_index: u32 => 2,
        base_vertex: i32 => 3,
        // Non-zero only with `Features::INDIRECT_FIRST_INSTANCE`; otherwise the draw is skipped.
        first_instance: u32 => 4,
    }
);

indirect_args!(
    // The buffer element read by `dispatch_indirect`.
    DispatchIndirectArgs, 12, {
        x: u32 => 0,
        y: u32 => 1,
        z: u32 => 2,
    }
);

// Indirect offsets (argument and count buffers alike) must be multiples of this.
pub const INDIRECT_ALIGNMENT: u64 = 4;

// Size of the `u32` a `…Count` draw reads from its count buffer.
pub const INDIRECT_COUNT_SIZE: u64 = 4;

// Rare and large (two buffers), so it is boxed in `Command` to keep the common commands small.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MultiDrawCount {
    pub args: BufferId,
    pub args_offset: u64,
    pub count_buffer: BufferId,
    pub count_offset: u64,
    // Upper bound: the draw count read from `count_buffer` is clamped to it, and `args` must hold this many elements.
    pub max_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_the_native_indirect_structures() {
        assert_eq!(std::mem::size_of::<DrawIndirectArgs>() as u64, DrawIndirectArgs::SIZE);
        assert_eq!(
            std::mem::size_of::<DrawIndexedIndirectArgs>() as u64,
            DrawIndexedIndirectArgs::SIZE
        );
        assert_eq!(
            std::mem::size_of::<DispatchIndirectArgs>() as u64,
            DispatchIndirectArgs::SIZE
        );
    }

    #[test]
    fn indexed_args_serialize_in_field_order_little_endian() {
        let args = DrawIndexedIndirectArgs {
            index_count: 1,
            instance_count: 2,
            first_index: 3,
            base_vertex: -4,
            first_instance: 5,
        };
        let mut expected = Vec::new();
        for word in [1_u32, 2, 3, (-4_i32).cast_unsigned(), 5] {
            expected.extend_from_slice(&word.to_le_bytes());
        }
        assert_eq!(args.to_bytes().as_slice(), expected);
        assert_eq!(DrawIndexedIndirectArgs::from_bytes(args.to_bytes()), args);
    }

    #[test]
    fn draw_and_dispatch_args_round_trip() {
        let draw = DrawIndirectArgs {
            vertex_count: 3,
            instance_count: 1,
            first_vertex: 7,
            first_instance: 0,
        };
        assert_eq!(DrawIndirectArgs::from_bytes(draw.to_bytes()), draw);
        let dispatch = DispatchIndirectArgs { x: 8, y: 4, z: 2 };
        assert_eq!(dispatch.to_bytes()[..4], 8_u32.to_le_bytes());
        assert_eq!(DispatchIndirectArgs::from_bytes(dispatch.to_bytes()), dispatch);
    }
}
