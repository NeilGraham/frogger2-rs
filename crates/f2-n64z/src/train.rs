//! Per-class dictionary training.
//!
//! Content: a COVER-style greedy selection (Liao, Petri, Moffat, Wirth 2016 — the algorithm
//! behind `zstd --train`): every `D`-byte substring ("d-mer") is scored by the number of
//! *other* training files that contain it; the data is split into epochs and the best
//! `K`-byte segment of each epoch (sum of the scores of its distinct, not yet covered
//! d-mers) is selected; the covered d-mers are then zeroed so later segments add new
//! content. Segments are ordered by score so the most useful sit at the end of the
//! dictionary (smallest match distances).
//!
//! Default entropy tables: every training file is compressed with the dictionary content;
//! the per-file symbol distributions are averaged with equal weight (the tables matter most
//! for small files, which would otherwise be drowned by large ones) and normalised.

use std::collections::HashMap;

use crate::dict::EncDict;
use crate::enc::{compress_stats, Options};
use crate::rans::enc::normalize;
use crate::rans::Freqs;
use crate::transform::Segment;

/// d-mer length and segment length.
pub const DMER: usize = 8;
pub const SEGMENT: usize = 256;
const HBITS: u32 = 24;

#[inline]
fn dhash(b: &[u8]) -> usize {
    let v = u64::from_le_bytes(b[..8].try_into().unwrap());
    (v.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - HBITS)) as usize
}

/// Selects up to `size` bytes of dictionary content from `samples` (already transformed).
pub fn train_content(samples: &[&[u8]], size: usize) -> Vec<u8> {
    if size == 0 {
        return Vec::new();
    }
    // score = number of distinct files containing the d-mer, minus one
    let mut freq = vec![0u16; 1 << HBITS];
    let mut stamp = vec![u32::MAX; 1 << HBITS];
    for (fi, s) in samples.iter().enumerate() {
        if s.len() < DMER {
            continue;
        }
        for p in 0..=s.len() - DMER {
            let h = dhash(&s[p..]);
            if stamp[h] != fi as u32 {
                stamp[h] = fi as u32;
                freq[h] = freq[h].saturating_add(1);
            }
        }
    }
    for f in freq.iter_mut() {
        *f = f.saturating_sub(1);
    }
    // candidate positions: (sample, offset) concatenated logically
    let total: usize = samples.iter().map(|s| s.len().saturating_sub(SEGMENT - 1)).sum();
    if total == 0 {
        let mut all: Vec<u8> = samples.iter().flat_map(|s| s.iter().copied()).collect();
        all.truncate(size);
        return all;
    }
    let nseg = size.div_ceil(SEGMENT);
    let epoch = (total / nseg).max(1);
    let mut chosen: Vec<(u64, usize, usize)> = Vec::new(); // (score, sample, offset)
    let mut flat: Vec<(usize, usize)> = Vec::new(); // (sample, max start)
    for (si, s) in samples.iter().enumerate() {
        if s.len() >= SEGMENT {
            flat.push((si, s.len() - SEGMENT + 1));
        }
    }
    let mut cursor = (0usize, 0usize); // (index into flat, offset)
    for _ in 0..nseg {
        // scan `epoch` candidate starts beginning at `cursor`
        let mut best: Option<(u64, usize, usize)> = None;
        let mut left = epoch;
        while left > 0 && cursor.0 < flat.len() {
            let (si, nstart) = flat[cursor.0];
            let s = samples[si];
            let from = cursor.1;
            let to = (from + left).min(nstart);
            // sliding window over d-mers of s[p..p+SEGMENT]
            let mut inwin: HashMap<usize, u32> = HashMap::new();
            let mut score = 0u64;
            let nd = SEGMENT - DMER + 1;
            for q in from..from + nd {
                let h = dhash(&s[q..]);
                let c = inwin.entry(h).or_insert(0);
                if *c == 0 {
                    score += u64::from(freq[h]);
                }
                *c += 1;
            }
            for p in from..to {
                if best.is_none_or(|b| score > b.0) {
                    best = Some((score, si, p));
                }
                if p + 1 < to {
                    let h_out = dhash(&s[p..]);
                    let c = inwin.get_mut(&h_out).unwrap();
                    *c -= 1;
                    if *c == 0 {
                        score -= u64::from(freq[h_out]);
                        inwin.remove(&h_out);
                    }
                    let h_in = dhash(&s[p + nd..]);
                    let c = inwin.entry(h_in).or_insert(0);
                    if *c == 0 {
                        score += u64::from(freq[h_in]);
                    }
                    *c += 1;
                }
            }
            left -= to - from;
            if to >= nstart {
                cursor = (cursor.0 + 1, 0);
            } else {
                cursor.1 = to;
            }
        }
        let Some((score, si, p)) = best else { break };
        if score == 0 {
            continue;
        }
        for q in p..p + SEGMENT - DMER + 1 {
            freq[dhash(&samples[si][q..])] = 0;
        }
        chosen.push((score, si, p));
    }
    chosen.sort_by_key(|c| c.0);
    let mut out = Vec::with_capacity(size);
    // keep the highest-scoring segments if over budget (they are last)
    let skip = chosen.len().saturating_sub(size / SEGMENT);
    for &(_, si, p) in &chosen[skip..] {
        out.extend_from_slice(&samples[si][p..p + SEGMENT]);
    }
    out
}

