//! Optical drives: enumeration, TOC, raw CD-DA reads and secure-ish ripping.
//!
//! * [`list_drives`] finds CD/DVD drives and whether a Frogger 2 disc is in them
//!   (volume label `FROGGER2` or `FROGGER2/DATA1.CAB`), and for Windows virtual drives
//!   (Explorer "Mount") the image file behind them.
//! * [`open_drive`] gives a [`CdReader`]: `IOCTL_CDROM_READ_TOC` / `IOCTL_CDROM_RAW_READ`
//!   on Windows, `CDROMREADTOCHDR`/`CDROMREADTOCENTRY`/`CDROMREADAUDIO` on Linux. All
//!   `unsafe` lives in the per-OS modules behind safe wrappers.
//! * [`read_area`] reads a sector range with retries (batch → single sectors after
//!   slowing the drive → zero fill), [`find_offset`] searches the sample offset that makes
//!   a ripped track match a known Redump hash (drives differ in read offset; Redump images
//!   are write-offset corrected).
//! * [`ImageDrive`] emulates a drive over a disc image (with a configurable read offset
//!   and injected read errors) so the whole rip path is testable without hardware.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use sha1::{Digest, Sha1};

use crate::disc::{Disc, TrackKind, RAW};
use crate::vfs::hex;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

/// Samples (stereo frames) per CD sector.
pub const SAMPLES_PER_SECTOR: i64 = 588;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TocTrack {
    pub number: u8,
    /// INDEX 01 LBA (0 = first sector of the program area).
    pub lba: u32,
    pub audio: bool,
}

/// A disc's table of contents.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Toc {
    pub tracks: Vec<TocTrack>,
    pub leadout: u32,
}

impl Toc {
    /// Sector after the last sector of track index `i` (next track's INDEX 01 or lead-out).
    pub fn end_of(&self, i: usize) -> u32 {
        self.tracks.get(i + 1).map_or(self.leadout, |t| t.lba)
    }

    pub fn track(&self, number: u8) -> Option<(usize, &TocTrack)> {
        self.tracks.iter().enumerate().find(|(_, t)| t.number == number)
    }

    pub fn audio_count(&self) -> usize {
        self.tracks.iter().filter(|t| t.audio).count()
    }

    /// Parses a Windows `CDROM_TOC` / MMC READ TOC format 0 response (4-byte header, then
    /// 8-byte descriptors with `[reserved, adr/ctl, track, reserved, addr[4]]`). `msf`
    /// selects MSF addresses (`[0, m, s, f]`) instead of big-endian LBAs.
    pub fn from_mmc(b: &[u8], msf: bool) -> io::Result<Toc> {
        if b.len() < 4 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "short TOC"));
        }
        let len = (u16::from_be_bytes([b[0], b[1]]) as usize + 2).min(b.len());
        let mut toc = Toc::default();
        for d in b[4..len].chunks_exact(8) {
            let addr = if msf { (u32::from(d[5]) * 60 + u32::from(d[6])) * 75 + u32::from(d[7]) - 150 } else { u32::from_be_bytes([d[4], d[5], d[6], d[7]]) };
            let audio = d[1] & 0x04 == 0;
            match d[2] {
                0xaa => toc.leadout = addr,
                n @ 1..=99 => toc.tracks.push(TocTrack { number: n, lba: addr, audio }),
                _ => {}
            }
        }
        if toc.tracks.is_empty() || toc.leadout == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "TOC has no tracks or no lead-out"));
        }
        Ok(toc)
    }
}

/// Raw access to an optical disc (or an emulation of one).
pub trait CdReader: Send {
    fn read_toc(&mut self) -> io::Result<Toc>;
    /// Reads `count` CD-DA sectors (2352 bytes each, little-endian samples) at `lba`.
    fn read_cdda(&mut self, lba: u32, count: u32, out: &mut [u8]) -> io::Result<()>;
    /// Largest `count` the device accepts in one read.
    fn max_batch(&self) -> u32 {
        20
    }
    /// Asks the drive to read more slowly (after read errors). Best effort.
    fn slow_down(&mut self) {}
    fn describe(&self) -> String;
}

