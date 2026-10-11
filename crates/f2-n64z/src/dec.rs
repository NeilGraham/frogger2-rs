//! The `no_std`, allocation-free F2Z decoder.

use crate::bits::BitReader;
use crate::codes::{self, LL_DLOG, ML_DLOG, OF_DLOG, REP_CODES};
use crate::dict::Dict;
use crate::litctx::{nctx, MAX_CTX, NMODES, PMASK, PREV, PSHIFT};
use crate::rans::{Freqs, RansDec, Table};
use crate::transform::{self, Segment};
use crate::{Cursor, Error, Result, INIT_REPS, MAGIC};

/// Maximum transform segments per stream.
pub const MAX_SEGMENTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub raw_size: u32,
    /// 0 = no dictionary.
    pub dict_id: u8,
    pub nseg: u8,
}

impl Header {
    /// A scratch buffer of `raw_size` bytes is needed when transforms are present.
    pub fn needs_scratch(&self) -> bool {
        self.nseg > 0
    }
}

/// Parses the fixed header.
pub fn header(src: &[u8]) -> Result<Header> {
    let mut c = Cursor::new(src);
    if c.take(4).map_err(|_| Error::BadMagic)? != MAGIC {
        return Err(Error::BadMagic);
    }
    let raw_size = c.u32()?;
    let dict_id = c.u8()?;
    let nseg = c.u8()?;
    if nseg as usize > MAX_SEGMENTS {
        return Err(Error::Corrupt("too many segments"));
    }
    Ok(Header { raw_size, dict_id, nseg })
}

/// Decoder state: working tables for the current block plus the pre-built default tables
/// of the loaded dictionary (24 tables of 1.3 KiB ≈ 31 KB). Allocate once and reuse.
pub struct Decoder {
    /// Literal tables per context, plus the matched-literal table at index `MAX_CTX`.
    lit: [Table; MAX_CTX + 1],
    codes: [Table; 3],
    dlit: [Table; MAX_CTX + 1],
    dcodes: [Table; 3],
    dlit_present: [bool; MAX_CTX + 1],
    dcodes_present: [bool; 3],
    dlit_mode: u8,
    dict_id: u8,
}

/// Literal decoding parameters of one block: the table per context, the matched-literal
/// table and the context function `cls[prev] | (pos & pmask) << pshift` (see `litctx`).
struct Lits<'t> {
    tabs: [&'t Table; MAX_CTX],
    am: &'t Table,
    cls: &'static [u8; 256],
    pmask: usize,
    pshift: u32,
}

impl Lits<'_> {
    /// Decodes `n` ≥ 1 literals at `w[op..]`; `pred` is the matched-literal prediction
    /// (the first literal is coded XOR `pred` with the `am` table) when it applies.
    #[inline(always)]
    fn decode(&self, d: &mut RansDec, w: &mut [u8], op: usize, n: usize, pred: Option<u8>, dict_last: u8) {
        let mut prev = if op > 0 { w[op - 1] } else { dict_last };
        let out = &mut w[op..op + n];
        let mut start = 0;
        if let Some(p) = pred {
            let b = (d.get(self.am) as u8) ^ p;
            out[0] = b;
            prev = b;
            start = 1;
        }
        for (pos, x) in (op + start..).zip(out[start..].iter_mut()) {
            // contexts are < nctx ≤ MAX_CTX, so the mask never changes the index
            let c = usize::from(self.cls[usize::from(prev)]) | ((pos & self.pmask) << self.pshift);
            let b = d.get(self.tabs[c & (MAX_CTX - 1)]) as u8;
            *x = b;
            prev = b;
        }
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder::new()
    }
}

const T: Table = Table::new();

impl Decoder {
    pub const fn new() -> Decoder {
        Decoder {
            lit: [T; MAX_CTX + 1],
            codes: [T; 3],
            dlit: [T; MAX_CTX + 1],
            dcodes: [T; 3],
            dlit_present: [false; MAX_CTX + 1],
            dcodes_present: [false; 3],
            dlit_mode: 0,
            dict_id: 0,
        }
    }

