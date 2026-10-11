//! **F2Z** — the Frogger 2 N64 asset codec.
//!
//! LZ77 with price-based optimal parsing (3 repeat offsets) + static rANS entropy coding of
//! four symbol streams (literals, literal-run codes, match-length codes, offset codes), with
//! an optional **shared dictionary per asset class** (LZ prefix content + default entropy
//! tables, resident in RAM at runtime) and **size-preserving structure transforms** applied
//! before LZ (byte-plane transposition of struct arrays, per-field delta + zigzag).
//!
//! The decoder ([`dec`], [`rans`], [`bits`], [`codes`], [`transform`], [`dict`]) is
//! `no_std`, allocation-free, uses only 32-bit integer arithmetic, reads every multi-byte
//! value bytewise (no unaligned access, endian-neutral) and keeps its tables in one
//! caller-owned [`dec::Decoder`] (~41 KB). The encoder and dictionary trainer
//! (feature `encoder`) are host-only.
//!
//! # Container (all multi-byte fields little-endian, read bytewise)
//! ```text
//! file   := "F2Z1" u32 raw_size u8 dict_id(0 = none) u8 nseg u16 0
//!           nseg × segment                     (8 B each, see transform::Segment)
//!           u32 nblocks  nblocks × block
//! block  := u32 out_len u32 nseq
//!           4 × stream_hdr                     (LIT, LL, ML, OF)
//!           u32 extra_len
//!           LIT payload, LL payload, ML payload, OF payload, extra-bits payload
//! stream_hdr := u8 mode  [mode 1: table] [mode 2: u8 symbol] [mode 0/1: u32 payload_len]
//!           mode 0 = dictionary default table, 1 = explicit table, 2 = single symbol
//!           (no payload), 3 = unused stream (no payload)
//! table  := u16 nsym, then nsym frequencies as LEB128 (sum = 4096; a 0 is followed by a
//!           LEB128 count of additional zeros)
//! ```
//! A block decodes `nseq` sequences `(ll, ml, of)`: copy `ll` literals, then a match of
//! length `ml + 2` at offset `of` (codes 0..2 = repeat offsets, otherwise a distance);
//! after the last sequence the rest of `out_len` is literals. Repeat offsets persist across
//! blocks and start as `[1, 4, 8]`. Matches may reach back into the dictionary content,
//! which logically precedes the output.

#![cfg_attr(not(feature = "encoder"), no_std)]
#![forbid(unsafe_code)]

pub mod bits;
pub mod codes;
pub mod dec;
pub mod dict;
pub mod rans;
pub mod transform;

#[cfg(feature = "encoder")]
pub mod enc;
#[cfg(feature = "encoder")]
pub mod train;

pub use dec::{header, Decoder, Header};
pub use dict::Dict;
pub use transform::Segment;

pub const MAGIC: [u8; 4] = *b"F2Z1";
pub const DICT_MAGIC: [u8; 4] = *b"F2ZD";

/// Stream indices.
pub const LIT: usize = 0;
pub const LL: usize = 1;
pub const ML: usize = 2;
pub const OF: usize = 3;

/// Initial repeat offsets.
pub const INIT_REPS: [u32; 3] = [1, 4, 8];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not an F2Z stream / dictionary.
    BadMagic,
    /// Truncated or inconsistent stream.
    Corrupt(&'static str),
    /// The stream needs dictionary `id`, which was not supplied (or a different one was).
    WrongDict(u8),
    /// Output (or scratch) buffer too small.
    OutputTooSmall,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::BadMagic => write!(f, "not an F2Z stream"),
            Error::Corrupt(m) => write!(f, "corrupt F2Z stream: {m}"),
            Error::WrongDict(id) => write!(f, "F2Z stream needs dictionary {id}"),
            Error::OutputTooSmall => write!(f, "F2Z output buffer too small"),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// Bytewise little-endian reader over a slice (no panics: errors on truncation).
pub(crate) struct Cursor<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, pos: 0 }
    }
    pub fn u8(&mut self) -> Result<u8> {
        let b = *self.buf.get(self.pos).ok_or(Error::Corrupt("truncated"))?;
        self.pos += 1;
        Ok(b)
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from(self.u8()?) | (u16::from(self.u8()?) << 8))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from(self.u16()?) | (u32::from(self.u16()?) << 16))
    }
    pub fn leb(&mut self) -> Result<u32> {
        let mut v = 0u32;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            if shift >= 32 {
                return Err(Error::Corrupt("leb128 overflow"));
            }
            v |= u32::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error::Corrupt("length overflow"))?;
        let s = self.buf.get(self.pos..end).ok_or(Error::Corrupt("truncated payload"))?;
        self.pos = end;
        Ok(s)
    }
}
