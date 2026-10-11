//! Probe (e): F2Z decode speed on the VR4300, against libdragon's mkasset codecs on the
//! same decoded bytes. Input is staged into the DFS by tools/n64/stage-f2z-probe.py (game
//! derived, never committed): `rom:/f2z/<pack path>` are pack files, `rom:/mka<N>/<pack
//! path>` their host-decoded bytes re-compressed with `mkasset -c N -w 256`, and
//! `rom:/f2z/list.txt` lists `<path> <dict path|-> <raw size> <fnv1a> <mka0..3 sizes>`.
//!
//! Per file: ROM read of the F2Z stream (`asset_load` of an uncompressed DFS file), then
//! `Decoder::decode` timed `REPS` times (first and best), FNV-1a of the output checked
//! against the host value; then `asset_load` of each mkasset variant (read + decompress).
//! Dictionaries are decoded once (no dictionary), parsed and their default tables built
//! with `Decoder::load_dict` (timed separately); they stay resident.

use crate::kernels::fnv1a;
use crate::log;
use crate::rt::{self, ticks, ticks_to_us};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::alloc::Layout;
use f2_n64z::transform::{self, Kind, Segment};
use f2_n64z::{header, Decoder, Dict};

const REPS: usize = 4;

struct Entry {
    path: String,
    dict: Option<String>,
    raw: usize,
    fnv: u32,
    mka: [usize; 4],
}

fn parse_list(text: &str) -> Vec<Entry> {
    let mut v = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 8 {
            continue;
        }
        let num = |s: &str| s.parse::<usize>().unwrap_or(0);
        v.push(Entry {
            path: f[0].into(),
            dict: (f[1] != "-").then(|| f[1].into()),
            raw: num(f[2]),
            fnv: u32::from_str_radix(f[3], 16).unwrap_or(0),
            mka: [num(f[4]), num(f[5]), num(f[6]), num(f[7])],
        });
    }
    v
}

/// The decoder (~31 KB), allocated zeroed straight on the heap (never on the stack).
fn new_decoder() -> Box<Decoder> {
    let layout = Layout::new::<Decoder>();
    // SAFETY: `Decoder` consists only of integer and bool arrays/fields, for which the
    // all-zero bit pattern is valid; it equals `Decoder::new()`. The pointer comes from
    // the global allocator with `Decoder`'s layout, as `Box::from_raw` requires.
    unsafe {
        let p = alloc::alloc::alloc_zeroed(layout) as *mut Decoder;
        if p.is_null() {
            alloc::alloc::handle_alloc_error(layout);
        }
        Box::from_raw(p)
    }
}

/// `bytes` per `t` ticks as "M.mm" MB/s (10^6 bytes per second).
fn mbs(bytes: usize, t: u32) -> (u64, u64) {
    let us = u64::from(ticks_to_us(t).max(1));
    let kbs = bytes as u64 * 1000 / us; // bytes per ms = kB/s
    (kbs / 1000, kbs % 1000 / 10)
}

#[derive(Default, Clone, Copy)]
struct Tot {
    files: u32,
    comp: usize,
    raw: usize,
    read: u32,
    dec: u32,
    xf: u32,
    mka: [usize; 4],
    mka_t: [u32; 4],
}

impl Tot {
    fn add(&mut self, o: &Tot) {
        self.files += o.files;
        self.comp += o.comp;
        self.raw += o.raw;
        self.read += o.read;
        self.dec += o.dec;
        self.xf += o.xf;
        for i in 0..4 {
            self.mka[i] += o.mka[i];
            self.mka_t[i] += o.mka_t[i];
        }
    }
}

fn best_of<F: FnMut() -> u32>(runs: u32, mut f: F) -> u32 {
    (0..runs).map(|_| f()).min().unwrap_or(0)
}

