//! Audit of the deterministic float paths (`f2_math::detfloat`, docs/NETPLAY.md §9).
//!
//! Compares the integer results of `rsin`/`rcos`/`util_calc_angle`/`arccos`/`arccos_f64` and
//! the `view.rs` `InitMaths` tables computed with the pure-Rust `libm` (what the game uses)
//! against the same formulas on the platform C library (`f64::sin` etc.: glibc on Linux, the
//! UCRT on Windows), over every input the game can produce (exhaustive where feasible).
//!
//! ```text
//! cargo run --release -p f2-math --example float_audit              # representative (fast)
//! cargo run --release -p f2-math --example float_audit -- full      # all 2^32 rsin/rcos inputs
//! cargo run --release -p f2-math --example float_audit -- digest    # result hashes (cross-OS)
//! cargo run --release -p f2-math --example float_audit -- dump-trig R | dump-acos
//! ```

use f2_math::{arccos, arccos_f64, detfloat, div_pc24, ftol32, mul_pc24, rcos, rcos_f64, rsin, rsin_f64, util_calc_angle, util_calc_angle_f64, PI2};

// ---- the same formulas (x87 PC24 and 53-bit), on the platform libm ------------------------
fn arg_pc24(x: i32) -> f64 {
    mul_pc24((x as f32 * (1.0 / 4096.0)) as f64, PI2) as f64
}
fn rsin_sys(x: i32) -> i32 {
    ftol32((arg_pc24(x).sin() as f32) as f64 * 4096.0)
}
fn rcos_sys(x: i32) -> i32 {
    ftol32((arg_pc24(x).cos() as f32) as f64 * 4096.0)
}
fn angle_sys(adj: i32, opp: i32) -> i32 {
    ftol32(div_pc24((adj as f64).atan2(opp as f64), 6.283185308) as f64 * 4096.0)
}
fn rsin_f64_sys(x: i32) -> i32 {
    ftol32((x as f64 * (1.0 / 4096.0) * PI2).sin() * 4096.0)
}
fn rcos_f64_sys(x: i32) -> i32 {
    ftol32((x as f64 * (1.0 / 4096.0) * PI2).cos() * 4096.0)
}
fn angle_f64_sys(adj: i32, opp: i32) -> i32 {
    ftol32(((adj as f64).atan2(opp as f64) / 6.283185308) * 4096.0)
}
fn arccos_sys(cos: i32) -> i32 {
    let c = cos as f32 / 4096.0f32;
    let a = (c as f64).acos() as f32;
    let r = (a as f64 * (4096.0 / PI2)) as f32;
    ftol32(r as f64)
}
fn arccos_f64_sys(cos: i32) -> i32 {
    ftol32((cos as f64 / 4096.0).acos() * (4096.0 / PI2))
}

/// `view.rs` `InitMaths` tables (acos, sin) via `f`/`g`.
fn view_tables(acos: fn(f64) -> f64, sin: fn(f64) -> f64) -> (Vec<u32>, Vec<u32>) {
    const VPI2: f64 = std::f64::consts::PI * 2.0;
    let a = (0..4096).map(|i| (acos((i as f64 - 2048.0) / 2048.0) as f32).to_bits()).collect();
    let s = (0..4096).map(|i| (sin((i as f64 * VPI2) / 4096.0) as f32).to_bits()).collect();
    (a, s)
}

struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
}

/// xorshift for the sampled atan2 inputs (fixed seed: identical on every platform).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Run `f(x)` for x in `lo..=hi` on 8 threads; returns the mismatching inputs (first 20) and count.
fn par_check(lo: i64, hi: i64, f: impl Fn(i32) -> bool + Sync) -> (u64, Vec<i32>) {
    let n = 8i64;
    let span = (hi - lo + 1 + n - 1) / n;
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..n)
            .map(|t| {
                let f = &f;
                s.spawn(move || {
                    let (a, b) = (lo + t * span, (lo + (t + 1) * span - 1).min(hi));
                    let (mut cnt, mut first) = (0u64, Vec::new());
                    let mut x = a;
                    while x <= b {
                        if !f(x as i32) {
                            cnt += 1;
                            if first.len() < 20 {
                                first.push(x as i32);
                            }
                        }
                        x += 1;
                    }
                    (cnt, first)
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).fold((0, Vec::new()), |(c, mut v), (c2, v2)| {
            v.extend(v2);
            v.truncate(20);
            (c + c2, v)
        })
    })
}

fn report(name: &str, inputs: u64, (cnt, first): (u64, Vec<i32>)) -> u64 {
    println!("{name:<44} inputs {inputs:>12}  integer mismatches {cnt}{}", if first.is_empty() { String::new() } else { format!("  e.g. {first:?}") });
    cnt
}

