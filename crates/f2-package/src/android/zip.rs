//! Minimal zip reading (the shell APK) and writing (the exported APK) with zipalign-style
//! alignment: stored entries start at a multiple of 4 bytes, stored native libraries
//! (`*.so`) at a multiple of 16 KiB (page-aligned so they can be mapped from the APK with
//! `extractNativeLibs="false"`). Padding goes into an extra field with the id `0xd935` (as
//! apksigner writes it). Entries are written without data descriptors; no zip64 (APKs over
//! 4 GiB are refused).

use std::io::{self, Read, Seek, SeekFrom, Write};

pub const STORED: u16 = 0;
pub const DEFLATED: u16 = 8;
/// Extra-field id apksigner uses for alignment padding.
const ALIGN_EXTRA_ID: u16 = 0xd935;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// An entry of an existing zip (from its central directory).
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub time: u16,
    pub date: u16,
    pub crc: u32,
    pub csize: u32,
    pub usize: u32,
    /// Offset of the entry's local header.
    #[allow(dead_code)]
    pub header_offset: u64,
    /// Offset of the entry's data (after its local header).
    pub data_offset: u64,
}

/// A zip read fully into memory (the shell APK is ~30 MB).
pub struct Archive {
    pub bytes: Vec<u8>,
    pub entries: Vec<Entry>,
}

impl Archive {
    pub fn parse(bytes: Vec<u8>) -> Result<Archive, String> {
        let n = bytes.len();
        if n < 22 {
            return Err("not a zip file (too short)".into());
        }
        let lo = n.saturating_sub(22 + 65535);
        let eocd = (lo..=n - 22).rev().find(|&i| u32le(&bytes, i) == 0x0605_4b50).ok_or("not a zip file (no end of central directory)")?;
        let count = u16le(&bytes, eocd + 10) as usize;
        let cd_size = u32le(&bytes, eocd + 12) as usize;
        let cd_off = u32le(&bytes, eocd + 16) as usize;
        if cd_off == 0xffff_ffff || count == 0xffff {
            return Err("zip64 archives are not supported".into());
        }
        if cd_off + cd_size > eocd {
            return Err("corrupt zip (central directory out of range)".into());
        }
        let mut entries = Vec::with_capacity(count);
        let mut p = cd_off;
        for _ in 0..count {
            if p + 46 > eocd || u32le(&bytes, p) != 0x0201_4b50 {
                return Err("corrupt zip (bad central directory entry)".into());
            }
            let nlen = u16le(&bytes, p + 28) as usize;
            let xlen = u16le(&bytes, p + 30) as usize;
            let clen = u16le(&bytes, p + 32) as usize;
            let lho = u32le(&bytes, p + 42) as usize;
            let name = String::from_utf8_lossy(&bytes[p + 46..p + 46 + nlen]).into_owned();
            if lho + 30 > n || u32le(&bytes, lho) != 0x0403_4b50 {
                return Err(format!("corrupt zip (bad local header for {name})"));
            }
            let data_offset = (lho + 30 + u16le(&bytes, lho + 26) as usize + u16le(&bytes, lho + 28) as usize) as u64;
            let e = Entry {
                name,
                method: u16le(&bytes, p + 10),
                time: u16le(&bytes, p + 12),
                date: u16le(&bytes, p + 14),
                crc: u32le(&bytes, p + 16),
                csize: u32le(&bytes, p + 20),
                usize: u32le(&bytes, p + 24),
                header_offset: lho as u64,
                data_offset,
            };
            if e.data_offset as usize + e.csize as usize > n {
                return Err(format!("corrupt zip ({} out of range)", e.name));
            }
            entries.push(e);
            p += 46 + nlen + xlen + clen;
        }
        Ok(Archive { bytes, entries })
    }

    /// The entry's raw (possibly compressed) data.
    pub fn raw(&self, e: &Entry) -> &[u8] {
        &self.bytes[e.data_offset as usize..e.data_offset as usize + e.csize as usize]
    }

    /// The entry's uncompressed contents.
    pub fn read(&self, e: &Entry) -> Result<Vec<u8>, String> {
        match e.method {
            STORED => Ok(self.raw(e).to_vec()),
            DEFLATED => miniz_oxide::inflate::decompress_to_vec(self.raw(e)).map_err(|err| format!("{}: inflate failed: {err:?}", e.name)),
            m => Err(format!("{}: unsupported compression method {m}", e.name)),
        }
    }

    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }
}

/// Alignment for an entry's data: stored `.so` 16 KiB, other stored entries 4, compressed 1.
pub fn alignment(name: &str, method: u16) -> u64 {
    match method {
        STORED if name.ends_with(".so") => 16384,
        STORED => 4,
        _ => 1,
    }
}

