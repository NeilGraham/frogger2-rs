//! Host pre-pass of the N64 build (phase 2G): turns a rendition's `game/` tree into the
//! DFS tree the ROM reads through `crates/frogger-n64/src/data.rs`, so the N64 never parses
//! PE DLLs or whole PCM sound banks.
//!
//! Layout (all paths lowercase, `/`-separated, relative to the DFS root):
//! - every file of `maps/`, `collision/`, `scenics/`, `demos/`, `textures/` (recursively)
//!   copied unchanged (the game parses them itself);
//! - `objects/<name>.dll` → `objects/<name>.dll.pcd`: the parsed
//!   [`ObjectBank`](f2_formats::objbank::ObjectBank), postcard-encoded;
//! - `sfx/<bank>.wus` → `sfx/<bank>.wus.meta`: the [`SoundBank`] with every sample's PCM
//!   removed (uid, loop flag, format), postcard-encoded; plus one WAV per sample,
//!   `<wav dir>/sfx/<bank>/<uid:08x>.wav`, which `tools/n64/build.sh` converts with
//!   `audioconv64` into `sfx/<bank>/<uid:08x>.wav64` (VADPCM, streamed from ROM).
//!
//! Never run on data you do not own; the output is derived game data (gitignored `target/`).

use f2_formats::objbank::ObjectBank;
use f2_formats::wus::SoundBank;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Suffix of a pre-parsed object bank.
pub const OBJBANK_SUFFIX: &str = ".pcd";
/// Suffix of a sound bank's metadata.
pub const SFX_META_SUFFIX: &str = ".meta";

/// Directories copied unchanged.
pub const RAW_DIRS: [&str; 5] = ["maps", "collision", "scenics", "demos", "textures"];

/// Encodes an object bank DLL as the N64 reads it.
pub fn encode_object_bank(dll: &[u8]) -> Result<Vec<u8>, String> {
    let bank = ObjectBank::parse(dll).map_err(|e| e.to_string())?;
    postcard::to_allocvec(&bank).map_err(|e| e.to_string())
}

/// Splits a `.wus` bank into its metadata (PCM removed, postcard) and the samples.
pub fn split_sound_bank(wus: &[u8]) -> Result<(Vec<u8>, SoundBank), String> {
    let bank = SoundBank::parse(wus).map_err(|e| e.to_string())?;
    let mut meta = bank.clone();
    for s in &mut meta.samples {
        s.pcm = Vec::new();
    }
    Ok((postcard::to_allocvec(&meta).map_err(|e| e.to_string())?, bank))
}

/// File name of a sample's WAV / wav64 (without extension).
pub fn sample_stem(uid: u32) -> String {
    format!("{uid:08x}")
}

/// What [`prepare`] wrote.
#[derive(Debug, Default)]
pub struct Report {
    pub raw_files: usize,
    pub raw_bytes: u64,
    pub object_banks: usize,
    pub object_bytes_in: u64,
    pub object_bytes_out: u64,
    pub sound_banks: usize,
    pub samples: usize,
    pub sample_bytes: u64,
}

fn copy_tree(src: &Path, dst: &Path, rep: &mut Report) -> io::Result<()> {
    for e in fs::read_dir(src)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_lowercase();
        let (s, d) = (e.path(), dst.join(&name));
        if e.file_type()?.is_dir() {
            fs::create_dir_all(&d)?;
            copy_tree(&s, &d, rep)?;
        } else {
            fs::copy(&s, &d)?;
            rep.raw_files += 1;
            rep.raw_bytes += e.metadata()?.len();
        }
    }
    Ok(())
}

fn sorted_files(dir: &Path, ext: &str) -> io::Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> =
        fs::read_dir(dir)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext))).collect();
    v.sort();
    Ok(v)
}

