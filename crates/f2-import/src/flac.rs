//! Minimal pure-Rust FLAC encoder for 16-bit 44.1 kHz stereo CD audio.
//!
//! Streaming: construct with the total sample count (known from the disc table of contents),
//! feed raw little-endian interleaved PCM in arbitrary chunks, then [`FlacWriter::finish`].
//! Uses fixed predictors (order 0-4), partitioned Rice residuals, adaptive stereo decorrelation
//! and verbatim fallback. The STREAMINFO MD5 is left as zero ("unknown"), which every decoder
//! accepts. Decoded samples are bit-exact with the input.

use std::io::{self, Write};

const BLOCK: usize = 4096;
const CHANNELS: usize = 2;
const BPS: u32 = 16;
const RATE: u32 = 44_100;

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn new() -> Self {
        Bits { out: Vec::with_capacity(BLOCK * 4), acc: 0, n: 0 }
    }
    /// Writes the low `bits` (<= 32) bits of `v`.
    fn put(&mut self, v: u64, bits: u32) {
        debug_assert!(bits <= 32);
        if bits == 0 {
            return;
        }
        self.acc = (self.acc << bits) | (v & ((1u64 << bits) - 1));
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
        self.acc &= (1u64 << self.n) - 1;
    }
    fn put_signed(&mut self, v: i32, bits: u32) {
        self.put(v as u32 as u64, bits);
    }
    fn unary(&mut self, mut q: u32) {
        while q >= 32 {
            self.put(0, 32);
            q -= 32;
        }
        self.put(1, q + 1);
    }
    fn align(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put(0, pad);
        }
    }
}

fn crc8(data: &[u8]) -> u8 {
    let mut c = 0u8;
    for &b in data {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    c
}

const fn crc16_table() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut j = 0;
        while j < 8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
            j += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}
static CRC16: [u16; 256] = crc16_table();

fn crc16(data: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in data {
        c = (c << 8) ^ CRC16[((c >> 8) as u8 ^ b) as usize];
    }
    c
}

fn utf8_number(out: &mut Vec<u8>, n: u64) {
    if n < 0x80 {
        out.push(n as u8);
        return;
    }
    let mut bytes = 2;
    while n >= 1u64 << (5 * bytes + 1) {
        bytes += 1;
    }
    let lead = !(0xFFu32 >> bytes) as u8;
    out.push(lead | (n >> (6 * (bytes - 1))) as u8);
    for i in (0..bytes - 1).rev() {
        out.push(0x80 | ((n >> (6 * i)) & 0x3F) as u8);
    }
}

fn zigzag(r: i32) -> u32 {
    ((r << 1) ^ (r >> 31)) as u32
}

/// Fixed-predictor residual of `order` (first `order` samples are warm-up, not included).
fn residual(x: &[i32], order: usize) -> Vec<i32> {
    let mut r = Vec::with_capacity(x.len() - order);
    for i in order..x.len() {
        let v = match order {
            0 => x[i],
            1 => x[i] - x[i - 1],
            2 => x[i] - 2 * x[i - 1] + x[i - 2],
            3 => x[i] - 3 * x[i - 1] + 3 * x[i - 2] - x[i - 3],
            _ => x[i] - 4 * x[i - 1] + 6 * x[i - 2] - 4 * x[i - 3] + x[i - 4],
        };
        r.push(v);
    }
    r
}

/// Cheap cost proxy for each fixed order: sum of |residual| over samples 4.. .
fn order_costs(x: &[i32]) -> [u64; 5] {
    let mut s = [0u64; 5];
    for i in 4..x.len() {
        let (a, b, c, d, e) = (x[i] as i64, x[i - 1] as i64, x[i - 2] as i64, x[i - 3] as i64, x[i - 4] as i64);
        s[0] += a.unsigned_abs();
        s[1] += (a - b).unsigned_abs();
        s[2] += (a - 2 * b + c).unsigned_abs();
        s[3] += (a - 3 * b + 3 * c - d).unsigned_abs();
        s[4] += (a - 4 * b + 6 * c - 4 * d + e).unsigned_abs();
    }
    s
}

