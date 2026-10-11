//! Size-preserving structure transforms applied before LZ (inverse applied after decode).
//!
//! The data is a concatenation of segments; each segment is transformed independently:
//! * [`Kind::Raw`] — unchanged.
//! * [`Kind::Planes`] — records of `stride` bytes are transposed into `stride` byte planes
//!   (all byte 0s, then all byte 1s, …); a tail shorter than a record stays raw.
//! * [`Kind::DeltaPlanes`] — records are `stride / width` words of `width` (1, 2 or 4)
//!   bytes (big- or little-endian); each word is replaced by the zigzag of its difference
//!   to the same word of the previous record (wrapping), then the planes transform runs.
//!
//! Segment record (8 bytes): `u8 kind, u8 width | 0x80 if big-endian, u16 stride, u32 len`.

use crate::{Cursor, Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Raw = 0,
    Planes = 1,
    DeltaPlanes = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub kind: Kind,
    /// Word width for `DeltaPlanes` (1, 2, 4).
    pub width: u8,
    pub big_endian: bool,
    pub stride: u16,
    pub len: u32,
}

impl Segment {
    pub const fn raw(len: u32) -> Segment {
        Segment { kind: Kind::Raw, width: 1, big_endian: false, stride: 1, len }
    }
    pub const fn planes(stride: u16, len: u32) -> Segment {
        Segment { kind: Kind::Planes, width: 1, big_endian: false, stride, len }
    }
    pub const fn delta(stride: u16, width: u8, big_endian: bool, len: u32) -> Segment {
        Segment { kind: Kind::DeltaPlanes, width, big_endian, stride, len }
    }

    pub(crate) fn read(c: &mut Cursor) -> Result<Segment> {
        let kind = match c.u8()? {
            0 => Kind::Raw,
            1 => Kind::Planes,
            2 => Kind::DeltaPlanes,
            _ => return Err(Error::Corrupt("unknown transform")),
        };
        let wb = c.u8()?;
        let stride = c.u16()?;
        let len = c.u32()?;
        let width = wb & 0x7f;
        let s = Segment { kind, width, big_endian: wb & 0x80 != 0, stride, len };
        s.check()?;
        Ok(s)
    }

    pub fn check(&self) -> Result<()> {
        match self.kind {
            Kind::Raw => Ok(()),
            Kind::Planes if self.stride >= 1 => Ok(()),
            Kind::DeltaPlanes if matches!(self.width, 1 | 2 | 4) && self.stride >= 1 && self.stride % u16::from(self.width) == 0 => Ok(()),
            _ => Err(Error::Corrupt("bad transform parameters")),
        }
    }

    #[cfg(feature = "encoder")]
    pub fn write(&self, out: &mut Vec<u8>) {
        out.push(self.kind as u8);
        out.push(self.width | if self.big_endian { 0x80 } else { 0 });
        out.extend_from_slice(&self.stride.to_le_bytes());
        out.extend_from_slice(&self.len.to_le_bytes());
    }
}

#[inline]
fn read_word(b: &[u8], width: usize, be: bool) -> u32 {
    let mut v = 0u32;
    for i in 0..width {
        let byte = u32::from(b[if be { i } else { width - 1 - i }]);
        v = (v << 8) | byte;
    }
    v
}

#[inline]
fn write_word(b: &mut [u8], width: usize, be: bool, v: u32) {
    for i in 0..width {
        let sh = 8 * (width - 1 - i);
        b[if be { i } else { width - 1 - i }] = (v >> sh) as u8;
    }
}

#[inline]
fn mask(width: usize) -> u32 {
    if width == 4 {
        u32::MAX
    } else {
        (1u32 << (8 * width)) - 1
    }
}

#[inline]
fn unzigzag(v: u32, width: usize) -> u32 {
    let m = mask(width);
    ((v >> 1) ^ (0u32.wrapping_sub(v & 1))) & m
}

/// Records per tile of the inverse planes transform.
const TILE: usize = 16;

