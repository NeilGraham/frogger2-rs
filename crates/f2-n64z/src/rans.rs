//! Static rANS: 32-bit state, byte renormalisation, 12-bit probabilities.
//!
//! Decode step (per symbol): `slot = x & 4095; s = sym[slot];
//! x = freq[s] * (x >> 12) + slot - cum[s]; while x < 2^23: x = x << 8 | next_byte`.
//! One 32×32 multiply, three table loads, usually no refill.

use crate::{Cursor, Error, Result};

pub const PROB_BITS: u32 = 12;
pub const PROB_SCALE: u32 = 1 << PROB_BITS;
pub const RANS_L: u32 = 1 << 23;
pub const MAX_SYM: usize = 256;

/// Normalised frequencies (sum = [`PROB_SCALE`]) for up to 256 symbols.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Freqs {
    pub n: u16,
    pub f: [u16; MAX_SYM],
}

impl Freqs {
    pub const EMPTY: Freqs = Freqs { n: 0, f: [0; MAX_SYM] };

    pub fn from_slice(f: &[u16]) -> Result<Freqs> {
        if f.len() > MAX_SYM {
            return Err(Error::Corrupt("too many symbols"));
        }
        let mut o = Freqs::EMPTY;
        o.n = f.len() as u16;
        o.f[..f.len()].copy_from_slice(f);
        o.validate()?;
        Ok(o)
    }

    pub fn validate(&self) -> Result<()> {
        let s: u32 = self.f[..self.n as usize].iter().map(|&x| u32::from(x)).sum();
        if s != PROB_SCALE {
            return Err(Error::Corrupt("frequencies do not sum to 4096"));
        }
        Ok(())
    }

    /// Parses the table encoding (see crate docs).
    pub(crate) fn read(c: &mut Cursor) -> Result<Freqs> {
        let n = c.u16()? as usize;
        if n > MAX_SYM {
            return Err(Error::Corrupt("too many symbols"));
        }
        let mut o = Freqs::EMPTY;
        o.n = n as u16;
        let mut i = 0;
        while i < n {
            let v = c.leb()?;
            if v > PROB_SCALE {
                return Err(Error::Corrupt("frequency too large"));
            }
            o.f[i] = v as u16;
            i += 1;
            if v == 0 {
                let run = c.leb()? as usize;
                if run > n - i {
                    return Err(Error::Corrupt("zero run too long"));
                }
                i += run;
            }
        }
        o.validate()?;
        Ok(o)
    }

    #[cfg(feature = "encoder")]
    pub fn write(&self, out: &mut Vec<u8>) {
        let n = self.n as usize;
        out.extend_from_slice(&(n as u16).to_le_bytes());
        let mut i = 0;
        while i < n {
            leb(out, u32::from(self.f[i]));
            if self.f[i] == 0 {
                let mut run = 0;
                while i + 1 + run < n && self.f[i + 1 + run] == 0 {
                    run += 1;
                }
                leb(out, run as u32);
                i += run;
            }
            i += 1;
        }
    }
}

#[cfg(feature = "encoder")]
pub fn leb(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// Decoding table: slot → symbol, and per symbol `freq << 16 | cum`.
#[derive(Clone)]
pub struct Table {
    pub sym: [u8; PROB_SCALE as usize],
    pub fc: [u32; MAX_SYM],
    pub nsym: u16,
}

impl Table {
    pub const fn new() -> Table {
        Table { sym: [0; PROB_SCALE as usize], fc: [0; MAX_SYM], nsym: 0 }
    }

    pub fn build(&mut self, f: &Freqs) {
        let mut cum = 0u32;
        for s in 0..f.n as usize {
            let fr = u32::from(f.f[s]);
            self.fc[s] = (fr << 16) | cum;
            for slot in cum..cum + fr {
                self.sym[slot as usize] = s as u8;
            }
            cum += fr;
        }
        self.nsym = f.n;
    }
}

impl Default for Table {
    fn default() -> Self {
        Table::new()
    }
}

pub struct RansDec<'a> {
    x: u32,
    buf: &'a [u8],
    pos: usize,
}

