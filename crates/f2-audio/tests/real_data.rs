//! Loads every retail `.wus` bank and renders mixed audio offline. Reads `F2_GAME_DIR`
//! (default `game-data/usa`); skips when the data is absent. Never commits/copies data.

use f2_audio::*;
use std::path::PathBuf;

fn game_dir() -> Option<PathBuf> {
    let d = std::env::var_os("F2_GAME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../game-data/usa"));
    d.join("game/sfx").is_dir().then_some(d)
}

#[test]
fn every_bank_loads_and_mixes() {
    let Some(dir) = game_dir() else {
        eprintln!("game data not found; skipping");
        return;
    };
    let mut banks: Vec<PathBuf> = std::fs::read_dir(dir.join("game/sfx"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wus")))
        .collect();
    banks.sort();
    assert!(!banks.is_empty());
    let mut total = 0;
    let mut rates = std::collections::BTreeSet::new();
    for path in &banks {
        let bank = load_bank_file(path).unwrap();
        for s in &bank.samples {
            assert_eq!((s.format_tag, s.channels, s.bits), (1, 1, 16), "{}", path.display());
            assert!((8000..=48000).contains(&s.sample_rate), "{} rate {}", path.display(), s.sample_rate);
            rates.insert(s.sample_rate);
        }
        let mut e = AudioEngine::null(48000);
        e.load_bank(&bank);
        let uids: Vec<u32> = bank.samples.iter().map(|s| s.uid).collect();
        for (i, &uid) in uids.iter().enumerate().take(16) {
            let pan = (i as i32 * 37) % 511 - 255;
            e.play(uid, Play { pan, volume: 200, ..Play::default() });
        }
        let out = e.render_offline(48000);
        assert!(out.iter().all(|s| s.is_finite()));
        if bank.samples.iter().any(|s| s.pcm.iter().any(|&b| b != 0)) {
            assert!(out.iter().any(|s| s.abs() > 1e-4), "{} rendered silence", path.display());
        }
        total += bank.samples.len();
    }
    eprintln!("{} banks, {} samples, rates {rates:?}", banks.len(), total);

    // Generic bank lookup by name, as LoadSfx does.
    let mut e = AudioEngine::null(44100);
    e.load_bank(&load_bank_file(&dir.join("game/sfx/generic.wus")).unwrap());
    assert!(e.find_sample(f2_formats::crc("hopongrass")).is_some());

    // Music: one second of a real track, if imported.
    e.set_music_dir(dir.join("music"));
    if e.music_path(track::GARDEN).unwrap().exists() {
        e.prepare_song(song::WORLDID_GARDEN, true);
        let out = e.render_offline(44100);
        assert!(out.iter().any(|s| s.abs() > 1e-3));
        assert!(e.is_song_playing());
    }
}

/// Opens the default device and plays 0.3 s of silence-level noise. Run manually:
/// `cargo test -p f2-audio --test real_data -- --ignored cpal_device`.
#[cfg(feature = "cpal")]
#[test]
#[ignore]
fn cpal_device() {
    let out = CpalOutput::new().expect("no device");
    let e = AudioEngine::new(Box::new(out));
    assert!(e.is_realtime());
    eprintln!("device rate {}", e.sample_rate());
    std::thread::sleep(std::time::Duration::from_millis(300));
}
