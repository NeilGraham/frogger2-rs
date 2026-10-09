//! Opening a whole disc image: every track, data and audio.
//!
//! Supported containers:
//! * `.cue` with one `FILE` per track (Redump) or one `FILE` for all tracks
//!   (single-bin images, tracks located by `INDEX 01` offsets);
//! * a Redump `.zip` holding a `.cue` and its `.bin` files (read in place);
//! * bare `.iso` (2048-byte sectors) or bare raw `.bin` (2352, data only).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use crate::cue::{self, CueTrack};
use crate::sector::{FileSource, MemSource, RawFormat, SectorSource, TrackMode, SYNC};
use crate::vfs::hex;

/// Bytes in one raw CD sector (data or audio).
pub const RAW: u64 = 2352;

/// Where the bytes of a cue `FILE` live.
#[derive(Clone, Debug)]
pub enum Backing {
    File(PathBuf),
    Zip { zip: PathBuf, member: String },
}

impl Backing {
    fn open(&self) -> io::Result<Box<dyn Read>> {
        match self {
            Backing::File(p) => Ok(Box::new(File::open(p)?)),
            Backing::Zip { zip, member } => Ok(Box::new(ZipMemberReader::new(zip, member)?)),
        }
    }

    fn len(&self) -> io::Result<u64> {
        match self {
            Backing::File(p) => Ok(std::fs::metadata(p)?.len()),
            Backing::Zip { zip, member } => {
                let mut a = zip::ZipArchive::new(File::open(zip)?).map_err(zerr)?;
                let e = a.by_name(member).map_err(zerr)?;
                Ok(e.size())
            }
        }
    }

    pub fn name(&self) -> String {
        match self {
            Backing::File(p) => p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            Backing::Zip { member, .. } => member.clone(),
        }
    }
}

fn zerr(e: zip::result::ZipError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("zip: {e}"))
}

/// Streams one zip member, owning its file handle (stored or deflated members).
struct ZipMemberReader {
    inner: Box<dyn Read>,
}

impl ZipMemberReader {
    fn new(zip: &Path, member: &str) -> io::Result<Self> {
        let (start, csize, method) = {
            let mut archive = zip::ZipArchive::new(File::open(zip)?).map_err(zerr)?;
            let e = archive.by_name(member).map_err(zerr)?;
            (e.data_start(), e.compressed_size(), e.compression())
        };
        let mut f = File::open(zip)?;
        f.seek(SeekFrom::Start(start))?;
        let raw = f.take(csize);
        let inner: Box<dyn Read> = match method {
            zip::CompressionMethod::Stored => Box::new(raw),
            zip::CompressionMethod::Deflated => Box::new(flate2::read::DeflateDecoder::new(io::BufReader::with_capacity(1 << 20, raw))),
            m => return Err(bad(format!("unsupported zip compression {m:?}"))),
        };
        Ok(ZipMemberReader { inner })
    }
}

impl Read for ZipMemberReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Data(RawFormat, TrackMode),
    Audio,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub number: u32,
    pub kind: TrackKind,
    pub backing: Backing,
    /// Byte offset of INDEX 01 inside the backing file.
    pub start: u64,
    /// Sectors from INDEX 01 to the end of the track (pregap of the next track excluded).
    pub sectors: u32,
    /// True if the backing file holds exactly this track (Redump style); its
    /// SHA-1 then equals the published per-track hash.
    pub whole_file: bool,
}