    /// Pre-builds the dictionary's default tables (done automatically on first use).
    pub fn load_dict(&mut self, d: &Dict) {
        for c in 0..MAX_CTX {
            self.dlit_present[c] = d.lit[c].is_some();
            if let Some(f) = &d.lit[c] {
                self.dlit[c].build(f);
            }
        }
        self.dlit_present[MAX_CTX] = d.am.is_some();
        if let Some(f) = &d.am {
            self.dlit[MAX_CTX].build(f);
        }
        for k in 0..3 {
            self.dcodes_present[k] = d.codes[k].is_some();
            if let Some(f) = &d.codes[k] {
                self.dcodes[k].build(f);
            }
        }
        self.dlit_mode = d.lit_mode;
        self.dict_id = d.id;
    }

    /// Decodes `src` into `out` (`out.len()` must equal the header's `raw_size`). `tmp`
    /// must also hold `raw_size` bytes when [`Header::needs_scratch`], else may be empty.
    pub fn decode(&mut self, src: &[u8], dict: Option<&Dict>, out: &mut [u8], tmp: &mut [u8]) -> Result<()> {
        let h = header(src)?;
        let n = h.raw_size as usize;
        if out.len() != n {
            return Err(Error::OutputTooSmall);
        }
        let dict_content: &[u8] = match (h.dict_id, dict) {
            (0, _) => &[],
            (id, Some(d)) if d.id == id => {
                if self.dict_id != id {
                    self.load_dict(d);
                }
                d.content
            }
            (id, _) => return Err(Error::WrongDict(id)),
        };
        let mut c = Cursor::new(src);
        c.pos = 12;
        let mut segs = [Segment::raw(0); MAX_SEGMENTS];
        for s in segs.iter_mut().take(h.nseg as usize) {
            *s = Segment::read(&mut c)?;
        }
        if h.nseg == 0 {
            self.lz(&mut c, h.dict_id != 0, dict_content, out)?;
        } else {
            let segs = &segs[..h.nseg as usize];
            let t = tmp.get_mut(..n).ok_or(Error::OutputTooSmall)?;
            let raw: u64 = segs.iter().filter(|s| s.kind == transform::Kind::Raw).map(|s| u64::from(s.len)).sum();
            if raw * 2 >= n as u64 {
                // Mostly raw: LZ into `out`, then invert the transformed segments in place
                // (raw segments are not copied; `tmp` holds the transformed ones).
                self.lz(&mut c, h.dict_id != 0, dict_content, out)?;
                transform::inverse_in_place(segs, out, t)?;
            } else {
                self.lz(&mut c, h.dict_id != 0, dict_content, t)?;
                transform::inverse(segs, t, out)?;
            }
        }
        Ok(())
    }

