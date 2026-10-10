// Parser adapted from theme-park-world-recomp (crates/tpw-import/src/mds.rs), MIT, same author;
// extended with audio tracks, pregaps, RAW+SUB and a writer.
//! Alcohol 120% MDS (media descriptor, version 1.x) + MDF image data.
//!
//! Layout (little endian): header 0x58 bytes (`MEDIA DESCRIPTOR`, version at 0x10,
//! session count at 0x14, session table offset at 0x50); session blocks of 24 bytes
//! (start/end LBA, total blocks at +10, track-block offset at +20); track blocks of
//! 80 bytes: mode +0 (0xA9 audio, 0xAA Mode 1, 0xAB–0xAD Mode 2), subchannel +1 (8 =
//! 96 interleaved bytes), point +4, extra offset +12, sector size +16, start LBA +36,
//! start byte offset in the MDF (u64) +40, file count +48, footer offset +52. The extra
//! block holds pregap(u32) and length(u32); the footer points at the file name
//! (`*.mdf` = same stem as the descriptor).
//!
//! The MDF stores each track's pregap (except track 1's 150-sector lead-in gap) in front
//! of its INDEX 01 sector; `start_offset` points at the stored pregap. Images where it
//! points at INDEX 01 instead (`start_offset == start_lba * size` in a contiguous MDF)
//! are recognised too.

use std::io;