impl<'a> RansDec<'a> {
    pub fn new(buf: &'a [u8]) -> Result<Self> {
        if buf.len() < 4 {
            return Err(Error::Corrupt("rANS stream too short"));
        }
        let x = u32::from(buf[0]) | (u32::from(buf[1]) << 8) | (u32::from(buf[2]) << 16) | (u32::from(buf[3]) << 24);
        Ok(RansDec { x, buf, pos: 4 })
    }

    #[inline]
    pub fn get(&mut self, t: &Table) -> u32 {
        let slot = self.x & (PROB_SCALE - 1);
        let s = u32::from(t.sym[slot as usize]);
        let fc = t.fc[s as usize];
        self.x = (fc >> 16).wrapping_mul(self.x >> PROB_BITS).wrapping_add(slot).wrapping_sub(fc & 0xffff);
        while self.x < RANS_L {
            let b = self.buf.get(self.pos).copied().unwrap_or(0);
            self.pos += 1;
            self.x = (self.x << 8) | u32::from(b);
        }
        s
    }

    /// The whole payload was consumed exactly and the state returned to its initial value.
    pub fn finished(&self) -> bool {
        self.pos == self.buf.len() && self.x == RANS_L
    }
}

#[cfg(feature = "encoder")]
pub mod enc {
    use super::*;

    /// Normalises symbol counts to sum 4096 keeping every used symbol ≥ 1, minimising the
    /// coded size (greedy marginal-cost correction).
    #[allow(clippy::needless_range_loop)]
    pub fn normalize(counts: &[u32]) -> Freqs {
        let n = counts.iter().rposition(|&c| c > 0).map_or(0, |p| p + 1);
        let total: u64 = counts.iter().map(|&c| u64::from(c)).sum();
        let mut o = Freqs::EMPTY;
        o.n = n.max(1) as u16;
        if total == 0 {
            o.f[0] = PROB_SCALE as u16;
            return o;
        }
        let mut sum = 0i64;
        for i in 0..n {
            if counts[i] > 0 {
                let v = ((u64::from(counts[i]) * u64::from(PROB_SCALE)) as f64 / total as f64).round().max(1.0) as i64;
                o.f[i] = v as u16;
                sum += v;
            }
        }
        // Fix the sum: move units where the cost change per unit is smallest.
        while sum != i64::from(PROB_SCALE) {
            let up = sum < i64::from(PROB_SCALE);
            let mut best = usize::MAX;
            let mut best_cost = f64::INFINITY;
            for i in 0..n {
                let c = counts[i];
                if c == 0 {
                    continue;
                }
                let f = f64::from(o.f[i]);
                let delta = if up {
                    // gain from f -> f+1 (negative cost)
                    -(f64::from(c) * ((f + 1.0) / f).log2())
                } else {
                    if o.f[i] <= 1 {
                        continue;
                    }
                    f64::from(c) * (f / (f - 1.0)).log2()
                };
                if delta < best_cost {
                    best_cost = delta;
                    best = i;
                }
            }
            if up {
                o.f[best] += 1;
                sum += 1;
            } else {
                o.f[best] -= 1;
                sum -= 1;
            }
        }
        o
    }

    /// Exact cost in bits of coding `counts` with `f` (∞ if a used symbol has frequency 0).
    pub fn cost_bits(counts: &[u32], f: &Freqs) -> f64 {
        let mut b = 0.0;
        for (i, &c) in counts.iter().enumerate() {
            if c == 0 {
                continue;
            }
            let fr = if i < f.n as usize { f.f[i] } else { 0 };
            if fr == 0 {
                return f64::INFINITY;
            }
            b += f64::from(c) * (f64::from(PROB_SCALE) / f64::from(fr)).log2();
        }
        b
    }

