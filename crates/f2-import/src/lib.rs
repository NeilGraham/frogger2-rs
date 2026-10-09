//! Import pipeline: user's disc/install → `<data_root>/<rendition>/`.
//!
//! Layout written:
//! ```text
//! manifest.json            identification + per-file SHA-1
//! game/                    DATA1.CAB "Data files" (lowercase paths)
//! video/*.bik              Bink FMVs from the disc
//! music/trackNN.wav        Red Book tracks 2–12 (INDEX 01 → end); `trackNN.flac` instead with `--flac`
//! oracle/                  (opt-in) retail exe + binkw32.dll for parity testing only
//! ```

pub mod flac;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use f2_formats as fmt;
use f2_media::{iscab::IsCab, DirFs, Disc, IsoFs, Names, Vfs};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

/// Known retail renditions, identified by the SHA-1 of `FROGGER2/DATA1.CAB`.
pub const KNOWN: &[(&str, &str, &str)] = &[
    ("usa", "Frogger 2: Swampy's Revenge (USA)", "b4f7cca8cab0820ffca02d7e8bbfdfa8f792dc55"),
    ("eu", "Frogger 2: Swampy's Revenge (Europe) (En,Fr,De,It)", "9866393ca8f50b1917db6c7e473fb12aca9b104e"),
];

/// Per-track SHA-1 of Redump dumps (tracks 1–12).
/// EU: checked against the redump.org entry (serial 99220.DL, write offset -12).
/// USA: computed from the owner's Redump-named dump; not yet cross-checked with redump.org.
pub const KNOWN_TRACKS: &[(&str, [&str; 12])] = &[
    (
        "eu",
        [
            "66b2157b62bffe43d89143a26572b98f4ddbd3a3",
            "9f8a44abfd00ddee8dbd9ca560b058e0987ca109",
            "2ff6256ab13f1e6396895e57bde67342698dc9a5",
            "0a4e54983d64a875dfc2b1ea38d2b83f4ddbc242",
            "3b4d36b7975b184fe96c250d487ea3a0b639613f",
            "58b6629c091d35f27f0a7ee12782e6a231e6bc53",
            "507c1df240e85b609d389d1dacd42d19dda80dfa",
            "ad7ae219563db42a5d894f5629508fb74be77198",
            "23bcd63becea718bc205ba26929be5025dffeb44",
            "cd0f1ad197d0cb8571994b7ae8ef89cabbfaacfd",
            "213935faccb0028cc614190d7ea5bdae97d3a31c",
            "212cbcadfa6ed4a1cb444a443897c8d6258cba42",
        ],
    ),
    (
        "usa",
        [
            "6cbaea5810a99483a123a7e6d10e9f3a3ebbebfd",
            "84294d22b365507444a201ed203d0d57a92b0a57",
            "0f469fb25b3ade4e0bb4fbd0c0988973359a0e78",
            "846863a5e7a036df5586d1422dfddef68421046c",
            "f92749146a4cdea35dd880f6368fcd585f57f7de",
            "b57530cfd7030398a319757ad2e6e0ab240d4db2",
            "e1f298222d8fa09b2d72e6aadb27bab30bd63bfe",
            "6b1dd346020b386c8721618c94664ea4f741892c",
            "16a2d7d6e8238d87c056cd959128d5d8509a8a96",
            "3ff74e0578e54387dcbd6b15f48d854a91209041",
            "396d606797fbcf16cb11414fb8722a2a988b6535",
            "01ff7bf992ab69fc3a53a0bf547d75ceb31e7526",
        ],
    ),
];