    fn lz(&mut self, c: &mut Cursor, has_dict: bool, dict: &[u8], w: &mut [u8]) -> Result<()> {
        let nblocks = c.u32()?;
        let mut op = 0usize;
        let mut reps = INIT_REPS;
        let mut after_match = false;
        for _ in 0..nblocks {
            op = self.block(c, has_dict, dict, w, op, &mut reps, &mut after_match)?;
        }
        if op != w.len() {
            return Err(Error::Corrupt("output size mismatch"));
        }
        if c.pos != c.buf.len() {
            return Err(Error::Corrupt("trailing bytes"));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn block(
        &mut self,
        c: &mut Cursor,
        has_dict: bool,
        dict: &[u8],
        w: &mut [u8],
        mut op: usize,
        reps: &mut [u32; 3],
        after_match: &mut bool,
    ) -> Result<usize> {
        let out_len = c.u32()? as usize;
        let nseq = c.u32()?;
        let end = op.checked_add(out_len).filter(|&e| e <= w.len()).ok_or(Error::Corrupt("block overruns output"))?;
        if nseq as usize > out_len {
            return Err(Error::Corrupt("too many sequences"));
        }
        // literal stream: mode, per-context table modes, one payload
        let lbyte = c.u8()?;
        let (lmode, am_on) = (lbyte & 0x7f, lbyte & 0x80 != 0);
        if lmode as usize >= NMODES {
            return Err(Error::Corrupt("bad literal mode"));
        }
        let nc = nctx(lmode);
        let mut use_dict = [false; MAX_CTX + 1];
        let mut any_lit = false;
        let parts: &[usize] = &[0, 1, 2, 3, 4, 5, 6, 7, MAX_CTX];
        for &ctx in parts.iter().filter(|&&p| p < nc || (p == MAX_CTX && am_on)) {
            let ud = &mut use_dict[ctx];
            match c.u8()? {
                0 => {
                    let mode_ok = ctx == MAX_CTX || self.dlit_mode == lmode;
                    if !has_dict || !mode_ok || !self.dlit_present[ctx] {
                        return Err(Error::Corrupt("default literal table missing"));
                    }
                    *ud = true;
                    any_lit = true;
                }
                1 => {
                    let f = Freqs::read(c)?;
                    self.lit[ctx].build(&f);
                    any_lit = true;
                }
                2 => {
                    self.lit[ctx].build_single(c.u8()?);
                    any_lit = true;
                }
                3 => self.lit[ctx].build_single(0),
                _ => return Err(Error::Corrupt("bad stream mode")),
            }
        }
        let lit_len = if any_lit { c.u32()? as usize } else { 0 };
        // LL, ML, OF
        let mut modes = [3u8; 3];
        let mut sym = [0u32; 3];
        let mut lens = [0usize; 3];
        for k in 0..3 {
            let m = c.u8()?;
            match m {
                0 => {
                    if !has_dict || !self.dcodes_present[k] {
                        return Err(Error::Corrupt("default table missing"));
                    }
                }
                1 => {
                    let f = Freqs::read(c)?;
                    self.codes[k].build(&f);
                }
                2 => sym[k] = u32::from(c.u8()?),
                3 => {}
                _ => return Err(Error::Corrupt("bad stream mode")),
            }
            if m <= 1 {
                lens[k] = c.u32()? as usize;
            }
            modes[k] = m;
        }
        let extra_len = c.u32()? as usize;
        let lit_payload = c.take(lit_len)?;
        let mut payload: [&[u8]; 3] = [&[]; 3];
        for k in 0..3 {
            payload[k] = c.take(lens[k])?;
        }
        let extra = c.take(extra_len)?;
        // Every sequence reads LL, ML and OF: an unused stream is only valid without sequences.
        if nseq > 0 && modes.contains(&3) {
            return Err(Error::Corrupt("read from unused stream"));
        }
        // Single-symbol streams decode through a degenerate table with a constant state, so
        // all three streams share one branch-free path.
        for k in 0..3 {
            if modes[k] == 2 {
                self.codes[k].build_single(sym[k] as u8);
            }
        }
        let (codes_t, dcodes_t) = (&self.codes, &self.dcodes);
        let mk = |k: usize| -> Result<(RansDec, &Table)> {
            Ok(match modes[k] {
                0 => (RansDec::new(payload[k])?, &dcodes_t[k]),
                1 => (RansDec::new(payload[k])?, &codes_t[k]),
                _ => (RansDec::constant(), &codes_t[k]),
            })
        };
        let (mut ll, llt) = mk(0)?;
        let (mut ml, mlt) = mk(1)?;
        let (mut of, oft) = mk(2)?;
        let pick = |ctx: usize| if use_dict[ctx] { &self.dlit[ctx] } else { &self.lit[ctx] };
        let mut tabs = [&self.lit[0]; MAX_CTX];
        for (ctx, t) in tabs.iter_mut().enumerate().take(nc) {
            *t = pick(ctx);
        }
        let lm = usize::from(lmode);
        let lt = Lits { tabs, am: pick(MAX_CTX), cls: &PREV[lm], pmask: PMASK[lm] as usize, pshift: PSHIFT[lm] };
        let mut lit = if any_lit { Some(RansDec::new(lit_payload)?) } else { None };
        let dict_last = dict.last().copied().unwrap_or(0);
        // The matched-literal prediction for literals that directly follow a match (rep0).
        #[inline(always)]
        fn predict(w: &[u8], dict: &[u8], op: usize, rep0: usize) -> Option<u8> {
            if rep0 <= op {
                Some(w[op - rep0])
            } else if rep0 - op <= dict.len() {
                Some(dict[dict.len() - (rep0 - op)])
            } else {
                None
            }
        }
        let pred = |w: &[u8], op: usize, am: bool, rep0: usize| if am && am_on { predict(w, dict, op, rep0) } else { None };
        let mut bits = BitReader::new(extra);
        let mut r = *reps;
        let mut am = *after_match;
        for _ in 0..nseq {
            let (b, nb) = codes::decode(ll.get(llt), LL_DLOG).ok_or(Error::Corrupt("bad LL code"))?;
            let nlit = b.wrapping_add(bits.read(nb)) as usize;
            if nlit > end - op {
                return Err(Error::Corrupt("literals overrun block"));
            }
            if nlit > 0 {
                let d = lit.as_mut().ok_or(Error::Corrupt("literals without a literal stream"))?;
                lt.decode(d, w, op, nlit, pred(w, op, am, r[0] as usize), dict_last);
            }
            op += nlit;
            let (b, nb) = codes::decode(ml.get(mlt), ML_DLOG).ok_or(Error::Corrupt("bad ML code"))?;
            let mlen = (b.wrapping_add(bits.read(nb)) as usize).wrapping_add(2);
            let oc = of.get(oft);
            let d = if oc < REP_CODES {
                let d = r[oc as usize];
                match oc {
                    1 => r.swap(0, 1),
                    2 => r = [r[2], r[0], r[1]],
                    _ => {}
                }
                d
            } else {
                let (b, nb) = codes::decode(oc - REP_CODES, OF_DLOG).ok_or(Error::Corrupt("bad OF code"))?;
                let d = b.wrapping_add(bits.read(nb)).wrapping_add(1);
                r = [d, r[0], r[1]];
                d
            } as usize;
            if mlen > end - op {
                return Err(Error::Corrupt("match overruns block"));
            }
            op = copy_match(w, dict, op, d, mlen)?;
            am = true;
        }
        if end > op {
            let d = lit.as_mut().ok_or(Error::Corrupt("literals without a literal stream"))?;
            lt.decode(d, w, op, end - op, pred(w, op, am, r[0] as usize), dict_last);
            am = false;
        }
        *reps = r;
        *after_match = am;
        let lit_done = lit.as_ref().is_none_or(|d| d.finished());
        if !(lit_done && ll.finished() && ml.finished() && of.finished() && bits.ok()) {
            return Err(Error::Corrupt("stream not fully consumed"));
        }
        Ok(end)
    }
}

#[inline(always)]
fn copy_match(w: &mut [u8], dict: &[u8], op: usize, d: usize, len: usize) -> Result<usize> {
    // Fast path: source inside the output and 8 bytes of room after the match: copy 8-byte
    // chunks. A chunk may write up to 7 bytes past the match; those positions lie inside
    // `w` and are always rewritten later (output is produced in order and never read ahead
    // of `op`). A distance d < 8 (overlapping, period d) is first extended bytewise to
    // dd = the smallest multiple of d ≥ 8; the output then also repeats with period dd.
    if d != 0 && d <= op && w.len() - op >= len + 8 {
        let mut i = 0;
        let mut dd = d;
        if d < 8 {
            while dd < 8 {
                dd += d;
            }
            let first = len.min(dd);
            while i < first {
                w[op + i] = w[op + i - d];
                i += 1;
            }
        }
        while i < len {
            let s = op + i - dd;
            let mut chunk = [0u8; 8];
            chunk.copy_from_slice(&w[s..s + 8]);
            w[op + i..op + i + 8].copy_from_slice(&chunk);
            i += 8;
        }
        return Ok(op + len);
    }
    copy_match_slow(w, dict, op, d, len)
}

#[inline(never)]
fn copy_match_slow(w: &mut [u8], dict: &[u8], mut op: usize, d: usize, mut len: usize) -> Result<usize> {
    if d == 0 {
        return Err(Error::Corrupt("zero distance"));
    }
    if d > op {
        let back = d - op;
        if back > dict.len() {
            return Err(Error::Corrupt("distance beyond dictionary"));
        }
        let start = dict.len() - back;
        let n = back.min(len);
        w[op..op + n].copy_from_slice(&dict[start..start + n]);
        op += n;
        len -= n;
        if len == 0 {
            return Ok(op);
        }
    }
    if d >= len {
        w.copy_within(op - d..op - d + len, op);
    } else if d == 1 {
        let b = w[op - 1];
        w[op..op + len].fill(b);
    } else {
        // overlapping: byte by byte (the pattern repeats with period d)
        let s = &mut w[op - d..op + len];
        for i in d..s.len() {
            s[i] = s[i - d];
        }
    }
    Ok(op + len)
}
