//! Port of `Common/fixed.c` / `Common/fixed.h` (20.12 fixed point).

/// Port of `fixed` (Common/fixed.h:10): MSVC `long`, 20.12 fixed point.
pub type Fixed = i32;

/// Port of `ToFixed` (Common/fixed.h:18): `((int)(x)) << 12`, wrapping.
#[inline]
pub const fn to_fixed(x: i32) -> Fixed {
    x.wrapping_shl(12)
}

/// Port of `FMul` (Common/fixed.c:8).
///
/// Not `(a*b)>>12`: splits both operands into integer/fraction parts and sums four
/// wrapping 32-bit partial products.
///
/// Phase 3S: computed as `((x as i64 * y as i64) >> 12) as i32`, which is the same value:
/// with `x = 4096a + b`, `y = 4096c + d` (`0 <= b, d < 4096`), `x·y = 4096(4096ac + ad + bc)
/// + bd` and `0 <= bd < 2^24`, so `floor(x·y / 4096) = 4096ac + ad + bc + (bd >> 12)` exactly;
/// the original sums the same four terms modulo 2^32, i.e. that floor truncated to 32 bits
/// (the 64-bit product cannot overflow). One 64-bit multiply instead of four 32-bit ones on
/// the VR4300. Checked against [`fmul_parts`] in the tests.
#[inline]
pub fn fmul(x: Fixed, y: Fixed) -> Fixed {
    ((x as i64 * y as i64) >> 12) as i32
}

/// [`fmul`] as written in `Common/fixed.c` (four wrapping partial products), for tests.
#[inline]
pub fn fmul_parts(x: Fixed, y: Fixed) -> Fixed {
    let a = x >> 12;
    let b = x & 4095;
    let c = y >> 12;
    let d = y & 4095;

    let res1 = d.wrapping_mul(b) >> 12;
    let res2 = d.wrapping_mul(a);
    let res3 = c.wrapping_mul(b);
    let res4 = c.wrapping_mul(a).wrapping_shl(12);

    res1.wrapping_add(res2).wrapping_add(res3).wrapping_add(res4)
}

/// Port of `NewFDiv` (Common/fixed.c:23), i.e. the `FDiv(X, Y)` macro (Common/fixed.h:8).
///
/// Reciprocal approximation: `invDiv = 2147483648L / y`. In MSVC `2147483648L` does not
/// fit `long`, so it is `unsigned long` and the division is **unsigned** (a negative `y`
/// is reinterpreted as a large unsigned value, giving `invDiv == 0` except for `i32::MIN`).
///
/// # Panics
/// Panics when `y == 0` (the original raises an integer divide-by-zero exception).
#[inline]
pub fn fdiv(x: Fixed, y: Fixed) -> Fixed {
    let inv_div = (2_147_483_648u32 / (y as u32)) as i32;

    let a = x >> 12;
    let b = x & 4095;
    let c = inv_div >> 12;
    let d = inv_div & 4095;

    let res1 = d.wrapping_mul(b) >> 19;
    let res2 = d.wrapping_mul(a) >> 7;
    let res3 = c.wrapping_mul(b) >> 7;
    let res4 = c.wrapping_mul(a).wrapping_shl(5);

    res1.wrapping_add(res2).wrapping_add(res3).wrapping_add(res4)
}

/// Port of `FDiv2` (Common/fixed.c:59): returns 0 for `y == 0`, pre-shifts both operands
/// right until `y <= 16777216`, then [`fdiv`].
#[inline]
pub fn fdiv2(mut x: Fixed, mut y: Fixed) -> Fixed {
    if y == 0 {
        return 0;
    }
    while y > 16_777_216 {
        x >>= 1;
        y >>= 1;
    }
    fdiv(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmul_matches_partial_products() {
        let edge = [0, 1, -1, 4095, 4096, -4096, 4097, -4097, i32::MAX, i32::MIN, i32::MAX - 1, i32::MIN + 1, 1 << 20, -(1 << 20), 0x7fff_f000, -0x7fff_f000];
        for &x in &edge {
            for &y in &edge {
                assert_eq!(fmul(x, y), fmul_parts(x, y), "fmul({x}, {y})");
            }
        }
        let mut seed = 0x0123_4567_89ab_cdefu64;
        for i in 0..20_000_000u64 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let (x, y) = (seed as i32, (seed >> 32) as i32);
            // mix full-range operands with game-sized ones
            let (x, y) = match i % 3 {
                0 => (x, y),
                1 => (x >> 12, y),
                _ => (x >> 16, y >> 14),
            };
            assert_eq!(fmul(x, y), fmul_parts(x, y), "fmul({x}, {y})");
        }
    }
}
