//! Opening a whole disc image: every track, data and audio.
//!
//! Supported containers (see `docs/asset-import.md` for the full matrix):
//! * `.cue` with one `FILE` per track (Redump) or one `FILE` for all tracks
//!   (single-bin images, tracks located by `INDEX 01` offsets);
//! * CloneCD `.ccd` + `.img` (+ ignored `.sub`), [`crate::ccd`];
//! * Alcohol 120% `.mds` + `.mdf` (2352 or 2448 RAW+SUB), [`crate::mds`];
//! * Nero `.nrg` (v1 and v2, DAO and TAO), [`crate::nrg`];
//! * cdrdao `.toc` + `.bin` (big-endian audio unless `SWAP`), [`crate::toc`];
//! * a `.zip` holding any of the above (read in place);
//! * bare `.iso` (2048-byte sectors, or raw 2352 sniffed by the sync pattern) or bare raw `.bin`.
//!
//! MAME `.chd`, DiscJuggler `.cdi`, `.7z` and `.rar` are recognised and rejected with a
//! conversion hint (no pure-Rust readers are available offline).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use crate::cue::{self, CueTrack};
use crate::sector::{FileSource, MemSource, RawFormat, SectorSource, TrackMode, SYNC};
use crate::vfs::hex;

/// Bytes in one raw CD sector (data or audio).
pub const RAW: u64 = 2352;
/// Bytes of interleaved subchannel data after a raw sector (RAW+SUB images).
pub const SUB: u64 = 96;

/// Where the bytes of an image file live.
#[derive(Clone, Debug)]
pub enum Backing {
    File(PathBuf),
    Zip { zip: PathBuf, member: String },
}

impl Backing {
    /// A sequential reader from the start of the file.
    pub fn open(&self) -> io::Result<Box<dyn Read>> {
        match self {
            Backing::File(p) => Ok(Box::new(File::open(p)?)),
            Backing::Zip { zip, member } => Ok(Box::new(ZipMemberReader::new(zip, member)?)),
        }
    }

    /// A sequential reader from byte `offset` (zip members are inflated up to it).
    pub fn open_at(&self, offset: u64) -> io::Result<Box<dyn Read>> {
        match self {
            Backing::File(p) => {
                let mut f = File::open(p)?;
                f.seek(SeekFrom::Start(offset))?;
                Ok(Box::new(f))
            }
            b => {
                let mut r = b.open()?;
                let n = io::copy(&mut (&mut r).take(offset), &mut io::sink())?;
                if n < offset {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "offset beyond end of file"));
                }
                Ok(r)
            }
        }
    }

    /// Reads `len` bytes at `offset`.
    pub fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut v = vec![0u8; len];
        self.open_at(offset)?.read_exact(&mut v)?;
        Ok(v)
    }

    pub fn len(&self) -> io::Result<u64> {
        match self {
            Backing::File(p) => Ok(std::fs::metadata(p)?.len()),
            Backing::Zip { zip, member } => {
                let mut a = zip::ZipArchive::new(File::open(zip)?).map_err(zerr)?;
                let e = a.by_name(member).map_err(zerr)?;
                Ok(e.size())
            }
        }
    }

    pub fn is_empty(&self) -> io::Result<bool> {
        Ok(self.len()? == 0)
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
    /// Pregap sectors stored in the backing immediately before `start` (INDEX 00 → 01).
    pub pregap: u32,
    /// True if the backing file holds exactly this track (Redump style); its
    /// SHA-1 then equals the published per-track hash.
    pub whole_file: bool,
    /// Audio sectors are followed by 96 bytes of subchannel (Alcohol RAW+SUB, NRG 2448).
    pub sub: bool,
    /// Audio samples are stored big-endian (cdrdao without `SWAP`).
    pub swap: bool,
}

impl Track {
    /// A track with the common defaults (no pregap, no subchannel, little-endian audio).
    pub fn new(number: u32, kind: TrackKind, backing: Backing, start: u64, sectors: u32) -> Track {
        Track { number, kind, backing, start, sectors, pregap: 0, whole_file: false, sub: false, swap: false }
    }

    /// Bytes per stored sector.
    pub fn stride(&self) -> u64 {
        match self.kind {
            TrackKind::Data(f, _) => f.stride() as u64,
            TrackKind::Audio => RAW + if self.sub { SUB } else { 0 },
        }
    }