/// What kind of drive a letter/device is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Physical,
    /// A virtual drive (Windows Explorer "Mount", Daemon Tools…). `image` is the backing
    /// file when it could be determined.
    Virtual { image: Option<PathBuf> },
}

/// One optical drive.
#[derive(Clone, Debug)]
pub struct DriveInfo {
    /// `D:` on Windows, `/dev/sr0` on Linux.
    pub device: String,
    /// Where its file system is visible (`D:\`, or a Linux mount point), if mounted.
    pub root: Option<PathBuf>,
    pub label: Option<String>,
    pub media: bool,
    /// The disc looks like Frogger 2 (label `FROGGER2` or `FROGGER2/DATA1.CAB` exists).
    pub frogger: bool,
    pub kind: DriveKind,
    /// Vendor/product string when known.
    pub model: String,
}

impl DriveInfo {
    pub fn image(&self) -> Option<&PathBuf> {
        match &self.kind {
            DriveKind::Virtual { image } => image.as_ref(),
            DriveKind::Physical => None,
        }
    }

    /// One-line description for menus and `--list-drives`.
    pub fn summary(&self) -> String {
        let mut s = format!("{} ", self.device);
        s += match (&self.media, &self.label) {
            (false, _) => "(no disc)".into(),
            (true, Some(l)) => format!("\"{l}\""),
            (true, None) => "(disc, no label)".into(),
        }
        .as_str();
        if self.frogger {
            s += " - Frogger 2 disc";
        }
        match &self.kind {
            DriveKind::Physical => {}
            DriveKind::Virtual { image: Some(p) } => s += &format!(" [mounted image {}]", p.display()),
            DriveKind::Virtual { image: None } => s += " [virtual drive]",
        }
        if !self.model.is_empty() {
            s += &format!(" ({})", self.model);
        }
        s
    }
}

pub(crate) fn looks_like_frogger(root: Option<&std::path::Path>, label: Option<&str>) -> bool {
    label.is_some_and(|l| l.trim().eq_ignore_ascii_case("FROGGER2"))
        || root.is_some_and(|r| {
            crate::disc::find_case_insensitive(r, "FROGGER2").and_then(|d| crate::disc::find_case_insensitive(&d, "DATA1.CAB")).is_ok()
        })
}

/// Lists optical drives (empty on platforms without support).
pub fn list_drives() -> Vec<DriveInfo> {
    #[cfg(windows)]
    {
        windows::list_drives()
    }
    #[cfg(target_os = "linux")]
    {
        linux::list_drives()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Vec::new()
    }
}

/// The drive a user-supplied path refers to: `D:`, `D:\`, `\\.\D:` (Windows),
/// `/dev/sr0` or a mount point of an optical drive (Linux).
pub fn drive_for_path(path: &std::path::Path) -> Option<DriveInfo> {
    let s = path.to_string_lossy().replace('/', "\\");
    let s = s.trim_start_matches("\\\\.\\").trim_end_matches('\\').to_ascii_uppercase();
    let dev_norm = |d: &str| d.replace('/', "\\").trim_end_matches('\\').to_ascii_uppercase();
    let canon = std::fs::canonicalize(path).ok();
    list_drives().into_iter().find(|d| {
        dev_norm(&d.device) == s || d.root.as_ref().is_some_and(|r| dev_norm(&r.to_string_lossy()) == s || canon.as_ref() == Some(r) || std::fs::canonicalize(r).ok() == canon && canon.is_some())
    })
}

/// Opens a drive for TOC and CD-DA reads.
pub fn open_drive(device: &str) -> io::Result<Box<dyn CdReader>> {
    #[cfg(windows)]
    {
        windows::open(device).map(|d| Box::new(d) as Box<dyn CdReader>)
    }
    #[cfg(target_os = "linux")]
    {
        linux::open(device).map(|d| Box::new(d) as Box<dyn CdReader>)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = device;
        Err(io::Error::new(io::ErrorKind::Unsupported, "raw CD access is not supported on this platform"))
    }
}

