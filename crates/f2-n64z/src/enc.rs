//! Host-side F2Z encoder: hash-chain match finder, price-based optimal parse (iterated to
//! convergence of the block statistics), static rANS block writer.

use crate::bits::BitWriter;
use crate::codes::{self, LL_DLOG, MIN_MATCH, MIN_REP, ML_DLOG, NLL, NML, NOF, OF_DLOG, REP_CODES};
use crate::dict::EncDict;
use crate::rans::enc::{cost_bits, encode as rans_encode, normalize};
use crate::rans::{Freqs, PROB_SCALE};
use crate::transform::{self, Segment};
use crate::{INIT_REPS, LIT, LL, MAGIC, ML, OF};

#[derive(Clone, Debug)]
pub struct Options {
    /// Hash-chain candidates examined per position.
    pub max_chain: u32,
    /// A match at least this long is taken immediately.
    pub nice_len: u32,
    /// Optimal-parse passes per block (statistics re-estimated between passes).
    pub passes: u32,
    /// Output bytes per block (entropy tables are per block).
    pub block_size: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options::level(9)
    }
}

impl Options {
    /// 1 (fast) … 9 (best).
    pub fn level(l: u32) -> Options {
        match l {
            0..=3 => Options { max_chain: 8, nice_len: 32, passes: 1, block_size: 256 << 10 },
            4..=6 => Options { max_chain: 48, nice_len: 128, passes: 2, block_size: 256 << 10 },
            _ => Options { max_chain: 256, nice_len: 273, passes: 3, block_size: 256 << 10 },
        }
    }
}

/// Symbol counts of the four streams (for dictionary table training).
#[derive(Clone, Debug)]
pub struct Stats {
    pub counts: [Vec<u32>; 4],
}

impl Default for Stats {
    fn default() -> Self {
        Stats { counts: [vec![0; 256], vec![0; NLL], vec![0; NML], vec![0; NOF]] }
    }
}

