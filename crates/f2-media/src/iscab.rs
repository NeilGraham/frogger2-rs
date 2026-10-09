//! InstallShield 5 cabinet reader (`ISc(` signature), single volume with the
//! header embedded in `data1.cab` — the layout used by the Frogger 2 discs.
//!
//! Structure knowledge from the MIT-licensed `unshield` project
//! (<https://github.com/twogood/unshield>, `lib/cabfile.h`, `libunshield.c`,
//! `file.c`). Only the paths Frogger 2 needs are implemented:
//! major version 0/5 descriptors and the "old compression" chunk scheme
//! (`unshield -O`): compressed data is a sequence of raw-deflate chunks, each
//! terminated by the bytes `00 00 FF FF`.

use std::io;

use flate2::{Decompress, FlushDecompress, Status};

pub const SIGNATURE: u32 = 0x2863_5349; // "ISc("
const FILE_COMPRESSED: u16 = 4;
const FILE_INVALID: u16 = 8;
const FILE_OBFUSCATED: u16 = 2;
const MAX_GROUPS: usize = 71;
const END_OF_CHUNK: [u8; 4] = [0, 0, 0xff, 0xff];

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("iscab: {}", msg.into()))
}

fn u16_at(d: &[u8], o: usize) -> io::Result<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(|| bad(format!("truncated at {o:#x}")))
}

fn u32_at(d: &[u8], o: usize) -> io::Result<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| bad(format!("truncated at {o:#x}")))
}

fn cstr(d: &[u8], o: usize) -> io::Result<String> {
    let s = d.get(o..).ok_or_else(|| bad("string offset out of range"))?;
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    // Names are Windows-1252; Latin-1 decoding covers everything on these discs.
    Ok(s[..end].iter().map(|&b| b as char).collect())
}

#[derive(Clone, Debug)]
pub struct CabFile {
    pub index: usize,
    pub name: String,
    pub directory: String,
    pub group: Option<String>,
    pub flags: u16,
    pub expanded_size: u32,
    pub compressed_size: u32,
    pub data_offset: u32,
}

impl CabFile {
    /// `group/dir/name` style path with `/` separators (directory may be empty).
    pub fn path(&self) -> String {
        let mut p = String::new();
        if let Some(g) = &self.group {
            p.push_str(g);
            p.push('/');
        }
        if !self.directory.is_empty() {
            p.push_str(&self.directory.replace('\\', "/"));
            p.push('/');
        }
        p.push_str(&self.name);
        p
    }
}

#[derive(Clone, Debug)]
pub struct FileGroup {
    pub name: String,
    pub first: u32,
    pub last: u32,
}

/// A parsed cabinet held in memory (Frogger 2's is ~53 MB).
pub struct IsCab {
    data: Vec<u8>,
    pub version: u32,
    pub groups: Vec<FileGroup>,
    pub files: Vec<CabFile>,
}

impl IsCab {
    pub fn parse(data: Vec<u8>) -> io::Result<IsCab> {
        if u32_at(&data, 0)? != SIGNATURE {
            return Err(bad("not an InstallShield cabinet (missing ISc( signature)"));
        }
        let version = u32_at(&data, 4)?;
        let major = if version >> 24 == 1 { (version >> 12) & 0xf } else { 99 };
        if major != 0 && major != 5 {
            return Err(bad(format!("unsupported cabinet version {version:#010x}")));
        }
        let desc = u32_at(&data, 12)? as usize;
        if u32_at(&data, 16)? == 0 {
            return Err(bad("no cabinet descriptor (header in a separate .hdr file?)"));
        }
        let file_table_offset = u32_at(&data, desc + 0x0c)? as usize;
        let directory_count = u32_at(&data, desc + 0x1c)? as usize;
        let file_count = u32_at(&data, desc + 0x28)? as usize;
        let ft = desc + file_table_offset;
        let table: Vec<usize> = (0..directory_count + file_count).map(|i| u32_at(&data, ft + 4 * i).map(|v| v as usize)).collect::<io::Result<_>>()?;
        let dirs: Vec<String> = (0..directory_count).map(|i| cstr(&data, ft + table[i])).collect::<io::Result<_>>()?;

        // File groups: linked lists hanging off 71 offsets at desc+0x3e.
        let mut groups = Vec::new();
        for g in 0..MAX_GROUPS {
            let mut next = u32_at(&data, desc + 0x3e + 4 * g)? as usize;
            let mut guard = 0;
            while next != 0 && guard < 1024 {
                guard += 1;
                let p = desc + next;
                let name_off = u32_at(&data, p)? as usize;
                let descriptor = u32_at(&data, p + 4)? as usize;
                next = u32_at(&data, p + 8)? as usize;
                let dp = desc + descriptor;
                let gname = cstr(&data, desc + u32_at(&data, dp)? as usize)?;
                let _ = name_off;
                let first = u32_at(&data, dp + 4 + 0x48)?;
                let last = u32_at(&data, dp + 8 + 0x48)?;
                groups.push(FileGroup { name: gname, first, last });
            }
        }

        let mut files = Vec::with_capacity(file_count);
        for i in 0..file_count {
            let p = ft + table[directory_count + i];
            let name_offset = u32_at(&data, p)? as usize;
            let dir_index = u16_at(&data, p + 4)? as usize;
            let flags = u16_at(&data, p + 8)?;
            let expanded_size = u32_at(&data, p + 10)?;
            let compressed_size = u32_at(&data, p + 14)?;
            let data_offset = u32_at(&data, p + 18 + 0x14)?;
            let group = groups.iter().find(|g| (g.first as usize..=g.last as usize).contains(&i)).map(|g| g.name.clone());
            // Unused descriptor slots can hold garbage name offsets; treat
            // them as invalid entries rather than failing the whole cabinet.
            let name = ft.checked_add(name_offset).filter(|&o| o < data.len()).map(|o| cstr(&data, o)).transpose()?;
            let flags = if name.is_none() { flags | FILE_INVALID } else { flags };
            files.push(CabFile {
                index: i,
                name: name.unwrap_or_default(),
                directory: dirs.get(dir_index).cloned().unwrap_or_default(),
                group,
                flags,
                expanded_size,
                compressed_size,
                data_offset,
            });
        }
        Ok(IsCab { data, version, groups, files })
    }

