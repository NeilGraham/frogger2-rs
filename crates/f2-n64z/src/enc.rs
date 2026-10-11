//! Host-side F2Z encoder: hash-chain match finder, price-based optimal parse (iterated so
//! the prices converge to the block's own statistics), static rANS block writer.
//!
//! Literal modelling (all decided per block by exact cost):
//! * a context mode selecting the literal table from the previous byte ([`crate::litctx`]);
//! * optionally **matched literals**: the first literal after a match is coded as
//!   `lit ^ w[pos - rep0]` with its own table (LZMA's "matched literal" idea with a static
//!   table) — for record arrays the byte one record back predicts it well. Its price
//!   depends on the parse state (arrived by a match, rep0), which the optimal parser tracks.

use crate::bits::BitWriter;
use crate::codes::{self, LL_DLOG, MIN_MATCH, MIN_REP, ML_DLOG, NLL, NML, NOF, OF_DLOG, REP_CODES};
use crate::dict::EncDict;
use crate::litctx::{ctx as lit_ctx, nctx, MAX_CTX, NMODES};
use crate::rans::enc::{cost_bits, encode as rans_encode, encode_ctx, normalize};
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
    /// Space–speed trade-off: the encoder minimises `decode cycles + size_weight × bytes`
    /// (estimated with [`CycleModel`]) per block and inside the parse. `f64::INFINITY` =
    /// smallest output; ≈ 19 = minimum load time (VR4300 at 93.75 MHz reading ROM at ~5 MB/s
    /// spends ~19 cycles per stored byte).
    pub size_weight: f64,
    /// Block types the encoder may use.
    pub allow_rans: bool,
    pub allow_token: bool,
    pub cycles: CycleModel,
}

/// Estimated VR4300 decode cost (CPU cycles) per literal, per sequence and per match byte,
/// for each block type. rANS values are the least-squares fit of the ares measurement
/// (`research/n64-performance.md` (e)); token values from the token-block probe.
#[derive(Clone, Copy, Debug)]
pub struct CycleModel {
    pub rans_lit: f64,
    pub rans_seq: f64,
    pub rans_mbyte: f64,
    pub tok_lit: f64,
    pub tok_seq: f64,
    pub tok_mbyte: f64,
}

impl Default for CycleModel {
    fn default() -> Self {
        CycleModel { rans_lit: 140.0, rans_seq: 360.0, rans_mbyte: 18.0, tok_lit: 4.0, tok_seq: 50.0, tok_mbyte: 4.0 }
    }
}

/// Measured VR4300 cost of the inverse structure transforms per transformed byte (ares).
pub const TRANSFORM_CYCLES_PER_BYTE: f64 = 21.0;

/// Minimum-load-time weight (cycles per stored byte), see [`Options::size_weight`].
pub const LOAD_TIME_WEIGHT: f64 = 19.0;

impl Default for Options {
    fn default() -> Self {
        Options::level(9)
    }
}

impl Options {
    /// 1 (fast) … 9 (best).
    pub fn level(l: u32) -> Options {
        match l {
            0..=3 => Options { max_chain: 8, nice_len: 32, passes: 1, ..Options::base() },
            4..=6 => Options { max_chain: 48, nice_len: 128, passes: 2, ..Options::base() },
            _ => Options { max_chain: 256, nice_len: 273, passes: 3, ..Options::base() },
        }
    }

    fn base() -> Options {
        Options {
            max_chain: 256,
            nice_len: 273,
            passes: 3,
            block_size: 256 << 10,
            size_weight: f64::INFINITY,
            allow_rans: true,
            allow_token: true,
            cycles: CycleModel::default(),
        }
    }

    /// Level 9 with a space–speed weight (see [`Options::size_weight`]).
    pub fn speed(size_weight: f64) -> Options {
        Options { size_weight, ..Options::level(9) }
    }
}

