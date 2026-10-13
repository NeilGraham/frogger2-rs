//! Port of `Common/general.c`: string helpers, tile searches and the animation→sound
//! map (`LoadSfxMapping`, plus its PC lookup `FindSfxMapping` from `PC/pcaudio.c`).
//! The `TIMER` macros of `general.h` live in [`crate::timer`].

use crate::prelude::*;

use f2_formats::level::{AmbientSfx, SfxMapping};
use f2_math::{distance_between_points_ss, fmul, rcos, set_vector_sf, set_vector_sv, FVector, Fixed, SVector, Vector};
use serde::{Deserialize, Serialize};

use crate::define::tilestate;
use crate::ids::TileId;
use crate::map::{GameTile, Map};

/// `NUM_FROG_ANIMS`, `MULTI_NUM_ANIMS`, `NUM_NME_ANIMS` (record sizes of the sfx map).
pub const NUM_FROG_ANIMS: usize = 43;
pub const MULTI_NUM_ANIMS: usize = 17;
pub const NUM_NME_ANIMS: usize = 4;

/// Globals defined in `general.c`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneralState {
    /// `sfx_anim_map`: flattened `uid, type, samples...` records terminated by 0.
    /// Sample entries hold the sample uid (the original stores `FindSample(uid)`;
    /// the audio layer resolves uids, 0 = `NULL`). Empty = `NULL` map.
    pub sfx_anim_map: Vec<u32>,
    /// Tile-anchored ambient sounds from the map (`AddAmbientSound` calls in
    /// `LoadSfxMapping`), handed to the audio port.
    pub ambient_tile_sfx: Vec<AmbientTileSfx>,
}

/// One tile ambient sound of the sfx map (`AddAmbientSound(FindSample(sample),
/// &firstTile[tile].centre, 12000, AMBIENT_VOLUME, ...)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmbientTileSfx {
    pub tile: TileId,
    pub sample: u32,
}

// ---- string helpers --------------------------------------------------------

/// Port of `gstrlen` (Common/general.c:36).
// BUG(original): the loop condition is `b[0]==0`, so it counts leading NUL bytes
// (returns 0 for any non-empty string). Ported literally.
pub fn gstrlen(a: &[u8]) -> u32 {
    a.iter().take_while(|&&c| c == 0).count() as u32
}

/// Port of `gstrcmp` (Common/general.c:44): 0 when equal, 1 otherwise.
pub fn gstrcmp(a: &str, b: &str) -> u32 {
    (a.as_bytes() != b.as_bytes()) as u32
}

/// Port of `gstrlwr` (Common/general.c:62): ASCII only.
pub fn gstrlwr(s: &str) -> String {
    s.bytes().map(|c| c.to_ascii_lowercase() as char).collect()
}

/// Port of `gstrupr` (Common/general.c:76): ASCII only.
pub fn gstrupr(s: &str) -> String {
    s.bytes().map(|c| c.to_ascii_uppercase() as char).collect()
}

fn replace_first_ext(name: &str, from: &str, to: &str) -> String {
    match name.find(from) {
        Some(p) => format!("{}{}{}", &name[..p], to, &name[p + from.len()..]),
        None => name.to_string(),
    }
}

/// Port of `stringChange` (Common/general.c:90): replaces the first `.obe` with `.psi`.
pub fn string_change(name: &str) -> String {
    replace_first_ext(name, ".obe", ".psi")
}

/// Port of `stringChangePSIToOBE` (Common/general.c:112): first `.psi` → `.obe`.
pub fn string_change_psi_to_obe(name: &str) -> String {
    replace_first_ext(name, ".psi", ".obe")
}

// ---- tile searches ---------------------------------------------------------

/// `DotProductFF(d, &tile->dirVector[i])`: the macro applied to an `SVECTOR`.
#[inline]
fn dot_fs(d: &FVector, s: &SVector) -> Fixed {
    fmul(d.vx, s.vx as i32).wrapping_add(fmul(d.vy, s.vy as i32)).wrapping_add(fmul(d.vz, s.vz as i32))
}