    /// True if the stored sectors carry a full 2352-byte raw sector (Redump-hashable).
    pub fn is_raw(&self) -> bool {
        match self.kind {
            TrackKind::Data(f, _) => matches!(f, RawFormat::Raw2352 | RawFormat::Raw2448),
            TrackKind::Audio => true,
        }
    }
}

pub struct Disc {
    pub path: PathBuf,
    pub container: String,
    pub tracks: Vec<Track>,
}

pub(crate) fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn ext_lower(p: &str) -> String {
    match p.rsplit_once('.') {
        Some((_, e)) if !e.contains('/') && !e.contains('\\') => e.to_lowercase(),
        _ => String::new(),
    }
}

fn stem_of(p: &str) -> &str {
    let base = p.rsplit(['/', '\\']).next().unwrap_or(p);
    base.rsplit_once('.').map_or(base, |(s, _)| s)
}

/// Extensions [`Disc::open`] accepts (lowercase, without dot).
pub const IMAGE_EXTENSIONS: &[&str] = &["cue", "ccd", "img", "mds", "mdf", "nrg", "toc", "bin", "iso", "zip"];

/// Image-like extensions that are recognised but not supported.
pub const UNSUPPORTED_EXTENSIONS: &[(&str, &str)] = &[
    ("chd", "MAME CHD images are not supported yet: convert with `chdman extractcd -i game.chd -o game.cue` and import the .cue"),
    ("cdi", "DiscJuggler .cdi images are not supported: convert to .cue/.bin (e.g. with IsoBuster or cdi2iso for the data track)"),
    ("7z", "7-Zip archives are not supported: extract the archive and import the .cue/.ccd/.mds inside"),
    ("rar", "RAR archives are not supported: extract the archive and import the .cue/.ccd/.mds inside"),
    ("ecm", "ECM-packed images are not supported: unpack with `unecm` first"),
    ("mp3", "MP3 audio cannot be decoded; use WAV or FLAC tracks with --music"),
    ("ogg", "Ogg Vorbis audio cannot be decoded; use WAV or FLAC tracks with --music"),
];

/// True if `path` has an extension [`Disc::open`] handles.
pub fn is_image_path(path: &Path) -> bool {
    let e = ext_lower(&path.to_string_lossy());
    IMAGE_EXTENSIONS.contains(&e.as_str())
}

/// Descriptor kinds in the order a zip is searched for them.
const ZIP_PRIORITY: &[&str] = &["cue", "ccd", "mds", "toc", "nrg", "iso", "mdf", "img", "bin"];

