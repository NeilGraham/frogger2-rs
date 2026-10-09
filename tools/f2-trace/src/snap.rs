//! Breakpoint snapshots: raw bytes of chosen globals at one tick.
//!
//! The oracle (`crates/f2fix`, `snap.ticks` / `snap.regions`) writes one
//! `f2-snap-<act_frame_count>.json` per requested tick, after that tick's
//! `GameLoop` returned (the same point as a trace line):
//!
//! ```json
//! {"snap":1,"seq":41,"act_frame_count":42,"mode":10,
//!  "regions":[{"name":"player","addr":"0x036c1f00","len":624,"deref":false,"ptr":null,"hex":"0a00..."},
//!             {"name":"frog0","addr":"0x00489698","len":64,"deref":true,"ptr":"0x0abc1230","hex":"..."}]}
//! ```
//!
//! `addr` is the global; for `deref` regions `ptr` is the pointer read from
//! it and `hex` the bytes it points at (`hex` is `null` when the pointer is
//! NULL or unreadable). A Rust-side emitter can write the same shape from its
//! own state laid out as the exe does; [`compare`] lists differing bytes.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub name: String,
    pub addr: String,
    pub len: u32,
    #[serde(default)]
    pub deref: bool,
    #[serde(default)]
    pub ptr: Option<String>,
    pub hex: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub snap: u32,
    #[serde(default)]
    pub seq: u32,
    pub act_frame_count: u32,
    #[serde(default)]
    pub mode: u32,
    pub regions: Vec<Region>,
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string())).collect()
}

/// Byte ranges `[start, end)` where `a` and `b` differ (a length difference is one range).
pub fn differing_ranges(a: &[u8], b: &[u8]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let n = a.len().max(b.len());
    for i in 0..n {
        if a.get(i) != b.get(i) {
            match out.last_mut() {
                Some(r) if r.1 == i => r.1 = i + 1,
                _ => out.push((i, i + 1)),
            }
        }
    }
    out
}

/// Human-readable comparison; `true` when every region matches.
pub fn compare(a: &Snapshot, b: &Snapshot) -> Result<(String, bool), String> {
    let mut s = format!("snapshot A: act_frame_count {} (mode {}), B: act_frame_count {} (mode {})\n", a.act_frame_count, a.mode, b.act_frame_count, b.mode);
    let mut same = true;
    for ra in &a.regions {
        let Some(rb) = b.regions.iter().find(|r| r.name == ra.name) else {
            s += &format!("  {}: only in A\n", ra.name);
            same = false;
            continue;
        };
        let (ha, hb) = match (&ra.hex, &rb.hex) {
            (Some(x), Some(y)) => (unhex(x)?, unhex(y)?),
            (x, y) if x == y => {
                s += &format!("  {}: unreadable on both sides\n", ra.name);
                continue;
            }
            _ => {
                s += &format!("  {}: readable on one side only\n", ra.name);
                same = false;
                continue;
            }
        };
        let d = differing_ranges(&ha, &hb);
        if d.is_empty() {
            s += &format!("  {}: identical ({} bytes)\n", ra.name, ha.len());
            continue;
        }
        same = false;
        let bytes: usize = d.iter().map(|r| r.1 - r.0).sum();
        s += &format!("  {}: {} byte(s) differ in {} range(s)\n", ra.name, bytes, d.len());
        for &(lo, hi) in d.iter().take(32) {
            let hx = |v: &[u8]| v.get(lo..hi.min(v.len())).map_or(String::new(), |b| b.iter().map(|x| format!("{x:02x}")).collect());
            let (x, y) = (hx(&ha), hx(&hb));
            s += &format!("    +{lo:#06x}..+{hi:#06x}: {} != {}\n", if x.len() > 32 { &x[..32] } else { &x }, if y.len() > 32 { &y[..32] } else { &y });
        }
        if d.len() > 32 {
            s += &format!("    ... {} more range(s)\n", d.len() - 32);
        }
    }
    for rb in &b.regions {
        if !a.regions.iter().any(|r| r.name == rb.name) {
            s += &format!("  {}: only in B\n", rb.name);
            same = false;
        }
    }
    Ok((s, same))
}

pub fn read(path: &std::path::Path) -> Result<Snapshot, String> {
    let t = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn compare_files(a: &std::path::Path, b: &std::path::Path) -> Result<(String, bool), String> {
    compare(&read(a)?, &read(b)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_compare() {
        assert_eq!(differing_ranges(&[1, 2, 3, 4], &[1, 9, 9, 4, 5]), vec![(1, 3), (4, 5)]);
        let line = f2fix::trace::sample_snapshot();
        let a: Snapshot = serde_json::from_str(&line).expect("oracle snapshot parses");
        assert_eq!(a.regions[0].hex.as_deref(), Some("0102ff"));
        let mut b = a.clone();
        assert!(compare(&a, &b).unwrap().1);
        b.regions[0].hex = Some("0103ff".into());
        let (t, same) = compare(&a, &b).unwrap();
        assert!(!same);
        assert!(t.contains("rseed: 1 byte(s) differ in 1 range(s)"), "{t}");
    }
}
