// Adapted from theme-park-world-recomp (tpw-import), MIT, same author.
//! Raw CD sector layouts and 2048-byte user-data sources.
//!
//! Every reader in this crate ends up as a [`SectorSource`]: something that
//! yields cooked 2048-byte user-data sectors by logical block address. Raw
//! formats (2352, 2336, 2448 with subchannel) are converted per sector, with
//! the sync/header/subheader and EDC/ECC stripped.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

/// Cooked user-data bytes per sector (Mode 1 / Mode 2 Form 1).
pub const USER: usize = 2048;

/// The 12-byte CD sync pattern that starts every raw data sector.
pub const SYNC: [u8; 12] = [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0];

/// How sectors are stored in an image file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawFormat {
    /// Plain ISO: 2048 bytes of user data per sector.
    Cooked2048,
    /// Full raw sector: sync(12) + header(4) + [subheader(8)] + data + EDC/ECC.
    Raw2352,
    /// Mode 2 without sync/header: subheader(8) + data + EDC/ECC.
    Raw2336,
    /// Raw 2352 followed by 96 bytes of subchannel (Alcohol "RAW+SUB" MDF).
    Raw2448,
}

impl RawFormat {
    /// Bytes per sector in the image file.
    pub fn stride(self) -> usize {
        match self {
            RawFormat::Cooked2048 => 2048,
            RawFormat::Raw2352 => 2352,
            RawFormat::Raw2336 => 2336,
            RawFormat::Raw2448 => 2448,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RawFormat::Cooked2048 => "cooked2048",
            RawFormat::Raw2352 => "raw2352",
            RawFormat::Raw2336 => "raw2336",
            RawFormat::Raw2448 => "raw2448+sub",
        }
    }
}

/// Data track mode, used when a raw sector has no readable header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackMode {
    Mode1,
    Mode2,
}

/// Returns the 2048 bytes of user data of one stored sector.
///
/// For 2352/2448 sectors the mode byte (offset 15) decides between Mode 1
/// (data at 16) and Mode 2 Form 1 (data at 24, after the 8-byte subheader);
/// a sector without sync falls back to `default_mode`. `raw` must be at least
/// `fmt.stride()` bytes long.
pub fn user_data(fmt: RawFormat, raw: &[u8], default_mode: TrackMode) -> &[u8] {
    let off = match fmt {
        RawFormat::Cooked2048 => 0,
        RawFormat::Raw2336 => 8,
        RawFormat::Raw2352 | RawFormat::Raw2448 => {
            let mode = if raw[..12] == SYNC {
                match raw[15] {
                    2 => TrackMode::Mode2,
                    _ => TrackMode::Mode1,
                }
            } else {
                default_mode
            };
            match mode {
                TrackMode::Mode1 => 16,
                TrackMode::Mode2 => 24,
            }
        }
    };
    &raw[off..off + USER]
}

/// Random access to cooked 2048-byte sectors.
pub trait SectorSource {
    /// Number of sectors available.
    fn sector_count(&self) -> u32;
    /// Appends `count` cooked sectors starting at `lba` to `out`.
    fn read_sectors(&mut self, lba: u32, count: u32, out: &mut Vec<u8>) -> io::Result<()>;
}

impl<S: SectorSource + ?Sized> SectorSource for Box<S> {
    fn sector_count(&self) -> u32 {
        (**self).sector_count()
    }
    fn read_sectors(&mut self, lba: u32, count: u32, out: &mut Vec<u8>) -> io::Result<()> {
        (**self).read_sectors(lba, count, out)
    }
}

fn range_check(lba: u32, count: u32, total: u32) -> io::Result<()> {
    if u64::from(lba) + u64::from(count) > u64::from(total) {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("sector range {lba}+{count} beyond end of image ({total} sectors)")));
    }
    Ok(())
}

/// Cooked sectors held in memory (e.g. inflated from a zip member).
pub struct MemSource {
    data: Vec<u8>,
}