/// Symbol counts (for table selection and dictionary training).
/// * `counts[LIT]`: order-0 histogram of all literals; `counts[LL/ML/OF]`: code histograms.
/// * `lit[mode][ctx]`: all literals by context; `lit_noam[mode][ctx]`: literals that are not
///   first-after-match; `am`: histogram of `lit ^ predicted` of first-after-match literals.
#[derive(Clone, Debug)]
pub struct Stats {
    pub counts: [Vec<u32>; 4],
    pub lit: Vec<Vec<[u32; 256]>>,
    pub lit_noam: Vec<Vec<[u32; 256]>>,
    pub am: [u32; 256],
}

impl Default for Stats {
    fn default() -> Self {
        let z = || (0..NMODES).map(|m| vec![[0u32; 256]; nctx(m as u8)]).collect();
        Stats { counts: [vec![0; 256], vec![0; NLL], vec![0; NML], vec![0; NOF]], lit: z(), lit_noam: z(), am: [0; 256] }
    }
}

impl Stats {
    pub fn add(&mut self, o: &Stats) {
        for k in 0..4 {
            for (a, b) in self.counts[k].iter_mut().zip(&o.counts[k]) {
                *a += b;
            }
        }
        for (dst, src) in [(&mut self.lit, &o.lit), (&mut self.lit_noam, &o.lit_noam)] {
            for (a, b) in dst.iter_mut().zip(src) {
                for (x, y) in a.iter_mut().zip(b) {
                    for (p, q) in x.iter_mut().zip(y) {
                        *p += q;
                    }
                }
            }
        }
        for (a, b) in self.am.iter_mut().zip(&o.am) {
            *a += b;
        }
    }