impl Disc {
    /// Opens any supported image path.
    pub fn open(path: &Path) -> io::Result<Disc> {
        let name = path.to_string_lossy().into_owned();
        let ext = ext_lower(&name);
        if let Some((_, hint)) = UNSUPPORTED_EXTENSIONS.iter().find(|u| u.0 == ext) {
            return Err(io::Error::new(io::ErrorKind::Unsupported, *hint));
        }
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let resolve = |f: &str| -> io::Result<Backing> { Ok(Backing::File(find_case_insensitive(&dir, f)?)) };
        let exists = |f: &str| find_case_insensitive(&dir, f).is_ok();
        let stem = stem_of(&name).to_string();
        let kind = match ext.as_str() {
            // A raw image prefers its descriptor.
            "img" => ["ccd", "cue"].iter().find(|e| exists(&format!("{stem}.{e}"))).map(|e| e.to_string()),
            "bin" => ["cue", "toc"].iter().find(|e| exists(&format!("{stem}.{e}"))).map(|e| e.to_string()),
            "mdf" => exists(&format!("{stem}.mds")).then(|| "mds".to_string()),
            "zip" => {
                let mut a = zip::ZipArchive::new(File::open(path)?).map_err(zerr)?;
                let names: Vec<String> = a.file_names().map(String::from).collect();
                let (desc, dkind) = ZIP_PRIORITY
                    .iter()
                    .find_map(|k| names.iter().find(|n| ext_lower(n) == *k).map(|n| (n.clone(), k.to_string())))
                    .ok_or_else(|| bad("zip contains no disc image (.cue/.ccd/.mds/.toc/.nrg/.iso/.bin)"))?;
                let mut bytes = Vec::new();
                if matches!(dkind.as_str(), "cue" | "ccd" | "mds" | "toc") {
                    a.by_name(&desc).map_err(zerr)?.read_to_end(&mut bytes)?;
                }
                drop(a);
                let prefix = match desc.rfind('/') {
                    Some(i) => desc[..=i].to_string(),
                    None => String::new(),
                };
                let zpath = path.to_path_buf();
                let me = Backing::Zip { zip: zpath.clone(), member: desc.clone() };
                let mut zres = |f: &str| -> io::Result<Backing> {
                    let f = f.replace('\\', "/");
                    let f = f.rsplit('/').next().unwrap_or(&f);
                    let want = format!("{prefix}{f}").to_lowercase();
                    let m = names.iter().find(|n| n.to_lowercase() == want).ok_or_else(|| bad(format!("zip lacks {f}")))?;
                    Ok(Backing::Zip { zip: zpath.clone(), member: m.clone() })
                };
                return Self::from_descriptor(path, &format!("zip({dkind})"), &dkind, &desc, bytes, me, &mut zres);
            }
            _ => None,
        };
        if let Some(k) = kind {
            return Self::open(&dir.join(find_case_insensitive(&dir, &format!("{stem}.{k}"))?.file_name().unwrap()));
        }
        let bytes = if matches!(ext.as_str(), "cue" | "ccd" | "mds" | "toc") { std::fs::read(path)? } else { Vec::new() };
        let mut res = resolve;
        Self::from_descriptor(path, &ext, &ext, &name, bytes, Backing::File(path.to_path_buf()), &mut res)
    }

    /// Builds a disc from a descriptor of type `kind`. `me` is the descriptor file itself
    /// (used by the self-contained `nrg`/`iso`/`bin` kinds); `resolve` maps names the
    /// descriptor refers to onto backings.
    fn from_descriptor(
        path: &Path,
        container: &str,
        kind: &str,
        desc_name: &str,
        bytes: Vec<u8>,
        me: Backing,
        resolve: &mut dyn FnMut(&str) -> io::Result<Backing>,
    ) -> io::Result<Disc> {
        let stem = stem_of(desc_name).to_string();
        let tracks = match kind {
            "cue" => Self::cue_tracks(&String::from_utf8_lossy(&bytes), resolve)?,
            "ccd" => {
                let img = resolve(&format!("{stem}.img"))?;
                crate::ccd::tracks(&String::from_utf8_lossy(&bytes), img)?
            }
            "mds" => crate::mds::tracks(&bytes, &stem, resolve)?,
            "toc" => crate::toc::tracks(&String::from_utf8_lossy(&bytes), resolve)?,
            "nrg" => crate::nrg::tracks(me)?,
            "iso" | "bin" | "img" | "mdf" => vec![sniff_single(me)?],
            e => return Err(bad(format!("unsupported image extension .{e}"))),
        };
        if tracks.is_empty() {
            return Err(bad("image has no tracks"));
        }
        let container = match kind {
            "cue" => container.replace("cue", "bin/cue"),
            "ccd" => container.replace("ccd", "ccd/img"),
            "mds" => container.replace("mds", "mds/mdf"),
            "toc" => container.replace("toc", "toc/bin"),
            _ => container.to_string(),
        };
        Ok(Disc { path: path.into(), container, tracks })
    }

    fn cue_tracks(text: &str, resolve: &mut dyn FnMut(&str) -> io::Result<Backing>) -> io::Result<Vec<Track>> {
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
            let pregap = t.index0.map_or(0, |i0| t.index1.saturating_sub(i0));
            let mut tr = Track::new(t.number, kind, backing, u64::from(t.index1) * stride, sectors);
            tr.pregap = pregap;
            tr.whole_file = same_file_count == 1;
            tracks.push(tr);
        }
        Ok(tracks)
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
                let mut r = t.backing.open_at(t.start)?;
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