fn rice_k(sum: u64, n: usize) -> u32 {
    if n == 0 {
        return 0;
    }
    let mut k = 0u32;
    while k < 14 && ((n as u64) << (k + 1)) <= sum {
        k += 1;
    }
    k
}

fn rice_bits(u: &[u32], k: u32) -> u64 {
    u.iter().map(|&v| (v >> k) as u64 + 1 + k as u64).sum()
}

/// Encoded subframe plan.
struct Plan {
    order: usize,
    res: Vec<i32>,
    pord: u32,
    ks: Vec<u32>,
    bits: u64,
}

fn plan(x: &[i32]) -> Plan {
    let n = x.len();
    let (order, res) = if n >= 8 {
        let c = order_costs(x);
        let best = (0..5).min_by_key(|&o| c[o]).unwrap();
        (best, residual(x, best))
    } else {
        (0, residual(x, 0))
    };
    let u: Vec<u32> = res.iter().map(|&r| zigzag(r)).collect();
    // Choose partition order by estimated size.
    let mut best_p = 0u32;
    let mut best_est = u64::MAX;
    for p in 0..=8u32 {
        if !n.is_multiple_of(1usize << p) || (n >> p) <= order {
            break;
        }
        let parts = 1usize << p;
        let psize = n >> p;
        let mut est = 4 * parts as u64;
        let mut start = 0;
        for i in 0..parts {
            let len = if i == 0 { psize - order } else { psize };
            let sum: u64 = u[start..start + len].iter().map(|&v| v as u64).sum();
            let k = rice_k(sum, len);
            est += len as u64 * (k as u64 + 1) + (sum >> k);
            start += len;
        }
        if est < best_est {
            best_est = est;
            best_p = p;
        }
    }
    // Exact k per partition.
    let parts = 1usize << best_p;
    let psize = n >> best_p;
    let mut ks = Vec::with_capacity(parts);
    let mut total = 0u64;
    let mut start = 0;
    for i in 0..parts {
        let len = if i == 0 { psize - order } else { psize };
        let slice = &u[start..start + len];
        let sum: u64 = slice.iter().map(|&v| v as u64).sum();
        let k0 = rice_k(sum, len);
        let (mut bk, mut bb) = (k0, u64::MAX);
        for k in k0.saturating_sub(1)..=(k0 + 1).min(14) {
            let b = rice_bits(slice, k);
            if b < bb {
                bb = b;
                bk = k;
            }
        }
        ks.push(bk);
        total += bb + 4;
        start += len;
    }
    // header(8) + warmup + method(2) + partition order(4) + partitions
    let bits = 8 + 2 + 4 + total;
    Plan { order, res, pord: best_p, ks, bits }
}

fn write_subframe(b: &mut Bits, x: &[i32], bps: u32) {
    let n = x.len();
    let p = plan(x);
    let order = p.order;
    let fixed_bits = p.bits + order as u64 * bps as u64;
    let verbatim_bits = 8 + n as u64 * bps as u64;
    if fixed_bits >= verbatim_bits {
        b.put(0b0000010, 8); // pad 0, type verbatim (000001), no wasted bits
        for &s in x {
            b.put_signed(s, bps);
        }
        return;
    }
    b.put(((8 + order as u64) << 1) & 0x7E, 8);
    for &s in &x[..order] {
        b.put_signed(s, bps);
    }
    b.put(0, 2); // 4-bit Rice parameters
    b.put(p.pord as u64, 4);
    let parts = 1usize << p.pord;
    let psize = n >> p.pord;
    let mut start = 0;
    for i in 0..parts {
        let len = if i == 0 { psize - order } else { psize };
        let k = p.ks[i];
        b.put(k as u64, 4);
        for &r in &p.res[start..start + len] {
            let u = zigzag(r);
            b.unary(u >> k);
            b.put((u & ((1u32 << k) - 1)) as u64, k);
        }
        start += len;
    }
}

fn est_cost(x: &[i32]) -> u64 {
    if x.len() < 8 {
        return x.len() as u64 * 17;
    }
    let c = order_costs(x);
    *c.iter().min().unwrap()
}