/// Inverse delta + zigzag over whole records of `st` bytes (words of `W` bytes, `st % W ==
/// 0`): each word becomes the previous record's word plus the unzigzagged value.
fn undelta<const W: usize, const BE: bool>(o: &mut [u8], st: usize) {
    let m = mask(W);
    let mut r = st;
    while r + st <= o.len() {
        let (prev, cur) = o[r - st..r + st].split_at_mut(st);
        for (p, c) in prev.as_chunks::<W>().0.iter().zip(cur.as_chunks_mut::<W>().0) {
            let v = read_word(p, W, BE).wrapping_add(unzigzag(read_word(c, W, BE), W)) & m;
            write_word(c, W, BE, v);
        }
        r += st;
    }
}

/// Inverse of all segments: `src` (transformed) → `out` (original). Same length.
pub fn inverse(segs: &[Segment], src: &[u8], out: &mut [u8]) -> Result<()> {
    if src.len() != out.len() {
        return Err(Error::OutputTooSmall);
    }
    let mut off = 0usize;
    for s in segs {
        let end = seg_end(s, off, src.len())?;
        let (i, o) = (&src[off..end], &mut out[off..end]);
        match s.kind {
            Kind::Raw => o.copy_from_slice(i),
            Kind::Planes | Kind::DeltaPlanes => inverse_segment(s, i, o),
        }
        off = end;
    }
    if off != src.len() {
        return Err(Error::Corrupt("segments do not cover data"));
    }
    Ok(())
}

/// [`inverse`] in place: `data` (transformed) becomes the original; `tmp` (at least
/// `data.len()` bytes) is scratch for the transformed segments only, so raw segments are
/// not copied at all.
pub fn inverse_in_place(segs: &[Segment], data: &mut [u8], tmp: &mut [u8]) -> Result<()> {
    let tmp = tmp.get_mut(..data.len()).ok_or(Error::OutputTooSmall)?;
    let mut off = 0usize;
    for s in segs {
        let end = seg_end(s, off, data.len())?;
        if s.kind != Kind::Raw {
            let t = &mut tmp[off..end];
            t.copy_from_slice(&data[off..end]);
            inverse_segment(s, t, &mut data[off..end]);
        }
        off = end;
    }
    if off != data.len() {
        return Err(Error::Corrupt("segments do not cover data"));
    }
    Ok(())
}

#[inline]
fn seg_end(s: &Segment, off: usize, total: usize) -> Result<usize> {
    off.checked_add(s.len as usize).filter(|&e| e <= total).ok_or(Error::Corrupt("segments exceed data"))
}

/// Inverse of one `Planes` / `DeltaPlanes` segment (`i` and `o` have the segment's length).
fn inverse_segment(s: &Segment, i: &[u8], o: &mut [u8]) {
    let len = i.len();
    let st = s.stride as usize;
    let nrec = len / st;
    let body = nrec * st;
    // Gather the planes back into records a tile of TILE records at a time: each
    // plane contributes TILE consecutive bytes (one 16-byte VR4300 cache line)
    // and the output tile stays cached, instead of one pass over the whole
    // output per plane (a cache miss per byte once the output exceeds the cache).
    // Strides 1, 2 and 4 (the common ones) interleave whole planes directly.
    match st {
        1 => o[..body].copy_from_slice(&i[..body]),
        2 => {
            let (p0, p1) = (&i[..nrec], &i[nrec..2 * nrec]);
            for ((r, &a), &b) in o[..body].as_chunks_mut::<2>().0.iter_mut().zip(p0).zip(p1) {
                r[0] = a;
                r[1] = b;
            }
        }
        4 => {
            let (p0, p1, p2, p3) = (&i[..nrec], &i[nrec..2 * nrec], &i[2 * nrec..3 * nrec], &i[3 * nrec..4 * nrec]);
            for ((((r, &a), &b), &c), &d) in o[..body].as_chunks_mut::<4>().0.iter_mut().zip(p0).zip(p1).zip(p2).zip(p3) {
                r[0] = a;
                r[1] = b;
                r[2] = c;
                r[3] = d;
            }
        }
        _ => {
            let mut r0 = 0;
            while r0 < nrec {
                let nr = TILE.min(nrec - r0);
                let tile = &mut o[r0 * st..(r0 + nr) * st];
                for p in 0..st {
                    let mut k = p;
                    for &b in &i[p * nrec + r0..p * nrec + r0 + nr] {
                        tile[k] = b;
                        k += st;
                    }
                }
                r0 += nr;
            }
        }
    }
    o[body..].copy_from_slice(&i[body..]);
    if s.kind == Kind::DeltaPlanes && nrec > 1 {
        let body = &mut o[..body];
        match (s.width, s.big_endian) {
            (1, _) => undelta::<1, false>(body, st),
            (2, false) => undelta::<2, false>(body, st),
            (2, true) => undelta::<2, true>(body, st),
            (4, false) => undelta::<4, false>(body, st),
            _ => undelta::<4, true>(body, st),
        }
    }
}

