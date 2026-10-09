//! Minimal pure-Rust streaming FLAC decoder (constant/verbatim/fixed/LPC subframes, all
//! stereo assignments, up to 24-bit). CRCs and the STREAMINFO MD5 are not verified.

use crate::wav::ReadSeek;
use std::io::{self, SeekFrom};

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

struct Bits {
    buf: Vec<u8>,
    pos: usize,
    acc: u64,
    n: u32,
}

impl Bits {
    fn new() -> Bits {
        Bits { buf: Vec::new(), pos: 0, acc: 0, n: 0 }
    }

    fn refill(&mut self, r: &mut dyn ReadSeek) -> io::Result<usize> {
        self.buf.resize(64 * 1024, 0);
        let k = r.read(&mut self.buf)?;
        self.buf.truncate(k);
        self.pos = 0;
        Ok(k)
    }

    fn fill(&mut self, r: &mut dyn ReadSeek, need: u32) -> io::Result<()> {
        while self.n < need {
            if self.pos >= self.buf.len() && self.refill(r)? == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.acc = (self.acc << 8) | self.buf[self.pos] as u64;
            self.pos += 1;
            self.n += 8;
        }
        Ok(())
    }
}

pub struct FlacDecoder {
    r: Box<dyn ReadSeek>,
    b: Bits,
    pub channels: u16,
    pub sample_rate: u32,
    pub total: u64,
    bps: u32,
    audio_start: u64,
    /// Decoded current frame, planar.
    ch: Vec<Vec<i32>>,
    blen: usize,
    bpos: usize,
}

impl FlacDecoder {
    pub fn from_reader(mut r: Box<dyn ReadSeek>) -> io::Result<FlacDecoder> {
        let mut m = [0u8; 4];
        r.read_exact(&mut m)?;
        if &m != b"fLaC" {
            return Err(bad("not a FLAC file"));
        }
        let mut info = None;
        loop {
            let mut h = [0u8; 4];
            r.read_exact(&mut h)?;
            let last = h[0] & 0x80 != 0;
            let ty = h[0] & 0x7f;
            let len = u32::from_be_bytes([0, h[1], h[2], h[3]]) as usize;
            let mut d = vec![0u8; len];
            r.read_exact(&mut d)?;
            if ty == 0 {
                if len < 34 {
                    return Err(bad("short STREAMINFO"));
                }
                let v = u64::from_be_bytes(d[10..18].try_into().unwrap());
                let rate = (v >> 44) as u32;
                let chans = ((v >> 41) & 7) as u16 + 1;
                let bps = ((v >> 36) & 31) as u32 + 1;
                let total = v & 0xF_FFFF_FFFF;
                info = Some((rate, chans, bps, total));
            }
            if last {
                break;
            }
        }
        let (rate, chans, bps, total) = info.ok_or_else(|| bad("no STREAMINFO"))?;
        if !(4..=24).contains(&bps) || rate == 0 {
            return Err(bad("unsupported FLAC format"));
        }
        let audio_start = r.stream_position()?;
        Ok(FlacDecoder {
            r,
            b: Bits::new(),
            channels: chans,
            sample_rate: rate,
            total,
            bps,
            audio_start,
            ch: vec![Vec::new(); chans as usize],
            blen: 0,
            bpos: 0,
        })
    }

    pub fn rewind(&mut self) -> io::Result<()> {
        self.r.seek(SeekFrom::Start(self.audio_start))?;
        self.b = Bits::new();
        self.blen = 0;
        self.bpos = 0;
        Ok(())
    }

