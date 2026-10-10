//! x86-64 only: compares the port's `rsin`/`rcos`/`util_calc_angle`/`arccos` with the retail
//! instruction sequences executed on the host's real x87 FPU under Direct3D's control word
//! (0x007F: 24-bit precision, round to nearest), as MSVC 6 compiled them:
//!
//! * `rsin(x)`  `fild x; fmul [1.0f/4096]; fmul [PI2]; fsin; fmul [4096.0]; _ftol`
//! * `utilCalcAngle(a,o)` `fild a; fild o; fpatan; fdiv [6.283185308]; fmul [4096.0]; _ftol`
//! * `arccos(c)` `fild c; fdiv [4096.0f]; acos = fpatan(fsqrt(1-x*x), x); fmul [4096/PI2]; _ftol`
//!
//! ```text
//! cargo run --release -p f2-math --example x87_check            # representative
//! cargo run --release -p f2-math --example x87_check -- full    # all 2^32 rsin/rcos inputs
//! ```

#[cfg(target_arch = "x86_64")]
mod x87 {
    use std::arch::asm;

    static CW_NEAR: u16 = 0x007F;
    static CW_CHOP: u16 = 0x0C7F;
    static INV4096F: f32 = 1.0 / 4096.0;
    static PI2: f64 = f2_math::PI2;
    static D4096: f64 = 4096.0;
    static TWOPI_C: f64 = 6.283185308;
    static F4096: f32 = 4096.0;
    static K: f64 = 4096.0 / f2_math::PI2;

    macro_rules! seq {
        ($body:literal, $($name:ident = $val:expr),*) => {{
            let mut save: u16 = 0;
            let mut r: i64 = 0;
            // SAFETY: only touches the x87 stack (empty under the x86-64 ABI, left empty) and
            // the control word, which is saved and restored.
            unsafe {
                asm!(
                    "fnstcw word ptr [{save}]",
                    "fldcw word ptr [{cn}]",
                    $body,
                    "fldcw word ptr [{cc}]",
                    "fistp qword ptr [{r}]",
                    "fldcw word ptr [{save}]",
                    save = in(reg) &mut save as *mut u16,
                    r = in(reg) &mut r as *mut i64,
                    cn = in(reg) &CW_NEAR,
                    cc = in(reg) &CW_CHOP,
                    $($name = in(reg) $val,)*
                    out("st(0)") _, out("st(1)") _, out("st(2)") _, out("st(3)") _,
                    out("st(4)") _, out("st(5)") _, out("st(6)") _, out("st(7)") _,
                );
            }
            r as i32 // `(long)` of the `_ftol` result
        }};
    }

    pub fn rsin(x: i32) -> i32 {
        seq!(
            "fild dword ptr [{x}]\nfmul dword ptr [{inv}]\nfmul qword ptr [{pi2}]\nfsin\nfmul qword ptr [{k}]",
            x = &x,
            inv = &INV4096F,
            pi2 = &PI2,
            k = &D4096
        )
    }
    pub fn rcos(x: i32) -> i32 {
        seq!(
            "fild dword ptr [{x}]\nfmul dword ptr [{inv}]\nfmul qword ptr [{pi2}]\nfcos\nfmul qword ptr [{k}]",
            x = &x,
            inv = &INV4096F,
            pi2 = &PI2,
            k = &D4096
        )
    }
    pub fn util_calc_angle(adj: i32, opp: i32) -> i32 {
        seq!("fild dword ptr [{a}]\nfild dword ptr [{o}]\nfpatan\nfdiv qword ptr [{c}]\nfmul qword ptr [{k}]", a = &adj, o = &opp, c = &TWOPI_C, k = &D4096)
    }
    pub fn arccos(c: i32) -> i32 {
        seq!("fild dword ptr [{c}]\nfdiv dword ptr [{f}]\nfld st(0)\nfmul st(0), st(0)\nfld1\nfsubrp st(1), st(0)\nfsqrt\nfxch st(1)\nfpatan\nfmul qword ptr [{k}]",
            c = &c, f = &F4096, k = &K)
    }
}

#[cfg(target_arch = "x86_64")]
fn main() {
    use f2_math::{arccos, rcos, rsin, util_calc_angle};
    let full = std::env::args().nth(1).as_deref() == Some("full");

    fn par(lo: i64, hi: i64, f: impl Fn(i32) -> bool + Sync) -> (u64, Vec<i32>) {
        let n = 8i64;
        let span = (hi - lo + n) / n;
        std::thread::scope(|s| {
            let hs: Vec<_> = (0..n)
                .map(|t| {
                    let f = &f;
                    s.spawn(move || {
                        let (mut c, mut v) = (0u64, Vec::new());
                        for x in (lo + t * span)..=((lo + (t + 1) * span - 1).min(hi)) {
                            if !f(x as i32) {
                                c += 1;
                                if v.len() < 12 {
                                    v.push(x as i32);
                                }
                            }
                        }
                        (c, v)
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).fold((0, vec![]), |(c, mut v), (c2, v2)| {
                v.extend(v2);
                v.truncate(12);
                (c + c2, v)
            })
        })
    }
    let rep = |name: &str, n: u64, (c, v): (u64, Vec<i32>)| {
        println!("{name:<46} inputs {n:>11}  port != x87 {c}{}", if v.is_empty() { String::new() } else { format!("  e.g. {v:?}") });
        c
    };

    let mut bad = 0;
    let (lo, hi) = if full { (i32::MIN as i64, i32::MAX as i64) } else { (-(1i64 << 22), 1i64 << 22) };
    let tag = if full { "all i32" } else { "|x| <= 2^22" };
    bad += rep("rsin, x in 0..4096", 4096, par(0, 4095, |x| rsin(x) == x87::rsin(x)));
    bad += rep("rcos, x in 0..4096", 4096, par(0, 4095, |x| rcos(x) == x87::rcos(x)));
    bad += rep(&format!("rsin ({tag})"), (hi - lo + 1) as u64, par(lo, hi, |x| rsin(x) == x87::rsin(x)));
    bad += rep(&format!("rcos ({tag})"), (hi - lo + 1) as u64, par(lo, hi, |x| rcos(x) == x87::rcos(x)));
    bad += rep("arccos (cos in -8192..=8192)", 16385, par(-8192, 8192, |c| arccos(c) == x87::arccos(c)));
    let r = 2048i64;
    bad += rep(
        "util_calc_angle (all |adj|,|opp| <= 2048)",
        ((2 * r + 1) * (2 * r + 1)) as u64,
        par(-r, r, |a| (-r as i32..=r as i32).all(|o| util_calc_angle(a, o) == x87::util_calc_angle(a, o))),
    );
    let samples = 1i64 << 26;
    bad += rep(
        "util_calc_angle (sampled, all magnitudes)",
        samples as u64,
        par(0, samples - 1, |i| {
            let mut s = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let mut next = || {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                s
            };
            let a = (next() as i32) >> (next() % 32);
            let o = (next() as i32) >> (next() % 32);
            util_calc_angle(a, o) == x87::util_calc_angle(a, o)
        }),
    );
    println!("{}", if bad == 0 { "OK: port == x87 (PC24) on all checked inputs" } else { "DIFFERENCES FOUND" });
}

#[cfg(not(target_arch = "x86_64"))]
fn main() {
    println!("x87_check needs an x86-64 host");
}
