//! Value ↔ (code, extra bits) mapping shared by the LL, ML and OF streams.
//!
//! With `D = 1 << dlog` direct codes: `v < D` → code `v`; otherwise with `k = msb(v)`
//! and `m` the bit below the msb: code `D + 2(k - dlog) + m`, `k - 1` extra bits holding
//! the low bits of `v`. Decoding is pure arithmetic (no tables).

/// Literal-run codes: 16 direct.
pub const LL_DLOG: u32 = 4;
/// Match-length codes (`len - 2`): 16 direct.
pub const ML_DLOG: u32 = 4;
/// Distance codes (`dist - 1`, after the 3 repeat codes): 4 direct.
pub const OF_DLOG: u32 = 2;
/// Offset codes 0..REP_CODES are repeat offsets.
pub const REP_CODES: u32 = 3;
pub const MIN_MATCH: u32 = 3;
pub const MIN_REP: u32 = 2;

pub const fn ncodes(dlog: u32) -> usize {
    (1usize << dlog) + 2 * (32 - dlog as usize)
}
pub const NLL: usize = ncodes(LL_DLOG);
pub const NML: usize = ncodes(ML_DLOG);
pub const NOF: usize = REP_CODES as usize + ncodes(OF_DLOG);

/// `(code, nbits, extra)` for value `v`.
#[inline]
pub fn encode(v: u32, dlog: u32) -> (u32, u32, u32) {
    let d = 1u32 << dlog;
    if v < d {
        return (v, 0, 0);
    }
    let k = 31 - v.leading_zeros();
    let m = (v >> (k - 1)) & 1;
    let nb = k - 1;
    (d + 2 * (k - dlog) + m, nb, v & ((1u32 << nb) - 1))
}

/// `(base, nbits)` for a code; value = base + extra. `None` for an invalid code.
#[inline(always)]
pub fn decode(code: u32, dlog: u32) -> Option<(u32, u32)> {
    let d = 1u32 << dlog;
    if code < d {
        return Some((code, 0));
    }
    let c = code - d;
    let k = dlog + (c >> 1);
    if k > 31 {
        return None;
    }
    let m = c & 1;
    Some(((2 | m) << (k - 1), k - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_codes() {
        for dlog in [2u32, 4] {
            let mut vals: Vec<u32> = (0..5000).collect();
            vals.extend([u32::MAX, u32::MAX - 1, 1 << 31, (1 << 31) - 1, 123_456_789]);
            for v in vals {
                let (c, nb, e) = encode(v, dlog);
                assert!((c as usize) < ncodes(dlog));
                let (b, nb2) = decode(c, dlog).unwrap();
                assert_eq!(nb, nb2);
                assert_eq!(b.wrapping_add(e), v, "v={v} dlog={dlog}");
            }
        }
    }
}