/// `ToFixed(100000)`.
const DIST_INIT: Fixed = 100000 << 12;

/// Port of `FindNearestTileV` (Common/general.c:152).
pub fn find_nearest_tile_v(map: &Map, v: Vector) -> Option<TileId> {
    let mut sv = SVector::default();
    set_vector_sv(&mut sv, &v);
    find_nearest_tile_s(map, sv)
}

/// Port of `FindNearestTileF` (Common/general.c:159).
pub fn find_nearest_tile_f(map: &Map, v: FVector) -> Option<TileId> {
    let mut sv = SVector::default();
    set_vector_sf(&mut sv, &v);
    find_nearest_tile_s(map, sv)
}

/// Port of `FindNearestTileS` (Common/general.c:166).
pub fn find_nearest_tile_s(map: &Map, v: SVector) -> Option<TileId> {
    // Phase 3S3: the grid built with the map (`crate::nearest`), when it applies.
    if let Some(r) = map.grid.find(&map.tiles, v) {
        debug_assert_eq!(r, find_nearest_tile_scan(map, v), "nearest-tile grid");
        return r;
    }
    find_nearest_tile_scan(map, v)
}

/// [`find_nearest_tile_s`] by scanning every tile.
fn find_nearest_tile_scan(map: &Map, v: SVector) -> Option<TileId> {
    // Phase 3S: `DistanceBetweenPointsSS(v, c) >> 12` is `g(u)` of the wrapped squared
    // distance `u` (`utilSqrt`), and `g` is non-decreasing for `u < NEAREST_SAFE` (above,
    // the float rounds to 2^32 and `utilSqrt` wraps to 0). `d_best` is a safe `u` with
    // `g(d_best) == closest_dist`, so a tile with safe `u >= d_best` cannot be strictly
    // closer and its square root is skipped. Same result as the plain scan (tested).
    let mut closest = None;
    let mut closest_dist = 500000;
    let mut d_best = u32::MAX;
    for (i, t) in map.tiles.iter().enumerate() {
        let (dx, dy, dz) = (v.vx as i32 - t.centre.vx as i32, v.vy as i32 - t.centre.vy as i32, v.vz as i32 - t.centre.vz as i32);
        let u = dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy)).wrapping_add(dz.wrapping_mul(dz)) as u32;
        let safe = u < NEAREST_SAFE;
        if safe && u >= d_best {
            continue;
        }
        let dist = f2_math::fsqrt(u as i32) >> 12;
        if dist < closest_dist {
            closest = Some(TileId(i as u32));
            closest_dist = dist;
            d_best = if safe { u } else { u32::MAX };
        } else if dist == closest_dist && safe && u < d_best {
            d_best = u;
        }
    }
    closest
}

/// Squared distances below this have a non-decreasing `utilSqrt` (see `find_nearest_tile_s`).
pub(crate) const NEAREST_SAFE: u32 = 0xFFFF_FF00;

/// The plain scan `find_nearest_tile_s` must equal (tests).
#[cfg(test)]
fn find_nearest_tile_s_ref(map: &Map, v: SVector) -> Option<TileId> {
    let mut closest = None;
    let mut closest_dist = 500000;
    for (i, t) in map.tiles.iter().enumerate() {
        let dist = distance_between_points_ss(&v, &t.centre) >> 12;
        if dist < closest_dist {
            closest = Some(TileId(i as u32));
            closest_dist = dist;
        }
    }
    closest
}

#[cfg(test)]
mod nearest_tests {
    use super::*;