/// Lays out the DFS tree of `game` (a rendition's `game/` directory) in `out` and the
/// sample WAVs in `wav_out`. `sound_banks`: only these `.wus` stems (empty = all).
pub fn prepare(game: &Path, out: &Path, wav_out: &Path, sound_banks: &[String]) -> Result<Report, String> {
    let io = |p: &Path, e: io::Error| format!("{}: {e}", p.display());
    let mut rep = Report::default();
    for d in RAW_DIRS {
        let src = game.join(d);
        if src.is_dir() {
            let dst = out.join(d);
            fs::create_dir_all(&dst).map_err(|e| io(&dst, e))?;
            copy_tree(&src, &dst, &mut rep).map_err(|e| io(&src, e))?;
        }
    }
    let objs = game.join("objects");
    let dst = out.join("objects");
    fs::create_dir_all(&dst).map_err(|e| io(&dst, e))?;
    for p in sorted_files(&objs, "dll").map_err(|e| io(&objs, e))? {
        let dll = fs::read(&p).map_err(|e| io(&p, e))?;
        let enc = encode_object_bank(&dll).map_err(|e| format!("{}: {e}", p.display()))?;
        let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
        let o = dst.join(format!("{name}{OBJBANK_SUFFIX}"));
        fs::write(&o, &enc).map_err(|e| io(&o, e))?;
        rep.object_banks += 1;
        rep.object_bytes_in += dll.len() as u64;
        rep.object_bytes_out += enc.len() as u64;
    }
    let sfx = game.join("sfx");
    let dst = out.join("sfx");
    fs::create_dir_all(&dst).map_err(|e| io(&dst, e))?;
    for p in sorted_files(&sfx, "wus").map_err(|e| io(&sfx, e))? {
        let stem = p.file_stem().unwrap().to_string_lossy().to_lowercase();
        if !sound_banks.is_empty() && !sound_banks.iter().any(|b| b.eq_ignore_ascii_case(&stem)) {
            continue;
        }
        let wus = fs::read(&p).map_err(|e| io(&p, e))?;
        let (meta, bank) = split_sound_bank(&wus).map_err(|e| format!("{}: {e}", p.display()))?;
        let o = dst.join(format!("{stem}.wus{SFX_META_SUFFIX}"));
        fs::write(&o, &meta).map_err(|e| io(&o, e))?;
        let wdir = wav_out.join("sfx").join(&stem);
        fs::create_dir_all(&wdir).map_err(|e| io(&wdir, e))?;
        for s in &bank.samples {
            let w = wdir.join(format!("{}.wav", sample_stem(s.uid)));
            fs::write(&w, s.to_wav()).map_err(|e| io(&w, e))?;
            rep.samples += 1;
            rep.sample_bytes += s.pcm.len() as u64;
        }
        rep.sound_banks += 1;
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game_dir() -> Option<PathBuf> {
        let d = PathBuf::from(std::env::var_os("F2_GAME_DIR")?);
        let d = if d.is_absolute() { d } else { Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(d) };
        d.join("objects").is_dir().then_some(d)
    }

    /// The postcard object bank decodes to the same bank (re-encoding is byte-identical).
    #[test]
    fn object_bank_round_trip() {
        let Some(g) = game_dir() else { return };
        let dll = fs::read(g.join("objects/garden1.dll")).unwrap();
        let enc = encode_object_bank(&dll).unwrap();
        let dec: ObjectBank = postcard::from_bytes(&enc).unwrap();
        assert_eq!(postcard::to_allocvec(&dec).unwrap(), enc);
        let orig = ObjectBank::parse(&dll).unwrap();
        assert_eq!(dec.models, orig.models);
        assert_eq!(dec.anim_past_end, orig.anim_past_end);
    }

    #[test]
    fn sound_meta_keeps_uids() {
        let Some(g) = game_dir() else { return };
        let wus = fs::read(g.join("sfx/garden.wus")).unwrap();
        let (meta, bank) = split_sound_bank(&wus).unwrap();
        let m: SoundBank = postcard::from_bytes(&meta).unwrap();
        assert_eq!(m.samples.len(), bank.samples.len());
        assert!(m.samples.iter().zip(&bank.samples).all(|(a, b)| a.uid == b.uid && a.looped == b.looped && a.pcm.is_empty()));
        assert!(meta.len() < 16 * 1024, "metadata is small: {}", meta.len());
    }
}
