// A buddy allocator of square tiles in one square atlas: sizes are powers of two, a freed tile merges back with its
// three siblings, and the free tile that is picked is always the first in (y, x) order so the layout is reproducible.

use std::collections::BTreeSet;

/// A square region of the atlas, in texels.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Tile {
    pub x: u32,
    pub y: u32,
    pub size: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct Atlas {
    size: u32,
    min_tile: u32,
    // Free tiles of order `k` (size `min_tile << k`), as `(y, x)` so the first one is the top-left-most.
    free: Vec<BTreeSet<(u32, u32)>>,
}

impl Atlas {
    /// `size` and `min_tile` are rounded up to powers of two; the atlas is at least one smallest tile.
    pub(crate) fn new(size: u32, min_tile: u32) -> Self {
        let min_tile = min_tile.max(1).next_power_of_two();
        let size = size.max(min_tile).next_power_of_two();
        let orders = (size / min_tile).ilog2() as usize + 1;
        let mut free = vec![BTreeSet::new(); orders];
        free[orders - 1].insert((0, 0));
        Self { size, min_tile, free }
    }

    pub(crate) fn size(&self) -> u32 {
        self.size
    }

    pub(crate) fn min_tile(&self) -> u32 {
        self.min_tile
    }

    fn order_of(&self, size: u32) -> Option<usize> {
        let size = size.max(self.min_tile).next_power_of_two();
        (size <= self.size).then(|| (size / self.min_tile).ilog2() as usize)
    }

    /// A tile of `size` rounded up to a power of two (at least the smallest tile), or `None` when no free block is big
    /// enough.
    pub(crate) fn allocate(&mut self, size: u32) -> Option<Tile> {
        let wanted = self.order_of(size)?;
        let found = (wanted..self.free.len()).find(|order| !self.free[*order].is_empty())?;
        let (y, x) = self.free[found].pop_first()?;
        // Split down to the wanted order; the first quarter is kept, the other three stay free.
        for order in (wanted..found).rev() {
            let half = self.min_tile << order;
            for (dx, dy) in [(half, 0), (0, half), (half, half)] {
                self.free[order].insert((y + dy, x + dx));
            }
        }
        Some(Tile {
            x,
            y,
            size: self.min_tile << wanted,
        })
    }

    pub(crate) fn release(&mut self, tile: Tile) {
        let Some(mut order) = self.order_of(tile.size) else {
            return;
        };
        let (mut x, mut y) = (tile.x, tile.y);
        self.free[order].insert((y, x));
        while order + 1 < self.free.len() {
            let size = self.min_tile << order;
            let (parent_x, parent_y) = (x & !(2 * size - 1), y & !(2 * size - 1));
            let siblings = [(0, 0), (size, 0), (0, size), (size, size)].map(|(dx, dy)| (parent_y + dy, parent_x + dx));
            if !siblings.iter().all(|sibling| self.free[order].contains(sibling)) {
                break;
            }
            for sibling in siblings {
                self.free[order].remove(&sibling);
            }
            order += 1;
            (x, y) = (parent_x, parent_y);
            self.free[order].insert((y, x));
        }
    }

    /// Texels not allocated.
    #[cfg(test)]
    pub(crate) fn free_texels(&self) -> u64 {
        self.free
            .iter()
            .enumerate()
            .map(|(order, tiles)| {
                let side = u64::from(self.min_tile << order);
                side * side * tiles.len() as u64
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disjoint(a: Tile, b: Tile) -> bool {
        a.x + a.size <= b.x || b.x + b.size <= a.x || a.y + a.size <= b.y || b.y + b.size <= a.y
    }

    #[test]
    fn tiles_are_powers_of_two_inside_the_atlas_and_do_not_overlap() {
        let mut atlas = Atlas::new(1024, 64);
        let mut tiles = Vec::new();
        for size in [512, 100, 256, 64, 300, 128, 64, 64] {
            let tile = atlas.allocate(size).expect("room");
            assert!(tile.size.is_power_of_two() && tile.size >= size.min(512));
            assert!(tile.x + tile.size <= 1024 && tile.y + tile.size <= 1024);
            assert!(
                tile.x.is_multiple_of(tile.size) && tile.y.is_multiple_of(tile.size),
                "buddy tiles are aligned"
            );
            tiles.push(tile);
        }
        for (index, a) in tiles.iter().enumerate() {
            assert!(tiles[index + 1..].iter().all(|b| disjoint(*a, *b)), "{a:?} overlaps");
        }
    }

    #[test]
    fn a_full_atlas_refuses_and_releasing_everything_makes_it_whole_again() {
        let mut atlas = Atlas::new(256, 64);
        let tiles: Vec<Tile> = (0..16).map(|_| atlas.allocate(64).expect("sixteen fit")).collect();

        assert_eq!(atlas.allocate(64), None);
        assert_eq!(atlas.free_texels(), 0);
        for tile in tiles {
            atlas.release(tile);
        }

        assert_eq!(atlas.free_texels(), 256 * 256);
        assert_eq!(
            atlas.allocate(256).map(|tile| tile.size),
            Some(256),
            "the siblings merged back"
        );
    }

    #[test]
    fn a_request_larger_than_the_atlas_is_refused() {
        let mut atlas = Atlas::new(512, 64);

        assert_eq!(atlas.allocate(1024), None);
    }

    #[test]
    fn fragmentation_can_refuse_a_tile_although_the_area_is_free() {
        let mut atlas = Atlas::new(128, 64);
        let first = atlas.allocate(64).expect("first");
        let _second = atlas.allocate(64).expect("second");
        let _third = atlas.allocate(64).expect("third");
        atlas.release(first);

        assert_eq!(atlas.free_texels(), 2 * 64 * 64);
        assert_eq!(atlas.allocate(128), None);
    }
}