    /// Encodes `syms` where symbol `i` uses table `tables[ctx[i]]`.
    pub fn encode_ctx(syms: &[u8], ctx: &[u8], tables: &[Freqs]) -> Vec<u8> {
        let cums: Vec<[u32; MAX_SYM + 1]> = tables
            .iter()
            .map(|f| {
                let mut cum = [0u32; MAX_SYM + 1];
                for s in 0..MAX_SYM {
                    cum[s + 1] = cum[s] + u32::from(f.f[s]);
                }
                cum
            })
            .collect();
        let mut out = Vec::with_capacity(syms.len() / 2 + 8);
        let mut x = RANS_L;
        for (&s, &c) in syms.iter().zip(ctx).rev() {
            let f = &tables[c as usize];
            let fr = u32::from(f.f[s as usize]);
            debug_assert!(fr > 0, "symbol {s} has zero frequency in context {c}");
            let x_max = ((RANS_L >> PROB_BITS) << 8) * fr;
            while x >= x_max {
                out.push(x as u8);
                x >>= 8;
            }
            x = ((x / fr) << PROB_BITS) + (x % fr) + cums[c as usize][s as usize];
        }
        out.push((x >> 24) as u8);
        out.push((x >> 16) as u8);
        out.push((x >> 8) as u8);
        out.push(x as u8);
        out.reverse();
        out
    }

    /// Encodes `syms` (in forward order) with `f`; returns the payload the decoder reads forward.
    pub fn encode(syms: &[u8], f: &Freqs) -> Vec<u8> {
        let mut cum = [0u32; MAX_SYM + 1];
        for s in 0..MAX_SYM {
            cum[s + 1] = cum[s] + u32::from(f.f[s]);
        }
        let mut out = Vec::with_capacity(syms.len() / 2 + 8);
        let mut x = RANS_L;
        for &s in syms.iter().rev() {
            let fr = u32::from(f.f[s as usize]);
            debug_assert!(fr > 0, "symbol {s} has zero frequency");
            let x_max = ((RANS_L >> PROB_BITS) << 8) * fr;
            while x >= x_max {
                out.push(x as u8);
                x >>= 8;
            }
            x = ((x / fr) << PROB_BITS) + (x % fr) + cum[s as usize];
        }
        out.push((x >> 24) as u8);
        out.push((x >> 16) as u8);
        out.push((x >> 8) as u8);
        out.push(x as u8);
        out.reverse();
        out
    }
}

#[cfg(all(test, feature = "encoder"))]
mod tests {
    use super::enc::*;
    use super::*;

    #[test]
    fn rans_round_trip() {
        let mut syms = Vec::new();
        let mut s = 12345u32;
        for _ in 0..50_000 {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let v = ((s >> 16) % 100) as u8;
            syms.push(if v < 60 { 0 } else { v });
        }
        let mut counts = [0u32; 256];
        for &x in &syms {
            counts[x as usize] += 1;
        }
        let f = normalize(&counts);
        f.validate().unwrap();
        let mut tbl = Vec::new();
        f.write(&mut tbl);
        let f2 = Freqs::read(&mut Cursor::new(&tbl)).unwrap();
        assert_eq!(f, f2);
        let payload = encode(&syms, &f);
        let mut t = Table::new();
        t.build(&f);
        let mut d = RansDec::new(&payload).unwrap();
        for &x in &syms {
            assert_eq!(d.get(&t), u32::from(x));
        }
        assert!(d.finished());
        let bits = cost_bits(&counts, &f);
        assert!((payload.len() as f64 * 8.0) < bits + 64.0);
    }

    #[test]
    fn normalize_edge_cases() {
        let f = normalize(&[0, 0, 5]);
        assert_eq!(f.n, 3);
        assert_eq!(f.f[2], 4096);
        let mut c = vec![1u32; 256];
        c[0] = 1_000_000;
        let f = normalize(&c);
        f.validate().unwrap();
        assert!(f.f[1..256].iter().all(|&x| x >= 1));
    }
}
