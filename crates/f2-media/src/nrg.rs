//! Nero `.nrg` images (v1 `NERO` and v2 `NER5` footers; DAO and TAO layouts).
//!
//! The track data comes first; a chain of big-endian chunks (`id[4]`, `size u32`) follows,
//! located by the footer: v2 ends with `NER5` + u64 chunk offset, v1 with `NERO` + u32.
//! * `DAOX` (v2) / `DAOI` (v1): 22-byte header (size, UPC[13], pad, toc type u16, first,
//!   last track) then per track: ISRC[12], sector size u16, mode u8, 3 unknown bytes and
//!   three file offsets (pregap/INDEX 00, INDEX 01, end) as u64 (DAOX) or u32 (DAOI).
//! * `ETN2` (v2) / `ETNF` (v1), TAO: per track offset, length, mode, start LBA, unknown.
//! * `CUEX`/`CUES`, `SINF`, `MTYP`, `END!` are skipped.
//!
//! Mode codes: 0x00 Mode 1/2048, 0x02 Mode 2 Form 1/2048, 0x03 Mode 2/2336, 0x05 Mode 1
//! raw, 0x06 Mode 2 raw, 0x07 audio (2352); 0x0f/0x10/0x11 the same with 96 bytes of
//! subchannel (2448). Audio samples are little-endian.

use std::io;

use crate::disc::{bad, Backing, Track, TrackKind};
use crate::sector::{RawFormat, TrackMode};

fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}
fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}
fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(b[o..o + 8].try_into().unwrap())
}

/// Track kind + subchannel flag for a Nero mode code and sector size.
fn kind_of(mode: u8, size: u16) -> io::Result<(TrackKind, bool)> {
    Ok(match (mode, size) {
        (0x07, 2352) => (TrackKind::Audio, false),
        (0x10, 2448) => (TrackKind::Audio, true),
        (0x00, _) => (TrackKind::Data(RawFormat::Cooked2048, TrackMode::Mode1), false),
        (0x02, _) => (TrackKind::Data(RawFormat::Cooked2048, TrackMode::Mode2), false),
        (0x03, _) => (TrackKind::Data(RawFormat::Raw2336, TrackMode::Mode2), false),
        (0x05, _) => (TrackKind::Data(RawFormat::Raw2352, TrackMode::Mode1), false),
        (0x06, _) => (TrackKind::Data(RawFormat::Raw2352, TrackMode::Mode2), false),
        (0x0f, _) => (TrackKind::Data(RawFormat::Raw2448, TrackMode::Mode1), false),
        (0x11, _) => (TrackKind::Data(RawFormat::Raw2448, TrackMode::Mode2), false),
        (m, s) => return Err(bad(format!("NRG: unsupported track mode 0x{m:02x} / sector size {s}"))),
    })
}

fn size_of_mode(mode: u8) -> u16 {
    match mode {
        0x00 | 0x02 => 2048,
        0x03 => 2336,
        0x0f..=0x11 => 2448,
        _ => 2352,
    }
}

/// Reads the chunk chain of an NRG backing and returns its tracks.
pub fn tracks(img: Backing) -> io::Result<Vec<Track>> {
    let len = img.len()?;
    if len < 12 {
        return Err(bad("NRG too small"));
    }
    let tail = img.read_at(len - 12, 12)?;
    let (v2, chunks_at) = if &tail[0..4] == b"NER5" {
        (true, be64(&tail, 4))
    } else if &tail[4..8] == b"NERO" {
        (false, u64::from(be32(&tail, 8)))
    } else {
        return Err(bad("not a Nero image (no NER5/NERO footer)"));
    };
    if chunks_at >= len {
        return Err(bad("NRG chunk offset beyond end of file"));
    }
    let chunks = img.read_at(chunks_at, (len - chunks_at) as usize)?;
    let mut out = Vec::new();
    let mut o = 0usize;
    while o + 8 <= chunks.len() {
        let id = &chunks[o..o + 4];
        let size = be32(&chunks, o + 4) as usize;
        let body = chunks.get(o + 8..o + 8 + size).ok_or_else(|| bad("NRG chunk overruns file"))?;
        match id {
            b"DAOX" | b"DAOI" => {
                let wide = id == b"DAOX";
                let per = if wide { 42 } else { 30 };
                if body.len() < 22 {
                    return Err(bad("NRG DAO chunk too small"));
                }
                let first = body[20] as u32;
                for (k, blk) in body[22..].chunks_exact(per).enumerate() {
                    let sector_size = be16(blk, 12);
                    let mode = blk[14];
                    let (i0, i1, end) = if wide {
                        (be64(blk, 18), be64(blk, 26), be64(blk, 34))
                    } else {
                        (u64::from(be32(blk, 18)), u64::from(be32(blk, 22)), u64::from(be32(blk, 26)))
                    };
                    let (kind, sub) = kind_of(mode, sector_size)?;
                    let stride = u64::from(sector_size);
                    let mut t = Track::new(first + k as u32, kind, img.clone(), i1, (end.saturating_sub(i1) / stride) as u32);
                    t.pregap = (i1.saturating_sub(i0) / stride) as u32;
                    t.sub = sub;
                    out.push(t);
                }
            }
            b"ETN2" | b"ETNF" => {
                let wide = id == b"ETN2";
                let per = if wide { 32 } else { 20 };
                for blk in body.chunks_exact(per) {
                    let (off, length, mode) =
                        if wide { (be64(blk, 0), be64(blk, 8), be32(blk, 16)) } else { (u64::from(be32(blk, 0)), u64::from(be32(blk, 4)), be32(blk, 8)) };
                    let size = size_of_mode(mode as u8);
                    let (kind, sub) = kind_of(mode as u8, size)?;
                    let mut t = Track::new(out.len() as u32 + 1, kind, img.clone(), off, (length / u64::from(size)) as u32);
                    t.sub = sub;
                    out.push(t);
                }
            }
            b"END!" => break,
            _ => {}
        }
        o += 8 + size;
    }
    let _ = v2;
    if out.is_empty() {
        return Err(bad("NRG has no DAO/TAO track information"));
    }
    Ok(out)
}