use crate::disc::{bad, Backing, Track, TrackKind};
use crate::sector::{RawFormat, TrackMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdsTrack {
    pub point: u8,
    pub mode: u8,
    pub subchannel: u8,
    pub sector_size: u16,
    pub start_sector: u32,
    pub start_offset: u64,
    /// Pregap and length in sectors (from the extra block), if present.
    pub pregap: Option<u32>,
    pub length: Option<u32>,
    /// Image file name from the footer (`*.mdf` means "same stem").
    pub file: Option<String>,
}

impl MdsTrack {
    pub fn is_audio(&self) -> bool {
        self.mode & 0x0f == 0x09
    }

    /// Stored format + default data mode for this (data) track.
    pub fn layout(&self) -> Option<(RawFormat, TrackMode)> {
        let fmt = match self.sector_size {
            2048 => RawFormat::Cooked2048,
            2336 => RawFormat::Raw2336,
            2352 => RawFormat::Raw2352,
            2448 => RawFormat::Raw2448,
            _ => return None,
        };
        let mode = match self.mode & 0x0f {
            0x0b..=0x0d => TrackMode::Mode2,
            _ => TrackMode::Mode1,
        };
        Some((fmt, mode))
    }
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Parses all real tracks (point 1..=99) of all sessions.
pub fn parse(b: &[u8]) -> Option<Vec<MdsTrack>> {
    if b.get(..16)? != b"MEDIA DESCRIPTOR" {
        return None;
    }
    let nsess = u16_at(b, 0x14)? as usize;
    let sess_off = u32_at(b, 0x50)? as usize;
    let mut out = Vec::new();
    for s in 0..nsess.min(99) {
        let so = sess_off + s * 24;
        let nblocks = *b.get(so + 10)? as usize;
        let toff = u32_at(b, so + 20)? as usize;
        for t in 0..nblocks {
            let o = toff + t * 80;
            let blk = b.get(o..o + 80)?;
            let point = blk[4];
            if point == 0 || point > 99 {
                continue;
            }
            let extra = u32_at(blk, 12)? as usize;
            let (pregap, length) = if extra != 0 { (u32_at(b, extra), u32_at(b, extra + 4)) } else { (None, None) };
            let footer = u32_at(blk, 52)? as usize;
            let file = (footer != 0)
                .then(|| {
                    let fo = u32_at(b, footer)? as usize;
                    let wide = u32_at(b, footer + 4)? != 0;
                    let name = if wide {
                        let units: Vec<u16> = b.get(fo..)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
                        String::from_utf16_lossy(&units)
                    } else {
                        String::from_utf8_lossy(b.get(fo..)?.split(|&c| c == 0).next()?).into_owned()
                    };
                    Some(name)
                })
                .flatten();
            out.push(MdsTrack {
                point,
                mode: blk[0],
                subchannel: blk[1],
                sector_size: u16_at(blk, 16)?,
                start_sector: u32_at(blk, 36)?,
                start_offset: u64_at(blk, 40)?,
                pregap,
                length,
                file,
            });
        }
    }
    Some(out)
}

/// Builds disc tracks; `stem` is the descriptor's file stem (for `*.mdf`).
pub fn tracks(bytes: &[u8], stem: &str, resolve: &mut dyn FnMut(&str) -> io::Result<Backing>) -> io::Result<Vec<Track>> {
    let mt = parse(bytes).ok_or_else(|| bad("not an Alcohol MDS descriptor"))?;
    let mut out = Vec::new();
    for (i, t) in mt.iter().enumerate() {
        let name = match t.file.as_deref() {
            None | Some("") => format!("{stem}.mdf"),
            Some(f) if f.starts_with("*.") => format!("{stem}{}", &f[1..]),
            Some(f) => f.to_string(),
        };
        let backing = resolve(&name)?;
        let size = u64::from(t.sector_size);
        let (kind, sub) = if t.is_audio() {
            match t.sector_size {
                2352 => (TrackKind::Audio, false),
                2448 => (TrackKind::Audio, true),
                s => return Err(bad(format!("MDS audio track {} has sector size {s}", t.point))),
            }
        } else {
            let (f, m) = t.layout().ok_or_else(|| bad(format!("MDS track {} has sector size {}", t.point, t.sector_size)))?;
            (TrackKind::Data(f, m), false)
        };
        // Stored pregap: track 1's lead-in gap is never stored.
        let pregap = if t.point == 1 { 0 } else { t.pregap.unwrap_or(0) };
        let at_index1 = t.start_offset == u64::from(t.start_sector) * size;
        let stored_pregap = if at_index1 { pregap.min(t.start_sector) } else { pregap };
        let start = if at_index1 { t.start_offset } else { t.start_offset + u64::from(pregap) * size };
        let sectors = match t.length {
            Some(l) => l,
            None => {
                // Up to the next track's pregap, else the end of the file.
                let next = mt.get(i + 1).filter(|n| n.file == t.file);
                let end = match next {
                    Some(n) => u64::from(n.start_sector - n.pregap.unwrap_or(0)) * size,
                    None => backing.len()?,
                };
                (end.saturating_sub(start) / size) as u32
            }
        };
        let mut tr = Track::new(u32::from(t.point), kind, backing, start, sectors);
        tr.pregap = stored_pregap;
        tr.sub = sub;
        out.push(tr);
    }
    Ok(out)
}

/// One track for [`write_mds`].
pub struct MdsOut {
    pub number: u8,
    pub audio: bool,
    /// INDEX 01 LBA.
    pub lba: u32,
    /// Pregap sectors (stored in the MDF before INDEX 01, except for track 1).
    pub pregap: u32,
    /// Sectors from INDEX 01 to the end of the track.
    pub length: u32,
    /// Byte offset in the MDF of the first stored sector (pregap included).
    pub offset: u64,
}

/// Writes an MDS v1.3 descriptor (single session, `*.mdf` file name).
/// `sector_size` is 2352 or 2448 (RAW+SUB, subchannel flag 8).
pub fn write_mds(tracks: &[MdsOut], sector_size: u16, leadout: u32) -> Vec<u8> {
    let n = tracks.len();
    let sess = 0x58usize;
    let blocks = sess + 24;
    let nblocks = n + 3;
    let extra = blocks + nblocks * 80;
    let footer = extra + n * 8;
    let fname = footer + 16;
    let mut b = vec![0u8; fname + 6];
    b[..16].copy_from_slice(b"MEDIA DESCRIPTOR");
    b[0x10] = 1;
    b[0x11] = 3;
    b[0x14..0x16].copy_from_slice(&1u16.to_le_bytes());
    b[0x50..0x54].copy_from_slice(&(sess as u32).to_le_bytes());
    b[sess..sess + 4].copy_from_slice(&(-150i32).to_le_bytes());
    b[sess + 4..sess + 8].copy_from_slice(&(leadout as i32).to_le_bytes());
    b[sess + 8..sess + 10].copy_from_slice(&1u16.to_le_bytes());
    b[sess + 10] = nblocks as u8;
    b[sess + 11] = 3;
    b[sess + 12..sess + 14].copy_from_slice(&u16::from(tracks.first().map_or(1, |t| t.number)).to_le_bytes());
    b[sess + 14..sess + 16].copy_from_slice(&u16::from(tracks.last().map_or(1, |t| t.number)).to_le_bytes());
    b[sess + 20..sess + 24].copy_from_slice(&(blocks as u32).to_le_bytes());
    // A0/A1/A2 entries (point only; mode 0).
    for (i, p) in [0xa0u8, 0xa1, 0xa2].iter().enumerate() {
        let o = blocks + i * 80;
        b[o + 2] = 0x14;
        b[o + 4] = *p;
    }
    let sub = sector_size == 2448;
    for (i, t) in tracks.iter().enumerate() {
        let o = blocks + (i + 3) * 80;
        b[o] = if t.audio { 0xa9 } else { 0xaa };
        b[o + 1] = if sub { 8 } else { 0 };
        b[o + 2] = if t.audio { 0x10 } else { 0x14 };
        b[o + 4] = t.number;
        let a = t.lba + 150;
        b[o + 9] = (a / 4500) as u8;
        b[o + 10] = ((a / 75) % 60) as u8;
        b[o + 11] = (a % 75) as u8;
        let eo = extra + i * 8;
        b[o + 12..o + 16].copy_from_slice(&(eo as u32).to_le_bytes());
        b[o + 16..o + 18].copy_from_slice(&sector_size.to_le_bytes());
        b[o + 36..o + 40].copy_from_slice(&t.lba.to_le_bytes());
        b[o + 40..o + 48].copy_from_slice(&t.offset.to_le_bytes());
        b[o + 48..o + 52].copy_from_slice(&1u32.to_le_bytes());
        b[o + 52..o + 56].copy_from_slice(&(footer as u32).to_le_bytes());
        b[eo..eo + 4].copy_from_slice(&(if t.number == 1 { 150 } else { t.pregap }).to_le_bytes());
        b[eo + 4..eo + 8].copy_from_slice(&t.length.to_le_bytes());
    }
    b[footer..footer + 4].copy_from_slice(&(fname as u32).to_le_bytes());
    b[fname..fname + 5].copy_from_slice(b"*.mdf");
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth(dir: &std::path::Path, size: u16) -> Vec<u8> {
        let s = size as usize;
        // track 1: 4 data sectors; track 2: 2 pregap + 5 audio sectors.
        let mut mdf = vec![0u8; 11 * s];
        for k in 0..4 {
            mdf[k * s..k * s + 12].copy_from_slice(&crate::sector::SYNC);
            mdf[k * s + 15] = 1;
            mdf[k * s + 16..k * s + 16 + 2048].fill(0xd0 + k as u8);
        }
        for k in 4..11 {
            mdf[k * s..k * s + 2352].fill(k as u8);
            if size == 2448 {
                mdf[k * s + 2352..k * s + 2448].fill(0xee);
            }
        }
        std::fs::write(dir.join("Frog.mdf"), &mdf).unwrap();
        let mds = write_mds(
            &[
                MdsOut { number: 1, audio: false, lba: 0, pregap: 0, length: 4, offset: 0 },
                MdsOut { number: 2, audio: true, lba: 6, pregap: 2, length: 5, offset: 4 * s as u64 },
            ],
            size,
            11,
        );
        std::fs::write(dir.join("Frog.mds"), &mds).unwrap();
        mds
    }

    #[test]
    fn synthetic_mds_2352_and_raw_sub() {
        for size in [2352u16, 2448] {
            let dir = std::env::temp_dir().join(format!("f2mds-{}-{size}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let mds = synth(&dir, size);
            let t = parse(&mds).unwrap();
            assert_eq!(t.len(), 2);
            assert_eq!(t[1].pregap, Some(2));
            assert_eq!(t[1].file.as_deref(), Some("*.mdf"));
            let d = crate::Disc::open(&dir.join("Frog.mdf")).unwrap();
            assert_eq!(d.container, "mds/mdf");
            assert_eq!(d.tracks[1].pregap, 2);
            assert_eq!(d.tracks[1].sectors, 5);
            assert_eq!(d.tracks[1].sub, size == 2448);
            let mut audio = Vec::new();
            d.stream_audio(&d.tracks[1], &mut |b| {
                audio.extend_from_slice(b);
                Ok(())
            })
            .unwrap();
            assert_eq!(audio.len(), 5 * 2352);
            assert!(audio[..2352].iter().all(|&b| b == 6) && audio[4 * 2352..].iter().all(|&b| b == 10));
            let mut src = d.data_source().unwrap();
            let mut out = Vec::new();
            src.read_sectors(3, 1, &mut out).unwrap();
            assert!(out.iter().all(|&b| b == 0xd3));
            std::fs::remove_dir_all(&dir).ok();
        }
        assert!(parse(b"not an mds").is_none());
    }
}