/// Forward transform (encoder). Segments must cover `data` exactly.
#[cfg(feature = "encoder")]
pub fn forward(segs: &[Segment], data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    let mut off = 0usize;
    for s in segs {
        let len = s.len as usize;
        let i = &data[off..off + len];
        let o = &mut out[off..off + len];
        match s.kind {
            Kind::Raw => o.copy_from_slice(i),
            Kind::Planes | Kind::DeltaPlanes => {
                let st = s.stride as usize;
                let nrec = len / st;
                let body = nrec * st;
                let mut rec = i.to_vec();
                if s.kind == Kind::DeltaPlanes {
                    let w = s.width as usize;
                    let m = mask(w);
                    for r in (1..nrec).rev() {
                        for f in (0..st).step_by(w) {
                            let prev = read_word(&i[(r - 1) * st + f..], w, s.big_endian);
                            let cur = read_word(&i[r * st + f..], w, s.big_endian);
                            let d = cur.wrapping_sub(prev) & m;
                            // zigzag within `w` bytes: sign-extend then map.
                            let sh = 32 - 8 * w as u32;
                            let sd = ((d << sh) as i32) >> sh;
                            let z = ((sd << 1) ^ (sd >> 31)) as u32 & m;
                            write_word(&mut rec[r * st + f..], w, s.big_endian, z);
                        }
                    }
                }
                for p in 0..st {
                    for r in 0..nrec {
                        o[p * nrec + r] = rec[r * st + p];
                    }
                }
                o[body..].copy_from_slice(&i[body..]);
            }
        }
        off += len;
    }
    assert_eq!(off, data.len(), "segments must cover the data");
    out
}

#[cfg(all(test, feature = "encoder"))]
mod tests {
    use super::*;

    #[test]
    fn transforms_round_trip() {
        let data: Vec<u8> = (0..10_007u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8 ^ (i as u8)).collect();
        let n = data.len() as u32;
        let cases = [
            vec![Segment::raw(n)],
            vec![Segment::planes(12, n)],
            vec![Segment::delta(12, 4, false, n)],
            vec![Segment::delta(6, 2, true, n)],
            vec![Segment::delta(7, 1, true, n)],
            vec![Segment::raw(100), Segment::delta(8, 4, true, 4000), Segment::planes(3, n - 4100)],
        ];
        for segs in cases {
            let t = forward(&segs, &data);
            let mut back = vec![0u8; data.len()];
            inverse(&segs, &t, &mut back).unwrap();
            assert_eq!(back, data, "{segs:?}");
            let mut in_place = t.clone();
            inverse_in_place(&segs, &mut in_place, &mut vec![0u8; t.len()]).unwrap();
            assert_eq!(in_place, data, "in place {segs:?}");
        }
    }

    #[test]
    fn delta_makes_ramps_small() {
        let mut d = Vec::new();
        for i in 0..100u32 {
            d.extend_from_slice(&(1000 + 3 * i).to_be_bytes());
        }
        let t = forward(&[Segment::delta(4, 4, true, 400)], &d);
        // planes 0..2 (high bytes) of the deltas are zero after the first record
        assert!(t[1..300].iter().filter(|&&b| b != 0).count() <= 3);
    }
}
