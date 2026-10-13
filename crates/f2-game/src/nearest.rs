//! Phase 3S3: a uniform grid over the tile centres for `FindNearestTileS`
//! ([`crate::general::find_nearest_tile_s`]), which the original answers by scanning every
//! tile (Halloween 1's rotating enemy: ~1 ms per frame on the N64, one D-cache miss per
//! tile). Not game state: built from the tiles when the collision map is loaded, never
//! serialized, compared or printed; a map whose tile list has been replaced since (other
//! length or buffer) falls back to the scan. Tile centres are never written after the load.
//!
//! The answer is the scan's: the lowest-index tile with the smallest
//! `DistanceBetweenPointsSS(v, centre) >> 12`. While every tile's squared distance is below
//! `NEAREST_SAFE` (checked against the bounding box of all centres), that value is a
//! non-decreasing function `g` of the exact squared distance, so a cell whose bounding box
//! is farther than the best tile in `g` (strictly) cannot hold a better or tying tile, and
//! the search stops once the cells not yet visited are all that far.

use crate::prelude::*;

use crate::ids::TileId;
use crate::map::GameTile;
use f2_math::SVector;

/// Cells are `2^SHIFT` units square on x/z (y is covered by each cell's bounding box).
const SHIFT: u32 = 11;

#[derive(Clone, Default)]
pub struct TileGrid {
    tiles_ptr: usize,
    tiles_len: usize,
    origin: [i32; 2],
    dims: [i32; 2],
    /// Per cell (x-major within a z row): the first index into `items` (`cells + 1` entries).
    start: Vec<u32>,
    /// Tile indices grouped by cell, ascending within a cell.
    items: Vec<u32>,
    /// Per cell: the bounding box of its tile centres (`[min x, min y, min z, max x, max y, max z]`).
    bounds: Vec<[i16; 6]>,
    /// Bounding box of all centres.
    all: [i32; 6],
}

impl PartialEq for TileGrid {
    /// A cache: equal whatever it holds.
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for TileGrid {}

impl core::fmt::Debug for TileGrid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("TileGrid")
    }
}

/// `DistanceBetweenPointsSS(v, c) >> 12` as the scan computes it (wrapping squares).
#[inline(always)]
fn scan_dist(v: SVector, c: SVector) -> (u32, i32) {
    let (dx, dy, dz) = (v.vx as i32 - c.vx as i32, v.vy as i32 - c.vy as i32, v.vz as i32 - c.vz as i32);
    let u = dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy)).wrapping_add(dz.wrapping_mul(dz)) as u32;
    (u, f2_math::fsqrt(u as i32) >> 12)
}

impl TileGrid {
    pub fn build(tiles: &[GameTile]) -> TileGrid {
        if tiles.is_empty() {
            return TileGrid::default();
        }
        let mut all = [i32::MAX, i32::MAX, i32::MAX, i32::MIN, i32::MIN, i32::MIN];
        for t in tiles {
            let c = [t.centre.vx as i32, t.centre.vy as i32, t.centre.vz as i32];
            for k in 0..3 {
                all[k] = all[k].min(c[k]);
                all[k + 3] = all[k + 3].max(c[k]);
            }
        }
        let origin = [all[0], all[2]];
        let dims = [((all[3] - all[0]) >> SHIFT) + 1, ((all[5] - all[2]) >> SHIFT) + 1];
        let cells = (dims[0] * dims[1]) as usize;
        let cell_of = |t: &GameTile| {
            let ix = (t.centre.vx as i32 - origin[0]) >> SHIFT;
            let iz = (t.centre.vz as i32 - origin[1]) >> SHIFT;
            (iz * dims[0] + ix) as usize
        };
        let mut count = alloc::vec![0u32; cells + 1];
        for t in tiles {
            count[cell_of(t) + 1] += 1;
        }
        for c in 0..cells {
            count[c + 1] += count[c];
        }
        let start = count.clone();
        let mut fill = count;
        let mut items = alloc::vec![0u32; tiles.len()];
        let mut bounds = alloc::vec![[i16::MAX, i16::MAX, i16::MAX, i16::MIN, i16::MIN, i16::MIN]; cells];
        for (i, t) in tiles.iter().enumerate() {
            let c = cell_of(t);
            items[fill[c] as usize] = i as u32;
            fill[c] += 1;
            let b = &mut bounds[c];
            let p = [t.centre.vx, t.centre.vy, t.centre.vz];
            for k in 0..3 {
                b[k] = b[k].min(p[k]);
                b[k + 3] = b[k + 3].max(p[k]);
            }
        }
        TileGrid { tiles_ptr: tiles.as_ptr() as usize, tiles_len: tiles.len(), origin, dims, start, items, bounds, all }
    }