/// CD-audio track → music cue (`PC/pcaudio.c` enum).
pub const MUSIC_TRACKS: &[(u32, &str)] = &[
    (2, "garden"),
    (3, "ancients"),
    (4, "space"),
    (5, "subterranean"),
    (6, "laboratory"),
    (7, "halloween"),
    (8, "superretro"),
    (9, "frontend"),
    (10, "levelcomplete"),
    (11, "gameover"),
    (12, "levelcompleteloop"),
];

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Also extract `Frogger2.exe` + `binkw32.dll` into `oracle/` (developer parity testing).
    pub with_oracle: bool,
    /// Skip ripping audio (faster; music will be missing).
    pub skip_music: bool,
    /// Compute the Redump track-1 hash (reads the whole data track once more).
    pub verify_track_hash: bool,
    /// Write music as lossless `trackNN.flac` instead of `trackNN.wav`.
    pub flac: bool,
    /// Override the rendition id (e.g. for unknown discs).
    pub rendition: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub importer_version: String,
    pub rendition: String,
    pub title: String,
    pub verified: bool,
    pub source: String,
    pub source_kind: String,
    pub cab_sha1: Option<String>,
    pub track1_sha1: Option<String>,
    pub redump_verified: Option<bool>,
    pub music_tracks: Vec<String>,
    pub videos: Vec<String>,
    pub stale_files: Vec<String>,
    pub files: BTreeMap<String, String>,
}

pub fn sha1_hex(b: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(b);
    f2_media::vfs::hex(&h.finalize())
}

