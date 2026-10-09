//! Little-endian bit reader (equivalent of FFmpeg's `BITSTREAM_READER_LE` `GetBitContext`).
//!
//! Reads past the end of the buffer return zero bits, mirroring FFmpeg's zero padding.

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    size_bits: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, size_bits: data.len() * 8 }
    }

    #[inline]
    fn load64(&self) -> u64 {
        let byte = self.pos >> 3;
        let mut buf = [0u8; 8];
        if byte + 8 <= self.data.len() {
            buf.copy_from_slice(&self.data[byte..byte + 8]);
        } else if byte < self.data.len() {
            let n = self.data.len() - byte;
            buf[..n].copy_from_slice(&self.data[byte..]);
        }
        u64::from_le_bytes(buf) >> (self.pos & 7)
    }

    /// Peek `n` (0..=32) bits without consuming them.
    #[inline]
    pub(crate) fn peek(&self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.load64() & ((1u64 << n) - 1)) as u32
    }

    #[inline]
    pub(crate) fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    /// Read `n` (0..=32) bits, first bit in the LSB.
    #[inline]
    pub(crate) fn get(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.pos += n as usize;
        v
    }

    #[inline]
    pub(crate) fn bit(&mut self) -> bool {
        self.get(1) != 0
    }

    #[inline]
    pub(crate) fn count(&self) -> usize {
        self.pos
    }

    #[inline]
    pub(crate) fn left(&self) -> isize {
        self.size_bits as isize - self.pos as isize
    }

    pub(crate) fn align32(&mut self) {
        let n = self.pos.wrapping_neg() & 31;
        self.pos += n;
    }
}