/// Read statistics of [`read_area`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadStats {
    /// Sectors re-read one by one (and recovered) after their batch failed.
    pub retried: u32,
    /// Sectors that could not be read and were zero-filled.
    pub unreadable: u32,
    /// Sectors outside the disc (before LBA 0 / after the lead-out) filled with silence.
    pub outside: u32,
}

/// Attempts per single sector after a batch failed.
pub const SECTOR_ATTEMPTS: u32 = 5;

/// Reads `count` sectors from `start` (which may lie before LBA 0 or reach past the
/// lead-out: those sectors are silence, matching how Redump pads offset-shifted
/// tracks). A failing batch is retried once, then the drive is slowed down and the
/// batch re-read sector by sector with [`SECTOR_ATTEMPTS`] attempts each; sectors
/// that still fail are zero-filled and counted. `progress` gets sectors done.
pub fn read_area(r: &mut dyn CdReader, start: i64, count: u32, leadout: u32, sink: &mut dyn FnMut(&[u8]) -> io::Result<()>, progress: &mut dyn FnMut(u32)) -> io::Result<ReadStats> {
    let mut st = ReadStats::default();
    let batch = r.max_batch().max(1);
    let mut buf = vec![0u8; batch as usize * RAW as usize];
    let mut done = 0u32;
    let mut slowed = false;
    while done < count {
        let lba = start + i64::from(done);
        if lba < 0 || lba >= i64::from(leadout) {
            // Silence outside the program area, one sector at a time.
            buf[..RAW as usize].fill(0);
            sink(&buf[..RAW as usize])?;
            st.outside += 1;
            done += 1;
            progress(done);
            continue;
        }
        let n = batch.min(count - done).min((i64::from(leadout) - lba) as u32);
        let out = &mut buf[..n as usize * RAW as usize];
        let ok = r.read_cdda(lba as u32, n, out).is_ok() || r.read_cdda(lba as u32, n, out).is_ok();
        if !ok {
            if !slowed {
                r.slow_down();
                slowed = true;
            }
            for k in 0..n {
                let sec = &mut out[k as usize * RAW as usize..(k as usize + 1) * RAW as usize];
                let mut got = false;
                for _ in 0..SECTOR_ATTEMPTS {
                    if r.read_cdda(lba as u32 + k, 1, sec).is_ok() {
                        got = true;
                        break;
                    }
                }
                if got {
                    st.retried += 1;
                } else {
                    sec.fill(0);
                    st.unreadable += 1;
                }
            }
        }
        sink(out)?;
        done += n;
        progress(done);
    }
    Ok(st)
}

/// Sample offsets tried first (0, then read offsets common in the AccurateRip drive
/// database, both signs); the full window is searched afterwards.
const COMMON_OFFSETS: &[i32] = &[0, 6, 12, 48, 91, 97, 102, 103, 116, 120, 564, 594, 667, 685, 696, 702, 738, 1164, 1176, 1858];

/// Finds the sample offset `o` (|o| ≤ `max`) such that the SHA-1 of `len` bytes starting
/// `o` samples after byte `at` of `area` is `want`. `area` must hold `max` samples of
/// margin on both sides of the track. Positive `o` = the drive returned audio late.
pub fn find_offset(area: &[u8], at: usize, len: usize, want: &str, max: i32) -> Option<i32> {
    let try_o = |o: i32| -> bool {
        let s = at as i64 + i64::from(o) * 4;
        if s < 0 || s as usize + len > area.len() {
            return false;
        }
        let mut h = Sha1::new();
        h.update(&area[s as usize..s as usize + len]);
        hex(&h.finalize()) == want
    };
    let mut tried = std::collections::HashSet::new();
    let order = COMMON_OFFSETS.iter().flat_map(|&o| [o, -o]).chain((1..=max).flat_map(|o| [o, -o]));
    for o in order {
        if o.abs() > max || !tried.insert(o) {
            continue;
        }
        if try_o(o) {
            return Some(o);
        }
    }
    None
}