pub struct Disc {
    pub path: PathBuf,
    pub container: String,
    pub tracks: Vec<Track>,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn ext_lower(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

impl Disc {
    /// Opens any supported image path.
    pub fn open(path: &Path) -> io::Result<Disc> {
        match ext_lower(path).as_str() {
            "cue" => {
                let text = std::fs::read(path)?;
                let dir = path.parent().unwrap_or(Path::new("."));
                Self::from_cue(path, "bin/cue", &String::from_utf8_lossy(&text), |f| Ok(Backing::File(find_case_insensitive(dir, f)?)))
            }
            "zip" => {
                let mut a = zip::ZipArchive::new(File::open(path)?).map_err(zerr)?;
                let names: Vec<String> = a.file_names().map(String::from).collect();
                let cue_name = names.iter().find(|n| n.to_lowercase().ends_with(".cue")).ok_or_else(|| bad("zip contains no .cue"))?.clone();
                let mut text = String::new();
                a.by_name(&cue_name).map_err(zerr)?.read_to_string(&mut text)?;
                let prefix = match cue_name.rfind('/') {
                    Some(i) => cue_name[..=i].to_string(),
                    None => String::new(),
                };
                Self::from_cue(path, "zip(bin/cue)", &text, |f| {
                    let want = format!("{prefix}{f}").to_lowercase();
                    let m = names.iter().find(|n| n.to_lowercase() == want).ok_or_else(|| bad(format!("zip lacks {f}")))?;
                    Ok(Backing::Zip { zip: path.to_path_buf(), member: m.clone() })
                })
            }
            "iso" => {
                let len = std::fs::metadata(path)?.len();
                Ok(Disc {
                    path: path.into(),
                    container: "iso".into(),
                    tracks: vec![Track {
                        number: 1,
                        kind: TrackKind::Data(RawFormat::Cooked2048, TrackMode::Mode1),
                        backing: Backing::File(path.into()),
                        start: 0,
                        sectors: (len / 2048) as u32,
                        whole_file: true,
                    }],
                })
            }
            "bin" | "img" => {
                // Prefer a sibling .cue.
                let cue = path.with_extension("cue");
                if cue.exists() {
                    return Self::open(&cue);
                }
                let mut head = [0u8; 16];
                File::open(path)?.read_exact(&mut head)?;
                let fmt = if head[..12] == SYNC { RawFormat::Raw2352 } else { RawFormat::Cooked2048 };
                let len = std::fs::metadata(path)?.len();
                Ok(Disc {
                    path: path.into(),
                    container: "bin".into(),
                    tracks: vec![Track {
                        number: 1,
                        kind: TrackKind::Data(fmt, TrackMode::Mode1),
                        backing: Backing::File(path.into()),
                        start: 0,
                        sectors: (len / fmt.stride() as u64) as u32,
                        whole_file: true,
                    }],
                })
            }
            e => Err(bad(format!("unsupported image extension .{e}"))),
        }
    }

    fn from_cue(path: &Path, container: &str, text: &str, mut resolve: impl FnMut(&str) -> io::Result<Backing>) -> io::Result<Disc> {
        let ct: Vec<CueTrack> = cue::parse(text);
        if ct.is_empty() {
            return Err(bad("cue sheet has no tracks"));
        }
        let mut tracks = Vec::new();
        for (i, t) in ct.iter().enumerate() {
            let backing = resolve(&t.file)?;
            let kind = if t.is_data() {
                let (f, m) = t.layout().ok_or_else(|| bad(format!("unsupported mode {}", t.mode)))?;
                TrackKind::Data(f, m)
            } else {
                TrackKind::Audio
            };
            let stride = match kind {
                TrackKind::Data(f, _) => f.stride() as u64,
                TrackKind::Audio => RAW,
            };
            let next_same_file = ct.get(i + 1).filter(|n| n.file == t.file);
            let end = match next_same_file {
                Some(n) => u64::from(n.index0.unwrap_or(n.index1)),
                None => backing.len()? / stride,
            };
            let sectors = end.saturating_sub(u64::from(t.index1)) as u32;
            let same_file_count = ct.iter().filter(|o| o.file == t.file).count();
            tracks.push(Track { number: t.number, kind, backing, start: u64::from(t.index1) * stride, sectors, whole_file: same_file_count == 1 });
        }
        Ok(Disc { path: path.into(), container: container.into(), tracks })
    }

    pub fn first_data_track(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| matches!(t.kind, TrackKind::Data(..)))
    }

    pub fn audio_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| t.kind == TrackKind::Audio)
    }

    /// A random-access cooked-sector source for the first data track.
    /// File-backed tracks are read on demand; zip members are inflated into memory.
    pub fn data_source(&self) -> io::Result<Box<dyn SectorSource>> {
        let t = self.first_data_track().ok_or_else(|| bad("no data track"))?;
        let TrackKind::Data(fmt, mode) = t.kind else { unreachable!() };
        match &t.backing {
            Backing::File(p) => Ok(Box::new(FileSource::new(File::open(p)?, fmt, mode, t.start, t.sectors))),
            Backing::Zip { .. } => {
                let mut r = t.backing.open()?;
                io::copy(&mut (&mut r).take(t.start), &mut io::sink())?;
                let stride = fmt.stride();
                let mut raw = vec![0u8; stride];
                let mut cooked = Vec::with_capacity(t.sectors as usize * 2048);
                for _ in 0..t.sectors {
                    r.read_exact(&mut raw)?;
                    cooked.extend_from_slice(crate::sector::user_data(fmt, &raw, mode));
                }
                Ok(Box::new(MemSource::new(cooked)))
            }
        }
    }

    /// Streams the audio of track `t` (16-bit little-endian stereo PCM, 44.1 kHz)
    /// from INDEX 01 to the end of the track.
    pub fn stream_audio(&self, t: &Track, sink: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
        if t.kind != TrackKind::Audio {
            return Err(bad("not an audio track"));
        }
        let mut r: Box<dyn Read> = match &t.backing {
            Backing::File(p) => {
                let mut f = File::open(p)?;
                f.seek(SeekFrom::Start(t.start))?;
                Box::new(f)
            }
            b => {
                let mut r = b.open()?;
                io::copy(&mut (&mut r).take(t.start), &mut io::sink())?;
                r
            }
        };
        let mut left = u64::from(t.sectors) * RAW;
        let mut buf = vec![0u8; 1 << 20];
        while left > 0 {
            let want = (buf.len() as u64).min(left) as usize;
            r.read_exact(&mut buf[..want])?;
            sink(&buf[..want])?;
            left -= want as u64;
        }
        Ok(())
    }

    /// SHA-1 of the whole backing file of a track (equals Redump's per-track
    /// hash when `whole_file`).
    pub fn backing_sha1(&self, t: &Track) -> io::Result<String> {
        let mut r = t.backing.open()?;
        let mut h = Sha1::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(hex(&h.finalize()))
    }
}