/// A training sample: original bytes and the transform segments it will be compressed with.
pub struct Sample<'a> {
    pub data: &'a [u8],
    pub segs: &'a [Segment],
}

/// Trains a full dictionary (content + default tables) for one asset class.
pub fn train(samples: &[Sample], size: usize, id: u8, opt: &Options) -> EncDict {
    let transformed: Vec<Vec<u8>> = samples
        .iter()
        .map(|s| if s.segs.iter().all(|g| g.kind == crate::transform::Kind::Raw) { s.data.to_vec() } else { crate::transform::forward(s.segs, s.data) })
        .collect();
    let refs: Vec<&[u8]> = transformed.iter().map(Vec::as_slice).collect();
    let content = train_content(&refs, size);
    let d0 = EncDict::content_only(id, content);
    let stats: Vec<crate::enc::Stats> = std::thread::scope(|sc| {
        let hs: Vec<_> = samples.iter().map(|s| sc.spawn(|| compress_stats(s.data, s.segs, Some(&d0), opt).1)).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    // equal weight per file: average the normalised distributions
    let avg = |get: &dyn Fn(&crate::enc::Stats) -> &[u32], n: usize| -> Option<Freqs> {
        let mut acc = vec![0f64; n];
        let mut any = false;
        for st in &stats {
            let c = get(st);
            let t: u64 = c.iter().map(|&x| u64::from(x)).sum();
            if t == 0 {
                continue;
            }
            any = true;
            for (a, &x) in acc.iter_mut().zip(c) {
                *a += f64::from(x) / t as f64;
            }
        }
        // every symbol keeps a small floor so default tables can code anything
        any.then(|| normalize(&acc.iter().map(|&a| (a * 1e6).round() as u32 + 1).collect::<Vec<_>>()))
    };
    // literal mode: the one cheapest over the whole class
    let mut total = crate::enc::Stats::default();
    for st in &stats {
        total.add(st);
    }
    let mut best = (f64::INFINITY, 0u8);
    for m in 0..crate::litctx::NMODES as u8 {
        let c: f64 = total.lit_noam[m as usize]
            .iter()
            .map(|counts| {
                let f = normalize(&counts.map(|x| x + 1));
                crate::rans::enc::cost_bits(counts, &f)
            })
            .sum();
        if c < best.0 {
            best = (c, m);
        }
    }
    let lit_mode = best.1;
    let lit = (0..crate::litctx::nctx(lit_mode)).map(|ctx| avg(&|st| &st.lit_noam[lit_mode as usize][ctx][..], 256)).collect();
    let am = avg(&|st| &st.am[..], 256);
    let codes =
        [avg(&|st| &st.counts[1][..], crate::codes::NLL), avg(&|st| &st.counts[2][..], crate::codes::NML), avg(&|st| &st.counts[3][..], crate::codes::NOF)];
    EncDict { id, content: d0.content, lit_mode, lit, am, codes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_prefers_shared_substrings() {
        let shared: Vec<u8> = (0..600u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut files = Vec::new();
        for k in 0..6u32 {
            let mut x = 0x9E37_79B9u32 ^ (k + 1).wrapping_mul(0x85EB_CA6B);
            let mut f: Vec<u8> = (0..3000)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    (x >> 24) as u8
                })
                .collect();
            f.splice(1000..1000, shared.iter().copied());
            files.push(f);
        }
        let refs: Vec<&[u8]> = files.iter().map(Vec::as_slice).collect();
        let d = train_content(&refs, 512);
        assert!(d.len() <= 512);
        // the dictionary should mostly consist of the shared block
        let hits = d.windows(8).filter(|w| shared.windows(8).any(|s| s == *w)).count();
        assert!(hits * 2 > d.len(), "{hits} of {}", d.len());
    }
}