impl MemSource {
    /// `data` is truncated to a whole number of sectors.
    pub fn new(mut data: Vec<u8>) -> Self {
        let whole = data.len() / USER * USER;
        data.truncate(whole);
        MemSource { data }
    }
}

impl SectorSource for MemSource {
    fn sector_count(&self) -> u32 {
        (self.data.len() / USER) as u32
    }
    fn read_sectors(&mut self, lba: u32, count: u32, out: &mut Vec<u8>) -> io::Result<()> {
        range_check(lba, count, self.sector_count())?;
        let a = lba as usize * USER;
        out.extend_from_slice(&self.data[a..a + count as usize * USER]);
        Ok(())
    }
}

/// Sectors read on demand from an image file of any [`RawFormat`].
pub struct FileSource {
    file: File,
    fmt: RawFormat,
    mode: TrackMode,
    base: u64,
    sectors: u32,
    buf: Vec<u8>,
}

impl FileSource {
    /// `base` is the byte offset of the track's first sector; `sectors` its length.
    pub fn new(file: File, fmt: RawFormat, mode: TrackMode, base: u64, sectors: u32) -> Self {
        FileSource { file, fmt, mode, base, sectors, buf: Vec::new() }
    }

    /// Opens a whole file; the sector count is derived from its length.
    pub fn whole(file: File, fmt: RawFormat, mode: TrackMode) -> io::Result<Self> {
        let len = file.metadata()?.len();
        let sectors = u32::try_from(len / fmt.stride() as u64).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "image too large"))?;
        Ok(Self::new(file, fmt, mode, 0, sectors))
    }
}

impl SectorSource for FileSource {
    fn sector_count(&self) -> u32 {
        self.sectors
    }
    fn read_sectors(&mut self, lba: u32, count: u32, out: &mut Vec<u8>) -> io::Result<()> {
        range_check(lba, count, self.sectors)?;
        let stride = self.fmt.stride();
        self.file.seek(SeekFrom::Start(self.base + u64::from(lba) * stride as u64))?;
        let n = count as usize * stride;
        if self.fmt == RawFormat::Cooked2048 {
            let start = out.len();
            out.resize(start + n, 0);
            return self.file.read_exact(&mut out[start..]);
        }
        self.buf.resize(n, 0);
        self.file.read_exact(&mut self.buf)?;
        out.reserve(count as usize * USER);
        for raw in self.buf.chunks_exact(stride) {
            out.extend_from_slice(user_data(self.fmt, raw, self.mode));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_sector(mode: u8, fill: u8) -> Vec<u8> {
        let mut s = vec![0u8; 2352];
        s[..12].copy_from_slice(&SYNC);
        s[15] = mode;
        let off = if mode == 2 { 24 } else { 16 };
        s[off..off + USER].fill(fill);
        s
    }

    #[test]
    fn user_data_mode1_mode2_2336() {
        let m1 = raw_sector(1, 0xaa);
        assert!(user_data(RawFormat::Raw2352, &m1, TrackMode::Mode2).iter().all(|&b| b == 0xaa));
        let m2 = raw_sector(2, 0xbb);
        assert!(user_data(RawFormat::Raw2352, &m2, TrackMode::Mode1).iter().all(|&b| b == 0xbb));
        let r2336 = m2[16..].to_vec();
        assert!(user_data(RawFormat::Raw2336, &r2336, TrackMode::Mode2).iter().all(|&b| b == 0xbb));
        let mut r2448 = m1.clone();
        r2448.extend_from_slice(&[0x55; 96]);
        assert!(user_data(RawFormat::Raw2448, &r2448, TrackMode::Mode1).iter().all(|&b| b == 0xaa));
    }

    #[test]
    fn mem_source_bounds() {
        let mut m = MemSource::new(vec![7u8; USER * 3 + 5]);
        assert_eq!(m.sector_count(), 3);
        let mut out = Vec::new();
        m.read_sectors(1, 2, &mut out).unwrap();
        assert_eq!(out.len(), 2 * USER);
        assert!(m.read_sectors(2, 2, &mut out).is_err());
    }
}
