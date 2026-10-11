//! The `no_std`, allocation-free F2Z decoder.

use crate::bits::BitReader;
use crate::codes::{self, LL_DLOG, ML_DLOG, OF_DLOG, REP_CODES};
use crate::dict::Dict;
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
/// of the loaded dictionary. ~41 KB; allocate once (static or heap) and reuse.
pub struct Decoder {
    tables: [Table; 4],
    dtables: [Table; 4],
    dpresent: [bool; 4],
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

impl Decoder {
    pub const fn new() -> Decoder {
        Decoder {
            tables: [Table::new(), Table::new(), Table::new(), Table::new()],
            dtables: [Table::new(), Table::new(), Table::new(), Table::new()],
            dpresent: [false; 4],
            dict_id: 0,
        }
    }

    /// Pre-builds the dictionary's default tables (done automatically on first use).
    pub fn load_dict(&mut self, d: &Dict) {
        for k in 0..4 {
            self.dpresent[k] = d.tables[k].is_some();
            if let Some(f) = &d.tables[k] {
                self.dtables[k].build(f);
            }
        }
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
        for _ in 0..nblocks {
            op = self.block(c, has_dict, dict, w, op, &mut reps)?;
        }
        if op != w.len() {
            return Err(Error::Corrupt("output size mismatch"));
        }
        if c.pos != c.buf.len() {
            return Err(Error::Corrupt("trailing bytes"));
        }
        Ok(())
    }

    fn block(&mut self, c: &mut Cursor, has_dict: bool, dict: &[u8], w: &mut [u8], mut op: usize, reps: &mut [u32; 3]) -> Result<usize> {
        let out_len = c.u32()? as usize;
        let nseq = c.u32()?;
        let end = op.checked_add(out_len).filter(|&e| e <= w.len()).ok_or(Error::Corrupt("block overruns output"))?;
        if nseq as usize > out_len {
            return Err(Error::Corrupt("too many sequences"));
        }
        // 0 = dict, 1 = explicit, 2 = single, 3 = unused
        let mut modes = [3u8; 4];
        let mut single = [0u32; 4];
        let mut lens = [0usize; 4];
        for k in 0..4 {
            let m = c.u8()?;
            match m {
                0 => {
                    if !has_dict || !self.dpresent[k] {
                        return Err(Error::Corrupt("default table missing"));
                    }
                }
                1 => {
                    let f = Freqs::read(c)?;
                    self.tables[k].build(&f);
                }
                2 => single[k] = u32::from(c.u8()?),
                3 => {}
                _ => return Err(Error::Corrupt("bad stream mode")),
            }
            if m <= 1 {
                lens[k] = c.u32()? as usize;
            }
            modes[k] = m;
        }
        let extra_len = c.u32()? as usize;
        let mut payload: [&[u8]; 4] = [&[]; 4];
        for k in 0..4 {
            payload[k] = c.take(lens[k])?;
        }
        let extra = c.take(extra_len)?;
        let (tables, dtables) = (&self.tables, &self.dtables);
        let mk = |k: usize| -> Result<Stream> {
            Ok(match modes[k] {
                0 => Stream::Rans(RansDec::new(payload[k])?, &dtables[k]),
                1 => Stream::Rans(RansDec::new(payload[k])?, &tables[k]),
                2 => Stream::Single(single[k]),
                _ => Stream::Unused,
            })
        };
        let (mut lit, mut ll, mut ml, mut of) = (mk(0)?, mk(1)?, mk(2)?, mk(3)?);
        let mut bits = BitReader::new(extra);
        for _ in 0..nseq {
            let (b, nb) = codes::decode(ll.next()?, LL_DLOG).ok_or(Error::Corrupt("bad LL code"))?;
            let nlit = b.wrapping_add(bits.read(nb)) as usize;
            if nlit > end - op {
                return Err(Error::Corrupt("literals overrun block"));
            }
            for x in &mut w[op..op + nlit] {
                *x = lit.next()? as u8;
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
        }
        for x in &mut w[op..end] {
            *x = lit.next()? as u8;
        }
        if !(lit.finished() && ll.finished() && ml.finished() && of.finished() && bits.ok()) {
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