/// A drive emulated over a disc image: tracks laid out back to back (pregaps included)
/// from LBA 0, like the program area of the pressed disc. `read_offset` shifts the audio
/// the way a real drive's read offset does (`+N` = samples arrive N late), and sectors in
/// `flaky` fail their first `n` reads.
pub struct ImageDrive {
    disc: Disc,
    /// (first absolute LBA of the stored range (pregap start), track index)
    layout: Vec<(u32, usize)>,
    toc: Toc,
    pub read_offset: i32,
    pub flaky: HashMap<u32, u32>,
    /// Sectors that never read.
    pub dead: Vec<u32>,
    pub reads: u32,
    pub slowed: bool,
}

impl ImageDrive {
    pub fn new(disc: Disc) -> io::Result<ImageDrive> {
        let mut lba = 0u32;
        let mut layout = Vec::new();
        let mut toc = Toc::default();
        for (i, t) in disc.tracks.iter().enumerate() {
            if !t.is_raw() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "ImageDrive needs raw (2352) tracks"));
            }
            layout.push((lba, i));
            toc.tracks.push(TocTrack { number: t.number as u8, lba: lba + t.pregap, audio: t.kind == TrackKind::Audio });
            lba += t.pregap + t.sectors;
        }
        toc.leadout = lba;
        Ok(ImageDrive { disc, layout, toc, read_offset: 0, flaky: HashMap::new(), dead: Vec::new(), reads: 0, slowed: false })
    }

    /// Copies true (offset-free) audio bytes from absolute byte position `pos` into `out`.
    fn true_bytes(&self, pos: i64, out: &mut [u8]) -> io::Result<()> {
        out.fill(0);
        let end_pos = pos + out.len() as i64;
        for &(first, ti) in &self.layout {
            let t = &self.disc.tracks[ti];
            let a = i64::from(first) * RAW as i64;
            let b = a + i64::from(t.pregap + t.sectors) * RAW as i64;
            let (lo, hi) = (pos.max(a), end_pos.min(b));
            if lo >= hi {
                continue;
            }
            // Byte `lo` of the disc is byte `lo - a` of the stored track (stride may include subchannel).
            let stride = t.stride() as i64;
            let base = t.start as i64 - i64::from(t.pregap) * stride;
            let mut p = lo;
            while p < hi {
                let rel = p - a;
                let (sec, within) = (rel / RAW as i64, rel % RAW as i64);
                let take = (RAW as i64 - within).min(hi - p) as usize;
                let src = t.backing.read_at((base + sec * stride + within) as u64, take)?;
                let o = (p - pos) as usize;
                out[o..o + take].copy_from_slice(&src);
                if t.swap {
                    // Byte order is per sample pair; `within` is even for aligned reads.
                    for q in out[o..o + take].chunks_exact_mut(2) {
                        q.swap(0, 1);
                    }
                }
                p += take as i64;
            }
        }
        Ok(())
    }
}

impl CdReader for ImageDrive {
    fn read_toc(&mut self) -> io::Result<Toc> {
        Ok(self.toc.clone())
    }

    fn read_cdda(&mut self, lba: u32, count: u32, out: &mut [u8]) -> io::Result<()> {
        self.reads += 1;
        for s in lba..lba + count {
            if self.dead.contains(&s) {
                return Err(io::Error::other(format!("emulated read error at {s}")));
            }
            if let Some(n) = self.flaky.get_mut(&s) {
                if *n > 0 {
                    *n -= 1;
                    return Err(io::Error::other(format!("emulated transient error at {s}")));
                }
            }
        }
        let pos = i64::from(lba) * RAW as i64 - i64::from(self.read_offset) * 4;
        self.true_bytes(pos, &mut out[..count as usize * RAW as usize])
    }

    fn slow_down(&mut self) {
        self.slowed = true;
    }

