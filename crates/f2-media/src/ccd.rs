//! CloneCD `.ccd` control files (`.img` raw 2352-byte sectors from LBA 0, `.sub` ignored).
//!
//! The `.ccd` is an INI file. `[TRACK n]` sections give `MODE=` (0 audio, 1 Mode 1,
//! 2 Mode 2) and `INDEX 0=` / `INDEX 1=` as absolute LBAs; the lead-out comes from the
//! `[Entry n]` with `Point=0xa2` (`PLBA=`). The `.img` stores every sector of the
//! program area contiguously, so a track's byte offset is `LBA * 2352`.

use std::collections::BTreeMap;
use std::io;

use crate::disc::{bad, Backing, Track, TrackKind, RAW};
use crate::sector::{RawFormat, TrackMode};

/// Parsed sections: lowercase section name → lowercase key → value.
pub fn parse_ini(text: &str) -> Vec<(String, BTreeMap<String, String>)> {
    let mut out: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.push((s.trim().to_lowercase(), BTreeMap::new()));
        } else if let (Some((k, v)), Some(sec)) = (line.split_once('='), out.last_mut()) {
            sec.1.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    out
}

fn num(v: &str) -> Option<i64> {
    let v = v.trim();
    match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(h) => i64::from_str_radix(h, 16).ok(),
        None => v.parse().ok(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CcdTrack {
    pub number: u32,
    pub mode: u32,
    pub index0: Option<u32>,
    pub index1: u32,
}

/// Tracks and lead-out LBA (if a 0xA2 entry exists).
pub fn parse(text: &str) -> io::Result<(Vec<CcdTrack>, Option<u32>)> {
    let ini = parse_ini(text);
    if !ini.iter().any(|(s, _)| s == "clonecd") {
        return Err(bad("not a CloneCD control file"));
    }
    let mut tracks = Vec::new();
    let mut leadout = None;
    for (sec, kv) in &ini {
        if let Some(n) = sec.strip_prefix("track ") {
            let number: u32 = n.trim().parse().map_err(|_| bad(format!("bad section [{sec}]")))?;
            let mode = kv.get("mode").and_then(|v| num(v)).unwrap_or(1) as u32;
            let index1 = kv.get("index 1").and_then(|v| num(v)).ok_or_else(|| bad(format!("[{sec}] has no INDEX 1")))?;
            let index0 = kv.get("index 0").and_then(|v| num(v)).map(|v| v as u32);
            tracks.push(CcdTrack { number, mode, index0, index1: index1 as u32 });
        } else if sec.starts_with("entry ") && kv.get("point").and_then(|v| num(v)) == Some(0xa2) {
            leadout = kv.get("plba").and_then(|v| num(v)).map(|v| v as u32);
        }
    }
    tracks.sort_by_key(|t| t.number);
    Ok((tracks, leadout))
}

/// Builds the disc tracks over the `.img` backing.
pub fn tracks(text: &str, img: Backing) -> io::Result<Vec<Track>> {
    let (ct, leadout) = parse(text)?;
    let img_sectors = (img.len()? / RAW) as u32;
    let end_all = leadout.unwrap_or(img_sectors).min(img_sectors);
    let mut out = Vec::new();
    for (i, t) in ct.iter().enumerate() {
        let kind = match t.mode {
            0 => TrackKind::Audio,
            2 => TrackKind::Data(RawFormat::Raw2352, TrackMode::Mode2),
            _ => TrackKind::Data(RawFormat::Raw2352, TrackMode::Mode1),
        };
        let end = ct.get(i + 1).map_or(end_all, |n| n.index0.unwrap_or(n.index1));
        let mut tr = Track::new(t.number, kind, img.clone(), u64::from(t.index1) * RAW, end.saturating_sub(t.index1));
        tr.pregap = t.index0.map_or(0, |i0| t.index1.saturating_sub(i0));
        out.push(tr);
    }
    Ok(out)
}

/// One track for [`write_ccd`]: number, audio?, INDEX 0 and INDEX 1 LBAs.
pub struct CcdOut {
    pub number: u32,
    pub audio: bool,
    pub index0: Option<u32>,
    pub index1: u32,
}

/// Writes a CloneCD control file in the layout CloneCD 5 produces (single session).
pub fn write_ccd(tracks: &[CcdOut], leadout: u32) -> String {
    let mut s = String::new();
    let entries = tracks.len() + 3;
    s += &format!("[CloneCD]\r\nVersion=3\r\n[Disc]\r\nTocEntries={entries}\r\nSessions=1\r\nDataTracksScrambled=0\r\nCDTextLength=0\r\n");
    s += "[Session 1]\r\nPreGapMode=1\r\nPreGapSubC=0\r\n";
    let msf = |lba: i64| {
        let a = lba + 150;
        (a / 4500, (a / 75) % 60, a % 75)
    };
    let first_ctl = if tracks.first().is_some_and(|t| t.audio) { 0 } else { 4 };
    let entry = |s: &mut String, n: usize, point: u32, ctl: u32, plba: i64, pmsf: (i64, i64, i64)| {
        *s += &format!(
            "[Entry {n}]\r\nSession=1\r\nPoint=0x{point:02x}\r\nADR=0x01\r\nControl=0x{ctl:02x}\r\nTrackNo=0\r\nAMin=0\r\nASec=0\r\nAFrame=0\r\nALBA=-150\r\nZero=0\r\nPMin={}\r\nPSec={}\r\nPFrame={}\r\nPLBA={plba}\r\n",
            pmsf.0, pmsf.1, pmsf.2
        );
    };
    let first = tracks.first().map_or(1, |t| t.number) as i64;
    let last = tracks.last().map_or(1, |t| t.number) as i64;
    entry(&mut s, 0, 0xa0, first_ctl, first * 4500 - 150, (first, 0, 0));
    entry(&mut s, 1, 0xa1, if tracks.last().is_some_and(|t| t.audio) { 0 } else { 4 }, last * 4500 - 150, (last, 0, 0));
    entry(&mut s, 2, 0xa2, first_ctl, leadout as i64, msf(leadout as i64));
    for (i, t) in tracks.iter().enumerate() {
        entry(&mut s, i + 3, t.number, if t.audio { 0 } else { 4 }, t.index1 as i64, msf(t.index1 as i64));
    }
    for t in tracks {
        s += &format!("[TRACK {}]\r\nMODE={}\r\n", t.number, if t.audio { 0 } else { 1 });
        if let Some(i0) = t.index0 {
            s += &format!("INDEX 0={i0}\r\n");
        }
        s += &format!("INDEX 1={}\r\n", t.index1);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_ccd_round_trip() {
        let dir = std::env::temp_dir().join(format!("f2ccd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 10 data sectors, 3 pregap + 7 audio, 2 pregap + 5 audio.
        let mut img = vec![0u8; 27 * 2352];
        img[..12].copy_from_slice(&crate::sector::SYNC);
        img[15] = 1;
        for (i, b) in img[13 * 2352..].iter_mut().enumerate() {
            *b = i as u8;
        }
        std::fs::write(dir.join("game.img"), &img).unwrap();
        let ccd = write_ccd(
            &[
                CcdOut { number: 1, audio: false, index0: None, index1: 0 },
                CcdOut { number: 2, audio: true, index0: Some(10), index1: 13 },
                CcdOut { number: 3, audio: true, index0: Some(20), index1: 22 },
            ],
            27,
        );
        std::fs::write(dir.join("game.ccd"), &ccd).unwrap();
        let d = crate::Disc::open(&dir.join("GAME.img")).unwrap();
        assert_eq!(d.container, "ccd/img");
        assert_eq!(d.tracks.len(), 3);
        assert_eq!((d.tracks[0].sectors, d.tracks[1].sectors, d.tracks[2].sectors), (10, 7, 5));
        assert_eq!((d.tracks[1].pregap, d.tracks[2].pregap), (3, 2));
        assert_eq!(d.tracks[1].kind, TrackKind::Audio);
        let mut first = None;
        d.stream_audio(&d.tracks[1], &mut |b| {
            first.get_or_insert(b[0]);
            Ok(())
        })
        .unwrap();
        assert_eq!(first, Some(0));
        let (_, lo) = parse(&ccd).unwrap();
        assert_eq!(lo, Some(27));
        std::fs::remove_dir_all(&dir).ok();
    }
}