    fn bits(&mut self, n: u32) -> io::Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        self.b.fill(&mut *self.r, n)?;
        self.b.n -= n;
        let v = (self.b.acc >> self.b.n) & ((1u64 << n) - 1);
        self.b.acc &= (1u64 << self.b.n) - 1;
        Ok(v as u32)
    }

    fn sbits(&mut self, n: u32) -> io::Result<i32> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.bits(n)? as i32;
        Ok((v << (32 - n)) >> (32 - n))
    }

    fn unary(&mut self) -> io::Result<u32> {
        let mut q = 0;
        while self.bits(1)? == 0 {
            q += 1;
            if q > 1 << 24 {
                return Err(bad("runaway unary"));
            }
        }
        Ok(q)
    }

    fn utf8(&mut self) -> io::Result<()> {
        let first = self.bits(8)?;
        let extra = if first < 0x80 { 0 } else { (first as u8).leading_ones() - 1 };
        for _ in 0..extra {
            self.bits(8)?;
        }
        Ok(())
    }

    /// Decodes the next frame into `self.ch`; returns false at clean end of stream.
    fn next_frame(&mut self) -> io::Result<bool> {
        if self.b.n == 0 && self.b.pos >= self.b.buf.len() && self.b.refill(&mut *self.r)? == 0 {
            return Ok(false);
        }
        if self.bits(14)? != 0x3FFE {
            return Err(bad("lost FLAC sync"));
        }
        self.bits(2)?; // reserved, blocking strategy
        let bs_code = self.bits(4)?;
        let sr_code = self.bits(4)?;
        let assign = self.bits(4)?;
        let ss_code = self.bits(3)?;
        self.bits(1)?;
        self.utf8()?;
        let blocksize = match bs_code {
            1 => 192,
            2..=5 => 576usize << (bs_code - 2),
            6 => self.bits(8)? as usize + 1,
            7 => self.bits(16)? as usize + 1,
            8..=15 => 256usize << (bs_code - 8),
            _ => return Err(bad("bad block size")),
        };
        match sr_code {
            12 => {
                self.bits(8)?;
            }
            13 | 14 => {
                self.bits(16)?;
            }
            _ => {}
        }
        self.bits(8)?; // crc8
        let bps = match ss_code {
            0 => self.bps,
            1 => 8,
            2 => 12,
            4 => 16,
            5 => 20,
            6 => 24,
            _ => return Err(bad("bad sample size")),
        };
        let nch = match assign {
            0..=7 => assign as usize + 1,
            8..=10 => 2,
            _ => return Err(bad("bad channel assignment")),
        };
        if nch != self.channels as usize {
            return Err(bad("channel count changed"));
        }
        for c in 0..nch {
            let extra = match (assign, c) {
                (8, 1) | (10, 1) | (9, 0) => 1,
                _ => 0,
            };
            let mut v = std::mem::take(&mut self.ch[c]);
            let r = self.subframe(&mut v, blocksize, bps + extra);
            self.ch[c] = v;
            r?;
        }
        match assign {
            8 => {
                for i in 0..blocksize {
                    self.ch[1][i] = self.ch[0][i].wrapping_sub(self.ch[1][i]);
                }
            }
            9 => {
                for i in 0..blocksize {
                    self.ch[0][i] = self.ch[0][i].wrapping_add(self.ch[1][i]);
                }
            }
            10 => {
                for i in 0..blocksize {
                    let side = self.ch[1][i];
                    let mid = (self.ch[0][i] << 1) | (side & 1);
                    self.ch[0][i] = mid.wrapping_add(side) >> 1;
                    self.ch[1][i] = mid.wrapping_sub(side) >> 1;
                }
            }
            _ => {}
        }
        // Align to byte, skip crc16.
        let pad = self.b.n % 8;
        self.bits(pad)?;
        self.bits(16)?;
        self.blen = blocksize;
        self.bpos = 0;
        Ok(true)
    }

    fn subframe(&mut self, out: &mut Vec<i32>, n: usize, bps: u32) -> io::Result<()> {
        out.clear();
        out.resize(n, 0);
        self.bits(1)?;
        let ty = self.bits(6)?;
        let mut bps = bps;
        let mut wasted = 0;
        if self.bits(1)? == 1 {
            wasted = self.unary()? + 1;
            bps = bps.saturating_sub(wasted);
        }
        match ty {
            0 => {
                let v = self.sbits(bps)?;
                out.fill(v);
            }
            1 => {
                for s in out.iter_mut() {
                    *s = self.sbits(bps)?;
                }
            }
            8..=15 => {
                let order = (ty & 7) as usize;
                if order > 4 || order > n {
                    return Err(bad("bad fixed order"));
                }
                for i in 0..order {
                    out[i] = self.sbits(bps)?;
                }
                self.residual(out, order)?;
                const COEF: [&[i64]; 5] = [&[], &[1], &[2, -1], &[3, -3, 1], &[4, -6, 4, -1]];
                let co = COEF[order];
                for i in order..n {
                    let mut p = 0i64;
                    for (j, &c) in co.iter().enumerate() {
                        p += c * out[i - 1 - j] as i64;
                    }
                    out[i] = out[i].wrapping_add(p as i32);
                }
            }
            32..=63 => {
                let order = (ty & 31) as usize + 1;
                if order > n {
                    return Err(bad("bad LPC order"));
                }
                for i in 0..order {
                    out[i] = self.sbits(bps)?;
                }
                let prec = self.bits(4)? + 1;
                if prec == 16 {
                    return Err(bad("bad LPC precision"));
                }
                let shift = self.sbits(5)?;
                if shift < 0 {
                    return Err(bad("negative LPC shift"));
                }
                let mut co = [0i64; 32];
                for c in co.iter_mut().take(order) {
                    *c = self.sbits(prec)? as i64;
                }
                self.residual(out, order)?;
                for i in order..n {
                    let mut p = 0i64;
                    for j in 0..order {
                        p += co[j] * out[i - 1 - j] as i64;
                    }
                    out[i] = out[i].wrapping_add((p >> shift) as i32);
                }
            }
            _ => return Err(bad("bad subframe type")),
        }
        if wasted > 0 {
            for s in out.iter_mut() {
                *s <<= wasted;
            }
        }
        Ok(())
    }

    fn residual(&mut self, out: &mut [i32], order: usize) -> io::Result<()> {
        let method = self.bits(2)?;
        if method > 1 {
            return Err(bad("bad residual method"));
        }
        let pbits = if method == 0 { 4 } else { 5 };
        let porder = self.bits(4)?;
        let n = out.len();
        let parts = 1usize << porder;
        if n % parts != 0 || (n >> porder) < order {
            return Err(bad("bad partition order"));
        }
        let psize = n >> porder;
        let mut i = order;
        for p in 0..parts {
            let end = (p + 1) * psize;
            let k = self.bits(pbits)?;
            if k == (1 << pbits) - 1 {
                let w = self.bits(5)?;
                while i < end {
                    out[i] = self.sbits(w)?;
                    i += 1;
                }
            } else {
                while i < end {
                    let q = self.unary()?;
                    let low = self.bits(k)?;
                    let u = (q << k) | low;
                    out[i] = ((u >> 1) as i32) ^ -((u & 1) as i32);
                    i += 1;
                }
            }
        }
        Ok(())
    }

    /// Reads up to `want` frames as interleaved stereo f32. Returns frames read.
    pub fn read_stereo(&mut self, out: &mut [f32], want: usize) -> io::Result<usize> {
        let scale = 1.0 / (1u64 << (self.bps - 1)) as f32;
        let mut done = 0;
        while done < want {
            if self.bpos >= self.blen && !self.next_frame()? {
                break;
            }
            let k = (self.blen - self.bpos).min(want - done);
            for i in 0..k {
                let l = self.ch[0][self.bpos + i] as f32 * scale;
                let r = if self.ch.len() > 1 { self.ch[1][self.bpos + i] as f32 * scale } else { l };
                out[2 * (done + i)] = l;
                out[2 * (done + i) + 1] = r;
            }
            self.bpos += k;
            done += k;
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use crate::wav::WavStream;

    struct W {
        v: Vec<u8>,
        acc: u64,
        n: u32,
    }
    impl W {
        fn put(&mut self, x: u64, bits: u32) {
            self.acc = (self.acc << bits) | (x & ((1u64 << bits) - 1));
            self.n += bits;
            while self.n >= 8 {
                self.n -= 8;
                self.v.push((self.acc >> self.n) as u8);
            }
            self.acc &= (1u64 << self.n) - 1;
        }
    }

    /// Frames of: left verbatim, right constant (independent channels). CRCs are zero.
    fn build(frames: &[(Vec<i16>, i16)]) -> Vec<u8> {
        let total: usize = frames.iter().map(|f| f.0.len()).sum();
        let mut out = b"fLaC".to_vec();
        out.extend_from_slice(&[0x80, 0, 0, 34]);
        let mut si = vec![0u8; 34];
        si[0..2].copy_from_slice(&16u16.to_be_bytes());
        si[2..4].copy_from_slice(&16u16.to_be_bytes());
        let v: u64 = (44100u64 << 44) | (1u64 << 41) | (15u64 << 36) | total as u64;
        si[10..18].copy_from_slice(&v.to_be_bytes());
        out.extend(si);
        for (i, (l, r)) in frames.iter().enumerate() {
            let mut w = W { v: Vec::new(), acc: 0, n: 0 };
            w.put(0x3FFE, 14);
            w.put(0, 2);
            w.put(7, 4); // 16-bit blocksize-1 follows
            w.put(9, 4); // 44.1k
            w.put(1, 4); // 2 ch independent
            w.put(4, 3); // 16 bit
            w.put(0, 1);
            w.put(i as u64, 8);
            w.put(l.len() as u64 - 1, 16);
            w.put(0, 8);
            w.put(0, 1);
            w.put(1, 6); // verbatim
            w.put(0, 1);
            for s in l {
                w.put(*s as u16 as u64, 16);
            }
            w.put(0, 1);
            w.put(0, 6); // constant
            w.put(0, 1);
            w.put(*r as u16 as u64, 16);
            w.put(0, 16);
            out.extend(w.v);
        }
        out
    }

    #[test]
    fn decodes_and_rewinds() {
        let a: Vec<i16> = (0..300).map(|i| (i * 100 - 15000) as i16).collect();
        let b: Vec<i16> = vec![-5, 7, 32767, -32768];
        let data = build(&[(a.clone(), 1234), (b.clone(), -9)]);
        let mut s = WavStream::from_flac_reader(Box::new(std::io::Cursor::new(data))).unwrap();
        assert_eq!((s.channels, s.sample_rate, s.frames), (2, 44100, 304));
        let mut buf = vec![0f32; 2 * 400];
        for _ in 0..2 {
            let mut got = 0;
            while got < 304 {
                let end = 2 * (got + 50).min(400);
                got += s.read_stereo(&mut buf[2 * got..end]).unwrap();
            }
            assert_eq!(s.read_stereo(&mut buf).unwrap(), 0);
            let all: Vec<i16> = a.iter().chain(b.iter()).copied().collect();
            for (i, &x) in all.iter().enumerate() {
                assert_eq!(buf[2 * i], x as f32 / 32768.0);
            }
            assert_eq!(buf[1], 1234.0 / 32768.0);
            assert_eq!(buf[2 * 303 + 1], -9.0 / 32768.0);
            s.rewind().unwrap();
        }
    }

    /// Real-data check: F2_FLAC_CMP="a.flac:b.wav".
    #[test]
    fn real_flac_matches_wav() {
        let Ok(spec) = std::env::var("F2_FLAC_CMP") else { return };
        let (f, w) = spec.split_once(':').unwrap();
        let mut a = WavStream::open_any(std::path::Path::new(f)).unwrap();
        let mut b = WavStream::open_any(std::path::Path::new(w)).unwrap();
        assert_eq!(a.frames, b.frames);
        let (mut x, mut y) = (vec![0f32; 8192], vec![0f32; 8192]);
        let mut total = 0;
        loop {
            let n = a.read_stereo(&mut x).unwrap_or_else(|e| panic!("flac err {e} after {total} frames of {}", a.frames));
            let m = b.read_stereo(&mut y).unwrap();
            assert_eq!(n, m);
            if n == 0 {
                break;
            }
            assert!(x[..2 * n] == y[..2 * n], "mismatch near frame {total}");
            total += n;
        }
        assert_eq!(total as u64, a.frames);
        eprintln!("compared {total} frames");
    }
}