    #[test]
    fn nearest_tile_matches_plain_scan() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..400 {
            let mut map = Map::default();
            let n = (rnd() % 300) as usize;
            let spread = [100i64, 3000, 32767][round % 3];
            let c = |r: u64| ((r % (2 * spread as u64 + 1)) as i64 - spread) as i16;
            for _ in 0..n {
                let t = crate::map::GameTile { centre: SVector::new(c(rnd()), c(rnd()), c(rnd())), ..Default::default() };
                map.tiles.push(t);
            }
            // duplicate centres and equal distances (ties)
            if n > 4 {
                let t = map.tiles[1].clone();
                map.tiles[3] = t;
            }
            map.grid = crate::nearest::TileGrid::build(&map.tiles);
            for k in 0..50 {
                // points among the tiles, near them and anywhere
                let v = if k % 3 == 0 && n > 0 {
                    let t = map.tiles[(rnd() % n as u64) as usize].centre;
                    let mut j = |a: i16| (a as i32 + (rnd() % 41) as i32 - 20).clamp(-32768, 32767) as i16;
                    SVector::new(j(t.vx), j(t.vy), j(t.vz))
                } else {
                    SVector::new(c(rnd()), c(rnd()), c(rnd()))
                };
                assert_eq!(find_nearest_tile_s(&map, v), find_nearest_tile_s_ref(&map, v));
                if let Some(r) = map.grid.find(&map.tiles, v) {
                    assert_eq!(r, find_nearest_tile_s_ref(&map, v), "grid");
                }
            }
        }
    }
}

#[inline]
fn joinable(t: &GameTile) -> bool {
    t.state != tilestate::BARRED && t.state != tilestate::JOIN
}

/// Port of `FindNearestJoinedTile` (Common/general.c:187).
pub fn find_nearest_joined_tile(map: &Map, tile: TileId, pos: &SVector) -> TileId {
    let mut closest = tile;
    let mut closest_dist = distance_between_points_ss(pos, &map.tile(tile).centre);
    for i in 0..4 {
        let Some(t) = map.tile(tile).tile_ptrs[i] else {
            continue;
        };
        if !joinable(map.tile(t)) {
            continue;
        }
        let dist = distance_between_points_ss(pos, &map.tile(t).centre);
        if dist < closest_dist {
            closest = t;
            closest_dist = dist;
        }
        for j in 0..4 {
            let Some(u) = map.tile(t).tile_ptrs[j] else {
                continue;
            };
            if joinable(map.tile(u)) {
                let dist = distance_between_points_ss(pos, &map.tile(u).centre);
                if dist < closest_dist {
                    closest = u;
                    closest_dist = dist;
                }
            }
        }
    }
    closest
}

/// Port of `FindJoinedTileByDirection` (Common/general.c:226).
pub fn find_joined_tile_by_direction(map: &Map, st: TileId, d: &FVector) -> Option<TileId> {
    let mut distance = DIST_INIT;
    let mut res = None;
    let s = map.tile(st);
    for i in 0..4 {
        let t = dot_fs(d, &s.dir_vector[i]);
        if t < distance {
            if let Some(n) = s.tile_ptrs[i] {
                let state = map.tile(n).state;
                if state != tilestate::DEADLY && state != tilestate::SINK && state != tilestate::BARRED && state != tilestate::JOIN {
                    distance = t;
                    res = Some(n);
                }
            }
        }
    }
    res
}

/// Port of `FindJoinedTileByDirectionConstrained` (Common/general.c:248).
pub fn find_joined_tile_by_direction_constrained(map: &Map, st: TileId, d: &FVector, angle: Fixed) -> Option<TileId> {
    let mut distance = DIST_INIT;
    let mut res = None;
    let s = map.tile(st);
    for i in 0..4 {
        let t = dot_fs(d, &s.dir_vector[i]);
        if t < distance && t < rcos(angle) {
            if let Some(n) = s.tile_ptrs[i] {
                let state = map.tile(n).state;
                if state != tilestate::FALL
                    && state != tilestate::DEADLYFALL
                    && state != tilestate::DEADLY
                    && state != tilestate::SINK
                    && state != tilestate::BARRED
                    && state != tilestate::JOIN
                {
                    distance = t;
                    res = Some(n);
                }
            }
        }
    }
    res
}

/// Port of `FindJoinedTileByDirectionAndType` (Common/general.c:269).
pub fn find_joined_tile_by_direction_and_type(map: &Map, st: TileId, d: &FVector, ty: u32) -> Option<TileId> {
    let mut distance = DIST_INIT;
    let mut res = None;
    let s = map.tile(st);
    for i in 0..4 {
        let t = dot_fs(d, &s.dir_vector[i]);
        if t < distance {
            if let Some(n) = s.tile_ptrs[i] {
                if map.tile(n).state == ty {
                    distance = t;
                    res = Some(n);
                }
            }
        }
    }
    res
}