    /// `pos`: output position; `pred` = byte at rep0 when the literal is first after a match.
    fn add_lit(&mut self, prev: u8, pos: usize, b: u8, pred: Option<u8>) {
        self.counts[LIT][b as usize] += 1;
        for m in 0..NMODES {
            let c = lit_ctx(m as u8, prev, pos);
            self.lit[m][c][b as usize] += 1;
            if pred.is_none() {
                self.lit_noam[m][c][b as usize] += 1;
            }
        }
        if let Some(p) = pred {
            self.am[(b ^ p) as usize] += 1;
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

/// Literal coding choice of a block.
#[derive(Clone, Debug)]
struct LitModel {
    mode: u8,
    am_on: bool,
    /// One table per context (used for pricing / encoding when not a dictionary table).
    tables: Vec<Freqs>,
    am: Freqs,
}

struct Prices {
    /// Token block pricing (bytes) instead of rANS.
    token: bool,
    /// Decode-cycle costs in price units (per literal, per sequence, per match byte).
    c_lit: u32,
    c_seq: u32,
    c_mbyte: u32,
    lit_mode: u8,
    am_on: bool,
    lit: [[u32; 256]; MAX_CTX],
    am: [u32; 256],
    ll: [u32; NLL],
    ml: [u32; NML],
    of: [u32; NOF],
}

fn sym_price(f: u16) -> u32 {
    let f = if f == 0 { 0.25 } else { f64::from(f) };
    ((f64::from(PROB_SCALE) / f).log2() * f64::from(1 << PRICE_SHIFT)).round() as u32
}

fn fill(dst: &mut [u32], f: &Freqs) {
    for (i, x) in dst.iter_mut().enumerate() {
        *x = sym_price(if i < f.n as usize { f.f[i] } else { 0 });
    }
}

impl Prices {
    fn new(lm: &LitModel, t: [&Freqs; 3]) -> Prices {
        let mut p = Prices {
            token: false,
            c_lit: 0,
            c_seq: 0,
            c_mbyte: 0,
            lit_mode: lm.mode,
            am_on: lm.am_on,
            lit: [[0; 256]; MAX_CTX],
            am: [0; 256],
            ll: [0; NLL],
            ml: [0; NML],
            of: [0; NOF],
        };
        for (c, f) in lm.tables.iter().enumerate() {
            fill(&mut p.lit[c], f);
        }
        fill(&mut p.am, &lm.am);
        fill(&mut p.ll, t[0]);
        fill(&mut p.ml, t[1]);
        fill(&mut p.of, t[2]);
        p
    }

    /// First-pass guess: literal prices from the block's bytes, flat-ish code prices.
    fn initial(w: &[u8], base: usize, start: usize, end: usize) -> Prices {
        let mut st = Stats::default();
        for pos in start..end {
            st.add_lit(if pos > 0 { w[pos - 1] } else { 0 }, pos - base, w[pos], None);
        }
        let mut lm = best_lit_model(&st, None);
        lm.am_on = false;
        let mut p = Prices::new(&lm, [&Freqs::EMPTY, &Freqs::EMPTY, &Freqs::EMPTY]);
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

    /// Token block prices (8 bits per byte) plus its cycle costs.
    fn token(opt: &Options) -> Prices {
        let lm = LitModel { mode: 0, am_on: false, tables: vec![Freqs::EMPTY], am: Freqs::EMPTY };
        let mut p = Prices::new(&lm, [&Freqs::EMPTY, &Freqs::EMPTY, &Freqs::EMPTY]);
        p.token = true;
        p.with_cycles(opt, true)
    }

    fn with_cycles(mut self, opt: &Options, token: bool) -> Prices {
        let k = if opt.size_weight.is_finite() && opt.size_weight > 0.0 { f64::from(8u32 << PRICE_SHIFT) / opt.size_weight } else { 0.0 };
        let m = &opt.cycles;
        let (l, s, b) = if token { (m.tok_lit, m.tok_seq, m.tok_mbyte) } else { (m.rans_lit, m.rans_seq, m.rans_mbyte) };
        self.c_lit = (l * k).round() as u32;
        self.c_seq = (s * k).round() as u32;
        self.c_mbyte = (b * k).round() as u32;
        self
    }

    #[inline]
    fn lit_price(&self, prev: u8, pos: usize, b: u8, pred: Option<u8>) -> u32 {
        if self.token {
            return (8 << PRICE_SHIFT) + self.c_lit;
        }
        self.c_lit
            + match pred {
                Some(p) if self.am_on => self.am[(b ^ p) as usize],
                _ => self.lit[lit_ctx(self.lit_mode, prev, pos)][b as usize],
            }
    }
    #[inline]
    fn ll_price(&self, v: u32) -> u32 {
        if self.token {
            return if v < 15 { 0 } else { (8 << PRICE_SHIFT) * (1 + (v - 15) / 255) };
        }
        let (c, nb, _) = codes::encode(v, LL_DLOG);
        self.ll[c as usize] + (nb << PRICE_SHIFT)
    }
    /// Match length price, including the per-sequence and per-byte decode costs (and the
    /// token byte in token blocks).
    #[inline]
    fn ml_price(&self, len: u32) -> u32 {
        let cyc = self.c_seq + self.c_mbyte * len;
        if self.token {
            let m = len - 2;
            return cyc + (8 << PRICE_SHIFT) + if m < 15 { 0 } else { (8 << PRICE_SHIFT) * (1 + (m - 15) / 255) };
        }
        let (c, nb, _) = codes::encode(len - 2, ML_DLOG);
        cyc + self.ml[c as usize] + (nb << PRICE_SHIFT)
    }
    #[inline]
    fn dist_price(&self, dist: u32) -> u32 {
        if self.token {
            return (8 << PRICE_SHIFT) * leb_len(dist + 2);
        }
        let (c, nb, _) = codes::encode(dist - 1, OF_DLOG);
        self.of[(c + REP_CODES) as usize] + (nb << PRICE_SHIFT)
    }
    #[inline]
    fn rep_price(&self, r: u32) -> u32 {
        if self.token {
            8 << PRICE_SHIFT
        } else {
            self.of[r as usize]
        }
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

/// Predicted byte for a first-after-match literal at window position `pos`.
#[inline]
fn pred_at(w: &[u8], pos: usize, rep0: u32) -> Option<u8> {
    (rep0 as usize <= pos && rep0 > 0).then(|| w[pos - rep0 as usize])
}

/// Parses `w[start..end]` (window coordinates) given the matches at each position.
/// `am0`: the position before `start` ended a match. Returns the sequences, final reps
/// and whether the block ends right after a match.
#[allow(clippy::too_many_arguments)]
fn parse_block(
    w: &[u8],
    base: usize,
    start: usize,
    end: usize,
    mstart: &[u32],
    matches: &[(u32, u32)],
    reps0: [u32; 3],
    am0: bool,
    p: &Prices,
    opt: &Options,
) -> (Vec<Seq>, [u32; 3], bool) {
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
            let prev = if pos > 0 { w[pos - 1] } else { 0 };
            let after_match = nd.litrun == 0 && (i > 0 || am0);
            let pred = if after_match { pred_at(w, pos, nd.reps[0]) } else { None };
            let c = i64::from(nd.cost) + i64::from(p.lit_price(prev, pos - base, w[pos], pred)) + i64::from(p.ll_price(nd.litrun + 1))
                - i64::from(p.ll_price(nd.litrun));
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
            let base = nd.cost + p.rep_price(r) + ll0;
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
    let ends_with_match = path.last().is_some_and(|p| p.0 > 0);
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
    (seqs, nodes[n].reps, if n == 0 { am0 } else { ends_with_match })
}

struct BlockSyms {
    lits: Vec<u8>,
    /// previous output byte of each literal
    prevs: Vec<u8>,
    /// predicted byte (rep0) of first-after-match literals
    preds: Vec<Option<u8>>,
    /// output position of each literal
    poss: Vec<usize>,
    ll: Vec<u8>,
    ml: Vec<u8>,
    of: Vec<u8>,
    bits: BitWriter,
    stats: Stats,
}

fn block_syms(w: &[u8], base: usize, start: usize, end: usize, seqs: &[Seq], reps0: [u32; 3], am0: bool) -> BlockSyms {
    let mut s = BlockSyms {
        lits: Vec::new(),
        prevs: Vec::new(),
        preds: Vec::new(),
        poss: Vec::new(),
        ll: Vec::new(),
        ml: Vec::new(),
        of: Vec::new(),
        bits: BitWriter::default(),
        stats: Stats::default(),
    };
    let mut pos = start;
    let mut reps = reps0;
    let mut after_match = am0;
    let push_lits = |s: &mut BlockSyms, from: usize, to: usize, after_match: bool, rep0: u32| {
        for p in from..to {
            s.lits.push(w[p]);
            s.prevs.push(if p > 0 { w[p - 1] } else { 0 });
            s.preds.push(if p == from && after_match { pred_at(w, p, rep0) } else { None });
            s.poss.push(p - base);
        }
    };
    for q in seqs {
        push_lits(&mut s, pos, pos + q.ll as usize, after_match, reps[0]);
        pos += q.ll as usize + q.ml as usize + 2;
        reps = next_reps(reps, q.ofv);
        after_match = true;
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
    push_lits(&mut s, pos, end, after_match, reps[0]);
    for i in 0..s.lits.len() {
        s.stats.add_lit(s.prevs[i], s.poss[i], s.lits[i], s.preds[i]);
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

fn leb_len(v: u32) -> u32 {
    match v {
        0..=0x7f => 1,
        0x80..=0x3fff => 2,
        0x4000..=0x1f_ffff => 3,
        0x20_0000..=0x0fff_ffff => 4,
        _ => 5,
    }
}

fn put_ext(o: &mut Vec<u8>, mut v: usize) {
    // v >= 15 already signalled by the nibble; store v - 15 as 255-continued bytes
    v -= 15;
    while v >= 255 {
        o.push(255);
        v -= 255;
    }
    o.push(v as u8);
}

/// Token block payload for `seqs` over `w[start..end]`.
fn token_payload(w: &[u8], start: usize, end: usize, seqs: &[Seq]) -> Vec<u8> {
    let mut o = Vec::with_capacity((end - start) / 2 + 16);
    let mut pos = start;
    for q in seqs {
        let (ll, ml) = (q.ll as usize, q.ml as usize);
        o.push(((ll.min(15) as u8) << 4) | ml.min(15) as u8);
        if ll >= 15 {
            put_ext(&mut o, ll);
        }
        o.extend_from_slice(&w[pos..pos + ll]);
        if ml >= 15 {
            put_ext(&mut o, ml);
        }
        if q.ofv < REP_CODES {
            o.push(q.ofv as u8);
        } else {
            let mut v = q.ofv; // distance + 2
            loop {
                let b = (v & 0x7f) as u8;
                v >>= 7;
                if v == 0 {
                    o.push(b);
                    break;
                }
                o.push(b | 0x80);
            }
        }
        pos += ll + ml + 2;
    }
    o.extend_from_slice(&w[pos..end]);
    o
}

/// An encoded rANS block candidate: bytes, final reps, ends-after-match, estimated cycles, stats.
type RansBlock = (Vec<u8>, [u32; 3], bool, f64, Stats);

/// (literals, sequences, match bytes) of a parse.
fn parse_counts(seqs: &[Seq], n: usize) -> (f64, f64, f64) {
    let mbytes: usize = seqs.iter().map(|q| q.ml as usize + 2).sum();
    ((n - mbytes) as f64, seqs.len() as f64, mbytes as f64)
}

fn table_bytes(f: &Freqs) -> usize {
    let mut v = Vec::new();
    f.write(&mut v);
    v.len()
}

/// Coded cost in bits of one table-selected stream part (mode byte + table + payload
/// estimate), and the own table when that beats the dictionary's.
fn part_cost(counts: &[u32], dict: Option<&Freqs>) -> (f64, Option<Freqs>) {
    let used = counts.iter().filter(|&&c| c > 0).count();
    match used {
        0 => (8.0, None),
        1 => (16.0, None),
        _ => {
            let own = normalize(counts);
            let own_cost = cost_bits(counts, &own) + 8.0 * table_bytes(&own) as f64 + 8.0;
            let dict_cost = dict.map_or(f64::INFINITY, |d| cost_bits(counts, d) + 8.0);
            if dict_cost <= own_cost {
                (dict_cost, None)
            } else {
                (own_cost, Some(own))
            }
        }
    }
}

/// The table actually used for a part (own, dictionary, or a smoothed own table for pricing).
fn part_table(counts: &[u32], dict: Option<&Freqs>) -> Freqs {
    match part_cost(counts, dict) {
        (_, Some(own)) => own,
        _ => match dict {
            Some(d) if counts.iter().filter(|&&c| c > 0).count() > 1 => *d,
            _ => normalize(&counts.iter().map(|c| c + 1).collect::<Vec<_>>()),
        },
    }
}

/// Picks the cheapest literal model (context mode × matched literals on/off).
fn best_lit_model(st: &Stats, dict: Option<&EncDict>) -> LitModel {
    let mut best = (f64::INFINITY, 0u8, false);
    for am_on in [false, true] {
        for m in 0..NMODES as u8 {
            let src = if am_on { &st.lit_noam } else { &st.lit };
            let mut c = 32.0;
            for (ctx, counts) in src[m as usize].iter().enumerate() {
                c += part_cost(counts, dict.and_then(|d| d.lit_table(m, ctx))).0;
            }
            if am_on {
                c += part_cost(&st.am, dict.and_then(|d| d.am.as_ref())).0;
            }
            if c < best.0 {
                best = (c, m, am_on);
            }
        }
    }
    let (_, mode, am_on) = best;
    let src = if am_on { &st.lit_noam } else { &st.lit };
    let tables = src[mode as usize].iter().enumerate().map(|(ctx, counts)| part_table(counts, dict.and_then(|d| d.lit_table(mode, ctx)))).collect();
    let am = part_table(&st.am, dict.and_then(|d| d.am.as_ref()));
    LitModel { mode, am_on, tables, am }
}

/// Writes one table-selected part header; returns the table to encode with (None = unused).
fn write_part(hdr: &mut Vec<u8>, counts: &[u32], dict: Option<&Freqs>) -> Option<Freqs> {
    let used = counts.iter().filter(|&&x| x > 0).count();
    if used == 0 {
        hdr.push(3);
        return None;
    }
    if used == 1 {
        let sym = counts.iter().position(|&x| x > 0).unwrap();
        hdr.push(2);
        hdr.push(sym as u8);
        let mut f = Freqs::EMPTY;
        f.n = sym as u16 + 1;
        f.f[sym] = PROB_SCALE as u16;
        return Some(f);
    }
    match part_cost(counts, dict) {
        (_, Some(own)) => {
            hdr.push(1);
            own.write(hdr);
            Some(own)
        }
        _ => {
            hdr.push(0);
            Some(*dict.unwrap())
        }
    }
}

/// Writes the literal stream header (mode | am flag, per-context tables, [AM table]) and
/// returns its payload.
fn write_lits(hdr: &mut Vec<u8>, s: &BlockSyms, lm: &LitModel, dict: Option<&EncDict>) -> Vec<u8> {
    let mode = lm.mode;
    hdr.push(mode | if lm.am_on { 0x80 } else { 0 });
    let nc = nctx(mode);
    let src = if lm.am_on { &s.stats.lit_noam } else { &s.stats.lit };
    let mut tables = vec![Freqs::EMPTY; MAX_CTX + 1];
    let mut any = false;
    for ctx in 0..nc {
        if let Some(t) = write_part(hdr, &src[mode as usize][ctx], dict.and_then(|d| d.lit_table(mode, ctx))) {
            tables[ctx] = t;
            any = true;
        }
    }
    if lm.am_on {
        if let Some(t) = write_part(hdr, &s.stats.am, dict.and_then(|d| d.am.as_ref())) {
            tables[MAX_CTX] = t;
            any = true;
        }
    }
    if !any {
        return Vec::new();
    }
    let mut syms = Vec::with_capacity(s.lits.len());
    let mut ctxs = Vec::with_capacity(s.lits.len());
    for i in 0..s.lits.len() {
        match s.preds[i] {
            Some(p) if lm.am_on => {
                syms.push(s.lits[i] ^ p);
                ctxs.push(MAX_CTX as u8);
            }
            _ => {
                syms.push(s.lits[i]);
                ctxs.push(lit_ctx(mode, s.prevs[i], s.poss[i]) as u8);
            }
        }
    }
    let payload = encode_ctx(&syms, &ctxs, &tables);
    hdr.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    payload
}

/// Writes one code stream header + returns its payload.
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
    let (mode, f) = match part_cost(counts, dict) {
        (_, Some(own)) => (1u8, own),
        _ => (0u8, *dict.unwrap()),
    };
    hdr.push(mode);
    if mode == 1 {
        f.write(hdr);
    }
    let payload = rans_encode(syms, &f);
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
    let mut am = false;
    let mut total = Stats::default();
    let dict_codes: [Option<&Freqs>; 3] = match dict {
        Some(d) => [d.codes[0].as_ref(), d.codes[1].as_ref(), d.codes[2].as_ref()],
        None => [None; 3],
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

        let n = end - start;
        // candidate 1: rANS block
        let mut rans: Option<RansBlock> = None;
        if opt.allow_rans {
            let mut prices = match dict {
                Some(d) if dict_codes.iter().all(|t| t.is_some()) && d.lit.iter().all(|t| t.is_some()) => {
                    let lm = LitModel {
                        mode: d.lit_mode,
                        am_on: d.am.is_some(),
                        tables: d.lit.iter().map(|t| t.unwrap()).collect(),
                        am: d.am.unwrap_or(Freqs::EMPTY),
                    };
                    Prices::new(&lm, [dict_codes[0].unwrap(), dict_codes[1].unwrap(), dict_codes[2].unwrap()])
                }
                _ => Prices::initial(&w, base, start, end),
            }
            .with_cycles(opt, false);
            let mut best: Option<(Vec<Seq>, [u32; 3], bool)> = None;
            for pass in 0..opt.passes.max(1) {
                let r = parse_block(&w, base, start, end, &mstart, &matches, reps, am, &prices, opt);
                if pass + 1 < opt.passes {
                    let s = block_syms(&w, base, start, end, &r.0, reps, am);
                    let lm = best_lit_model(&s.stats, dict);
                    let t: Vec<Freqs> = (0..3).map(|k| part_table(&s.stats.counts[k + 1], dict_codes[k])).collect();
                    prices = Prices::new(&lm, [&t[0], &t[1], &t[2]]).with_cycles(opt, false);
                }
                best = Some(r);
            }
            let (seqs, r, am_end) = best.unwrap();
            let s = block_syms(&w, base, start, end, &seqs, reps, am);
            let mut blk = Vec::new();
            blk.extend_from_slice(&(n as u32).to_le_bytes());
            blk.extend_from_slice(&(seqs.len() as u32).to_le_bytes());
            let mut hdr = Vec::new();
            let lm = best_lit_model(&s.stats, dict);
            let payloads = [
                write_lits(&mut hdr, &s, &lm, dict),
                write_stream(&mut hdr, &s.ll, &s.stats.counts[LL], dict_codes[0]),
                write_stream(&mut hdr, &s.ml, &s.stats.counts[ML], dict_codes[1]),
                write_stream(&mut hdr, &s.of, &s.stats.counts[OF], dict_codes[2]),
            ];
            let extra = s.bits.finish();
            blk.extend_from_slice(&hdr);
            blk.extend_from_slice(&(extra.len() as u32).to_le_bytes());
            for p in &payloads {
                blk.extend_from_slice(p);
            }
            blk.extend_from_slice(&extra);
            let (l, q, b) = parse_counts(&seqs, n);
            let m = &opt.cycles;
            let cyc = l * m.rans_lit + q * m.rans_seq + b * m.rans_mbyte;
            rans = Some((blk, r, am_end, cyc, s.stats));
        }
        // candidate 2: token block
        let mut token: Option<(Vec<u8>, [u32; 3], bool, f64)> = None;
        if opt.allow_token || !opt.allow_rans {
            let prices = Prices::token(opt);
            let (seqs, r, am_end) = parse_block(&w, base, start, end, &mstart, &matches, reps, am, &prices, opt);
            let payload = token_payload(&w, start, end, &seqs);
            let mut blk = Vec::with_capacity(payload.len() + 16);
            blk.extend_from_slice(&(n as u32).to_le_bytes());
            blk.extend_from_slice(&(seqs.len() as u32).to_le_bytes());
            blk.push(crate::dec::TOKEN_BLOCK);
            blk.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            blk.extend_from_slice(&payload);
            let (l, q, b) = parse_counts(&seqs, n);
            let m = &opt.cycles;
            let cyc = l * m.tok_lit + q * m.tok_seq + b * m.tok_mbyte;
            token = Some((blk, r, am_end, cyc));
        }
        let score = |bytes: usize, cyc: f64| if opt.size_weight.is_finite() { cyc + opt.size_weight * bytes as f64 } else { bytes as f64 };
        let use_token = match (&rans, &token) {
            (Some(r), Some(t)) => score(t.0.len(), t.3) < score(r.0.len(), r.3),
            (None, Some(_)) => true,
            _ => false,
        };
        if use_token {
            let (blk, r, am_end, _) = token.unwrap();
            out.extend_from_slice(&blk);
            reps = r;
            am = am_end;
        } else {
            let (blk, r, am_end, _, st) = rans.unwrap();
            out.extend_from_slice(&blk);
            reps = r;
            am = am_end;
            total.add(&st);
        }
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

    /// Struct-array-like data where matched literals pay off.
    fn records(n: usize) -> Vec<u8> {
        let mut v = Vec::new();
        let mut s = 1u32;
        let mut i = 0u32;
        while v.len() < n {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            v.extend_from_slice(&(i * 3).to_be_bytes());
            v.extend_from_slice(&[(s >> 24) as u8 & 7, 0, 1, (s >> 16) as u8]);
            i += 1;
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
            let r = records(30_000);
            rt(&r, &[], None, &opt);
            let zeros = vec![0u8; 300_000];
            assert!(rt(&zeros, &[], None, &opt) < 400);
        }
    }

    #[test]
    fn round_trip_token_blocks() {
        let tok = Options { allow_rans: false, ..Options::speed(LOAD_TIME_WEIGHT) };
        let mixed = Options { block_size: 3000, ..Options::speed(60.0) };
        for opt in [tok, mixed, Options::speed(LOAD_TIME_WEIGHT)] {
            rt(&[], &[], None, &opt);
            rt(b"ab", &[], None, &opt);
            rt(&sample(40_000), &[], None, &opt);
            rt(&records(40_000), &[Segment::planes(4, 40_000)], None, &opt);
            rt(&vec![7u8; 100_000], &[], None, &opt);
            // long literal runs and long matches (extension bytes)
            let mut v: Vec<u8> = (0..5000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
            v.extend_from_within(..4000);
            rt(&v, &[], None, &opt);
        }
        // load-time profile decodes cheaper than the size profile on struct data
        let r = records(60_000);
        let small = compress(&r, &[], None, &Options::level(9)).len();
        let fast = compress(&r, &[], None, &Options::speed(LOAD_TIME_WEIGHT)).len();
        assert!(fast >= small);
    }

    #[test]
    fn round_trip_small_blocks_and_transforms() {
        let s = sample(50_000);
        let opt = Options { block_size: 4096, ..Options::level(9) };
        rt(&s, &[], None, &opt);
        rt(&records(50_000), &[], None, &opt);
        rt(&s, &[Segment::raw(10), Segment::delta(12, 4, true, 30_000), Segment::planes(5, 19_990)], None, &opt);
    }

    #[test]
    fn round_trip_with_dictionary() {
        let s = sample(20_000);
        let mut st = Stats::default();
        let content = s[..8000].to_vec();
        let d0 = EncDict::content_only(3, content.clone());
        let (_, stats) = compress_stats(&s[8000..], &[], Some(&d0), &Options::level(9));
        st.add(&stats);
        let d = EncDict {
            id: 3,
            content,
            lit_mode: 2,
            lit: st.lit_noam[2].iter().map(|c| Some(normalize(&c.map(|x| x + 1)))).collect(),
            am: Some(normalize(&st.am.map(|x| x + 1))),
            codes: [Some(normalize(&st.counts[1])), Some(normalize(&st.counts[2])), Some(normalize(&st.counts[3]))],
        };
        let plain = rt(&s[10_000..12_000], &[], None, &Options::level(9));
        let with = rt(&s[10_000..12_000], &[], Some(&d), &Options::level(9));
        assert!(with < plain, "dict {with} vs plain {plain}");
        // small blocks spanning dictionary-relative matched literals
        rt(&s[12_000..20_000], &[], Some(&d), &Options { block_size: 1000, ..Options::level(9) });
        // wrong / missing dictionary is rejected, not mis-decoded
        let z = compress(&s[..100], &[], Some(&d), &Options::level(9));
        let mut out = vec![0u8; 100];
        let mut dec = Box::new(Decoder::new());
        assert_eq!(dec.decode(&z, None, &mut out, &mut []), Err(crate::Error::WrongDict(3)));
    }

    #[test]
    fn corrupt_input_never_panics() {
        let s = records(5000);
        let zr = compress(&s, &[Segment::planes(4, 5000)], None, &Options::level(5));
        let zt = compress(&s, &[Segment::planes(4, 5000)], None, &Options { allow_rans: false, ..Options::speed(LOAD_TIME_WEIGHT) });
        let mut out = vec![0u8; s.len()];
        let mut tmp = vec![0u8; s.len()];
        let mut dec = Box::new(Decoder::new());
        let mut x = 99u32;
        for i in 0..4000 {
            let mut c = if i % 2 == 0 { zr.clone() } else { zt.clone() };
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
