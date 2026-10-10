//! cdrdao `.toc` files (as written by `cdrdao read-cd`) over raw `.bin` data files.
//!
//! Recognised statements (comments `//` ignored):
//! * `TRACK <mode> [RW|RW_RAW]` — `AUDIO` (2352), `MODE1` / `MODE2_FORM1` (2048),
//!   `MODE1_RAW` / `MODE2_RAW` (2352), `MODE2` / `MODE2_FORM_MIX` (2336); a subchannel
//!   mode adds 96 bytes per sector.
//! * `DATAFILE "<file>" [#<byte offset>] [<length>]`
//! * `FILE|AUDIOFILE "<file>" [#<byte offset>] <start> [<length>]` — `start`/`length` as
//!   `mm:ss:ff` or a sample count; a `.wav` file skips its 44-byte header.
//! * `START [mm:ss:ff]` — the pregap length at the start of this track's data.
//! * `SWAP` — audio samples are little-endian (cdrdao's raw default is big-endian).
//!
//! Without an explicit length a track runs to the next track's data in the same file
//! (or to the end of the file).

use std::io;

use crate::disc::{bad, Backing, Track, TrackKind, SUB};
use crate::sector::{RawFormat, TrackMode};

fn msf(s: &str) -> Option<u64> {
    let mut it = s.split(':');
    let (m, sec, f) = (it.next()?.parse::<u64>().ok()?, it.next()?.parse::<u64>().ok()?, it.next()?.parse::<u64>().ok()?);
    Some((m * 60 + sec) * 75 + f)
}

/// A length/position in sectors: `mm:ss:ff`, or a sample count (588 per sector).
fn sectors_of(s: &str) -> Option<u64> {
    if s.contains(':') {
        msf(s)
    } else {
        s.parse::<u64>().ok().map(|n| n / 588)
    }
}

#[derive(Default, Debug)]
struct Pending {
    kind: Option<TrackKind>,
    sub: bool,
    file: Option<String>,
    offset: Option<u64>,
    length: Option<u64>,
    start: u64,
    swap: bool,
}

