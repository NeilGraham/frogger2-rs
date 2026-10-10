//! Ripping the music from an optical drive with read-offset detection.
//!
//! Drives return CD-DA shifted by their *read offset* (a few to ~1800 samples), and the
//! pressing has a *write offset* (EU: -12); Redump tracks are corrected for both. We read
//! the whole audio area once (plus a margin), then search the sample shift that makes a
//! short track match its Redump hash ([`f2_media::cdrom::find_offset`]). With the shift
//! known every track is cut exactly like the Redump image, so a verified drive rip
//! produces the same `music/` files as importing the Redump image.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use f2_media::cdrom::{find_offset, read_area, CdReader, Toc, SAMPLES_PER_SECTOR};
use f2_media::disc::RAW;
use sha1::{Digest, Sha1};

use crate::{log, write_music_track, Options, Progress, RipReport, KNOWN_TRACKS, MUSIC_TRACKS, REDUMP_PREGAPS};

/// Largest offset (samples) searched; the read margin covers it.
pub const MAX_OFFSET: i32 = 3000;
const MARGIN: u32 = (MAX_OFFSET as u32).div_ceil(SAMPLES_PER_SECTOR as u32) + 1;

fn pregap(n: u8) -> u32 {
    REDUMP_PREGAPS.get(n as usize).copied().unwrap_or(0)
}

/// (Redump range start, INDEX 01, end) in LBAs of TOC track index `i`.
fn ranges(toc: &Toc, i: usize) -> (u32, u32, u32) {
    let t = &toc.tracks[i];
    let end = match toc.tracks.get(i + 1) {
        Some(n) => n.lba - pregap(n.number),
        None => toc.leadout,
    };
    (t.lba - pregap(t.number), t.lba, end)
}

fn sha(b: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(b);
    f2_media::vfs::hex(&h.finalize())
}

/// Rips tracks 2–12 into `music/`; returns what happened. `rendition` selects the hashes.
pub fn rip(r: &mut dyn CdReader, rendition: &str, tmp_root: &Path, opts: &Options, files: &mut BTreeMap<String, String>, out: &mut Vec<String>) -> io::Result<RipReport> {
    let toc = r.read_toc()?;
    let mut rep = RipReport { drive: r.describe(), ..Default::default() };
    let music: Vec<usize> = (0..toc.tracks.len()).filter(|&i| toc.tracks[i].audio && MUSIC_TRACKS.iter().any(|m| m.0 == u32::from(toc.tracks[i].number))).collect();
    let Some(&first) = music.first() else {
        log("The disc in the drive has no audio tracks");
        return Ok(rep);
    };
    let start = i64::from(ranges(&toc, first).0) - i64::from(MARGIN);
    let count = (i64::from(toc.leadout) + i64::from(MARGIN) - start) as u32;
    log(format!("Reading CD audio from {} ({} sectors, {:.0} MB)", rep.drive, count, count as f64 * RAW as f64 / 1e6));
    let mut area = Vec::with_capacity(count as usize * RAW as usize);
    let mut prog = Progress::new("read CD audio", u64::from(count));
    let st = read_area(r, start, count, toc.leadout, &mut |b| {
        area.extend_from_slice(b);
        Ok(())
    }, &mut |d| prog.set(u64::from(d)))?;
    prog.done();
    rep.retried_sectors = st.retried;
    rep.unreadable_sectors = st.unreadable;
    if st.unreadable > 0 {
        log(format!("Warning: {} sectors could not be read and were replaced by silence (dirty or scratched disc?)", st.unreadable));
    } else if st.retried > 0 {
        log(format!("Recovered {} sectors after read errors", st.retried));
    }
    let pos = |lba: u32| ((i64::from(lba) - start) * RAW as i64) as usize;
    let known = KNOWN_TRACKS.iter().find(|k| k.0 == rendition).map(|k| k.1);

    // Offset: forced, or detected on up to three of the shortest tracks.
    match (opts.drive_offset, known) {
        (Some(o), _) => rep.offset = o,
        (None, Some(k)) => {
            let mut cands: Vec<usize> = music.clone();
            cands.sort_by_key(|&i| {
                let (a, _, e) = ranges(&toc, i);
                e - a
            });
            log("Detecting the drive read offset");
            for &i in cands.iter().take(3) {
                let (a, _, e) = ranges(&toc, i);
                let want = k[toc.tracks[i].number as usize - 1];
                if let Some(o) = find_offset(&area, pos(a), (e - a) as usize * RAW as usize, want, MAX_OFFSET) {
                    rep.offset = o;
                    rep.offset_detected = true;
                    log(format!("Read offset {o:+} samples (track {:02} matches Redump)", toc.tracks[i].number));
                    break;
                }
            }
            if !rep.offset_detected {
                log("Could not match any track against the known Redump hashes; ripping with offset 0 (unverified)");
            }
        }
        (None, None) => log("Unknown disc version: no reference hashes, ripping with offset 0 (unverified)"),
    }
    let shift = |p: usize| (p as i64 + i64::from(rep.offset) * 4) as usize;

    for &i in &music {
        let n = toc.tracks[i].number;
        let (a, b, e) = ranges(&toc, i);
        if let Some(k) = known {
            if sha(&area[shift(pos(a))..shift(pos(e))]) == k[n as usize - 1] {
                rep.verified_tracks.push(u32::from(n));
            }
        }
        let pcm = &area[shift(pos(b))..shift(pos(e))];
        let rel = write_music_track(tmp_root, u32::from(n), pcm.len() as u64, opts.flac, files, &mut |w| {
            for c in pcm.chunks(1 << 20) {
                w(c)?;
            }
            Ok(())
        })?;
        let cue = MUSIC_TRACKS.iter().find(|m| m.0 == u32::from(n)).map_or("", |m| m.1);
        let ok = if rep.verified_tracks.contains(&u32::from(n)) { "verified" } else { "unverified" };
        log(format!("Ripped track {n:02} ({cue}, {:.1}s, {ok})", pcm.len() as f64 / 176_400.0));
        out.push(rel);
    }
    Ok(rep)
}