    /// Streams `sectors` stored sectors of `t` starting at byte `from` as 2352-byte raw
    /// sectors (subchannel stripped, audio byte order normalised to little-endian).
    fn stream_raw(&self, t: &Track, from: u64, sectors: u32, sink: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
        let stride = t.stride() as usize;
        let mut r = t.backing.open_at(from)?;
        let per = ((1 << 20) / stride).max(1);
        let mut buf = vec![0u8; per * stride];
        let mut out = Vec::with_capacity(per * RAW as usize);
        let mut left = sectors as usize;
        while left > 0 {
            let n = per.min(left);
            r.read_exact(&mut buf[..n * stride])?;
            out.clear();
            for s in buf[..n * stride].chunks_exact(stride) {
                out.extend_from_slice(&s[..RAW as usize]);
            }
            if t.swap && t.kind == TrackKind::Audio {
                for p in out.chunks_exact_mut(2) {
                    p.swap(0, 1);
                }
            }
            sink(&out)?;
            left -= n;
        }
        Ok(())
    }

    /// Streams the audio of track `t` (16-bit little-endian stereo PCM, 44.1 kHz)
    /// from INDEX 01 to the end of the track.
    pub fn stream_audio(&self, t: &Track, sink: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
        if t.kind != TrackKind::Audio {
            return Err(bad("not an audio track"));
        }
        self.stream_raw(t, t.start, t.sectors, sink)
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

    /// Streams the track as Redump stores it: raw 2352-byte sectors from the pregap
    /// (INDEX 00) to the end of the track, subchannel stripped, audio little-endian.
    pub fn stream_redump(&self, t: &Track, sink: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
        if !t.is_raw() {
            return Err(bad("track is not stored as raw sectors"));
        }
        let from = t.start - u64::from(t.pregap) * t.stride();
        self.stream_raw(t, from, t.pregap + t.sectors, sink)
    }

    /// SHA-1 of the track as Redump publishes it (see [`Disc::stream_redump`]).
    /// `None` for cooked data tracks.
    pub fn redump_sha1(&self, t: &Track) -> io::Result<Option<String>> {
        if !t.is_raw() {
            return Ok(None);
        }
        if t.whole_file && t.pregap == 0 && !t.sub && !t.swap && t.stride() == RAW && t.start == 0 {
            return self.backing_sha1(t).map(Some);
        }
        let mut h = Sha1::new();
        self.stream_redump(t, &mut |b| {
            h.update(b);
            Ok(())
        })?;
        Ok(Some(hex(&h.finalize())))
    }
}

/// One bare image file (`.iso`, `.bin`, `.img`, `.mdf` without descriptor): a single data
/// track whose sector size is sniffed (sync pattern → 2352 or 2448, else 2048).
fn sniff_single(b: Backing) -> io::Result<Track> {
    let len = b.len()?;
    let head = b.read_at(0, 16.min(len as usize))?;
    let fmt = if head.len() >= 12 && head[..12] == SYNC {
        // Distinguish 2352 from 2448 by where the next sync is.
        match b.read_at(2448, 12) {
            Ok(n) if n[..] == SYNC => RawFormat::Raw2448,
            _ => RawFormat::Raw2352,
        }
    } else {
        RawFormat::Cooked2048
    };
    let mode = if head.len() > 15 && head[15] == 2 { TrackMode::Mode2 } else { TrackMode::Mode1 };
    let mut t = Track::new(1, TrackKind::Data(fmt, mode), b, 0, (len / fmt.stride() as u64) as u32);
    t.whole_file = true;
    Ok(t)
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
        assert_eq!(d.tracks[1].pregap, 5);
        assert!(!d.tracks[1].whole_file);
        let mut n = 0;
        d.stream_audio(&d.tracks[1], &mut |b| {
            n += b.len();
            Ok(())
        })
        .unwrap();
        assert_eq!(n, 20 * 2352);
        // Opening the .bin finds the sibling cue.
        assert_eq!(Disc::open(&bin).unwrap().tracks.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unsupported_formats_explain_themselves() {
        let e = Disc::open(Path::new("/nonexistent/game.chd")).err().unwrap();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        assert!(e.to_string().contains("chdman"));
        assert!(Disc::open(Path::new("x.7z")).is_err());
    }

    #[test]
    fn names() {
        assert_eq!(ext_lower("a/b.c/Game.CUE"), "cue");
        assert_eq!(ext_lower("a.b/noext"), "");
        assert_eq!(stem_of("dir/Frogger 2.mds"), "Frogger 2");
    }
}