/// DOS date/time 1981-01-01 00:00 (zip timestamps of written entries: reproducible).
pub const DOS_TIME: u16 = 0;
pub const DOS_DATE: u16 = (1 << 9) | (1 << 5) | 1;

struct Written {
    name: String,
    method: u16,
    time: u16,
    date: u16,
    crc: u32,
    csize: u32,
    usize: u32,
    offset: u32,
}

/// Writes a zip with aligned entries to a seekable output. [`Writer::finish`] returns the
/// central directory and end record separately so the APK signer can put its block before
/// them.
pub struct Writer<W: Write + Seek> {
    out: W,
    pos: u64,
    done: Vec<Written>,
}

impl<W: Write + Seek> Writer<W> {
    pub fn new(out: W) -> Writer<W> {
        Writer { out, pos: 0, done: Vec::new() }
    }

    /// Local header (with alignment padding) for an entry whose sizes and CRC may still be
    /// placeholders; returns the header offset.
    #[allow(clippy::too_many_arguments)]
    fn header(&mut self, name: &str, method: u16, time: u16, date: u16, crc: u32, csize: u32, usize: u32) -> io::Result<u64> {
        if self.pos > u32::MAX as u64 {
            return Err(io::Error::other("APK larger than 4 GiB (zip64 is not supported)"));
        }
        let align = alignment(name, method);
        let offset = self.pos;
        let base = offset + 30 + name.len() as u64;
        let extra: Vec<u8> = if align > 1 {
            // id, length, alignment, zero padding: at least 6 bytes.
            let pad = (align - (base + 6) % align) % align;
            let mut x = Vec::with_capacity(6 + pad as usize);
            x.extend_from_slice(&ALIGN_EXTRA_ID.to_le_bytes());
            x.extend_from_slice(&((2 + pad) as u16).to_le_bytes());
            x.extend_from_slice(&(align.min(u16::MAX as u64) as u16).to_le_bytes());
            x.resize(6 + pad as usize, 0);
            x
        } else {
            Vec::new()
        };
        let mut h = Vec::with_capacity(30 + name.len() + extra.len());
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes());
        // UTF-8 names (bit 11), no data descriptor.
        h.extend_from_slice(&0x0800u16.to_le_bytes());
        h.extend_from_slice(&method.to_le_bytes());
        h.extend_from_slice(&time.to_le_bytes());
        h.extend_from_slice(&date.to_le_bytes());
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&csize.to_le_bytes());
        h.extend_from_slice(&usize.to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        h.extend_from_slice(name.as_bytes());
        h.extend_from_slice(&extra);
        self.out.write_all(&h)?;
        self.pos += h.len() as u64;
        debug_assert_eq!(self.pos % align, 0);
        Ok(offset)
    }

    /// An entry whose (possibly compressed) data is already known, e.g. copied from another
    /// zip.
    #[allow(clippy::too_many_arguments)]
    pub fn add_raw(&mut self, name: &str, method: u16, time: u16, date: u16, crc: u32, usize: u32, raw: &[u8]) -> io::Result<()> {
        let csize = u32::try_from(raw.len()).map_err(|_| io::Error::other(format!("{name}: larger than 4 GiB")))?;
        let offset = self.header(name, method, time, date, crc, csize, usize)?;
        self.out.write_all(raw)?;
        self.pos += raw.len() as u64;
        self.done.push(Written { name: name.into(), method, time, date, crc, csize, usize, offset: offset as u32 });
        Ok(())
    }

    /// A stored entry with the given contents.
    pub fn add_stored(&mut self, name: &str, data: &[u8]) -> io::Result<()> {
        let size = u32::try_from(data.len()).map_err(|_| io::Error::other(format!("{name}: larger than 4 GiB")))?;
        self.add_raw(name, STORED, DOS_TIME, DOS_DATE, crc32fast::hash(data), size, data)
    }

    /// A stored entry streamed from `src` (`len` bytes): the CRC is computed while copying
    /// and patched into the local header afterwards.
    pub fn add_stored_stream(&mut self, name: &str, src: &mut dyn Read, len: u64) -> io::Result<()> {
        let size = u32::try_from(len).map_err(|_| io::Error::other(format!("{name}: larger than 4 GiB")))?;
        let offset = self.header(name, STORED, DOS_TIME, DOS_DATE, 0, size, size)?;
        let mut crc = crc32fast::Hasher::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut left = len;
        while left > 0 {
            let want = buf.len().min(left as usize);
            let n = src.read(&mut buf[..want])?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("{name}: file shrank while being packed")));
            }
            crc.update(&buf[..n]);
            self.out.write_all(&buf[..n])?;
            left -= n as u64;
        }
        self.pos += len;
        let crc = crc.finalize();
        self.out.seek(SeekFrom::Start(offset + 14))?;
        self.out.write_all(&crc.to_le_bytes())?;
        self.out.seek(SeekFrom::Start(self.pos))?;
        self.done.push(Written { name: name.into(), method: STORED, time: DOS_TIME, date: DOS_DATE, crc, csize: size, usize: size, offset: offset as u32 });
        Ok(())
    }

    /// Ends the entries; returns the output, the offset where the central directory goes
    /// (= the end of the entries), the central directory and the end-of-central-directory
    /// record (pointing at that offset). Nothing after the entries is written.
    pub fn finish(self) -> io::Result<(W, u64, Vec<u8>, Vec<u8>)> {
        if self.pos > u32::MAX as u64 || self.done.len() > u16::MAX as usize {
            return Err(io::Error::other("APK too large for a zip without zip64"));
        }
        let mut cd = Vec::new();
        for w in &self.done {
            cd.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            cd.extend_from_slice(&20u16.to_le_bytes()); // made by
            cd.extend_from_slice(&20u16.to_le_bytes()); // needed
            cd.extend_from_slice(&0x0800u16.to_le_bytes());
            cd.extend_from_slice(&w.method.to_le_bytes());
            cd.extend_from_slice(&w.time.to_le_bytes());
            cd.extend_from_slice(&w.date.to_le_bytes());
            cd.extend_from_slice(&w.crc.to_le_bytes());
            cd.extend_from_slice(&w.csize.to_le_bytes());
            cd.extend_from_slice(&w.usize.to_le_bytes());
            cd.extend_from_slice(&(w.name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0; 2 + 2 + 2 + 2 + 4]); // extra, comment, disk, attrs
            cd.extend_from_slice(&w.offset.to_le_bytes());
            cd.extend_from_slice(w.name.as_bytes());
        }
        let eocd = eocd(self.done.len() as u16, cd.len() as u32, self.pos as u32);
        Ok((self.out, self.pos, cd, eocd))
    }
}

