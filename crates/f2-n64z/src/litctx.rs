//! Literal context modes: the literal table is selected by a class of the previous output
//! byte and/or the low bits of the output position (alignment inside 2/4-byte fields).
//! Both are known to the decoder independently of the parse, so the optimal parser prices
//! literals exactly. Chosen per block by the encoder.
//!
//! `ctx = PREV[mode][p] | ((pos & PMASK[mode]) << PSHIFT[mode])`, where `p` is the previous
//! output byte (before the first output byte: the last dictionary byte, else 0) and `pos`
//! the output position (LZ domain, after transforms).
//!
//! | mode | ctx | previous-byte class | position bits |
//! |---|---|---|---|
//! | 0 | 1 | — | — |
//! | 1 | 2 | `p >> 7` | — |
//! | 2 | 4 | `p >> 6` | — |
//! | 3 | 8 | `p >> 5` | — |
//! | 4 | 8 | magnitude: 0 → 0, 0xFF → 1, <16 → 2, <64 → 3, <128 → 4, <192 → 5, <240 → 6, else 7 | — |
//! | 5 | 2 | — | `pos & 1` |
//! | 6 | 4 | — | `pos & 3` |
//! | 7 | 8 | `p >> 6` | `pos & 1` |
//! | 8 | 8 | `p >> 7` | `pos & 3` |
//! | 9 | 8 | 0 → 0, 0xFF → 1, <128 → 2, else 3 | `pos & 1` |

pub const NMODES: usize = 10;
pub const MAX_CTX: usize = 8;

pub const fn nctx(mode: u8) -> usize {
    match mode {
        0 => 1,
        1 | 5 => 2,
        2 | 6 => 4,
        _ => 8,
    }
}

const fn mag8(p: u8) -> u8 {
    if p == 0 {
        0
    } else if p == 0xFF {
        1
    } else if p < 16 {
        2
    } else if p < 64 {
        3
    } else if p < 128 {
        4
    } else if p < 192 {
        5
    } else if p < 240 {
        6
    } else {
        7
    }
}

const fn prev_class(mode: u8, p: u8) -> u8 {
    match mode {
        1 | 8 => p >> 7,
        2 | 7 => p >> 6,
        3 => p >> 5,
        4 => mag8(p),
        9 => {
            if p == 0 {
                0
            } else if p == 0xFF {
                1
            } else if p < 128 {
                2
            } else {
                3
            }
        }
        _ => 0,
    }
}

const fn build() -> [[u8; 256]; NMODES] {
    let mut t = [[0u8; 256]; NMODES];
    let mut m = 0;
    while m < NMODES {
        let mut p = 0;
        while p < 256 {
            t[m][p] = prev_class(m as u8, p as u8);
            p += 1;
        }
        m += 1;
    }
    t
}

/// `PREV[mode][prev]` = previous-byte class.
pub static PREV: [[u8; 256]; NMODES] = build();
/// Position mask and shift per mode.
pub const PMASK: [u32; NMODES] = [0, 0, 0, 0, 0, 1, 3, 1, 3, 1];
pub const PSHIFT: [u32; NMODES] = [0, 0, 0, 0, 0, 0, 0, 2, 1, 2];

#[inline]
pub fn ctx(mode: u8, prev: u8, pos: usize) -> usize {
    let m = mode as usize;
    PREV[m][prev as usize] as usize | ((pos as u32 & PMASK[m]) << PSHIFT[m]) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contexts_in_range() {
        for m in 0..NMODES as u8 {
            for p in 0..=255u8 {
                for pos in 0..8 {
                    assert!(ctx(m, p, pos) < nctx(m), "mode {m}");
                }
            }
        }
    }
}
