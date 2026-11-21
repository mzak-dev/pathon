use bitflags::bitflags;

bitflags! {
    // The state a resource is in after a barrier: every way it may be used until the next barrier.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct Access: u16 {
        const COPY_SRC = 1 << 0;
        const COPY_DST = 1 << 1;
        const SAMPLED = 1 << 2;
        const STORAGE_WRITE = 1 << 3;
        const COLOR_ATTACHMENT = 1 << 4;
        const DEPTH_READ = 1 << 5;
        const DEPTH_WRITE = 1 << 6;
        const PRESENT = 1 << 7;
        const VERTEX = 1 << 8;
        const INDEX = 1 << 9;
        const INDIRECT = 1 << 10;
        const UNIFORM = 1 << 11;
        const STORAGE_READ = 1 << 12;

        const WRITES = Self::COPY_DST.bits()
            | Self::STORAGE_WRITE.bits()
            | Self::COLOR_ATTACHMENT.bits()
            | Self::DEPTH_WRITE.bits();
    }
}

impl Access {
    #[must_use]
    pub fn writes(self) -> Self {
        self.intersection(Self::WRITES)
    }

    // A state may combine any reads, or exactly one kind of write; storage and depth read-write are the mixed cases.
    pub fn is_valid_state(self) -> bool {
        let writes = self.writes();
        if writes.is_empty() {
            return !self.contains(Self::PRESENT) || self == Self::PRESENT;
        }
        if writes.bits().count_ones() > 1 {
            return false;
        }
        let reads = self.difference(writes);
        match writes {
            Self::STORAGE_WRITE => reads.is_empty() || reads == Self::STORAGE_READ,
            Self::DEPTH_WRITE => reads.is_empty() || reads == Self::DEPTH_READ,
            _ => reads.is_empty(),
        }
    }

    // A texture also has a layout, so its reads must all belong to one layout class.
    pub fn is_valid_texture_state(self) -> bool {
        const LAYOUT_CLASSES: [Access; 7] = [
            Access::SAMPLED.union(Access::DEPTH_READ),
            Access::STORAGE_READ.union(Access::STORAGE_WRITE),
            Access::COPY_SRC,
            Access::COLOR_ATTACHMENT,
            Access::DEPTH_READ.union(Access::DEPTH_WRITE),
            Access::PRESENT,
            Access::COPY_DST,
        ];
        self.is_valid_state() && LAYOUT_CLASSES.iter().any(|class| class.contains(self))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SubresourceRange {
    pub base_mip: u32,
    pub mip_count: u32,
    pub base_layer: u32,
    pub layer_count: u32,
}

impl SubresourceRange {
    pub const ALL: Self = Self {
        base_mip: 0,
        mip_count: u32::MAX,
        base_layer: 0,
        layer_count: u32::MAX,
    };

    pub const fn mip(level: u32) -> Self {
        Self {
            base_mip: level,
            mip_count: 1,
            base_layer: 0,
            layer_count: u32::MAX,
        }
    }

    #[must_use]
    pub fn resolve(self, mip_levels: u32, layers: u32) -> Self {
        let base_mip = self.base_mip.min(mip_levels);
        let base_layer = self.base_layer.min(layers);
        Self {
            base_mip,
            mip_count: self.mip_count.min(mip_levels - base_mip),
            base_layer,
            layer_count: self.layer_count.min(layers - base_layer),
        }
    }

    pub fn subresources(self, mip_levels: u32, layers: u32) -> impl Iterator<Item = (u32, u32)> {
        let range = self.resolve(mip_levels, layers);
        let mips = range.base_mip..range.base_mip + range.mip_count;
        mips.flat_map(move |mip| {
            (range.base_layer..range.base_layer + range.layer_count).map(move |layer| (mip, layer))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_combine_but_writes_do_not() {
        assert!((Access::SAMPLED | Access::DEPTH_READ).is_valid_state());
        assert!((Access::STORAGE_READ | Access::STORAGE_WRITE).is_valid_state());
        assert!(!(Access::COLOR_ATTACHMENT | Access::STORAGE_WRITE).is_valid_state());
        assert!(!(Access::COLOR_ATTACHMENT | Access::SAMPLED).is_valid_state());
        assert!(!(Access::PRESENT | Access::SAMPLED).is_valid_state());
    }

    #[test]
    fn a_texture_state_keeps_its_reads_in_one_layout_class() {
        assert!((Access::SAMPLED | Access::DEPTH_READ).is_valid_texture_state());
        assert!((Access::STORAGE_READ | Access::STORAGE_WRITE).is_valid_texture_state());
        assert!(!(Access::SAMPLED | Access::COPY_SRC).is_valid_texture_state());
        assert!(!(Access::SAMPLED | Access::STORAGE_READ).is_valid_texture_state());
    }

    #[test]
    fn buffer_only_bits_are_not_texture_states() {
        assert!(!Access::VERTEX.is_valid_texture_state());
        assert!(!(Access::SAMPLED | Access::UNIFORM).is_valid_texture_state());
        assert!((Access::DEPTH_READ | Access::DEPTH_WRITE).is_valid_texture_state());
        assert!(!(Access::SAMPLED | Access::DEPTH_READ | Access::DEPTH_WRITE).is_valid_texture_state());
        assert!(!(Access::STORAGE_READ | Access::COPY_SRC).is_valid_texture_state());
        assert!(Access::PRESENT.is_valid_texture_state());
        assert!(Access::COLOR_ATTACHMENT.is_valid_texture_state());
    }

    #[test]
    fn buffer_reads_combine_freely() {
        assert!((Access::STORAGE_READ | Access::UNIFORM | Access::VERTEX).is_valid_state());
    }

    #[test]
    fn whole_range_clamps_to_texture() {
        let subresources: Vec<_> = SubresourceRange::ALL.subresources(2, 2).collect();
        assert_eq!(subresources, vec![(0, 0), (0, 1), (1, 0), (1, 1)]);
        assert_eq!(
            SubresourceRange::mip(1).subresources(3, 1).collect::<Vec<_>>(),
            vec![(1, 0)]
        );
    }
}