/// Splits a line into words, keeping quoted strings together (quotes removed).
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in line.chars() {
        match c {
            '"' => {
                if q {
                    out.push(std::mem::take(&mut cur));
                }
                q = !q;
            }
            c if c.is_whitespace() && !q => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn tracks(text: &str, resolve: &mut dyn FnMut(&str) -> io::Result<Backing>) -> io::Result<Vec<Track>> {
    let mut pend: Vec<Pending> = Vec::new();
    for raw in text.lines() {
        let line = raw.split("//").next().unwrap_or("").trim();
        let w = words(line);
        let Some(cmd) = w.first() else { continue };
        match cmd.to_ascii_uppercase().as_str() {
            "TRACK" => {
                let mode = w.get(1).map(|s| s.to_ascii_uppercase()).unwrap_or_default();
                let kind = match mode.as_str() {
                    "AUDIO" => TrackKind::Audio,
                    "MODE1" => TrackKind::Data(RawFormat::Cooked2048, TrackMode::Mode1),
                    "MODE2_FORM1" => TrackKind::Data(RawFormat::Cooked2048, TrackMode::Mode2),
                    "MODE1_RAW" => TrackKind::Data(RawFormat::Raw2352, TrackMode::Mode1),
                    "MODE2_RAW" => TrackKind::Data(RawFormat::Raw2352, TrackMode::Mode2),
                    "MODE2" | "MODE2_FORM_MIX" => TrackKind::Data(RawFormat::Raw2336, TrackMode::Mode2),
                    m => return Err(bad(format!("toc: unsupported track mode {m}"))),
                };
                let sub = w.get(2).is_some_and(|s| s.to_ascii_uppercase().starts_with("RW"));
                let kind = match (kind, sub) {
                    (TrackKind::Data(RawFormat::Raw2352, m), true) => TrackKind::Data(RawFormat::Raw2448, m),
                    (k, _) => k,
                };
                pend.push(Pending { kind: Some(kind), sub, ..Default::default() });
            }
            "DATAFILE" | "FILE" | "AUDIOFILE" => {
                let t = pend.last_mut().ok_or_else(|| bad("toc: file before TRACK"))?;
                let mut it = w.iter().skip(1);
                t.file = it.next().cloned();
                let mut rest: Vec<&String> = it.collect();
                if let Some(o) = rest.first().and_then(|s| s.strip_prefix('#')) {
                    t.offset = Some(o.parse().map_err(|_| bad("toc: bad #offset"))?);
                    rest.remove(0);
                }
                if cmd.eq_ignore_ascii_case("DATAFILE") {
                    t.length = rest.first().and_then(|s| sectors_of(s));
                } else {
                    let start = rest.first().and_then(|s| sectors_of(s)).unwrap_or(0);
                    t.length = rest.get(1).and_then(|s| sectors_of(s));
                    // A start position inside the file adds to the byte offset.
                    let stride = 2352 + if t.sub { SUB } else { 0 };
                    t.offset = Some(t.offset.unwrap_or(0) + start * stride);
                }
                if t.file.as_deref().is_some_and(|f| f.to_lowercase().ends_with(".wav")) {
                    t.offset = Some(t.offset.unwrap_or(0) + 44);
                    t.swap = true;
                }
            }
            "START" => {
                if let Some(t) = pend.last_mut() {
                    t.start = w.get(1).and_then(|s| sectors_of(s)).unwrap_or(0);
                }
            }
            "SWAP" => {
                if let Some(t) = pend.last_mut() {
                    t.swap = true;
                }
            }
            _ => {}
        }
    }
    // Resolve offsets: files without #offset continue where the previous track of the same file ended.
    let mut out: Vec<Track> = Vec::new();
    let mut cursor: Vec<(String, u64)> = Vec::new();
    let n = pend.len();
    for i in 0..n {
        let p = &pend[i];
        let file = p.file.clone().ok_or_else(|| bad(format!("toc: track {} has no data file", i + 1)))?;
        let kind = p.kind.unwrap();
        let backing = resolve(&file)?;
        let stride = match kind {
            TrackKind::Data(f, _) => f.stride() as u64,
            TrackKind::Audio => 2352 + if p.sub { SUB } else { 0 },
        };
        let key = file.to_lowercase();
        let at = p.offset.unwrap_or_else(|| cursor.iter().find(|c| c.0 == key).map_or(0, |c| c.1));
        let total = match p.length {
            Some(l) => l,
            None => {
                let next = pend[i + 1..].iter().find(|q| q.file.as_deref().map(str::to_lowercase) == Some(key.clone()) && q.offset.is_some());
                let end = match next.and_then(|q| q.offset) {
                    Some(o) => o,
                    None => backing.len()?,
                };
                end.saturating_sub(at) / stride
            }
        };
        let end = at + total * stride;
        match cursor.iter_mut().find(|c| c.0 == key) {
            Some(c) => c.1 = end,
            None => cursor.push((key, end)),
        }
        let pregap = p.start.min(total);
        let mut t = Track::new(i as u32 + 1, kind, backing, at + pregap * stride, (total - pregap) as u32);
        t.pregap = pregap as u32;
        t.sub = p.sub && kind == TrackKind::Audio;
        // cdrdao raw audio is big-endian; SWAP (or a WAV file) means little-endian.
        t.swap = kind == TrackKind::Audio && !p.swap;
        out.push(t);
    }
    Ok(out)
}

/// One track for [`write_toc`].
pub struct TocOut {
    pub audio: bool,
    pub offset: u64,
    pub pregap: u32,
    /// Total sectors in the file for this track (pregap included).
    pub sectors: u32,
}

/// Writes a `cdrdao read-cd`-style toc file over one data file (`MODE1_RAW` + `AUDIO`).
pub fn write_toc(file: &str, tracks: &[TocOut]) -> String {
    let m = |n: u32| format!("{:02}:{:02}:{:02}", n / 4500, (n / 75) % 60, n % 75);
    let mut s = String::from(if tracks.iter().all(|t| t.audio) { "CD_DA\n\n" } else { "CD_ROM\n\n" });
    for (i, t) in tracks.iter().enumerate() {
        s += &format!("// Track {}\n", i + 1);
        if t.audio {
            s += "TRACK AUDIO\nNO COPY\nNO PRE_EMPHASIS\nTWO_CHANNEL_AUDIO\n";
            s += &format!("FILE \"{file}\" #{} 0 {}\n", t.offset, m(t.sectors));
        } else {
            s += "TRACK MODE1_RAW\nNO COPY\n";
            s += &format!("DATAFILE \"{file}\" #{} {} // length in bytes: {}\n", t.offset, m(t.sectors), t.sectors as u64 * 2352);
        }
        if t.pregap > 0 {
            s += &format!("START {}\n", m(t.pregap));
        }
        s += "\n";
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_toc_big_endian_audio() {
        let dir = std::env::temp_dir().join(format!("f2toc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut data = vec![0u8; 8 * 2352];
        data[..12].copy_from_slice(&crate::sector::SYNC);
        data[15] = 1;
        // Audio: 2 pregap + 4 sectors; sample bytes [0x01, 0x02] stored big-endian.
        for p in data[2 * 2352..].chunks_exact_mut(2) {
            p.copy_from_slice(&[0x02, 0x01]);
        }
        std::fs::write(dir.join("data.bin"), &data).unwrap();
        let toc = write_toc(
            "data.bin",
            &[TocOut { audio: false, offset: 0, pregap: 0, sectors: 2 }, TocOut { audio: true, offset: 2 * 2352, pregap: 2, sectors: 6 }],
        );
        std::fs::write(dir.join("data.toc"), &toc).unwrap();
        let d = crate::Disc::open(&dir.join("data.bin")).unwrap();
        assert_eq!(d.container, "toc/bin");
        assert_eq!(d.tracks.len(), 2);
        assert_eq!((d.tracks[0].sectors, d.tracks[1].pregap, d.tracks[1].sectors), (2, 2, 4));
        assert!(d.tracks[1].swap);
        let mut a = Vec::new();
        d.stream_audio(&d.tracks[1], &mut |b| {
            a.extend_from_slice(b);
            Ok(())
        })
        .unwrap();
        assert_eq!(a.len(), 4 * 2352);
        assert_eq!(&a[..2], &[0x01, 0x02]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_read_cd_output_without_offsets() {
        let text = "CD_ROM\nTRACK MODE1_RAW\nDATAFILE \"d.bin\" 00:00:10 // x\nTRACK AUDIO\nSWAP\nFILE \"d.bin\" #23520 0 00:00:05\nSTART 00:00:02\n";
        let dir = std::env::temp_dir().join(format!("f2toc2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("d.bin"), vec![0u8; 15 * 2352]).unwrap();
        let mut r = |f: &str| Ok(Backing::File(dir.join(f)));
        let t = tracks(text, &mut r).unwrap();
        assert_eq!((t[0].sectors, t[1].start, t[1].sectors, t[1].pregap, t[1].swap), (10, 12 * 2352, 3, 2, false));
        std::fs::remove_dir_all(&dir).ok();
    }
}