/// End-of-central-directory record.
pub fn eocd(count: u16, cd_size: u32, cd_offset: u32) -> Vec<u8> {
    let mut e = Vec::with_capacity(22);
    e.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    e.extend_from_slice(&[0; 4]);
    e.extend_from_slice(&count.to_le_bytes());
    e.extend_from_slice(&count.to_le_bytes());
    e.extend_from_slice(&cd_size.to_le_bytes());
    e.extend_from_slice(&cd_offset.to_le_bytes());
    e.extend_from_slice(&0u16.to_le_bytes());
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn writes_aligned_entries_that_read_back() {
        let mut w = Writer::new(Cursor::new(Vec::new()));
        w.add_stored("a.txt", b"hello").unwrap();
        w.add_stored("lib/x86_64/libx.so", &[7u8; 100]).unwrap();
        let deflated = miniz_oxide::deflate::compress_to_vec(b"compressed compressed compressed", 6);
        w.add_raw("c.bin", DEFLATED, 0, 0, crc32fast::hash(b"compressed compressed compressed"), 32, &deflated).unwrap();
        let big = vec![3u8; 3 << 20];
        w.add_stored_stream("assets/big.bin", &mut &big[..], big.len() as u64).unwrap();
        let (out, cd_off, cd, eocd) = w.finish().unwrap();
        let mut bytes = out.into_inner();
        assert_eq!(bytes.len() as u64, cd_off);
        bytes.extend_from_slice(&cd);
        bytes.extend_from_slice(&eocd);
        let a = Archive::parse(bytes).unwrap();
        assert_eq!(a.entries.len(), 4);
        let e = a.find("a.txt").unwrap();
        assert_eq!(e.data_offset % 4, 0);
        assert_eq!(a.read(e).unwrap(), b"hello");
        let so = a.find("lib/x86_64/libx.so").unwrap();
        assert_eq!(so.data_offset % 16384, 0);
        assert_eq!(a.read(a.find("c.bin").unwrap()).unwrap(), b"compressed compressed compressed");
        let big_e = a.find("assets/big.bin").unwrap();
        assert_eq!(big_e.data_offset % 4, 0);
        assert_eq!(big_e.crc, crc32fast::hash(&big));
        // The streamed entry's local header has the real CRC too.
        assert_eq!(u32le(&a.bytes, big_e.header_offset as usize + 14), big_e.crc);
        assert_eq!(a.read(big_e).unwrap(), big);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Archive::parse(vec![0; 100]).is_err());
    }
}