/// Streaming FLAC writer.
pub struct FlacWriter<W: Write> {
    w: W,
    pending: Vec<i16>, // interleaved
    frame_no: u64,
    remaining: u64,
}

impl<W: Write> FlacWriter<W> {
    /// `total_samples` is the number of sample frames (per channel) that will be written.
    pub fn new(mut w: W, total_samples: u64) -> io::Result<Self> {
        let mut h = Vec::with_capacity(42);
        h.extend_from_slice(b"fLaC");
        h.push(0x80); // last metadata block, type STREAMINFO
        h.extend_from_slice(&[0, 0, 34]);
        h.extend_from_slice(&(BLOCK as u16).to_be_bytes());
        h.extend_from_slice(&(BLOCK as u16).to_be_bytes());
        h.extend_from_slice(&[0, 0, 0]); // min frame size unknown
        h.extend_from_slice(&[0, 0, 0]); // max frame size unknown
        let packed: u64 = ((RATE as u64) << 44) | (((CHANNELS - 1) as u64) << 41) | (((BPS - 1) as u64) << 36) | (total_samples & 0xF_FFFF_FFFF);
        h.extend_from_slice(&packed.to_be_bytes());
        h.extend_from_slice(&[0u8; 16]); // MD5 unknown
        w.write_all(&h)?;
        Ok(FlacWriter { w, pending: Vec::new(), frame_no: 0, remaining: total_samples })
    }

    /// Feeds little-endian interleaved 16-bit stereo PCM bytes (any chunking).
    pub fn write_pcm(&mut self, bytes: &[u8]) -> io::Result<()> {
        // Carry odd bytes via pending as raw: keep it simple by requiring whole samples per call
        // after re-chunking through a byte stash.
        let mut i = 0;
        while i + 1 < bytes.len() {
            self.pending.push(i16::from_le_bytes([bytes[i], bytes[i + 1]]));
            i += 2;
        }
        if i < bytes.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "odd PCM chunk length"));
        }
        while self.pending.len() >= BLOCK * CHANNELS {
            let rest = self.pending.split_off(BLOCK * CHANNELS);
            let block = std::mem::replace(&mut self.pending, rest);
            self.frame(&block)?;
        }
        Ok(())
    }

    fn frame(&mut self, il: &[i16]) -> io::Result<()> {
        let n = il.len() / CHANNELS;
        let l: Vec<i32> = il.iter().step_by(2).map(|&v| v as i32).collect();
        let r: Vec<i32> = il.iter().skip(1).step_by(2).map(|&v| v as i32).collect();
        let m: Vec<i32> = l.iter().zip(&r).map(|(a, b)| (a + b) >> 1).collect();
        let s: Vec<i32> = l.iter().zip(&r).map(|(a, b)| a - b).collect();
        let (cl, cr, cm, cs) = (est_cost(&l), est_cost(&r), est_cost(&m), est_cost(&s));
        // (assignment code, channels, bps each)
        let opts = [(1u8, cl + cr), (8, cl + cs), (9, cs + cr), (10, cm + cs)];
        let mode = opts.iter().min_by_key(|o| o.1).unwrap().0;

        let mut hdr = vec![0xFF, 0xF8];
        let (bs_code, extra): (u8, Option<u16>) = if n == BLOCK { (12, None) } else { (7, Some((n - 1) as u16)) };
        hdr.push(bs_code << 4); // sample rate: from STREAMINFO
        hdr.push((mode << 4) | (0b100 << 1)); // 16 bps
        utf8_number(&mut hdr, self.frame_no);
        if let Some(e) = extra {
            hdr.extend_from_slice(&e.to_be_bytes());
        }
        hdr.push(crc8(&hdr));

        let mut b = Bits::new();
        b.out.extend_from_slice(&hdr);
        match mode {
            1 => {
                write_subframe(&mut b, &l, 16);
                write_subframe(&mut b, &r, 16);
            }
            8 => {
                write_subframe(&mut b, &l, 16);
                write_subframe(&mut b, &s, 17);
            }
            9 => {
                write_subframe(&mut b, &s, 17);
                write_subframe(&mut b, &r, 16);
            }
            _ => {
                write_subframe(&mut b, &m, 16);
                write_subframe(&mut b, &s, 17);
            }
        }
        b.align();
        let c = crc16(&b.out);
        b.out.extend_from_slice(&c.to_be_bytes());
        self.w.write_all(&b.out)?;
        self.frame_no += 1;
        self.remaining = self.remaining.saturating_sub(n as u64);
        Ok(())
    }

    /// Flushes the final partial frame and returns the inner writer.
    pub fn finish(mut self) -> io::Result<W> {
        if !self.pending.is_empty() {
            let block = std::mem::take(&mut self.pending);
            self.frame(&block)?;
        }
        if self.remaining != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} samples short of the declared length", self.remaining)));
        }
        self.w.flush()?;
        Ok(self.w)
    }
}