    /// Whether a file entry carries data.
    pub fn is_valid(f: &CabFile) -> bool {
        f.flags & FILE_INVALID == 0 && f.data_offset != 0
    }

    /// Extracts one file.
    pub fn extract(&self, f: &CabFile) -> io::Result<Vec<u8>> {
        if !Self::is_valid(f) {
            return Err(bad(format!("{} is not a valid file entry", f.name)));
        }
        if f.flags & FILE_OBFUSCATED != 0 {
            return Err(bad(format!("{} is obfuscated (unsupported)", f.name)));
        }
        let start = f.data_offset as usize;
        let end = start + f.compressed_size as usize;
        let raw = self.data.get(start..end).ok_or_else(|| bad(format!("{}: data beyond end of cabinet", f.name)))?;
        if f.flags & FILE_COMPRESSED == 0 {
            return Ok(raw.to_vec());
        }
        let mut out = Vec::with_capacity(f.expanded_size as usize);
        let mut rest = raw;
        while !rest.is_empty() && out.len() < f.expanded_size as usize {
            let mut chunk = find(rest, &END_OF_CHUNK).ok_or_else(|| bad(format!("{}: no end-of-chunk marker", f.name)))?;
            // A marker followed by a byte with bit 0 set was really inside the
            // chunk (unshield's Qtime.smk workaround).
            while chunk + 4 < rest.len() && rest[chunk + 4] & 1 != 0 {
                let more = find(&rest[chunk + 4..], &END_OF_CHUNK).ok_or_else(|| bad(format!("{}: no end-of-chunk marker", f.name)))?;
                chunk += 4 + more;
            }
            inflate_chunk(&rest[..chunk], &mut out, f.expanded_size as usize).map_err(|e| bad(format!("{}: {e}", f.name)))?;
            rest = &rest[(chunk + 4).min(rest.len())..];
        }
        if out.len() != f.expanded_size as usize {
            return Err(bad(format!("{}: expanded to {} bytes, expected {}", f.name, out.len(), f.expanded_size)));
        }
        Ok(out)
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Inflates one raw-deflate chunk (the marker that ends its sync-flush block has
/// been cut off; a trailing zero byte is appended like unshield does).
fn inflate_chunk(chunk: &[u8], out: &mut Vec<u8>, limit: usize) -> Result<(), String> {
    let mut input = Vec::with_capacity(chunk.len() + 5);
    input.extend_from_slice(chunk);
    // Re-append the stripped sync marker so the final stored block is complete.
    input.extend_from_slice(&END_OF_CHUNK);
    let mut d = Decompress::new(false);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let before_in = d.total_in();
        let before_out = d.total_out();
        let status = d.decompress(&input[d.total_in() as usize..], &mut buf, FlushDecompress::Sync).map_err(|e| format!("inflate: {e}"))?;
        let produced = (d.total_out() - before_out) as usize;
        out.extend_from_slice(&buf[..produced]);
        if out.len() > limit {
            return Err("output exceeds expanded size".into());
        }
        let consumed_all = d.total_in() as usize >= input.len();
        let stalled = d.total_in() == before_in && produced == 0;
        if status == Status::StreamEnd || consumed_all || stalled {
            return Ok(());
        }
    }
}