/// The stream's transform segments (8-byte records after the 12-byte header).
fn parse_segments(z: &[u8], n: usize) -> Vec<Segment> {
    (0..n)
        .map(|i| {
            let r = &z[12 + 8 * i..20 + 8 * i];
            let kind = match r[0] {
                1 => Kind::Planes,
                2 => Kind::DeltaPlanes,
                _ => Kind::Raw,
            };
            let stride = u16::from_le_bytes([r[2], r[3]]);
            let len = u32::from_le_bytes([r[4], r[5], r[6], r[7]]);
            Segment { kind, width: r[1] & 0x7f, big_endian: r[1] & 0x80 != 0, stride, len }
        })
        .collect()
}

/// rANS symbol decode cost with 1 vs several tables (D-cache footprint per table: 1.3 KiB;
/// 5 KiB with the slot table used before, see research/n64-performance.md).
fn rans_micro(d: &Dict) {
    use f2_n64z::rans::{RansDec, Table};
    let mut tabs: Vec<Table> = Vec::new();
    for f in d.lit.iter().flatten() {
        let mut t = Table::new();
        t.build(f);
        tabs.push(t);
    }
    let n = 32768u32;
    let payload = d.content;
    // ares cost calibration: 4 ALU ops, a cached load, an uncached-line load, a multiply.
    {
        use core::hint::black_box as bb;
        let buf = vec![1u8; 256 * 1024];
        let iters = 16384u32;
        let mut a = bb(1u32);
        let t_alu = best_of(2, || {
            let t0 = ticks();
            for i in 0..iters {
                a = (a ^ i).wrapping_add(i << 3) ^ (a >> 5);
            }
            ticks() - t0
        });
        let small = &buf[..4096];
        let mut s = 0u32;
        let t_hit = best_of(2, || {
            let t0 = ticks();
            for i in 0..iters as usize {
                s = s.wrapping_add(u32::from(small[(i * 67) & 4095]));
            }
            ticks() - t0
        });
        let t_miss = best_of(2, || {
            let t0 = ticks();
            for i in 0..iters as usize {
                s = s.wrapping_add(u32::from(buf[(i * 16 * 7 + 3) & (256 * 1024 - 1)]));
            }
            ticks() - t0
        });
        let mut m = bb(3u32);
        let t_mul = best_of(2, || {
            let t0 = ticks();
            for i in 0..iters {
                m = m.wrapping_mul(i | 1).wrapping_add(i);
            }
            ticks() - t0
        });
        let c = |t: u32| t as u64 * 2 * 100 / u64::from(iters);
        log!(
            "PROBE e ares calibration (cyc/iter x100): alu4 {} cached-load {} miss-load {} mul+add {} [{} {} {}]",
            c(t_alu),
            c(t_hit),
            c(t_miss),
            c(t_mul),
            a,
            s,
            m
        );
    }
    for k in [1usize, 2, 4, tabs.len()] {
        let mut sum = 0u32;
        let t = best_of(2, || {
            let mut r = RansDec::new(payload).unwrap();
            let mut prev = 0u32;
            let t0 = ticks();
            for _ in 0..n {
                prev = r.get(&tabs[prev as usize & (k - 1)]);
                sum = sum.wrapping_add(prev);
            }
            ticks() - t0
        });
        log!("PROBE e rans micro: {} tables (selected by previous symbol): {} cyc/symbol [{}]", k, t as u64 * 2 / u64::from(n), sum);
    }
}

fn read_timed(path: &str) -> (Option<Vec<u8>>, u32) {
    let t0 = ticks();
    let d = rt::file_read(path);
    (d, ticks() - t0)
}

/// Decodes `z` REPS times into `out`; returns (first ticks, best ticks, ok).
fn decode_timed(dec: &mut Decoder, z: &[u8], dict: Option<&Dict>, out: &mut [u8], tmp: &mut [u8]) -> (u32, u32, bool) {
    let mut first = 0;
    let mut best = u32::MAX;
    let mut ok = true;
    for r in 0..REPS {
        let t0 = ticks();
        let res = dec.decode(z, dict, out, tmp);
        let t = ticks() - t0;
        if let Err(e) = res {
            log!("PROBE e f2z: decode error {}", e);
            ok = false;
            break;
        }
        if r == 0 {
            first = t;
        }
        best = best.min(t);
    }
    (first, best, ok)
}