/// Default data root: `$F2_DATA_ROOT`, else the platform data dir + `frogger2-rs/data`.
pub fn default_data_root() -> PathBuf {
    if let Some(p) = std::env::var_os("F2_DATA_ROOT") {
        return PathBuf::from(p);
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("frogger2-rs").join("data")
}

fn log(msg: impl AsRef<str>) {
    eprintln!("{}", msg.as_ref());
}

/// Prints `  <stage>: NN%` lines (every 10%) to stderr.
struct Progress {
    stage: String,
    total: u64,
    last: u64,
    n: u64,
}

impl Progress {
    fn new(stage: impl Into<String>, total: u64) -> Self {
        Progress { stage: stage.into(), total: total.max(1), last: 0, n: 0 }
    }
    fn set(&mut self, done: u64) {
        let pct = (done.min(self.total) * 100 / self.total) / 10 * 10;
        if pct > self.last {
            self.last = pct;
            log(format!("  {}: {pct}%", self.stage));
        }
    }
    fn tick(&mut self) {
        self.n += 1;
        self.set(self.n);
    }
    fn done(&mut self) {
        if self.last < 100 {
            self.last = 100;
            log(format!("  {}: 100%", self.stage));
        }
    }
}

/// Writer that SHA-1s everything passing through.
struct HashWrite<W: Write> {
    inner: W,
    sha: Sha1,
}

impl<W: Write> Write for HashWrite<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(b)?;
        self.sha.update(&b[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn write_file(root: &Path, rel: &str, data: &[u8], files: &mut BTreeMap<String, String>) -> io::Result<()> {
    let p = root.join(rel);
    if let Some(d) = p.parent() {
        fs::create_dir_all(d)?;
    }
    fs::write(&p, data)?;
    files.insert(rel.to_string(), sha1_hex(data));
    Ok(())
}

/// What the source turned out to be.
enum Source {
    Disc(Disc),
    DiscRoot(PathBuf),
    Install(PathBuf),
}

fn classify(path: &Path) -> io::Result<Source> {
    if path.is_dir() {
        let has = |rel: &str| f2_media::disc::find_case_insensitive(path, rel).is_ok();
        if has("FROGGER2") && f2_media::disc::find_case_insensitive(&path.join(find_name(path, "FROGGER2")?), "DATA1.CAB").is_ok() {
            return Ok(Source::DiscRoot(path.to_path_buf()));
        }
        if has("objects") && has("collision") {
            return Ok(Source::Install(path.to_path_buf()));
        }
        return Err(io::Error::new(io::ErrorKind::NotFound, "directory is neither a Frogger 2 disc root nor an install folder"));
    }
    Ok(Source::Disc(Disc::open(path)?))
}

fn find_name(dir: &Path, name: &str) -> io::Result<String> {
    Ok(f2_media::disc::find_case_insensitive(dir, name)?.file_name().unwrap().to_string_lossy().into_owned())
}

/// Runs the import; returns the manifest written to `<root>/<rendition>/manifest.json`.
pub fn import(source: &Path, data_root: &Path, opts: &Options) -> io::Result<Manifest> {
    let src = classify(source)?;
    let mut files = BTreeMap::new();
    let tmp_root = data_root.join(".importing");
    if tmp_root.exists() {
        fs::remove_dir_all(&tmp_root)?;
    }
    fs::create_dir_all(&tmp_root)?;
    let mut m = Manifest {
        importer_version: env!("CARGO_PKG_VERSION").into(),
        rendition: String::new(),
        title: String::new(),
        verified: false,
        source: source.display().to_string(),
        source_kind: String::new(),
        cab_sha1: None,
        track1_sha1: None,
        redump_verified: None,
        music_tracks: vec![],
        videos: vec![],
        stale_files: vec![],
        files: BTreeMap::new(),
    };

    let mut fs_: Box<dyn Vfs> = match &src {
        Source::Disc(d) => {
            m.source_kind = d.container.clone();
            log(format!("Opening {} ({} tracks)", d.container, d.tracks.len()));
            Box::new(IsoFs::open(d.data_source()?, Names::PreferJoliet)?)
        }
        Source::DiscRoot(p) => {
            m.source_kind = "disc-root".into();
            Box::new(DirFs::open(p)?)
        }
        Source::Install(p) => {
            m.source_kind = "install".into();
            Box::new(DirFs::open(p)?)
        }
    };

    match &src {
        Source::Install(_) => {
            // Copy the data dirs as-is.
            let wanted = ["collision/", "maps/", "objects/", "scenics/", "sfx/", "textures/", "demos/"];
            let entries = fs_.entries().to_vec();
            for (i, e) in entries.iter().enumerate() {
                let lower = e.path.to_lowercase();
                if wanted.iter().any(|w| lower.starts_with(w)) {
                    let data = fs_.read_file(i)?;
                    write_file(&tmp_root, &format!("game/{lower}"), &data, &mut files)?;
                } else if lower.starts_with("video/") && lower.ends_with(".bik") {
                    let data = fs_.read_file(i)?;
                    let rel = lower.clone();
                    write_file(&tmp_root, &rel, &data, &mut files)?;
                    m.videos.push(rel);
                } else if opts.with_oracle && (lower == "frogger2.exe" || lower == "binkw32.dll") {
                    let data = fs_.read_file(i)?;
                    write_file(&tmp_root, &format!("oracle/{lower}"), &data, &mut files)?;
                }
            }
            m.rendition = opts.rendition.clone().unwrap_or_else(|| "install".into());
            m.title = "Frogger 2 (from install folder)".into();
        }
        _ => {
            let idx = fs_.find("FROGGER2/DATA1.CAB").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "FROGGER2/DATA1.CAB not found on disc"))?;
            let cab_bytes = fs_.read_file(idx)?;
            let cab_sha = sha1_hex(&cab_bytes);
            match KNOWN.iter().find(|k| k.2 == cab_sha) {
                Some(k) => {
                    m.rendition = k.0.into();
                    m.title = k.1.into();
                    m.verified = true;
                    log(format!("Detected: {}", k.1));
                }
                None => {
                    m.rendition = opts.rendition.clone().unwrap_or_else(|| format!("unknown-{}", &cab_sha[..8]));
                    m.title = "Unrecognised Frogger 2 disc".into();
                    log("Warning: DATA1.CAB is not a known retail version; continuing unverified");
                }
            }
            if let Some(r) = &opts.rendition {
                m.rendition = r.clone();
            }
            m.cab_sha1 = Some(cab_sha);
            let cab = IsCab::parse(cab_bytes)?;
            let mut n = 0;
            let total_files = cab.files.len() as u64;
            let mut prog = Progress::new("extract DATA1.CAB", total_files);
            for (fi, f) in cab.files.iter().enumerate() {
                prog.set(fi as u64);
                if !IsCab::is_valid(f) {
                    continue;
                }
                let group = f.group.as_deref().unwrap_or("");
                let rel = match group {
                    "Data files" => {
                        let mut p = String::from("game/");
                        if !f.directory.is_empty() {
                            p.push_str(&f.directory.replace('\\', "/").to_lowercase());
                            p.push('/');
                        }
                        p.push_str(&f.name.to_lowercase());
                        p
                    }
                    "Program Executable Files" if opts.with_oracle => format!("oracle/{}", f.name.to_lowercase()),
                    _ => continue,
                };
                let data = cab.extract(f)?;
                write_file(&tmp_root, &rel, &data, &mut files)?;
                n += 1;
            }
            prog.done();
            log(format!("Extracted {n} files from DATA1.CAB"));
            let entries = fs_.entries().to_vec();
            let nvid = entries.iter().filter(|e| e.path.to_lowercase().starts_with("video/")).count() as u64;
            let mut vprog = Progress::new("copy videos", nvid);
            let mut vdone = 0;
            for (i, e) in entries.iter().enumerate() {
                let lower = e.path.to_lowercase();
                if lower.starts_with("video/") && lower.ends_with(".bik") {
                    let data = fs_.read_file(i)?;
                    write_file(&tmp_root, &lower, &data, &mut files)?;
                    m.videos.push(lower);
                    vdone += 1;
                    vprog.set(vdone);
                }
            }
            vprog.done();
            log(format!("Copied {} videos", m.videos.len()));
        }
    }
    drop(fs_);

    if let Source::Disc(d) = &src {
        if opts.verify_track_hash {
            let known: Option<&[&str; 12]> = KNOWN_TRACKS.iter().find(|k| k.0 == m.rendition).map(|k| &k.1);
            let mut all_ok = true;
            let mut checked = 0;
            for t in d.tracks.iter().filter(|t| t.whole_file) {
                let h = d.backing_sha1(t)?;
                let ok = known.and_then(|k| k.get(t.number as usize - 1)).is_some_and(|e| *e == h);
                all_ok &= ok;
                checked += 1;
                if t.number == 1 {
                    m.track1_sha1 = Some(h.clone());
                }
                log(format!("Track {:02} SHA-1 {h} ({})", t.number, if ok { "matches Redump" } else { "unknown" }));
            }
            if checked == 0 {
                log("Per-track verification needs one file per track (Redump layout); skipped");
            } else {
                m.redump_verified = Some(all_ok && checked == 12);
            }
        }
        if !opts.skip_music {
            let tracks: Vec<_> = d.audio_tracks().filter(|t| MUSIC_TRACKS.iter().any(|x| x.0 == t.number)).collect();
            let total_bytes: u64 = tracks.iter().map(|t| t.sectors as u64 * f2_media::disc::RAW).sum();
            let mut prog = Progress::new(if opts.flac { "rip music (FLAC)" } else { "rip music" }, total_bytes);
            let mut done_bytes = 0u64;
            for t in tracks {
                let Some((_, cue)) = MUSIC_TRACKS.iter().find(|x| x.0 == t.number) else { continue };
                let ext = if opts.flac { "flac" } else { "wav" };
                let rel = format!("music/track{:02}.{ext}", t.number);
                let p = tmp_root.join(&rel);
                fs::create_dir_all(p.parent().unwrap())?;
                let mut out = HashWrite { inner: io::BufWriter::new(fs::File::create(&p)?), sha: Sha1::new() };
                let len = t.sectors as u64 * f2_media::disc::RAW;
                if opts.flac {
                    let mut fw = flac::FlacWriter::new(&mut out, len / 4)?;
                    d.stream_audio(t, &mut |b| {
                        done_bytes += b.len() as u64;
                        prog.set(done_bytes);
                        fw.write_pcm(b)
                    })?;
                    fw.finish()?;
                } else {
                    out.write_all(&f2_media::disc::cd_wav_header(len as u32))?;
                    d.stream_audio(t, &mut |b| {
                        done_bytes += b.len() as u64;
                        prog.set(done_bytes);
                        out.write_all(b)
                    })?;
                }
                out.flush()?;
                let sz = fs::metadata(&p)?.len();
                files.insert(rel.clone(), f2_media::vfs::hex(&out.sha.finalize()));
                m.music_tracks.push(rel);
                log(format!("Ripped track {:02} ({cue}, {:.1}s, {:.1} MB)", t.number, len as f64 / 176_400.0, sz as f64 / 1e6));
            }
            prog.done();
        }
    }
    if m.music_tracks.is_empty() {
        log("Note: no CD-audio tracks in this source; music will be silent unless you add music/trackNN.wav or .flac");
    }

    m.stale_files = validate(&tmp_root.join("game"))?;
    log(format!("Validated game data ({} stale files ignored by the game)", m.stale_files.len()));
    m.files = files;

    let final_dir = data_root.join(&m.rendition);
    let json = serde_json::to_vec_pretty(&m).map_err(io::Error::other)?;
    fs::write(tmp_root.join("manifest.json"), json)?;
    if final_dir.exists() {
        fs::remove_dir_all(&final_dir)?;
    }
    fs::rename(&tmp_root, &final_dir)?;
    log(format!("Installed to {}", final_dir.display()));
    Ok(m)
}

fn list(dir: &Path, ext: &str) -> io::Result<Vec<PathBuf>> {
    let mut v = Vec::new();
    if !dir.exists() {
        return Ok(v);
    }
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if p.extension().map(|e| e.to_string_lossy().eq_ignore_ascii_case(ext)).unwrap_or(false) {
            v.push(p);
        }
    }
    v.sort();
    Ok(v)
}