impl Stats {
    pub fn add(&mut self, o: &Stats) {
        for k in 0..4 {
            for (a, b) in self.counts[k].iter_mut().zip(&o.counts[k]) {
                *a += b;
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Seq {
    ll: u32,
    /// match length - 2
    ml: u32,
    /// 0..2 = repeat index, else distance + 2
    ofv: u32,
}

const PRICE_SHIFT: u32 = 4; // prices in 1/16 bit
const INF: u32 = u32::MAX / 2;

struct Prices {
    lit: [u32; 256],
    ll: [u32; NLL],
    ml: [u32; NML],
    of: [u32; NOF],
}

fn sym_price(f: u16) -> u32 {
    let f = if f == 0 { 0.25 } else { f64::from(f) };
    ((f64::from(PROB_SCALE) / f).log2() * f64::from(1 << PRICE_SHIFT)).round() as u32
}

impl Prices {
    fn from_freqs(t: [&Freqs; 4]) -> Prices {
        let mut p = Prices { lit: [0; 256], ll: [0; NLL], ml: [0; NML], of: [0; NOF] };
        for (i, x) in p.lit.iter_mut().enumerate() {
            *x = sym_price(if i < t[0].n as usize { t[0].f[i] } else { 0 });
        }
        for (i, x) in p.ll.iter_mut().enumerate() {
            *x = sym_price(if i < t[1].n as usize { t[1].f[i] } else { 0 });
        }
        for (i, x) in p.ml.iter_mut().enumerate() {
            *x = sym_price(if i < t[2].n as usize { t[2].f[i] } else { 0 });
        }
        for (i, x) in p.of.iter_mut().enumerate() {
            *x = sym_price(if i < t[3].n as usize { t[3].f[i] } else { 0 });
        }
        p
    }

    /// First-pass guess: literal prices from the block's byte histogram, flat-ish code prices.
    fn initial(block: &[u8]) -> Prices {
        let mut h = [1u32; 256];
        for &b in block {
            h[b as usize] += 1;
        }
        let lit = normalize(&h);
        let mut p = Prices::from_freqs([&lit, &Freqs::EMPTY, &Freqs::EMPTY, &Freqs::EMPTY]);
        for (c, x) in p.ll.iter_mut().enumerate() {
            *x = (2 + c as u32 / 2) << PRICE_SHIFT;
        }
        for (c, x) in p.ml.iter_mut().enumerate() {
            *x = (3 + c as u32 / 2) << PRICE_SHIFT;
        }
        for (c, x) in p.of.iter_mut().enumerate() {
            *x = if c < 3 { (2 + c as u32) << PRICE_SHIFT } else { (4 + (c as u32 - 3) / 3) << PRICE_SHIFT };
        }
        p
    }

    #[inline]
    fn ll_price(&self, v: u32) -> u32 {
        let (c, nb, _) = codes::encode(v, LL_DLOG);
        self.ll[c as usize] + (nb << PRICE_SHIFT)
    }
    #[inline]
    fn ml_price(&self, len: u32) -> u32 {
        let (c, nb, _) = codes::encode(len - 2, ML_DLOG);
        self.ml[c as usize] + (nb << PRICE_SHIFT)
    }
    #[inline]
    fn dist_price(&self, dist: u32) -> u32 {
        let (c, nb, _) = codes::encode(dist - 1, OF_DLOG);
        self.of[(c + REP_CODES) as usize] + (nb << PRICE_SHIFT)
    }
}

// ---- match finder ----------------------------------------------------------------

const HBITS: u32 = 18;
const NONE: u32 = u32::MAX;

struct HashChain {
    head: Vec<u32>,
    prev: Vec<u32>,
    inserted: usize,
}

#[inline]
fn hash3(w: &[u8], p: usize) -> usize {
    let v = u32::from(w[p]) | (u32::from(w[p + 1]) << 8) | (u32::from(w[p + 2]) << 16);
    (v.wrapping_mul(2_654_435_761) >> (32 - HBITS)) as usize
}

#[inline]
fn common(w: &[u8], a: usize, b: usize, max: usize) -> usize {
    let mut n = 0;
    while n + 8 <= max {
        let x = u64::from_le_bytes(w[a + n..a + n + 8].try_into().unwrap()) ^ u64::from_le_bytes(w[b + n..b + n + 8].try_into().unwrap());
        if x != 0 {
            return n + (x.trailing_zeros() / 8) as usize;
        }
        n += 8;
    }
    while n < max && w[a + n] == w[b + n] {
        n += 1;
    }
    n
}

impl HashChain {
    fn new(n: usize) -> HashChain {
        HashChain { head: vec![NONE; 1 << HBITS], prev: vec![NONE; n], inserted: 0 }
    }
    fn insert_upto(&mut self, w: &[u8], upto: usize) {
        let lim = upto.min(w.len().saturating_sub(2));
        while self.inserted < lim {
            let h = hash3(w, self.inserted);
            self.prev[self.inserted] = self.head[h];
            self.head[h] = self.inserted as u32;
            self.inserted += 1;
        }
        self.inserted = self.inserted.max(upto.min(w.len()));
    }
    /// Matches at `pos` with strictly increasing lengths (≥ MIN_MATCH), each the closest
    /// candidate of its length.
    fn find(&self, w: &[u8], pos: usize, maxlen: usize, opt: &Options, out: &mut Vec<(u32, u32)>) {
        if maxlen < MIN_MATCH as usize || pos + 2 >= w.len() {
            return;
        }
        let mut cand = self.head[hash3(w, pos)];
        let mut best = MIN_MATCH as usize - 1;
        let mut chain = opt.max_chain;
        while cand != NONE && chain > 0 {
            let c = cand as usize;
            if c < pos && w[c + best] == w[pos + best] {
                let l = common(w, c, pos, maxlen);
                if l > best {
                    best = l;
                    out.push((l as u32, (pos - c) as u32));
                    if l >= opt.nice_len as usize || l == maxlen {
                        break;
                    }
                }
            }
            cand = self.prev[c];
            chain -= 1;
        }
    }
}

// ---- optimal parse ----------------------------------------------------------------

#[derive(Clone, Copy)]
struct Node {
    cost: u32,
    /// 0 = arrived by a literal
    len: u32,
    ofv: u32,
    litrun: u32,
    reps: [u32; 3],
}

#[inline]
fn next_reps(reps: [u32; 3], ofv: u32) -> [u32; 3] {
    match ofv {
        0 => reps,
        1 => [reps[1], reps[0], reps[2]],
        2 => [reps[2], reps[0], reps[1]],
        v => [v - 2, reps[0], reps[1]],
    }
}

/// Parses `w[start..end]` (window coordinates) given the matches at each position.
fn parse_block(w: &[u8], start: usize, end: usize, mstart: &[u32], matches: &[(u32, u32)], reps0: [u32; 3], p: &Prices, opt: &Options) -> (Vec<Seq>, [u32; 3]) {
    let n = end - start;
    let mut nodes = vec![Node { cost: INF, len: 0, ofv: 0, litrun: 0, reps: reps0 }; n + 1];
    nodes[0].cost = p.ll_price(0);
    let nice = opt.nice_len as usize;
    let ll0 = p.ll_price(0);
    let mut skip_to = 0usize;
    for i in 0..n {
        let nd = nodes[i];
        if nd.cost >= INF || i < skip_to {
            continue;
        }
        let pos = start + i;
        // literal
        {
            let c = i64::from(nd.cost) + i64::from(p.lit[w[pos] as usize]) + i64::from(p.ll_price(nd.litrun + 1)) - i64::from(p.ll_price(nd.litrun));
            let c = c.max(0) as u32;
            let t = &mut nodes[i + 1];
            if c < t.cost {
                *t = Node { cost: c, len: 0, ofv: 0, litrun: nd.litrun + 1, reps: nd.reps };
            }
        }
        let maxlen = n - i;
        let mut longest = 0usize;
        // repeat offsets
        for r in 0..3u32 {
            let d = nd.reps[r as usize] as usize;
            if d == 0 || d > pos || (r > 0 && nd.reps[..r as usize].contains(&(d as u32))) {
                continue;
            }
            let l = common(w, pos - d, pos, maxlen);
            if l < MIN_REP as usize {
                continue;
            }
            longest = longest.max(l);
            let base = nd.cost + p.of[r as usize] + ll0;
            let reps = next_reps(nd.reps, r);
            let mut relax = |len: usize| {
                let c = base + p.ml_price(len as u32);
                let t = &mut nodes[i + len];
                if c < t.cost {
                    *t = Node { cost: c, len: len as u32, ofv: r, litrun: 0, reps };
                }
            };
            for len in MIN_REP as usize..=l.min(nice) {
                relax(len);
            }
            if l > nice {
                relax(l);
            }
        }
        // normal matches
        let ms = &matches[mstart[i] as usize..mstart[i + 1] as usize];
        let mut prev_len = MIN_MATCH as usize - 1;
        for &(ml, dist) in ms {
            let ml = (ml as usize).min(maxlen);
            if ml <= prev_len {
                continue;
            }
            longest = longest.max(ml);
            let base = nd.cost + p.dist_price(dist) + ll0;
            let ofv = dist + 2;
            let reps = [dist, nd.reps[0], nd.reps[1]];
            let mut relax = |len: usize| {
                let c = base + p.ml_price(len as u32);
                let t = &mut nodes[i + len];
                if c < t.cost {
                    *t = Node { cost: c, len: len as u32, ofv, litrun: 0, reps };
                }
            };
            for len in prev_len + 1..=ml.min(nice) {
                relax(len);
            }
            if ml > nice {
                relax(ml);
            }
            prev_len = ml;
        }
        if longest >= nice {
            // A long match: take it without expanding the positions it covers.
            skip_to = i + longest;
        }
    }
    // backtrack
    let mut path = Vec::new();
    let mut i = n;
    while i > 0 {
        let nd = nodes[i];
        if nd.len == 0 {
            path.push((0u32, 0u32));
            i -= 1;
        } else {
            path.push((nd.len, nd.ofv));
            i -= nd.len as usize;
        }
    }
    path.reverse();
    let mut seqs = Vec::new();
    let mut lits = 0u32;
    for (len, ofv) in path {
        if len == 0 {
            lits += 1;
        } else {
            seqs.push(Seq { ll: lits, ml: len - 2, ofv });
            lits = 0;
        }
    }
    (seqs, nodes[n].reps)
}

struct BlockSyms {
    lits: Vec<u8>,
    ll: Vec<u8>,
    ml: Vec<u8>,
    of: Vec<u8>,
    bits: BitWriter,
    stats: Stats,
}

fn block_syms(w: &[u8], start: usize, end: usize, seqs: &[Seq]) -> BlockSyms {
    let mut s = BlockSyms { lits: Vec::new(), ll: Vec::new(), ml: Vec::new(), of: Vec::new(), bits: BitWriter::default(), stats: Stats::default() };
    let mut pos = start;
    for q in seqs {
        s.lits.extend_from_slice(&w[pos..pos + q.ll as usize]);
        pos += q.ll as usize + q.ml as usize + 2;
        let (c, nb, e) = codes::encode(q.ll, LL_DLOG);
        s.ll.push(c as u8);
        s.bits.put(e, nb);
        let (c, nb, e) = codes::encode(q.ml, ML_DLOG);
        s.ml.push(c as u8);
        s.bits.put(e, nb);
        if q.ofv < REP_CODES {
            s.of.push(q.ofv as u8);
        } else {
            let (c, nb, e) = codes::encode(q.ofv - 3, OF_DLOG);
            s.of.push((c + REP_CODES) as u8);
            s.bits.put(e, nb);
        }
    }
    s.lits.extend_from_slice(&w[pos..end]);
    for &b in &s.lits {
        s.stats.counts[LIT][b as usize] += 1;
    }
    for &b in &s.ll {
        s.stats.counts[LL][b as usize] += 1;
    }
    for &b in &s.ml {
        s.stats.counts[ML][b as usize] += 1;
    }
    for &b in &s.of {
        s.stats.counts[OF][b as usize] += 1;
    }
    s
}

fn table_bytes(f: &Freqs) -> usize {
    let mut v = Vec::new();
    f.write(&mut v);
    v.len()
}

/// Writes one stream header + returns its payload.
fn write_stream(hdr: &mut Vec<u8>, syms: &[u8], counts: &[u32], dict: Option<&Freqs>) -> Vec<u8> {
    let used = counts.iter().filter(|&&c| c > 0).count();
    if used == 0 {
        hdr.push(3);
        return Vec::new();
    }
    if used == 1 {
        hdr.push(2);
        hdr.push(counts.iter().position(|&c| c > 0).unwrap() as u8);
        return Vec::new();
    }
    let own = normalize(counts);
    let own_cost = cost_bits(counts, &own) + 8.0 * table_bytes(&own) as f64;
    let dict_cost = dict.map_or(f64::INFINITY, |d| cost_bits(counts, d));
    let (mode, f) = if dict_cost <= own_cost { (0u8, dict.unwrap()) } else { (1u8, &own) };
    hdr.push(mode);
    if mode == 1 {
        f.write(hdr);
    }
    let payload = rans_encode(syms, f);
    hdr.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    payload
}

/// Compresses `data` (original bytes) with the given transform segments and dictionary.
pub fn compress(data: &[u8], segs: &[Segment], dict: Option<&EncDict>, opt: &Options) -> Vec<u8> {
    compress_stats(data, segs, dict, opt).0
}

/// Like [`compress`], also returning the symbol statistics.
pub fn compress_stats(data: &[u8], segs: &[Segment], dict: Option<&EncDict>, opt: &Options) -> (Vec<u8>, Stats) {
    let segs: Vec<Segment> = segs.iter().copied().filter(|s| s.len > 0).collect();
    let trivial = segs.iter().all(|s| s.kind == transform::Kind::Raw);
    let tdata = if trivial { data.to_vec() } else { transform::forward(&segs, data) };
    let dcontent: &[u8] = dict.map_or(&[], |d| &d.content);
    let mut w = Vec::with_capacity(dcontent.len() + tdata.len());
    w.extend_from_slice(dcontent);
    w.extend_from_slice(&tdata);
    let base = dcontent.len();

    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.push(dict.map_or(0, |d| d.id));
    out.push(if trivial { 0 } else { segs.len() as u8 });
    out.extend_from_slice(&[0, 0]);
    if !trivial {
        assert!(segs.len() <= crate::dec::MAX_SEGMENTS, "too many segments");
        for s in &segs {
            s.write(&mut out);
        }
    }
    let nblocks = tdata.len().div_ceil(opt.block_size.max(1));
    out.extend_from_slice(&(nblocks as u32).to_le_bytes());

    let mut hc = HashChain::new(w.len());
    let mut reps = INIT_REPS;
    let mut total = Stats::default();
    let dict_tables: [Option<&Freqs>; 4] = match dict {
        Some(d) => [d.tables[0].as_ref(), d.tables[1].as_ref(), d.tables[2].as_ref(), d.tables[3].as_ref()],
        None => [None; 4],
    };
    let mut mbuf: Vec<(u32, u32)> = Vec::new();
    for b in 0..nblocks {
        let start = base + b * opt.block_size;
        let end = (start + opt.block_size).min(w.len());
        // collect matches
        let mut mstart = Vec::with_capacity(end - start + 1);
        let mut matches: Vec<(u32, u32)> = Vec::new();
        let mut skip_until = start;
        for pos in start..end {
            mstart.push(matches.len() as u32);
            hc.insert_upto(&w, pos);
            if pos < skip_until {
                continue;
            }
            mbuf.clear();
            hc.find(&w, pos, end - pos, opt, &mut mbuf);
            if let Some(&(l, _)) = mbuf.last() {
                if l >= opt.nice_len {
                    skip_until = pos + l as usize;
                }
            }
            matches.extend_from_slice(&mbuf);
        }
        mstart.push(matches.len() as u32);
        hc.insert_upto(&w, end);

        let mut prices = if dict_tables.iter().all(|t| t.is_some()) {
            Prices::from_freqs([dict_tables[0].unwrap(), dict_tables[1].unwrap(), dict_tables[2].unwrap(), dict_tables[3].unwrap()])
        } else {
            Prices::initial(&w[start..end])
        };
        let mut best: Option<(Vec<Seq>, [u32; 3])> = None;
        for pass in 0..opt.passes.max(1) {
            let (seqs, r) = parse_block(&w, start, end, &mstart, &matches, reps, &prices, opt);
            if pass + 1 < opt.passes {
                let s = block_syms(&w, start, end, &seqs);
                let t: Vec<Freqs> = (0..4).map(|k| normalize(&s.stats.counts[k])).collect();
                prices = Prices::from_freqs([&t[0], &t[1], &t[2], &t[3]]);
            }
            best = Some((seqs, r));
        }
        let (seqs, r) = best.unwrap();
        reps = r;
        let s = block_syms(&w, start, end, &seqs);
        out.extend_from_slice(&((end - start) as u32).to_le_bytes());
        out.extend_from_slice(&(seqs.len() as u32).to_le_bytes());
        let mut hdr = Vec::new();
        let payloads = [
            write_stream(&mut hdr, &s.lits, &s.stats.counts[LIT], dict_tables[LIT]),
            write_stream(&mut hdr, &s.ll, &s.stats.counts[LL], dict_tables[LL]),
            write_stream(&mut hdr, &s.ml, &s.stats.counts[ML], dict_tables[ML]),
            write_stream(&mut hdr, &s.of, &s.stats.counts[OF], dict_tables[OF]),
        ];
        let extra = s.bits.finish();
        out.extend_from_slice(&hdr);
        out.extend_from_slice(&(extra.len() as u32).to_le_bytes());
        for p in &payloads {
            out.extend_from_slice(p);
        }
        out.extend_from_slice(&extra);
        total.add(&s.stats);
    }
    (out, total)
}

/// Tries the identity transform and the given candidate segmentations; returns the smallest.
pub fn compress_best(data: &[u8], candidates: &[Vec<Segment>], dict: Option<&EncDict>, opt: &Options) -> (Vec<u8>, usize) {
    let mut best = (compress(data, &[], dict, opt), usize::MAX);
    for (i, c) in candidates.iter().enumerate() {
        let z = compress(data, c, dict, opt);
        if z.len() < best.0.len() {
            best = (z, i);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dec::Decoder;
    use crate::dict::Dict;

    fn rt(data: &[u8], segs: &[Segment], dict: Option<&EncDict>, opt: &Options) -> usize {
        let z = compress(data, segs, dict, opt);
        let bytes = dict.map(|d| d.to_bytes());
        let pd = bytes.as_ref().map(|b| Dict::parse(b).unwrap());
        let mut dec = Box::new(Decoder::new());
        let mut out = vec![0u8; data.len()];
        let mut tmp = vec![0u8; data.len()];
        dec.decode(&z, pd.as_ref(), &mut out, &mut tmp).unwrap();
        assert_eq!(out, data);
        z.len()
    }

    fn sample(n: usize) -> Vec<u8> {
        let words = [&b"frogger "[..], b"swampy ", b"garden ", b"lillie ", b"\x00\x00\x00\x01", b"tongue", b"baby frog "];
        let mut s = 7u32;
        let mut v = Vec::new();
        while v.len() < n {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            if s >> 28 < 3 {
                v.push((s >> 8) as u8);
            } else {
                v.extend_from_slice(words[(s >> 16) as usize % words.len()]);
            }
        }
        v.truncate(n);
        v
    }

    #[test]
    fn round_trip_basic() {
        for opt in [Options::level(1), Options::level(5), Options::level(9)] {
            assert!(rt(&[], &[], None, &opt) < 40);
            rt(b"a", &[], None, &opt);
            rt(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &[], None, &opt);
            let s = sample(30_000);
            let n = rt(&s, &[], None, &opt);
            assert!(n < s.len() / 2, "{n}");
            let zeros = vec![0u8; 300_000];
            assert!(rt(&zeros, &[], None, &opt) < 400);
        }
    }

    #[test]
    fn round_trip_small_blocks_and_transforms() {
        let s = sample(50_000);
        let opt = Options { block_size: 4096, ..Options::level(9) };
        rt(&s, &[], None, &opt);
        rt(&s, &[Segment::raw(10), Segment::delta(12, 4, true, 30_000), Segment::planes(5, 19_990)], None, &opt);
    }

    #[test]
    fn round_trip_with_dictionary() {
        let s = sample(20_000);
        let mut st = Stats::default();
        let content = s[..8000].to_vec();
        let d0 = EncDict { id: 3, content: content.clone(), tables: [None, None, None, None] };
        let (_, stats) = compress_stats(&s[8000..], &[], Some(&d0), &Options::level(9));
        st.add(&stats);
        let tables = [Some(normalize(&st.counts[0])), Some(normalize(&st.counts[1])), Some(normalize(&st.counts[2])), Some(normalize(&st.counts[3]))];
        let d = EncDict { id: 3, content, tables };
        let plain = rt(&s[10_000..12_000], &[], None, &Options::level(9));
        let with = rt(&s[10_000..12_000], &[], Some(&d), &Options::level(9));
        assert!(with < plain, "dict {with} vs plain {plain}");
        // wrong / missing dictionary is rejected, not mis-decoded
        let z = compress(&s[..100], &[], Some(&d), &Options::level(9));
        let mut out = vec![0u8; 100];
        assert_eq!(Decoder::new().decode(&z, None, &mut out, &mut []), Err(crate::Error::WrongDict(3)));
    }

    #[test]
    fn corrupt_input_never_panics() {
        let s = sample(5000);
        let z = compress(&s, &[Segment::planes(4, 5000)], None, &Options::level(5));
        let mut out = vec![0u8; s.len()];
        let mut tmp = vec![0u8; s.len()];
        let mut dec = Box::new(Decoder::new());
        let mut x = 99u32;
        for i in 0..2000 {
            let mut c = z.clone();
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let p = (x as usize >> 8) % c.len();
            c[p] ^= 1 << (i % 8);
            if i % 7 == 0 {
                c.truncate(p);
            }
            let _ = dec.decode(&c, None, &mut out, &mut tmp);
        }
    }
}
