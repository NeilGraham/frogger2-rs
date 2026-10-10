//! `--music <folder>`: music tracks supplied as files (for data-only sources).
//!
//! Files are matched to CD tracks 2–12 by the number in their name (`02.wav`,
//! `Track 02.flac`, `track02.wav`, `Frogger 2 (Track 02).wav`, `02 - Garden.flac`) or by
//! the cue name (`garden.wav`, `frontend.flac`, see [`crate::MUSIC_TRACKS`]). WAV must be
//! 16-bit PCM and FLAC 4–24 bit (what the game's player decodes); other sample rates are
//! resampled at runtime. Files are copied unchanged (re-encoded only when `--flac` is
//! given for a 44.1 kHz stereo WAV). MP3/Ogg are rejected: no decoder is available.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{log, sha1_hex, MUSIC_TRACKS};

/// CD track number a music file name refers to (2..=12), if any.
pub fn track_number(stem: &str) -> Option<u32> {
    let s = stem.to_lowercase();
    let in_range = |n: u32| (2..=12).contains(&n).then_some(n);
    let digits_at = |t: &str| -> Option<u32> {
        let d: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        d.parse().ok()
    };
    if let Some(i) = s.find("track") {
        let rest = s[i + 5..].trim_start_matches([' ', '_', '-', '#', '(', '.']);
        if let Some(n) = digits_at(rest) {
            return in_range(n);
        }
    }
    if let Some(n) = digits_at(&s) {
        return in_range(n);
    }
    if let Some((n, _)) = MUSIC_TRACKS.iter().find(|(_, name)| s == *name || s.replace([' ', '_', '-'], "") == *name) {
        return Some(*n);
    }
    // A single number anywhere ("Frogger2 - 05").
    let nums: Vec<u32> = s.split(|c: char| !c.is_ascii_digit()).filter(|t| !t.is_empty()).filter_map(|t| t.parse().ok()).collect();
    match nums.as_slice() {
        [n] => in_range(*n),
        _ => None,
    }
}

/// Checks that a file is audio the game can play. Returns the extension to store it under.
pub fn check_audio(data: &[u8]) -> Result<&'static str, String> {
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
        let mut o = 12;
        while o + 8 <= data.len() {
            let len = u32::from_le_bytes(data[o + 4..o + 8].try_into().unwrap()) as usize;
            if &data[o..o + 4] == b"fmt " {
                let f = data.get(o + 8..o + 8 + len.max(16)).ok_or("short fmt chunk")?;
                let tag = u16::from_le_bytes([f[0], f[1]]);
                let bits = u16::from_le_bytes([f[14], f[15]]);
                return if tag == 1 && bits == 16 { Ok("wav") } else { Err(format!("WAV must be 16-bit PCM (found format {tag}, {bits}-bit)")) };
            }
            o += 8 + len + (len & 1);
        }
        return Err("WAV without fmt chunk".into());
    }
    if data.len() >= 42 && &data[0..4] == b"fLaC" {
        let bps = (((data[20] & 1) << 4) | (data[21] >> 4)) + 1;
        return if (4..=24).contains(&bps) { Ok("flac") } else { Err(format!("unsupported FLAC bit depth {bps}")) };
    }
    if data.starts_with(b"ID3") || data.starts_with(&[0xff, 0xfb]) || data.starts_with(b"OggS") {
        return Err("MP3/Ogg cannot be decoded; convert to WAV or FLAC".into());
    }
    Err("not a WAV or FLAC file".into())
}

/// Copies tracks the import still lacks from `dir` into `music/`.
pub fn import_folder(dir: &Path, tmp_root: &Path, _flac: bool, files: &mut BTreeMap<String, String>, out: &mut Vec<String>) -> io::Result<()> {
    let mut found: BTreeMap<u32, PathBuf> = BTreeMap::new();
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if !p.is_file() {
            continue;
        }
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !matches!(ext.as_str(), "wav" | "flac" | "mp3" | "ogg") {
            continue;
        }
        let stem = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        match track_number(&stem) {
            Some(n) if found.contains_key(&n) => log(format!("Music folder: {} ignored (track {n:02} already taken)", p.display())),
            Some(n) => {
                found.insert(n, p);
            }
            None => log(format!("Music folder: {} ignored (no track number 02-12 in the name)", p.display())),
        }
    }
    let mut n_ok = 0;
    for (n, p) in found {
        if out.iter().any(|r| r.contains(&format!("track{n:02}."))) {
            continue;
        }
        let data = fs::read(&p)?;
        let ext = match check_audio(&data) {
            Ok(e) => e,
            Err(why) => {
                log(format!("Music folder: {} skipped: {why}", p.display()));
                continue;
            }
        };
        let rel = format!("music/track{n:02}.{ext}");
        let dst = tmp_root.join(&rel);
        fs::create_dir_all(dst.parent().unwrap())?;
        fs::write(&dst, &data)?;
        files.insert(rel.clone(), sha1_hex(&data));
        out.push(rel);
        n_ok += 1;
    }
    log(format!("Music folder {}: {n_ok} tracks imported", dir.display()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_numbers_from_names() {
        for (s, n) in [
            ("02", Some(2)),
            ("Track 02", Some(2)),
            ("track12", Some(12)),
            ("Frogger 2 - Swampy's Revenge (USA) (Track 05)", Some(5)),
            ("03 - Ancients", Some(3)),
            ("garden", Some(2)),
            ("Level Complete", Some(10)),
            ("Frogger - 07", Some(7)),
            ("frogger2_07", None),
            ("Track 01", None),
            ("cover", None),
            ("13", None),
        ] {
            assert_eq!(track_number(s), n, "{s}");
        }
    }

    #[test]
    fn audio_checks() {
        let mut wav = f2_media::disc::cd_wav_header(4).to_vec();
        wav.extend_from_slice(&[0; 4]);
        assert_eq!(check_audio(&wav), Ok("wav"));
        let mut w8 = wav.clone();
        w8[34] = 8;
        assert!(check_audio(&w8).is_err());
        let mut flac = b"fLaC".to_vec();
        flac.extend_from_slice(&[0u8; 38]);
        flac[21] = 0xf0; // bits-per-sample - 1 = 15 (STREAMINFO bit 108..113)
        assert_eq!(check_audio(&flac), Ok("flac"));
        assert!(check_audio(b"ID3\x04........").is_err());
    }

    #[test]
    fn folder_import_maps_names() {
        let dir = std::env::temp_dir().join(format!("f2music-{}", std::process::id()));
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        let mut wav = f2_media::disc::cd_wav_header(4).to_vec();
        wav.extend_from_slice(&[1, 2, 3, 4]);
        fs::write(src.join("Track 02.wav"), &wav).unwrap();
        fs::write(src.join("frontend.WAV"), &wav).unwrap();
        fs::write(src.join("05.mp3"), b"ID3....").unwrap();
        fs::write(src.join("notes.txt"), b"x").unwrap();
        let mut files = BTreeMap::new();
        let mut out = vec!["music/track09.wav".to_string()];
        import_folder(&src, &dir.join("out"), false, &mut files, &mut out).unwrap();
        assert_eq!(out, vec!["music/track09.wav".to_string(), "music/track02.wav".to_string()]);
        assert!(dir.join("out/music/track02.wav").exists());
        fs::remove_dir_all(&dir).ok();
    }
}
