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
#[inline]
pub fn fmul(x: Fixed, y: Fixed) -> Fixed {
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
