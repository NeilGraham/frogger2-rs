//! The `no_std`, allocation-free F2Z decoder.

use crate::bits::BitReader;
use crate::codes::{self, LL_DLOG, ML_DLOG, OF_DLOG, REP_CODES};
use crate::dict::Dict;
use crate::litctx::{ctx as lit_ctx, nctx, MAX_CTX, NMODES};
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
/// of the loaded dictionary (24 tables of 5 KiB ≈ 123 KB). Allocate once and reuse.
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

enum Stream<'a> {
    Rans(RansDec<'a>, &'a Table),
    Single(u32),
    Unused,
}

impl Stream<'_> {
    #[inline]
    fn next(&mut self) -> Result<u32> {
        match self {
            Stream::Rans(d, t) => Ok(d.get(t)),
            Stream::Single(s) => Ok(*s),
            Stream::Unused => Err(Error::Corrupt("read from unused stream")),
        }
    }
    fn finished(&self) -> bool {
        match self {
            Stream::Rans(d, _) => d.finished(),
            _ => true,
        }
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder::new()
    }
}

const T: Table = Table::new();

/// A degenerate table that always yields `sym` and consumes no state.
fn single(t: &mut Table, sym: u8) {
    let mut f = Freqs::EMPTY;
    f.n = u16::from(sym) + 1;
    f.f[sym as usize] = crate::rans::PROB_SCALE as u16;
    t.build(&f);
}

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
            let t = tmp.get_mut(..n).ok_or(Error::OutputTooSmall)?;
            self.lz(&mut c, h.dict_id != 0, dict_content, t)?;
            transform::inverse(&segs[..h.nseg as usize], t, out)?;
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
                    single(&mut self.lit[ctx], c.u8()?);
                    any_lit = true;
                }
                3 => single(&mut self.lit[ctx], 0),
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
        let (codes_t, dcodes_t) = (&self.codes, &self.dcodes);
        let mk = |k: usize| -> Result<Stream> {
            Ok(match modes[k] {
                0 => Stream::Rans(RansDec::new(payload[k])?, &dcodes_t[k]),
                1 => Stream::Rans(RansDec::new(payload[k])?, &codes_t[k]),
                2 => Stream::Single(sym[k]),
                _ => Stream::Unused,
            })
        };
        let (mut ll, mut ml, mut of) = (mk(0)?, mk(1)?, mk(2)?);
        let mut ltab: [&Table; MAX_CTX + 1] = [&self.lit[0]; MAX_CTX + 1];
        for ctx in (0..nc).chain(core::iter::once(MAX_CTX)) {
            ltab[ctx] = if use_dict[ctx] { &self.dlit[ctx] } else { &self.lit[ctx] };
        }
        let mut lit = if any_lit { Some(RansDec::new(lit_payload)?) } else { None };
        let dict_last = dict.last().copied().unwrap_or(0);
        // `am`: these literals directly follow a match (rep0 predicts the first one)
        let mut lits = |w: &mut [u8], op: usize, n: usize, am: bool, rep0: usize| -> Result<()> {
            let d = lit.as_mut().ok_or(Error::Corrupt("literals without a literal stream"))?;
            let mut prev = if op > 0 { w[op - 1] } else { dict_last };
            let mut start = op;
            if am && am_on {
                let pred = if rep0 <= op {
                    Some(w[op - rep0])
                } else if rep0 - op <= dict.len() {
                    Some(dict[dict.len() - (rep0 - op)])
                } else {
                    None
                };
                if let Some(p) = pred {
                    let b = (d.get(ltab[MAX_CTX]) as u8) ^ p;
                    w[op] = b;
                    prev = b;
                    start += 1;
                }
            }
            for (i, x) in w[start..op + n].iter_mut().enumerate() {
                let b = d.get(ltab[lit_ctx(lmode, prev, start + i)]) as u8;
                *x = b;
                prev = b;
            }
            Ok(())
        };
        let mut bits = BitReader::new(extra);
        for _ in 0..nseq {
            let (b, nb) = codes::decode(ll.next()?, LL_DLOG).ok_or(Error::Corrupt("bad LL code"))?;
            let nlit = b.wrapping_add(bits.read(nb)) as usize;
            if nlit > end - op {
                return Err(Error::Corrupt("literals overrun block"));
            }
            if nlit > 0 {
                lits(w, op, nlit, *after_match, reps[0] as usize)?;
                *after_match = false;
            }
            op += nlit;
            let (b, nb) = codes::decode(ml.next()?, ML_DLOG).ok_or(Error::Corrupt("bad ML code"))?;
            let mlen = (b.wrapping_add(bits.read(nb)) as usize).wrapping_add(2);
            let oc = of.next()?;
            let d = if oc < REP_CODES {
                let d = reps[oc as usize];
                match oc {
                    1 => reps.swap(0, 1),
                    2 => *reps = [reps[2], reps[0], reps[1]],
                    _ => {}
                }
                d
            } else {
                let (b, nb) = codes::decode(oc - REP_CODES, OF_DLOG).ok_or(Error::Corrupt("bad OF code"))?;
                let d = b.wrapping_add(bits.read(nb)).wrapping_add(1);
                *reps = [d, reps[0], reps[1]];
                d
            } as usize;
            if mlen > end - op {
                return Err(Error::Corrupt("match overruns block"));
            }
            op = copy_match(w, dict, op, d, mlen)?;
            *after_match = true;
        }
        if end > op {
            lits(w, op, end - op, *after_match, reps[0] as usize)?;
            *after_match = false;
        }
        let lit_done = lit.as_ref().is_none_or(|d| d.finished());
        if !(lit_done && ll.finished() && ml.finished() && of.finished() && bits.ok()) {
            return Err(Error::Corrupt("stream not fully consumed"));
        }
        Ok(end)
    }
}

#[inline]
fn copy_match(w: &mut [u8], dict: &[u8], mut op: usize, d: usize, mut len: usize) -> Result<usize> {
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
        op += len;
    } else {
        for _ in 0..len {
            w[op] = w[op - d];
            op += 1;
        }
    }
    Ok(op)
}