/// Port of `FindClosestTileVector` (Common/general.c:290): index of the direction
/// vector that best matches `d` (the original returns `&tile->dirVector[i]`).
// BUG(original): `res` is uninitialised if every dot product is <= -100000 (cannot
// happen for unit vectors); the port returns 0 in that case.
pub fn find_closest_tile_vector(tile: &GameTile, d: &FVector) -> usize {
    let mut distance = -100000;
    let mut res = 0;
    for i in 0..4 {
        let t = dot_fs(d, &tile.dir_vector[i]);
        if t > distance {
            distance = t;
            res = i;
        }
    }
    res
}

// ---- sfx mapping -----------------------------------------------------------

/// File name used by `LoadSfxMapping` (PC): `MAPS\SFXANIM<world dec><level hex>.SAM`.
pub fn sfx_mapping_file_name(world: u32, level: u32) -> String {
    format!("MAPS\\SFXANIM{}{:x}.SAM", world as i32, level as i32)
}

impl GeneralState {
    /// Port of the data part of `LoadSfxMapping` (Common/general.c:330) on an already
    /// parsed `.sam` file: builds the flattened `sfx_anim_map` and the list of
    /// tile ambient sounds (`AddAmbientSound` is the audio port's job).
    pub fn load_sfx_mapping(&mut self, mappings: &[SfxMapping], ambient: &[AmbientSfx]) {
        let mut m = Vec::new();
        for r in mappings {
            m.push(r.uid as u32);
            m.push(r.kind as u32);
            m.extend(r.samples.iter().map(|&s| s as u32));
        }
        m.push(0);
        self.sfx_anim_map = m;
        self.ambient_tile_sfx = ambient.iter().map(|a| AmbientTileSfx { tile: TileId(a.tile as u32), sample: a.sample as u32 }).collect();
    }

    #[inline]
    fn map_at(&self, i: usize) -> u32 {
        // MALLOC0(size+8): anything past the written records is zero.
        self.sfx_anim_map.get(i).copied().unwrap_or(0)
    }

    /// Port of `FindSfxMapping` (PC/pcaudio.c:1465). Returns the new `sfxMapping`
    /// (index of the actor's first sample in `sfx_anim_map`) and, for a scenic (type
    /// 3) record, the sample uid of the ambient sound to attach to the actor.
    pub fn find_sfx_mapping(&self, uid: u32) -> (Option<usize>, Option<u32>) {
        let mut index = 0usize;
        if self.sfx_anim_map.is_empty() || uid == 0 {
            return (None, None);
        }
        let mut act_id = self.map_at(index);
        index += 1;
        if act_id == 0 {
            return (None, None);
        }
        loop {
            let ty = self.map_at(index);
            index += 1;
            if act_id == uid {
                if ty == 3 {
                    return (None, Some(self.map_at(index)));
                }
                return (Some(index), None);
            }
            // BUG(original): type 4 records (non-tile) are not skipped here although
            // `LoadSfxMapping` stored one sample for them; the walk desynchronises.
            match ty {
                0 => index += NUM_FROG_ANIMS,
                1 => index += MULTI_NUM_ANIMS,
                2 => index += NUM_NME_ANIMS,
                3 => index += 1,
                _ => {}
            }
            act_id = self.map_at(index);
            index += 1;
            if act_id == 0 || index > self.sfx_anim_map.len() + 1 {
                break;
            }
        }
        (None, None)
    }

