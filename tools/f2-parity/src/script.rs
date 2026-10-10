//! Deterministic input scripts in the `.key` format (PC/controll.cpp `RecordButtons`):
//! little-endian `u32` triplets `(actFrameCount, padData.digital, pad)`, one per change.
//!
//! The oracle (f2fix scenario mode) and `scenario_trace` replay them as held pad state:
//! a record with frame `f` is held from the tick with `actFrameCount == f + 1`.

/// PSX pad bits (PC/islpad.h).
pub const PAD_UP: u16 = 1 << 4;
pub const PAD_RIGHT: u16 = 1 << 5;
pub const PAD_DOWN: u16 = 1 << 6;
pub const PAD_LEFT: u16 = 1 << 7;
pub const PAD_CIRCLE: u16 = 1 << 13;
pub const PAD_CROSS: u16 = 1 << 14;
pub const PAD_SQUARE: u16 = 1 << 15;

/// Generator version: part of the cache key, bump when the generator changes.
pub const GENERATOR: &str = "f2-parity-gen-1";

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + (self.next() % u64::from(hi - lo + 1)) as u32
    }
}

/// Seeded pseudo-random play biased forward: hops (mostly `UP`), superhops (`UP` then
/// `UP|CROSS` mid-hop, or `CROSS` alone), tongue (`CIRCLE`), croak (`SQUARE`), a diagonal, with short
/// holds and gaps. Never `START` (pause) or camera buttons.
pub fn generate(world: u32, level: u32, seed: u64, ticks: u32) -> Vec<(u32, u16)> {
    let mut r = Rng(seed ^ (u64::from(world) << 40) ^ (u64::from(level) << 32) ^ 0x4632_5041_5249_5459);
    let mut out: Vec<(u32, u16)> = Vec::new();
    let mut t = r.range(20, 60);
    while t + 8 < ticks {
        let roll = r.range(0, 99);
        let hold = r.range(2, 6);
        match roll {
            0..=44 => out.push((t, PAD_UP)),
            45..=55 => out.push((t, PAD_LEFT)),
            56..=66 => out.push((t, PAD_RIGHT)),
            67..=71 => out.push((t, PAD_DOWN)),
            72..=78 => {
                out.push((t, PAD_UP));
                out.push((t + 2, PAD_UP | PAD_CROSS));
            }
            79..=82 => out.push((t, PAD_CROSS)),
            83..=95 => out.push((t, PAD_CIRCLE)),
            96..=97 => out.push((t, PAD_SQUARE)),
            _ => out.push((t, PAD_UP | PAD_LEFT)),
        }
        let up = t + hold.max(3);
        out.push((up, 0));
        t = up + r.range(4, 30);
    }
    out
}

/// `.key` bytes for player 0.
pub fn to_key_bytes(recs: &[(u32, u16)]) -> Vec<u8> {
    let mut b = Vec::with_capacity(recs.len() * 12);
    for &(f, buttons) in recs {
        b.extend_from_slice(&f.to_le_bytes());
        b.extend_from_slice(&u32::from(buttons).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
    }
    b
}

/// Generator version of the local multiplayer scripts (cache key; bump on change).
pub const GENERATOR_MP: &str = "f2-parity-mp-gen-1";

/// Local multiplayer: one [`generate`] stream per player (seed mixed with the player
/// index), merged into `(frame, buttons, player)` records ordered by frame, then player.
pub fn generate_multi(world: u32, level: u32, players: u32, seed: u64, ticks: u32) -> Vec<(u32, u16, u32)> {
    let mut out = Vec::new();
    for p in 0..players {
        let s = seed ^ (u64::from(p) + 1).wrapping_mul(0xa076_1d64_78bd_642f);
        out.extend(generate(world, level, s, ticks).into_iter().map(|(f, b)| (f, b, p)));
    }
    out.sort_by_key(|r| (r.0, r.2));
    out
}

/// `.key` bytes of player-tagged records.
pub fn to_key_bytes_multi(recs: &[(u32, u16, u32)]) -> Vec<u8> {
    let mut b = Vec::with_capacity(recs.len() * 12);
    for &(f, buttons, p) in recs {
        b.extend_from_slice(&f.to_le_bytes());
        b.extend_from_slice(&u32::from(buttons).to_le_bytes());
        b.extend_from_slice(&p.to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_well_formed() {
        let a = generate(1, 2, 7, 1800);
        assert_eq!(a, generate(1, 2, 7, 1800));
        assert_ne!(a, generate(1, 1, 7, 1800));
        assert!(a.windows(2).all(|w| w[0].0 < w[1].0), "strictly increasing frames");
        assert!(a.last().unwrap().0 < 1800);
        assert_eq!(a.last().unwrap().1, 0, "ends released");
        assert_eq!(to_key_bytes(&a).len(), a.len() * 12);
        let ups = a.iter().filter(|r| r.1 & PAD_UP != 0).count();
        assert!(ups * 3 > a.iter().filter(|r| r.1 != 0).count(), "biased forward");
    }

    #[test]
    fn multi_streams_merge_in_frame_order() {
        let m = generate_multi(5, 3, 4, 1, 1800);
        assert_eq!(m, generate_multi(5, 3, 4, 1, 1800));
        assert!(m.windows(2).all(|w| (w[0].0, w[0].2) < (w[1].0, w[1].2)), "ordered by frame, player");
        for p in 0..4 {
            let mine: Vec<_> = m.iter().filter(|r| r.2 == p).collect();
            assert!(mine.len() > 20, "player {p} plays");
            assert_eq!(mine.last().unwrap().1, 0, "player {p} ends released");
        }
        let p0: Vec<_> = m.iter().filter(|r| r.2 == 0).map(|r| r.0).collect();
        let p1: Vec<_> = m.iter().filter(|r| r.2 == 1).map(|r| r.0).collect();
        assert_ne!(p0, p1, "independent streams");
        let b = to_key_bytes_multi(&m);
        assert_eq!(b.len(), m.len() * 12);
        assert_eq!(u32::from_le_bytes([b[8], b[9], b[10], b[11]]), m[0].2);
    }
}
