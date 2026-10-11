//! MSB-first raw bit stream for the extra bits of LL/ML/OF codes.

/// Reads up to 32 bits at a time; past the end it yields zeros (the block decoder checks
/// consumption at the end).
pub struct BitReader<'a> {
    buf: &'a [u8],
    pos: usize,
    acc: u32,
    /// Valid bits in `acc` (left-aligned).
    n: u32,
}

impl<'a> BitReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        BitReader { buf, pos: 0, acc: 0, n: 0 }
    }

    #[inline]
    fn refill(&mut self) {
        while self.n <= 24 {
            let b = self.buf.get(self.pos).copied().unwrap_or(0);
            self.pos += 1;
            self.acc |= u32::from(b) << (24 - self.n);
            self.n += 8;
        }
    }

    /// Reads `k` ≤ 24 bits.
    #[inline]
    fn read_small(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        if self.n < k {
            self.refill();
        }
        let v = self.acc >> (32 - k);
        self.acc <<= k;
        self.n -= k;
        v
    }

    /// Reads `k` ≤ 32 bits.
    #[inline]
    pub fn read(&mut self, k: u32) -> u32 {
        if k <= 24 {
            self.read_small(k)
        } else {
            let hi = self.read_small(k - 16);
            (hi << 16) | self.read_small(16)
        }
    }

    /// True when no more than the real payload was consumed.
    pub fn ok(&self) -> bool {
        // Bytes loaded beyond the end are zero padding; bits still buffered are unconsumed.
        let consumed_bits = self.pos as u64 * 8 - u64::from(self.n);
        consumed_bits <= self.buf.len() as u64 * 8
    }
}

#[cfg(feature = "encoder")]
#[derive(Default)]
pub struct BitWriter {
    pub out: Vec<u8>,
    acc: u64,
    n: u32,
}

#[cfg(feature = "encoder")]
impl BitWriter {
    pub fn put(&mut self, v: u32, k: u32) {
        if k == 0 {
            return;
        }
        debug_assert!(k == 32 || v >> k == 0);
        self.acc = (self.acc << k) | u64::from(v);
        self.n += k;
        while self.n >= 8 {
            self.out.push((self.acc >> (self.n - 8)) as u8);
            self.n -= 8;
        }
        self.acc &= (1u64 << self.n) - 1;
    }
    pub fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push((self.acc << (8 - self.n)) as u8);
        }
        self.out
    }
}

#[cfg(all(test, feature = "encoder"))]
mod tests {
    use super::*;
    #[test]
    fn bits_round_trip() {
        let vals: Vec<(u32, u32)> = (0..2000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> (i % 33).min(31), (32 - (i % 33)).min(32))).collect();
        let mut w = BitWriter::default();
        for &(v, k) in &vals {
            let v = if k == 32 { v } else { v & ((1u32 << k) - 1) };
            w.put(v, k);
        }
        let b = w.finish();
        let mut r = BitReader::new(&b);
        for &(v, k) in &vals {
            let v = if k == 32 { v } else { v & ((1u32 << k) - 1) };
            assert_eq!(r.read(k), v);
        }
        assert!(r.ok());
    }
}