fn audit(full: bool) -> u64 {
    let mut bad = 0;
    let (lo, hi) = if full { (i32::MIN as i64, i32::MAX as i64) } else { (-(1 << 22), 1 << 22) };
    let tag = if full { "all i32" } else { "|x| <= 2^22" };
    bad += report(&format!("rsin ({tag})"), (hi - lo + 1) as u64, par_check(lo, hi, |x| rsin(x) == rsin_sys(x)));
    bad += report(&format!("rcos ({tag})"), (hi - lo + 1) as u64, par_check(lo, hi, |x| rcos(x) == rcos_sys(x)));
    bad += report(&format!("rsin_f64/rcos_f64 ({tag})"), (hi - lo + 1) as u64, par_check(lo, hi, |x| rsin_f64(x) == rsin_f64_sys(x) && rcos_f64(x) == rcos_f64_sys(x)));

    // arccos: |cos| > 4096 is NaN -> 0 on both; check a margin past the domain.
    bad += report("arccos (24-bit, cos in -8192..=8192)", 16385, par_check(-8192, 8192, |c| arccos(c) == arccos_sys(c)));
    bad += report("arccos_f64 (cos in -8192..=8192)", 16385, par_check(-8192, 8192, |c| arccos_f64(c) == arccos_f64_sys(c)));

    // util_calc_angle: every pair in a box (all game call sites take fixed-point position /
    // velocity differences), then sampled pairs at all magnitudes.
    let r = 2048i64;
    bad += report(
        "util_calc_angle (all pairs |adj|,|opp| <= 2048)",
        ((2 * r + 1) * (2 * r + 1)) as u64,
        par_check(-r, r, |a| (-r as i32..=r as i32).all(|o| util_calc_angle(a, o) == angle_sys(a, o) && util_calc_angle_f64(a, o) == angle_f64_sys(a, o))),
    );
    let samples = 50_000_000u64;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut cnt, mut first) = (0u64, Vec::new());
    for i in 0..samples {
        let shift = (i % 32) as u32; // magnitudes 2^0 .. 2^31
        let a = (rng.next() as i32) >> shift;
        let o = (rng.next() as i32) >> ((rng.next() % 32) as u32);
        if util_calc_angle(a, o) != angle_sys(a, o) || util_calc_angle_f64(a, o) != angle_f64_sys(a, o) {
            cnt += 1;
            if first.len() < 20 {
                first.push(a);
            }
        }
    }
    bad += report("util_calc_angle (sampled pairs, all magnitudes)", samples, (cnt, first));

    // view.rs tables: compare the f32 bits directly.
    let new = view_tables(detfloat::acos, detfloat::sin);
    let old = view_tables(f64::acos, f64::sin);
    let d = |a: &[u32], b: &[u32]| a.iter().zip(b).enumerate().filter(|(_, (x, y))| x != y).map(|(i, _)| i as i32).collect::<Vec<_>>();
    let (da, ds) = (d(&new.0, &old.0), d(&new.1, &old.1));
    bad += report("view acos table (f32 bits)", 4096, (da.len() as u64, da.into_iter().take(20).collect()));
    bad += report("view sin table (f32 bits)", 4096, (ds.len() as u64, ds.into_iter().take(20).collect()));
    bad
}

fn hashes() {
    let run = |name: &str, f: &dyn Fn(&mut Fnv)| {
        let mut h = Fnv::new();
        f(&mut h);
        println!("{name:<28} {:016x}", h.0);
    };
    const R: i32 = 1 << 20;
    run("rsin/rcos detfloat", &|h| (-R..=R).for_each(|x| {
        h.u32(rsin(x) as u32);
        h.u32(rcos(x) as u32)
    }));
    run("rsin/rcos system", &|h| (-R..=R).for_each(|x| {
        h.u32(rsin_sys(x) as u32);
        h.u32(rcos_sys(x) as u32)
    }));
    run("arccos detfloat", &|h| (-4100..=4100).for_each(|c| {
        h.u32(arccos(c) as u32);
        h.u32(arccos_f64(c) as u32)
    }));
    run("arccos system", &|h| (-4100..=4100).for_each(|c| {
        h.u32(arccos_sys(c) as u32);
        h.u32(arccos_f64_sys(c) as u32)
    }));
    let pairs = || {
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        (0..4_000_000u32).map(move |i| ((rng.next() as i32) >> (i % 32), (rng.next() as i32) >> ((i / 32) % 32)))
    };
    run("util_calc_angle detfloat", &|h| pairs().for_each(|(a, o)| h.u32(util_calc_angle(a, o) as u32)));
    run("util_calc_angle system", &|h| pairs().for_each(|(a, o)| h.u32(angle_sys(a, o) as u32)));
    // Raw f64 bits of the transcendental results (stricter than the integer results).
    run("raw f64 bits detfloat", &|h| (-R..=R).for_each(|x| {
        let t = x as f64 * (1.0 / 4096.0) * PI2;
        for v in [detfloat::sin(t), detfloat::cos(t), detfloat::atan2(x as f64, 1234.5), detfloat::acos(x as f64 / R as f64)] {
            let b = v.to_bits();
            h.u32(b as u32);
            h.u32((b >> 32) as u32)
        }
    }));
    run("raw f64 bits system", &|h| (-R..=R).for_each(|x| {
        let t = x as f64 * (1.0 / 4096.0) * PI2;
        for v in [t.sin(), t.cos(), (x as f64).atan2(1234.5), (x as f64 / R as f64).acos()] {
            let b = v.to_bits();
            h.u32(b as u32);
            h.u32((b >> 32) as u32)
        }
    }));
    let (a, s) = view_tables(detfloat::acos, detfloat::sin);
    run("view tables detfloat", &|h| a.iter().chain(&s).for_each(|&v| h.u32(v)));
    let (a, s) = view_tables(f64::acos, f64::sin);
    run("view tables system", &|h| a.iter().chain(&s).for_each(|&v| h.u32(v)));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("digest") => {
            println!("target {} {}", std::env::consts::OS, std::env::consts::ARCH);
            hashes()
        }
        Some("dump-trig") => {
            let r: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(8192);
            for x in -r..=r {
                println!("{x} {} {}", rsin(x), rcos(x));
            }
        }
        Some("dump-acos") => (-4100..=4100).for_each(|c| println!("{c} {}", arccos(c))),
        m => {
            let bad = audit(m == Some("full"));
            println!("{}", if bad == 0 { "OK: detfloat == platform libm on all audited inputs" } else { "DIFFERENCES FOUND" });
            std::process::exit((bad != 0) as i32);
        }
    }
}