/// Resolves `name` inside `dir`, falling back to a case-insensitive match.
pub fn find_case_insensitive(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let p = dir.join(name);
    if p.exists() {
        return Ok(p);
    }
    let want = name.to_lowercase();
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        if e.file_name().to_string_lossy().to_lowercase() == want {
            return Ok(e.path());
        }
    }
    Err(io::Error::new(io::ErrorKind::NotFound, format!("{} not found in {}", name, dir.display())))
}

/// Writes a canonical 44-byte RIFF/WAVE header for CD audio of `data_len` bytes.
pub fn cd_wav_header(data_len: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&2u16.to_le_bytes());
    h[24..28].copy_from_slice(&44100u32.to_le_bytes());
    h[28..32].copy_from_slice(&(44100u32 * 4).to_le_bytes());
    h[32..34].copy_from_slice(&4u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_file_cue_track_lengths() {
        let dir = std::env::temp_dir().join(format!("f2media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("g.bin");
        // 10 data sectors + 5 pregap + 20 audio sectors
        std::fs::write(&bin, vec![0u8; 35 * 2352]).unwrap();
        let cue = dir.join("g.cue");
        std::fs::write(&cue, "FILE \"G.BIN\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\n TRACK 02 AUDIO\n  INDEX 00 00:00:10\n  INDEX 01 00:00:15\n")
            .unwrap();
        let d = Disc::open(&cue).unwrap();
        assert_eq!(d.tracks.len(), 2);
        assert_eq!(d.tracks[0].sectors, 10);
        assert_eq!(d.tracks[1].sectors, 20);
        assert_eq!(d.tracks[1].start, 15 * 2352);
        assert!(!d.tracks[1].whole_file);
        let mut n = 0;
        d.stream_audio(&d.tracks[1], &mut |b| {
            n += b.len();
            Ok(())
        })
        .unwrap();
        assert_eq!(n, 20 * 2352);
        std::fs::remove_dir_all(&dir).ok();
    }
}
