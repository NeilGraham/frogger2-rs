//! "FlatPacker" FLA2 compression (`utilDecompressBuffer`, MDX `mdxFile.cpp`).
//!
//! `u32 'FLA2'`, `u32 decompressed size`, then an LZSS stream: a tag byte
//! gives 8 items MSB-first; bit 0 = literal byte, bit 1 = back-reference
//! `c = byte` (0 terminates), `offset = 4096 - ((c & 0xF0) * 16 + byte)`,
//! `length = (c & 0x0F) + 2`, read from a 4096-byte history ring.

use crate::prelude::*;

use crate::read::{Error, Result};

pub const MAGIC: &[u8; 4] = b"FLA2";

pub fn is_fla(data: &[u8]) -> bool {
    data.len() >= 8 && &data[..4] == MAGIC
}

/// Decompresses an FLA2 buffer.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    let err = |m: &str| Error::new("fla", m.to_string());
    if !is_fla(data) {
        return Err(err("missing FLA2 magic"));
    }
    let size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let mut out = Vec::with_capacity(size);
    let mut hist = [0u8; 4096];
    let mut hp = 0usize;
    let mut i = 8usize;
    let next = |i: &mut usize| -> Result<u8> {
        let b = *data.get(*i).ok_or_else(|| err("truncated stream"))?;
        *i += 1;
        Ok(b)
    };
    loop {
        let mut tag = next(&mut i)?;
        for _ in 0..8 {
            if tag & 0x80 != 0 {
                let c = next(&mut i)?;
                if c == 0 {
                    if out.len() != size {
                        return Err(err(&format!("decompressed {} bytes, header says {size}", out.len())));
                    }
                    return Ok(out);
                }
                let lo = next(&mut i)?;
                let off = 4096 - ((usize::from(c & 0xF0) * 16) + usize::from(lo));
                let len = usize::from(c & 0x0F) + 2;
                for _ in 0..len {
                    let b = hist[(hp + off) & 4095];
                    out.push(b);
                    hist[hp] = b;
                    hp = (hp + 1) & 4095;
                }
            } else {
                let b = next(&mut i)?;
                out.push(b);
                hist[hp] = b;
                hp = (hp + 1) & 4095;
            }
            tag <<= 1;
        }
        if out.len() > size + 4096 {
            return Err(err("output overruns header size"));
        }
    }
}

/// Like the game's `fileLoad`: decompresses when the buffer is FLA2, else returns it unchanged.
pub fn maybe_decompress(data: Vec<u8>) -> Result<Vec<u8>> {
    if is_fla(&data) {
        decompress(&data)
    } else {
        Ok(data)
    }
}

/// Compresses with literals only (valid FLA2 the game can read). Used by mod tooling and tests.
pub fn compress_store(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 9 / 8 + 16);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    let mut chunks = data.chunks(8).peekable();
    while let Some(chunk) = chunks.next() {
        if chunks.peek().is_none() && chunk.len() < 8 {
            // Last partial group: literals, then the terminating reference (c == 0)
            // in the same tag byte.
            out.push(0x80 >> chunk.len());
            out.extend_from_slice(chunk);
            out.push(0);
            return out;
        }
        out.push(0);
        out.extend_from_slice(chunk);
    }
    // terminator: a tag with a reference bit followed by c == 0
    out.push(0x80);
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_store() {
        let d: Vec<u8> = (0..1000u32).map(|x| (x * 7) as u8).collect();
        assert_eq!(decompress(&compress_store(&d)).unwrap(), d);
        for n in 0..20usize {
            let v: Vec<u8> = (0..n as u8).collect();
            assert_eq!(decompress(&compress_store(&v)).unwrap(), v, "len {n}");
        }
        assert_eq!(decompress(&compress_store(&[])).unwrap(), Vec::<u8>::new());
    }
    #[test]
    fn back_reference() {
        // tag 0b011..: literal 'a', reference (c=0x01, lo=1 -> offset 4095 = previous
        // byte, length 3), then a reference with c == 0 terminating the stream.
        let s = [b'F', b'L', b'A', b'2', 4, 0, 0, 0, 0b0110_0000, b'a', 0x01, 1, 0];
        assert_eq!(decompress(&s).unwrap(), b"aaaa");
    }
}