fn log_tot(name: &str, t: &Tot) {
    let (a, b) = mbs(t.raw, t.dec);
    let (ra, rb) = mbs(t.comp, t.read);
    let (ta, tb) = mbs(t.raw, t.dec + t.read);
    log!(
        "PROBE e total {}: {} files, f2z {} -> {} B ({}%), ROM read {} us ({}.{:02} MB/s in), decode {} us = {}.{:02} MB/s out (of which inverse transform {} us), read+decode {}.{:02} MB/s",
        name,
        t.files,
        t.comp,
        t.raw,
        t.comp * 100 / t.raw.max(1),
        ticks_to_us(t.read),
        ra,
        rb,
        ticks_to_us(t.dec),
        a,
        b,
        ticks_to_us(t.xf),
        ta,
        tb
    );
    for l in 0..4 {
        let (a, b) = mbs(t.raw, t.mka_t[l]);
        log!(
            "PROBE e total {} mkasset c{}: {} B ({}%), asset_load {} us = {}.{:02} MB/s out",
            name,
            l,
            t.mka[l],
            t.mka[l] * 100 / t.raw.max(1),
            ticks_to_us(t.mka_t[l]),
            a,
            b
        );
    }
}

pub fn f2z_probes() {
    let Some(list) = rt::file_read("rom:/f2z/list.txt") else {
        log!("PROBE e f2z: rom:/f2z/list.txt missing (stage with tools/n64/stage-f2z-probe.py)");
        return;
    };
    let entries = parse_list(core::str::from_utf8(&list).unwrap_or(""));
    drop(list);
    let mut dec = new_decoder();
    log!("PROBE e f2z: {} files, Decoder {} B on the heap, {} decodes each (first / best)", entries.len(), core::mem::size_of::<Decoder>(), REPS);

    // Pass 1: dictionaries (decoded without a dictionary, kept resident).
    let mut dict_bytes: Vec<(&str, Vec<u8>)> = Vec::new();
    let mut classes: Vec<(String, Tot)> = Vec::new();
    let mut all_ok = true;
    let mut process = |e: &Entry, dec: &mut Decoder, dict: Option<&Dict>, keep: bool| -> Option<Vec<u8>> {
        let (z, t_read) = read_timed(&alloc::format!("rom:/f2z/{}", e.path));
        let Some((z, h)) = z.and_then(|z| header(&z).ok().map(|h| (z, h))) else {
            log!("PROBE e f2z {}: missing or not F2Z", e.path);
            all_ok = false;
            return None;
        };
        let mut out = vec![0u8; h.raw_size as usize];
        let mut tmp = if h.needs_scratch() { vec![0u8; h.raw_size as usize] } else { Vec::new() };
        let (first, best, ok) = decode_timed(dec, &z, dict, &mut out, &mut tmp);
        let fnv = fnv1a(&out);
        // Profile split: the inverse structure transform alone, run as the decoder runs it
        // (in place when mostly raw, else tmp -> out; same segments; the cost does not
        // depend on the byte values; clobbers `out` after its checksum), the rest of the
        // decode is LZ + rANS.
        let t_xf = if h.nseg > 0 {
            let segs = parse_segments(&z, h.nseg as usize);
            let raw: usize = segs.iter().filter(|s| s.kind == Kind::Raw).map(|s| s.len as usize).sum();
            best_of(2, || {
                let t0 = ticks();
                let _ = if raw * 2 >= out.len() { transform::inverse_in_place(&segs, &mut out, &mut tmp) } else { transform::inverse(&segs, &tmp, &mut out) };
                ticks() - t0
            })
        } else {
            0
        };
        let good = ok && fnv == e.fnv && out.len() == e.raw;
        all_ok &= good;
        let (a, b) = mbs(out.len(), best);
        log!(
            "PROBE e f2z {}: {} -> {} B, nseg {}, ROM read {} us, decode first {} us best {} us = {}.{:02} MB/s out ({} cyc/B; inverse transform {} us), fnv {:08x} host {:08x} {}",
            e.path,
            z.len(),
            out.len(),
            h.nseg,
            ticks_to_us(t_read),
            ticks_to_us(first),
            ticks_to_us(best),
            a,
            b,
            best as u64 * 2 / out.len().max(1) as u64,
            ticks_to_us(t_xf),
            fnv,
            e.fnv,
            if good { "OK" } else { "MISMATCH" }
        );
        let mut tot = Tot { files: 1, comp: z.len(), raw: out.len(), read: t_read, dec: best, xf: t_xf, ..Tot::default() };
        drop(z);
        drop(tmp);
        let kept = if keep { Some(out) } else { None };
        // mkasset variants of the same decoded bytes.
        let mut line = String::new();
        for l in 0..4 {
            let (d, t) = read_timed(&alloc::format!("rom:/mka{}/{}", l, e.path));
            let ok = d.as_ref().is_some_and(|d| fnv1a(d) == e.fnv);
            all_ok &= ok;
            let (a, b) = mbs(e.raw, t);
            line.push_str(&alloc::format!(" | c{} {} B {} us {}.{:02} MB/s{}", l, e.mka[l], ticks_to_us(t), a, b, if ok { "" } else { " BAD" }));
            tot.mka[l] = e.mka[l];
            tot.mka_t[l] = t;
        }
        log!("PROBE e mkasset {}:{}", e.path, line);
        let class: &str = e.path.split('/').next().unwrap_or("");
        match classes.iter_mut().find(|(c, _)| c == class) {
            Some((_, t)) => t.add(&tot),
            None => classes.push((class.into(), tot)),
        }
        kept
    };
    for e in entries.iter().filter(|e| e.dict.is_none()) {
        if let Some(b) = process(e, &mut dec, None, true) {
            dict_bytes.push((&e.path, b));
        }
    }
    // Parse + build default tables (once per dictionary at boot).
    let mut dicts: Vec<(&str, Dict)> = Vec::new();
    for (p, b) in &dict_bytes {
        let t0 = ticks();
        let d = Dict::parse(b);
        let t1 = ticks();
        match d {
            Ok(d) => {
                dec.load_dict(&d);
                let t2 = ticks();
                log!(
                    "PROBE e dict {}: id {} content {} B, parse {} us, load_dict (build default tables) {} us",
                    p,
                    d.id,
                    d.content.len(),
                    ticks_to_us(t1 - t0),
                    ticks_to_us(t2 - t1)
                );
                dicts.push((p, d));
            }
            Err(e) => log!("PROBE e dict {}: parse error {}", p, e),
        }
    }
    // Pass 2: assets, grouped by dictionary (one table rebuild per switch, not timed in decode).
    let mut loaded: Option<&str> = None;
    for (dp, d) in &dicts {
        for e in entries.iter().filter(|e| e.dict.as_deref() == Some(*dp)) {
            if loaded != Some(*dp) {
                dec.load_dict(d);
                loaded = Some(*dp);
            }
            process(e, &mut dec, Some(d), false);
        }
    }
    if let Some((_, d)) = dicts.iter().find(|(_, d)| d.lit.iter().filter(|t| t.is_some()).count() >= 4) {
        rans_micro(d);
    }
    let mut assets = Tot::default();
    for (c, t) in &classes {
        log_tot(c, t);
        if c != "dict" {
            assets.add(t);
        }
    }
    log_tot("assets (excl. dict)", &assets);
    log!("PROBE e f2z correctness: {}", if all_ok { "all outputs match the host FNV-1a" } else { "MISMATCH (see above)" });
}