/// Encodes a whole PCM buffer (test/utility helper).
pub fn encode(pcm: &[u8]) -> io::Result<Vec<u8>> {
    let mut f = FlacWriter::new(Vec::new(), (pcm.len() / 4) as u64)?;
    f.write_pcm(pcm)?;
    f.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny reference decoder for round-trip tests (fixed + verbatim subframes only).
    struct BitR<'a> {
        d: &'a [u8],
        pos: usize,
    }
    impl BitR<'_> {
        fn bits(&mut self, n: u32) -> u64 {
            let mut v = 0u64;
            for _ in 0..n {
                let bit = (self.d[self.pos >> 3] >> (7 - (self.pos & 7))) & 1;
                v = (v << 1) | bit as u64;
                self.pos += 1;
            }
            v
        }
        fn signed(&mut self, n: u32) -> i32 {
            let v = self.bits(n) as i64;
            ((v << (64 - n)) >> (64 - n)) as i32
        }
        fn unary(&mut self) -> u32 {
            let mut q = 0;
            while self.bits(1) == 0 {
                q += 1;
            }
            q
        }
    }

    fn decode(f: &[u8]) -> Vec<u8> {
        assert_eq!(&f[..4], b"fLaC");
        let total = u64::from_be_bytes(f[18..26].try_into().unwrap()) & 0xF_FFFF_FFFF;
        let mut pos = 42;
        let mut out = Vec::new();
        let mut got = 0u64;
        while got < total {
            let start = pos;
            assert_eq!(f[pos], 0xFF);
            assert_eq!(f[pos + 1], 0xF8);
            let bs_code = f[pos + 2] >> 4;
            let mode = f[pos + 3] >> 4;
            pos += 4;
            let first = f[pos];
            let extra = if first < 0x80 { 0 } else { first.leading_ones() as usize - 1 };
            pos += 1 + extra;
            let n = if bs_code == 12 {
                4096
            } else {
                assert_eq!(bs_code, 7);
                pos += 2;
                u16::from_be_bytes([f[pos - 2], f[pos - 1]]) as usize + 1
            };
            assert_eq!(f[pos], crc8(&f[start..pos]), "header crc8");
            pos += 1;
            let mut r = BitR { d: f, pos: pos * 8 };
            let mut chans: Vec<Vec<i32>> = Vec::new();
            for c in 0..2 {
                let bps = match (mode, c) {
                    (8, 1) | (10, 1) | (9, 0) => 17,
                    _ => 16,
                };
                assert_eq!(r.bits(1), 0);
                let ty = r.bits(6);
                assert_eq!(r.bits(1), 0);
                let mut x = Vec::with_capacity(n);
                if ty == 1 {
                    for _ in 0..n {
                        x.push(r.signed(bps));
                    }
                } else {
                    let order = (ty - 8) as usize;
                    for _ in 0..order {
                        x.push(r.signed(bps));
                    }
                    assert_eq!(r.bits(2), 0);
                    let po = r.bits(4) as u32;
                    let parts = 1usize << po;
                    for i in 0..parts {
                        let len = if i == 0 { (n >> po) - order } else { n >> po };
                        let k = r.bits(4) as u32;
                        for _ in 0..len {
                            let q = r.unary();
                            let u = (q << k) | r.bits(k) as u32;
                            let res = ((u >> 1) as i32) ^ -((u & 1) as i32);
                            let i = x.len();
                            let v = match order {
                                0 => res,
                                1 => res + x[i - 1],
                                2 => res + 2 * x[i - 1] - x[i - 2],
                                3 => res + 3 * x[i - 1] - 3 * x[i - 2] + x[i - 3],
                                _ => res + 4 * x[i - 1] - 6 * x[i - 2] + 4 * x[i - 3] - x[i - 4],
                            };
                            x.push(v);
                        }
                    }
                }
                chans.push(x);
            }
            let (l, rr): (Vec<i32>, Vec<i32>) = match mode {
                1 => (chans[0].clone(), chans[1].clone()),
                8 => (chans[0].clone(), chans[0].iter().zip(&chans[1]).map(|(a, s)| a - s).collect()),
                9 => (chans[0].iter().zip(&chans[1]).map(|(s, b)| b + s).collect(), chans[1].clone()),
                _ => {
                    let mut l = vec![];
                    let mut rr = vec![];
                    for (m, s) in chans[0].iter().zip(&chans[1]) {
                        let mm = (m << 1) | (s & 1);
                        l.push((mm + s) >> 1);
                        rr.push((mm - s) >> 1);
                    }
                    (l, rr)
                }
            };
            for i in 0..n {
                out.extend_from_slice(&(l[i] as i16).to_le_bytes());
                out.extend_from_slice(&(rr[i] as i16).to_le_bytes());
            }
            let end = r.pos.div_ceil(8);
            assert_eq!(u16::from_be_bytes([f[end], f[end + 1]]), crc16(&f[start..end]), "frame crc16");
            pos = end + 2;
            got += n as u64;
        }
        assert_eq!(pos, f.len());
        out
    }

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn roundtrip_tone_noise_and_silence() {
        let mut pcm = Vec::new();
        let mut seed = 1;
        for i in 0..20_000i32 {
            let tone = ((i as f64 * 0.05).sin() * 12000.0) as i32;
            let l = tone as i16;
            let r = (tone / 2 + (lcg(&mut seed) & 7) as i32) as i16;
            pcm.extend_from_slice(&l.to_le_bytes());
            pcm.extend_from_slice(&r.to_le_bytes());
        }
        for _ in 0..9_000 {
            let a = lcg(&mut seed) as u16 as i16;
            let b = lcg(&mut seed) as u16 as i16;
            pcm.extend_from_slice(&a.to_le_bytes());
            pcm.extend_from_slice(&b.to_le_bytes());
        }
        pcm.extend(std::iter::repeat_n(0u8, 4 * 5000));
        // Extremes.
        for v in [i16::MIN, i16::MAX, i16::MIN, 0, i16::MAX, i16::MAX] {
            pcm.extend_from_slice(&v.to_le_bytes());
            pcm.extend_from_slice(&(!v).to_le_bytes());
        }
        let f = encode(&pcm).unwrap();
        assert!(f.len() < pcm.len(), "no compression: {} vs {}", f.len(), pcm.len());
        assert_eq!(decode(&f), pcm);
    }

    #[test]
    fn chunked_input_matches_whole() {
        let pcm: Vec<u8> = (0..4 * 9_999u32).map(|i| ((i * 7) >> 3) as u8).collect();
        let whole = encode(&pcm).unwrap();
        let mut w = FlacWriter::new(Vec::new(), 9_999).unwrap();
        for c in pcm.chunks(2352) {
            w.write_pcm(c).unwrap();
        }
        assert_eq!(w.finish().unwrap(), whole);
        assert_eq!(decode(&whole), pcm);
    }

    #[test]
    fn short_and_empty() {
        for n in [0usize, 1, 3, 7, 4097] {
            let pcm: Vec<u8> = (0..4 * n).map(|i| (i * 31) as u8).collect();
            let f = encode(&pcm).unwrap();
            assert_eq!(decode(&f), pcm, "n={n}");
        }
    }

    #[test]
    fn utf8_frame_numbers() {
        let mut v = Vec::new();
        utf8_number(&mut v, 0x7F);
        utf8_number(&mut v, 0x80);
        utf8_number(&mut v, 0x800);
        assert_eq!(v, [0x7F, 0xC2, 0x80, 0xE0, 0xA0, 0x80]);
    }
}