/// Runs every parser over an imported `game/` tree. Returns files the game itself
/// would reject (stale entity versions). Any other parse failure is an error.
pub fn validate(game: &Path) -> io::Result<Vec<String>> {
    let bad = |p: &Path, e: fmt::Error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", p.display()));
    let mut stale = Vec::new();
    let total: usize = [
        ("textures", "fla"),
        ("sfx", "wus"),
        ("collision", "col"),
        ("scenics", "sce"),
        ("maps", "dat"),
        ("maps", "fev"),
        ("maps", "sam"),
        ("objects", "dll"),
        ("demos", "key"),
    ]
    .iter()
    .map(|(d, e)| list(&game.join(d), e).map(|v| v.len()).unwrap_or(0))
    .sum();
    let mut prog = Progress::new("validate", total as u64);
    for p in list(&game.join("textures"), "fla")? {
        prog.tick();
        let raw = fmt::fla::decompress(&fs::read(&p)?).map_err(|e| bad(&p, e))?;
        let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
        if name.starts_with("artwork") || name == "title.fla" || name == "loading.fla" {
            fmt::texture::parse_backdrop(&raw).map_err(|e| bad(&p, e))?;
        } else {
            fmt::texture::TexBank::parse(&raw).map_err(|e| bad(&p, e))?;
        }
    }
    for p in list(&game.join("sfx"), "wus")? {
        prog.tick();
        fmt::wus::SoundBank::parse(&fs::read(&p)?).map_err(|e| bad(&p, e))?;
    }
    for p in list(&game.join("collision"), "col")? {
        prog.tick();
        fmt::level::Collision::parse(&fs::read(&p)?).map_err(|e| bad(&p, e))?;
    }
    for p in list(&game.join("scenics"), "sce")? {
        prog.tick();
        fmt::level::parse_scenics(&fs::read(&p)?).map_err(|e| bad(&p, e))?;
    }
    for p in list(&game.join("maps"), "dat")? {
        prog.tick();
        let raw = fs::read(&p)?;
        if fmt::entity::Entities::version(&raw) != Some(fmt::entity::ENTITY_VERSION) {
            stale.push(p.file_name().unwrap().to_string_lossy().into_owned());
            continue;
        }
        fmt::entity::Entities::parse(&raw).map_err(|e| bad(&p, e))?;
    }
    for p in list(&game.join("maps"), "fev")? {
        prog.tick();
        fmt::fev::Script::parse(&fs::read(&p)?).map_err(|e| bad(&p, e))?;
    }
    for p in list(&game.join("maps"), "sam")? {
        prog.tick();
        fmt::level::parse_sfx_mapping(&fs::read(&p)?, 0).map_err(|e| bad(&p, e))?;
    }
    let lang = game.join("maps/language.fla");
    if lang.exists() {
        fmt::lang::StringTable::parse(&fmt::fla::decompress(&fs::read(&lang)?).map_err(|e| bad(&lang, e))?).map_err(|e| bad(&lang, e))?;
    }
    for p in list(&game.join("objects"), "dll")? {
        prog.tick();
        fmt::objbank::ObjectBank::parse(&fs::read(&p)?).map_err(|e| bad(&p, e))?;
    }
    for p in list(&game.join("demos"), "key")? {
        prog.tick();
        fmt::misc::parse_key_file(&fs::read(&p)?).map_err(|e| bad(&p, e))?;
    }
    prog.done();
    Ok(stale)
}