    /// `map[act->animation->currentAnimation]` for a mapping run (sample uid, 0 = none).
    pub fn sfx_mapped_sample(&self, mapping: usize, current_animation: i16) -> u32 {
        if current_animation < 0 {
            return 0;
        }
        self.map_at(mapping + current_animation as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use f2_math::make_svector;

    fn tile(state: u32, x: i16, ptrs: [Option<u32>; 4]) -> GameTile {
        GameTile {
            tile_ptrs: ptrs.map(|p| p.map(TileId)),
            state,
            centre: make_svector(x, 0, 0),
            normal: make_svector(0, 4096, 0),
            // north, south, east, west
            dir_vector: [make_svector(0, 0, 4096), make_svector(0, 0, -4096), make_svector(4096, 0, 0), make_svector(-4096, 0, 0)],
        }
    }

    fn row() -> Map {
        // 0 - 1 - 2(barred) - 3 in a line along +x (east = index 2)
        Map {
            tiles: vec![
                tile(tilestate::NORMAL, 0, [None, None, Some(1), None]),
                tile(tilestate::NORMAL, 100, [None, None, Some(2), Some(0)]),
                tile(tilestate::BARRED, 200, [None, None, Some(3), Some(1)]),
                tile(tilestate::DEADLY, 300, [None, None, None, Some(2)]),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn nearest_tile() {
        let m = row();
        assert_eq!(find_nearest_tile_s(&m, make_svector(140, 0, 0)), Some(TileId(1)));
        assert_eq!(find_nearest_tile_s(&m, make_svector(260, 5, 0)), Some(TileId(3)));
        assert_eq!(find_nearest_tile_f(&m, FVector::new(299 << 12, 0, 0)), Some(TileId(3)));
        assert_eq!(find_nearest_tile_s(&Map::default(), make_svector(0, 0, 0)), None);
    }

    #[test]
    fn joined_tiles() {
        let m = row();
        // From 0, pos near tile 2: 2 is barred, so tile 1 wins.
        assert_eq!(find_nearest_joined_tile(&m, TileId(0), &make_svector(200, 0, 0)), TileId(1));
        // Direction: the dot product is *minimised*, so -east (west) picks the east tile.
        let west = FVector::new(-4096, 0, 0);
        assert_eq!(find_joined_tile_by_direction(&m, TileId(0), &west), Some(TileId(1)));
        // East neighbour is barred, so the next-smallest dot product (west, tile 0) wins.
        assert_eq!(find_joined_tile_by_direction(&m, TileId(1), &west), Some(TileId(0)));
        assert_eq!(find_joined_tile_by_direction_and_type(&m, TileId(2), &west, tilestate::DEADLY), Some(TileId(3)));
        assert_eq!(find_joined_tile_by_direction_constrained(&m, TileId(0), &west, 0), Some(TileId(1)));
        assert_eq!(find_closest_tile_vector(&m.tiles[0], &FVector::new(0, 0, -4096)), 1);
    }

    #[test]
    fn strings() {
        assert_eq!(string_change("abc.obe"), "abc.psi");
        assert_eq!(string_change_psi_to_obe("x.psi.psi"), "x.obe.psi");
        assert_eq!(gstrcmp("a", "a"), 0);
        assert_eq!(gstrcmp("a", "ab"), 1);
        assert_eq!(gstrlen(b"abc"), 0);
        assert_eq!(gstrlwr("AbC_9"), "abc_9");
        assert_eq!(sfx_mapping_file_name(7, 11), "MAPS\\SFXANIM7b.SAM");
    }

    #[test]
    fn sfx_lookup() {
        let mut g = GeneralState::default();
        g.load_sfx_mapping(
            &[
                SfxMapping { uid: 5, kind: 2, samples: vec![11, 12, 13, 14] },
                SfxMapping { uid: 6, kind: 3, samples: vec![99] },
                SfxMapping { uid: 7, kind: 2, samples: vec![21, 0, 0, 0] },
            ],
            &[],
        );
        assert_eq!(g.find_sfx_mapping(5), (Some(2), None));
        assert_eq!(g.find_sfx_mapping(6), (None, Some(99)));
        let (m, _) = g.find_sfx_mapping(7);
        assert_eq!(g.sfx_mapped_sample(m.unwrap(), 0), 21);
        assert_eq!(g.find_sfx_mapping(8), (None, None));
        assert_eq!(g.find_sfx_mapping(0), (None, None));
    }
}