    /// The scan's answer, or `None` when the grid does not apply (another tile list, or a
    /// point so far from the tiles that the scan's squared distances could wrap).
    pub fn find(&self, tiles: &[GameTile], v: SVector) -> Option<Option<TileId>> {
        if tiles.is_empty() || self.tiles_len != tiles.len() || self.tiles_ptr != tiles.as_ptr() as usize {
            return None;
        }
        let p = [v.vx as i64, v.vy as i64, v.vz as i64];
        let a = &self.all;
        let far = |k: usize| (p[k] - a[k] as i64).abs().max((a[k + 3] as i64 - p[k]).abs());
        let (fx, fy, fz) = (far(0), far(1), far(2));
        if fx * fx + fy * fy + fz * fz >= super::general::NEAREST_SAFE as i64 {
            return None;
        }
        let g = |u: i64| f2_math::fsqrt(u as i32) >> 12;
        let s = 1i64 << SHIFT;
        let (dx_, dz_) = (self.dims[0] as i64, self.dims[1] as i64);
        let cx = ((p[0] - self.origin[0] as i64) >> SHIFT).clamp(0, dx_ - 1);
        let cz = ((p[2] - self.origin[1] as i64) >> SHIFT).clamp(0, dz_ - 1);
        let mut best_g = i32::MAX;
        let mut best = u32::MAX;
        let visit = |ix: i64, iz: i64, best_g: &mut i32, best: &mut u32| {
            let c = (iz * dx_ + ix) as usize;
            let (lo, hi) = (self.start[c] as usize, self.start[c + 1] as usize);
            if lo == hi {
                return;
            }
            let b = &self.bounds[c];
            let gap = |k: usize| {
                let (mn, mx) = (b[k] as i64, b[k + 3] as i64);
                if p[k] < mn {
                    mn - p[k]
                } else if p[k] > mx {
                    p[k] - mx
                } else {
                    0
                }
            };
            let (gx, gy, gz) = (gap(0), gap(1), gap(2));
            if g(gx * gx + gy * gy + gz * gz) > *best_g {
                return;
            }
            for &i in &self.items[lo..hi] {
                let (_, d) = scan_dist(v, tiles[i as usize].centre);
                if d < *best_g || (d == *best_g && i < *best) {
                    *best_g = d;
                    *best = i;
                }
            }
        };
        let max_r = dx_.max(dz_);
        for r in 0..=max_r {
            let (x0, x1, z0, z1) = (cx - r, cx + r, cz - r, cz + r);
            for iz in z0.max(0)..=z1.min(dz_ - 1) {
                if iz == z0 || iz == z1 {
                    for ix in x0.max(0)..=x1.min(dx_ - 1) {
                        visit(ix, iz, &mut best_g, &mut best);
                    }
                } else {
                    if x0 >= 0 {
                        visit(x0, iz, &mut best_g, &mut best);
                    }
                    if x1 < dx_ {
                        visit(x1, iz, &mut best_g, &mut best);
                    }
                }
            }
            // Cells outside this ring's square are at least `gap` away on x or z.
            let mut gap = i64::MAX;
            if x0 > 0 {
                gap = gap.min(p[0] - (self.origin[0] as i64 + x0 * s));
            }
            if x1 < dx_ - 1 {
                gap = gap.min(self.origin[0] as i64 + (x1 + 1) * s - p[0]);
            }
            if z0 > 0 {
                gap = gap.min(p[2] - (self.origin[1] as i64 + z0 * s));
            }
            if z1 < dz_ - 1 {
                gap = gap.min(self.origin[1] as i64 + (z1 + 1) * s - p[2]);
            }
            if gap == i64::MAX {
                break; // every cell visited
            }
            if best != u32::MAX && gap > 0 && g(gap * gap) > best_g {
                break;
            }
        }
        Some(Some(TileId(best)))
    }
}