/// Lists imported renditions under a data root.
pub fn installed(data_root: &Path) -> Vec<Manifest> {
    let mut v = Vec::new();
    if let Ok(rd) = fs::read_dir(data_root) {
        for e in rd.flatten() {
            if let Ok(b) = fs::read(e.path().join("manifest.json")) {
                if let Ok(m) = serde_json::from_slice::<Manifest>(&b) {
                    v.push(m);
                }
            }
        }
    }
    v.sort_by(|a, b| a.rendition.cmp(&b.rendition));
    v
}

/// Result of re-hashing one installed rendition.
#[derive(Clone, Debug)]
pub struct VerifyReport {
    pub rendition: String,
    pub checked: usize,
    pub missing: Vec<String>,
    pub mismatched: Vec<String>,
    /// Files present under the rendition directory but absent from the manifest.
    pub extra: Vec<String>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.missing.is_empty() && self.mismatched.is_empty()
    }
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else if let Ok(r) = p.strip_prefix(base) {
                out.push(r.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// Re-hashes installed files against `manifest.json`. `rendition` = `None` verifies every
/// installed rendition. Prints percent progress per rendition to stderr.
pub fn verify_installed(data_root: &Path, rendition: Option<&str>) -> io::Result<Vec<VerifyReport>> {
    let ms: Vec<Manifest> = installed(data_root).into_iter().filter(|m| rendition.is_none_or(|r| m.rendition == r)).collect();
    if ms.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            match rendition {
                Some(r) => format!("rendition '{r}' is not installed under {}", data_root.display()),
                None => format!("nothing installed under {}", data_root.display()),
            },
        ));
    }
    let mut reports = Vec::new();
    for m in ms {
        let dir = data_root.join(&m.rendition);
        let mut rep = VerifyReport { rendition: m.rendition.clone(), checked: 0, missing: vec![], mismatched: vec![], extra: vec![] };
        let mut prog = Progress::new(format!("verify {}", m.rendition), m.files.len() as u64);
        for (i, (rel, want)) in m.files.iter().enumerate() {
            prog.set(i as u64);
            match fs::read(dir.join(rel)) {
                Ok(b) => {
                    rep.checked += 1;
                    if sha1_hex(&b) != *want {
                        rep.mismatched.push(rel.clone());
                    }
                }
                Err(_) => rep.missing.push(rel.clone()),
            }
        }
        prog.done();
        let mut all = Vec::new();
        walk(&dir, &dir, &mut all);
        rep.extra = all.into_iter().filter(|r| r != "manifest.json" && !m.files.contains_key(r)).collect();
        rep.extra.sort();
        reports.push(rep);
    }
    Ok(reports)
}
