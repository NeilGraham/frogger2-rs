//! Real-data tests against the retail Frogger 2 videos.
//!
//! Skips when the game data directory (`F2_GAME_DIR`, default `game-data/usa`) is missing.
//! Comparisons against FFmpeg are skipped when `ffmpeg` is not on `PATH`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn video_dir() -> Option<PathBuf> {
    let base = std::env::var("F2_GAME_DIR").unwrap_or_else(|_| "game-data/usa".into());
    let mut p = PathBuf::from(&base);
    if p.is_relative() {
        // Tests run with the crate dir as cwd; resolve relative to the workspace root.
        p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(base);
    }
    let v = p.join("video");
    if v.is_dir() {
        Some(v)
    } else {
        eprintln!("skipping: {} not found", v.display());
        None
    }
}

fn videos(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("bik")))
        .collect();
    v.sort();
    v
}

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

#[test]
fn decode_all_retail_videos() {
    let Some(dir) = video_dir() else { return };
    let files = videos(&dir);
    assert!(!files.is_empty());
    for f in files {
        let mut b = f2_bink::BinkFile::open(std::fs::read(&f).unwrap()).unwrap();
        assert_eq!((b.width(), b.height(), b.fps()), (640, 480, (25, 1)));
        let mut n = 0;
        let mut samples = 0usize;
        let mut first = None;
        while let Some(fr) = b.decode_next() {
            let fr = fr.unwrap_or_else(|e| panic!("{}: frame {n}: {e}", f.display()));
            assert_eq!(fr.index, n);
            if n == 0 {
                first = Some(fr.yuv.clone());
            }
            samples += fr.audio.iter().map(Vec::len).sum::<usize>();
            n += 1;
        }
        assert_eq!(n, b.frame_count());
        // Rewinding reproduces frame 0 exactly.
        b.seek_start();
        let again = b.decode_next().unwrap().unwrap();
        assert_eq!(again.index, 0);
        assert_eq!(again.yuv, first.clone().unwrap(), "{}: seek_start", f.display());
        println!("{}: {} frames, tracks {:?}, {} audio samples", f.display(), n, b.audio_tracks(), samples);
    }
}

#[test]
fn video_matches_ffmpeg() {
    let Some(dir) = video_dir() else { return };
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    for name in ["01s.bik", "07s.bik", "blitzs.bik", "hasbros.bik"] {
        let path = dir.join(name);
        let mut b = f2_bink::BinkFile::open(std::fs::read(&path).unwrap()).unwrap();
        let last = b.frame_count() - 1;
        // Every frame of the short Hasbro logo, a sample of the others.
        let picks: Vec<u32> = if name == "hasbros.bik" { (0..=last).collect() } else { vec![0, 10, 100, last / 2, last] };
        let expr = picks.iter().map(|n| format!("eq(n\\,{n})")).collect::<Vec<_>>().join("+");
        let out = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-map", "0:v:0", "-vf", &format!("select='{expr}'"), "-fps_mode", "passthrough"])
            .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
            .output()
            .unwrap();
        assert!(out.status.success(), "ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr));
        let fsize = 640 * 480 * 3 / 2;
        let mut uniq = picks.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(out.stdout.len(), fsize * uniq.len(), "{name}: ffmpeg frame count");
        let mut k = 0;
        let mut max_diff = 0u8;
        let mut mismatched = 0usize;
        while let Some(fr) = b.decode_next() {
            let fr = fr.unwrap();
            if k < uniq.len() && fr.index == uniq[k] {
                let r = &out.stdout[k * fsize..(k + 1) * fsize];
                let ours = [&fr.yuv.y[..], &fr.yuv.u[..], &fr.yuv.v[..]].concat();
                for (a, b) in ours.iter().zip(r) {
                    let d = a.abs_diff(*b);
                    if d != 0 {
                        mismatched += 1;
                    }
                    max_diff = max_diff.max(d);
                }
                k += 1;
            }
        }
        println!("{name}: {} frames {:?}..: max abs diff {max_diff}, mismatched bytes {mismatched}", uniq.len(), &uniq[..uniq.len().min(5)]);
        assert_eq!(k, uniq.len());
        assert_eq!(max_diff, 0, "{name}: YUV mismatch vs ffmpeg");
    }
}

#[test]
fn audio_matches_ffmpeg() {
    let Some(dir) = video_dir() else { return };
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    for name in ["01s.bik", "12s.bik", "blitzs.bik", "hasbros.bik"] {
        let path = dir.join(name);
        let mut b = f2_bink::BinkFile::open(std::fs::read(&path).unwrap()).unwrap();
        let mut ours = Vec::new();
        while let Some(fr) = b.decode_next() {
            ours.extend_from_slice(&fr.unwrap().audio[0]);
        }
        let out = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&path).args(["-map", "0:a:0", "-f", "f32le", "-"]).output().unwrap();
        assert!(out.status.success());
        let theirs: Vec<f32> = out.stdout.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
        let n = ours.len().min(theirs.len());
        let (mut sig, mut err) = (0f64, 0f64);
        for i in 0..n {
            sig += (theirs[i] as f64).powi(2);
            err += (ours[i] as f64 - theirs[i] as f64).powi(2);
        }
        let rms_err = (err / n as f64).sqrt();
        let snr = 10.0 * (sig / err.max(1e-30)).log10();
        println!("{name}: samples ours {} ffmpeg {}, RMS error {rms_err:.3e}, SNR {snr:.1} dB", ours.len(), theirs.len());
        assert_eq!(ours.len(), theirs.len(), "{name}: sample count");
        assert!(snr > 90.0, "{name}: SNR {snr} dB too low");
    }
}
