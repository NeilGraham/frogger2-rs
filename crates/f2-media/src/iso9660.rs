// Adapted from theme-park-world-recomp (tpw-import), MIT, same author.
//! ISO9660 + Joliet directory reader over a [`SectorSource`].
//!
//! All TPW discs carry a Joliet supplementary volume descriptor, and they
//! need it: the primary (ISO9660 level 1) tree has 8.3 upper-case names,
//! while the game data uses long mixed-case names such as
//! `Data/generic/defaulttexture/NotFound.tga` or `Data/Challenges.sam`.
//! [`IsoFs::open`] therefore prefers Joliet and falls back to the primary
//! tree only when no Joliet SVD exists.

use std::collections::HashSet;
use std::io;

use crate::sector::{SectorSource, USER};
use crate::vfs::{FileEntry, Vfs};

/// Which directory hierarchy to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Names {
    /// Joliet (UCS-2 long names) if present, else primary.
    PreferJoliet,
    /// Primary ISO9660 hierarchy only.
    Primary,
}

#[derive(Clone, Debug, Default)]
pub struct VolumeInfo {
    pub volume_id: String,
    /// PVD creation date, 17-char ISO9660 form (`YYYYMMDDhhmmsscc` + tz byte stripped).
    pub created: String,
    pub has_joliet: bool,
    /// True when the tree in use is the Joliet one.
    pub joliet: bool,
    pub volume_sectors: u32,
}

/// A parsed ISO9660 file system.
pub struct IsoFs<S: SectorSource> {
    src: S,
    pub info: VolumeInfo,
    entries: Vec<FileEntry>,
    extents: Vec<Vec<(u32, u64)>>,
    pub dirs: usize,
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// A multi-extent file being assembled: (path, extents, total size).
type Pending = (String, Vec<(u32, u64)>, u64);

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn decode_name(raw: &[u8], joliet: bool) -> String {
    let mut s = if joliet {
        let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        raw.iter().map(|&b| b as char).collect()
    };
    if let Some(i) = s.find(';') {
        s.truncate(i);
    }
    if !joliet && s.ends_with('.') {
        s.pop();
    }
    s
}

impl<S: SectorSource> IsoFs<S> {
    pub fn open(mut src: S, names: Names) -> io::Result<Self> {
        let mut pvd: Option<Vec<u8>> = None;
        let mut svd: Option<Vec<u8>> = None;
        for lba in 16..64u32 {
            if lba >= src.sector_count() {
                break;
            }
            let mut v = Vec::with_capacity(USER);
            src.read_sectors(lba, 1, &mut v)?;
            if &v[1..6] != b"CD001" {
                break;
            }
            match v[0] {
                1 if pvd.is_none() => pvd = Some(v),
                2 if svd.is_none() && v[88] == 0x25 && v[89] == 0x2f && matches!(v[90], 0x40 | 0x43 | 0x45) => svd = Some(v),
                255 => break,
                _ => {}
            }
        }
        let pvd = pvd.ok_or_else(|| bad("no ISO9660 primary volume descriptor"))?;
        let ascii = |b: &[u8]| String::from_utf8_lossy(b).trim_end_matches([' ', '\0']).to_string();
        let mut info = VolumeInfo {
            volume_id: ascii(&pvd[40..72]),
            created: ascii(&pvd[813..829]),
            has_joliet: svd.is_some(),
            joliet: false,
            volume_sectors: le32(&pvd, 80),
        };
        let vd = match (names, svd) {
            (Names::PreferJoliet, Some(s)) => {
                info.joliet = true;
                s
            }
            _ => pvd,
        };
        let root = &vd[156..156 + 34];
        let mut fs = IsoFs { src, info, entries: Vec::new(), extents: Vec::new(), dirs: 0 };
        let mut seen = HashSet::new();
        fs.walk(le32(root, 2), le32(root, 10), String::new(), 0, &mut seen)?;
        Ok(fs)
    }

    fn walk(&mut self, lba: u32, size: u32, prefix: String, depth: u32, seen: &mut HashSet<u32>) -> io::Result<()> {
        if depth > 64 || !seen.insert(lba) {
            return Ok(());
        }
        self.dirs += 1;
        let nsec = size.div_ceil(USER as u32);
        let mut d = Vec::with_capacity(nsec as usize * USER);
        self.src.read_sectors(lba, nsec, &mut d)?;
        let joliet = self.info.joliet;
        let mut subdirs = Vec::new();
        // (name, extents, size) of a multi-extent file still being assembled.
        let mut pending: Option<Pending> = None;
        let mut i = 0usize;
        while i < d.len() {
            let len = d[i] as usize;
            if len == 0 {
                i = (i / USER + 1) * USER; // records never cross sector boundaries
                continue;
            }
            if len < 34 || i + len > d.len() {
                return Err(bad(format!("corrupt directory record at sector {lba}")));
            }
            let r = &d[i..i + len];
            i += len;
            let nlen = r[32] as usize;
            if 33 + nlen > len {
                return Err(bad(format!("corrupt directory record name at sector {lba}")));
            }
            let raw_name = &r[33..33 + nlen];
            if raw_name == [0] || raw_name == [1] {
                continue;
            }
            let (elba, esize, flags) = (le32(r, 2), le32(r, 10), r[25]);
            let name = decode_name(raw_name, joliet);
            let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if flags & 2 != 0 {
                subdirs.push((elba, esize, path));
                continue;
            }
            let (mut ext, mut total) = match pending.take() {
                Some((p, e, t)) if p == path => (e, t),
                Some((p, e, t)) => {
                    self.push_file(p, e, t);
                    (Vec::new(), 0)
                }
                None => (Vec::new(), 0),
            };
            ext.push((elba, u64::from(esize)));
            total += u64::from(esize);
            if flags & 0x80 != 0 {
                pending = Some((path, ext, total)); // more extents follow
            } else {
                self.push_file(path, ext, total);
            }
        }
        if let Some((p, e, t)) = pending {
            self.push_file(p, e, t);
        }
        for (l, s, p) in subdirs {
            self.walk(l, s, p, depth + 1, seen)?;
        }
        Ok(())
    }

    fn push_file(&mut self, path: String, extents: Vec<(u32, u64)>, size: u64) {
        self.entries.push(FileEntry { path, size });
        self.extents.push(extents);
    }

    /// First LBA of a file (for diagnostics).
    pub fn lba(&self, idx: usize) -> Option<u32> {
        self.extents.get(idx)?.first().map(|e| e.0)
    }

    pub fn into_source(self) -> S {
        self.src
    }
}

impl<S: SectorSource> Vfs for IsoFs<S> {
    fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    fn stream_file(&mut self, idx: usize, sink: &mut dyn FnMut(&[u8])) -> io::Result<()> {
        const CHUNK: u64 = 512;
        let extents = self.extents.get(idx).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "bad file index"))?.clone();
        let mut buf = Vec::with_capacity(CHUNK as usize * USER);
        for (lba, size) in extents {
            let mut left = size;
            let mut cur = u64::from(lba);
            while left > 0 {
                let want = left.div_ceil(USER as u64).min(CHUNK);
                buf.clear();
                let start = u32::try_from(cur).map_err(|_| bad("extent beyond 32-bit LBA"))?;
                self.src.read_sectors(start, want as u32, &mut buf)?;
                let take = left.min(buf.len() as u64) as usize;
                sink(&buf[..take]);
                left -= take as u64;
                cur += want;
            }
        }
        Ok(())
    }
}