/// One track for [`write_nrg_tail`]: mode code, sector size and file offsets.
pub struct NrgOut {
    pub mode: u8,
    pub sector_size: u16,
    pub index0: u64,
    pub index1: u64,
    pub end: u64,
}

/// The chunk chain + footer to append after the track data (`data_len` bytes).
/// `v2` selects DAOX/`NER5` (else DAOI/`NERO`).
pub fn write_nrg_tail(tracks: &[NrgOut], data_len: u64, v2: bool) -> Vec<u8> {
    let mut c = Vec::new();
    let chunk = |c: &mut Vec<u8>, id: &[u8; 4], body: &[u8]| {
        c.extend_from_slice(id);
        c.extend_from_slice(&(body.len() as u32).to_be_bytes());
        c.extend_from_slice(body);
    };
    // CUEX/CUES: informational (not parsed back), one entry per index.
    let mut cue = Vec::new();
    for (i, t) in tracks.iter().enumerate() {
        let ctl = if t.mode == 0x07 || t.mode == 0x10 { 0x01 } else { 0x41 };
        let bcd = |n: u32| ((n / 10) << 4 | (n % 10)) as u8;
        for (idx, lba) in [(0u32, t.index0), (1, t.index1)] {
            cue.extend_from_slice(&[ctl, bcd(i as u32 + 1), bcd(idx), 0]);
            cue.extend_from_slice(&((lba / u64::from(t.sector_size)) as i32 - 150).to_be_bytes());
        }
    }
    chunk(&mut c, if v2 { b"CUEX" } else { b"CUES" }, &cue);
    let mut dao = vec![0u8; 22];
    let n = tracks.len() as u8;
    dao[20] = 1;
    dao[21] = n;
    for t in tracks {
        let mut blk = vec![0u8; 12];
        blk.extend_from_slice(&t.sector_size.to_be_bytes());
        blk.push(t.mode);
        blk.extend_from_slice(&[0, 0, 1]);
        for v in [t.index0, t.index1, t.end] {
            if v2 {
                blk.extend_from_slice(&v.to_be_bytes());
            } else {
                blk.extend_from_slice(&(v as u32).to_be_bytes());
            }
        }
        dao.extend_from_slice(&blk);
    }
    let dl = dao.len() as u32;
    dao[0..4].copy_from_slice(&dl.to_be_bytes());
    chunk(&mut c, if v2 { b"DAOX" } else { b"DAOI" }, &dao);
    chunk(&mut c, b"SINF", &(tracks.len() as u32).to_be_bytes());
    chunk(&mut c, b"END!", &[]);
    if v2 {
        c.extend_from_slice(b"NER5");
        c.extend_from_slice(&data_len.to_be_bytes());
    } else {
        c.extend_from_slice(b"NERO");
        c.extend_from_slice(&(data_len as u32).to_be_bytes());
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_nrg_v1_v2() {
        for v2 in [false, true] {
            let dir = std::env::temp_dir().join(format!("f2nrg-{}-{v2}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            // 3 data sectors (raw mode 1), then 2 pregap + 4 audio sectors.
            let mut data = vec![0u8; 9 * 2352];
            for k in 0..3 {
                data[k * 2352..k * 2352 + 12].copy_from_slice(&crate::sector::SYNC);
                data[k * 2352 + 15] = 1;
                data[k * 2352 + 16..k * 2352 + 2064].fill(0x40 + k as u8);
            }
            data[3 * 2352..].fill(0x77);
            data[5 * 2352] = 0x12;
            let tracks = [
                NrgOut { mode: 0x05, sector_size: 2352, index0: 0, index1: 0, end: 3 * 2352 },
                NrgOut { mode: 0x07, sector_size: 2352, index0: 3 * 2352, index1: 5 * 2352, end: 9 * 2352 },
            ];
            let len = data.len() as u64;
            data.extend_from_slice(&write_nrg_tail(&tracks, len, v2));
            let p = dir.join("frog.nrg");
            std::fs::write(&p, &data).unwrap();
            let d = crate::Disc::open(&p).unwrap();
            assert_eq!(d.tracks.len(), 2);
            assert_eq!(d.tracks[1].kind, TrackKind::Audio);
            assert_eq!((d.tracks[1].pregap, d.tracks[1].sectors), (2, 4));
            let mut a = Vec::new();
            d.stream_audio(&d.tracks[1], &mut |b| {
                a.extend_from_slice(b);
                Ok(())
            })
            .unwrap();
            assert_eq!((a.len(), a[0]), (4 * 2352, 0x12));
            let mut s = d.data_source().unwrap();
            let mut out = Vec::new();
            s.read_sectors(2, 1, &mut out).unwrap();
            assert!(out.iter().all(|&b| b == 0x42));
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn rejects_non_nrg() {
        let p = std::env::temp_dir().join(format!("f2nrg-bad-{}.nrg", std::process::id()));
        std::fs::write(&p, vec![0u8; 64]).unwrap();
        assert!(crate::Disc::open(&p).is_err());
        std::fs::remove_file(&p).ok();
    }
}