    fn describe(&self) -> String {
        format!("emulated drive over {} (read offset {:+})", self.disc.path.display(), self.read_offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disc::{Backing, Track};

    /// A 1-data + 2-audio disc with a counting sample pattern.
    fn synth_disc(dir: &std::path::Path) -> Disc {
        std::fs::create_dir_all(dir).unwrap();
        let mut img = vec![0u8; 40 * 2352];
        img[..12].copy_from_slice(&crate::sector::SYNC);
        for (i, s) in img[10 * 2352..].chunks_exact_mut(4).enumerate() {
            s.copy_from_slice(&(i as u32).wrapping_mul(2654435761).to_le_bytes());
        }
        let p = dir.join("x.img");
        std::fs::write(&p, &img).unwrap();
        let b = Backing::File(p.clone());
        let mut t2 = Track::new(2, TrackKind::Audio, b.clone(), 12 * 2352, 13);
        t2.pregap = 2;
        let mut t3 = Track::new(3, TrackKind::Audio, b.clone(), 27 * 2352, 13);
        t3.pregap = 2;
        Disc { path: p, container: "test".into(), tracks: vec![Track::new(1, TrackKind::Data(crate::sector::RawFormat::Raw2352, crate::sector::TrackMode::Mode1), b, 0, 10), t2, t3] }
    }

    fn sha(b: &[u8]) -> String {
        let mut h = Sha1::new();
        h.update(b);
        hex(&h.finalize())
    }

    #[test]
    fn toc_from_mmc_msf_and_lba() {
        // 2 tracks + lead-out, MSF.
        let mut b = vec![0, 26, 1, 2];
        b.extend_from_slice(&[0, 0x14, 1, 0, 0, 0, 2, 0]);
        b.extend_from_slice(&[0, 0x10, 2, 0, 0, 20, 44, 36]);
        b.extend_from_slice(&[0, 0x10, 0xaa, 0, 0, 45, 0, 0]);
        let t = Toc::from_mmc(&b, true).unwrap();
        assert_eq!(t.tracks[0], TocTrack { number: 1, lba: 0, audio: false });
        assert_eq!(t.tracks[1].lba, 93186);
        assert!(t.tracks[1].audio);
        assert_eq!(t.leadout, 45 * 4500 - 150);
        let mut l = vec![0, 18, 1, 1];
        l.extend_from_slice(&[0, 0x10, 1, 0, 0, 0, 0, 0]);
        l.extend_from_slice(&[0, 0x10, 0xaa, 0, 0, 0, 1, 0]);
        assert_eq!(Toc::from_mmc(&l, false).unwrap().leadout, 256);
    }

    #[test]
    fn emulated_drive_offset_detection_and_retries() {
        let dir = std::env::temp_dir().join(format!("f2cdrom-{}", std::process::id()));
        let disc = synth_disc(&dir);
        // Redump-style hash of track 2: pregap + 13 sectors.
        let truth = std::fs::read(dir.join("x.img")).unwrap();
        let want = sha(&truth[10 * 2352..25 * 2352]);
        for off in [0, 6, -12, 667, -1164] {
            let mut d = ImageDrive::new(synth_disc(&dir)).unwrap();
            d.read_offset = off;
            d.flaky.insert(14, 2);
            d.flaky.insert(15, 3);
            let toc = d.read_toc().unwrap();
            assert_eq!(toc.tracks[1].lba, 12);
            assert_eq!(toc.leadout, 40);
            let margin = 3i64;
            let start = 10 - margin;
            let mut area = Vec::new();
            let st = read_area(&mut d, start, 15 + 2 * margin as u32, toc.leadout, &mut |b| {
                area.extend_from_slice(b);
                Ok(())
            }, &mut |_| {})
            .unwrap();
            // The failing 20-sector batch was re-read sector by sector.
            assert_eq!(st.retried, 20, "offset {off}");
            assert_eq!(st.unreadable, 0);
            assert!(d.slowed);
            let at = margin as usize * 2352;
            assert_eq!(find_offset(&area, at, 15 * 2352, &want, 1500), Some(off), "offset {off}");
        }
        let mut dead = ImageDrive::new(disc).unwrap();
        dead.dead.push(20);
        let st = read_area(&mut dead, 18, 4, 40, &mut |_| Ok(()), &mut |_| {}).unwrap();
        assert_eq!(st.unreadable, 1);
        let st = read_area(&mut dead, 38, 4, 40, &mut |_| Ok(()), &mut |_| {}).unwrap();
        assert_eq!(st.outside, 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
