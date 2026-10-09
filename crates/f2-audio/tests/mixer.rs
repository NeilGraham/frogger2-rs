//! Offline mixer tests (synthetic data only).

use f2_audio::tables::{byte_to_db, db_to_gain};
use f2_audio::*;
use f2_formats::wus::{Sample, SoundBank};

fn sample(uid: u32, rate: u32, looped: bool, pcm: &[i16]) -> Sample {
    Sample { uid, looped, format_tag: 1, channels: 1, sample_rate: rate, bits: 16, pcm: pcm.iter().flat_map(|s| s.to_le_bytes()).collect() }
}

fn sine(rate: u32, hz: f32, n: usize) -> Vec<i16> {
    (0..n).map(|i| ((i as f32 * hz * std::f32::consts::TAU / rate as f32).sin() * 16000.0) as i16).collect()
}

fn engine(samples: Vec<Sample>) -> AudioEngine {
    let mut e = AudioEngine::null(44100);
    e.load_bank(&SoundBank { samples });
    e
}

fn crossings(ch: impl Iterator<Item = f32>) -> usize {
    let mut prev = 0.0f32;
    let mut n = 0;
    for s in ch {
        if (prev < 0.0) != (s < 0.0) && s != 0.0 {
            n += 1;
        }
        if s != 0.0 {
            prev = s;
        }
    }
    n
}

#[test]
fn resampling_preserves_pitch_and_duration() {
    // 1 s of 441 Hz at 11025 Hz, played on a 44100 Hz mixer.
    let mut e = engine(vec![sample(1, 11025, false, &sine(11025, 441.0, 11025))]);
    let v = e.play(1, Play::default()).unwrap();
    let out = e.render_offline(44100 - 8);
    assert!(e.is_playing(v));
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let z = crossings(left.iter().copied());
    assert!((878..=884).contains(&z), "zero crossings {z}");
    // Smooth (interpolated, not zero-stuffed): neighbouring output samples are close.
    let max_step = left.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
    assert!(max_step < 0.05, "max step {max_step}");
    e.render_offline(64);
    assert!(!e.is_playing(v), "one-shot must end after its length");
    assert_eq!(e.active_voices(), 0);
}

#[test]
fn pitch_changes_rate() {
    // pitch 113 -> 113*390 = 44070 Hz playback of an 11025 Hz sample: ~4x faster.
    let mut e = engine(vec![sample(1, 11025, false, &sine(11025, 441.0, 11025))]);
    let v = e.play(1, Play { pitch: 113, ..Play::default() }).unwrap();
    e.render_offline(11100);
    assert!(!e.is_playing(v));
}

#[test]
fn looping_and_stop() {
    let pcm = sine(22050, 1000.0, 2205);
    let mut e = engine(vec![sample(1, 22050, true, &pcm), sample(2, 22050, false, &pcm)]);
    let a = e.play(1, Play::default()).unwrap();
    let b = e.play(2, Play::default()).unwrap();
    let out = e.render_offline(44100); // 10x the sample length
    assert!(e.is_playing(a));
    assert!(!e.is_playing(b));
    // The loop is still audible at the end.
    let tail = &out[out.len() - 2000..];
    assert!(tail.iter().any(|s| s.abs() > 0.1));
    // Explicit loop override.
    let c = e.play(1, Play { looped: Some(false), ..Play::default() }).unwrap();
    e.render_offline(5000);
    assert!(!e.is_playing(c));
    assert!(e.stop_sample(1));
    assert!(!e.is_playing(a));
}

fn dc_engine() -> AudioEngine {
    engine(vec![sample(7, 44100, true, &vec![16384i16; 4410])])
}

#[test]
fn pan_law_matches_directsound() {
    let mut e = dc_engine();
    let check = |e: &mut AudioEngine, pan: i32| {
        let v = e.play(7, Play { pan, ..Play::default() }).unwrap();
        let out = e.render_offline(100);
        e.stop(v);
        (out[100], out[101])
    };
    let (l, r) = check(&mut e, 0);
    assert!((l - 0.5).abs() < 1e-4 && (r - 0.5).abs() < 1e-4);
    // Positive game pan -> DirectSound pan bytetoDB[255-pan] (negative = attenuate right).
    let (l, r) = check(&mut e, 255);
    assert!((l - 0.5).abs() < 1e-4 && r == 0.0, "{l} {r}");
    let (l, r) = check(&mut e, -255);
    assert!(l == 0.0 && (r - 0.5).abs() < 1e-4);
    let (l, r) = check(&mut e, 127);
    let want = 0.5 * db_to_gain(byte_to_db(128));
    assert!((l - 0.5).abs() < 1e-4 && (r - want).abs() < 1e-4, "{r} vs {want}");
}

#[test]
fn volume_table_and_sfx_volume() {
    let mut e = dc_engine();
    let v = e.play(7, Play { volume: 128, ..Play::default() }).unwrap();
    let out = e.render_offline(10);
    assert!((out[0] - 0.5 * db_to_gain(-994)).abs() < 1e-4);
    e.stop(v);
    e.set_sfx_volume(0);
    assert!(e.play(7, Play::default()).is_none(), "zero volume -> PlaySample returns FALSE");
    e.set_sfx_volume(50); // 255*50/100 = 127
    e.play(7, Play::default()).unwrap();
    let out = e.render_offline(10);
    assert!((out[0] - 0.5 * db_to_gain(byte_to_db(127))).abs() < 1e-4);
}

#[test]
fn bug_entry_238_keeps_full_volume() {
    // 238 -> bytetoDB = +100, rejected by SetVolume: the duplicate keeps 0 dB.
    let mut e = dc_engine();
    e.play(7, Play { volume: 238, ..Play::default() }).unwrap();
    let out = e.render_offline(10);
    assert!((out[0] - 0.5).abs() < 1e-4);
}

#[test]
fn positional_attenuation() {
    let mut e = dc_engine();
    // Camera at origin, identity view.
    assert!(e.play(7, Play { pos: Some([8000, 0, 0]), ..Play::default() }).is_none(), "beyond DEFAULT_SFX_DIST");
    let q = positional([3500, 0, 0], 0, e.listener());
    assert_eq!(q.att, 7000.0);
    assert!((q.dist - 3500.0).abs() < 300.0, "approximate utilSqrt: {}", q.dist);
    assert_eq!(q.pan, 255);
    // Camera source is 20.12 fixed point.
    e.set_listener(Listener { cam_source: [3500 << 12, 0, 0], ..Listener::default() });
    let q = positional([3500, 0, 0], 0, e.listener());
    assert_eq!(q.dist, 0.0);
    let v = e.play(7, Play { pos: Some([3500, 0, 0]), radius: 12000, ..Play::default() }).unwrap();
    assert!(e.is_playing(v));
    assert!(e.update_voice(v, 255, -1, Some([3500, 0, 30000]), 12000));
    let out = e.render_offline(10);
    assert_eq!(out[1], 0.0, "out of range -> volume 0 (silent)");
}

#[test]
fn ambient_scheduler() {
    let mut e = engine(vec![sample(1, 22050, false, &sine(22050, 500.0, 220)), sample(2, 22050, true, &sine(22050, 500.0, 220))]);
    let a = e.add_ambient_sound(1, None, 0, 255, -1, 2, 1, None).unwrap();
    let b = e.add_ambient_sound(2, Some([10, 0, 0]), 0, 255, -1, 0, 0, None).unwrap();
    let mut calls = vec![];
    let mut rnd = |n: i32| {
        calls.push(n);
        5
    };
    e.update_ambient_sounds(0, &mut rnd, &|_| None);
    let get = |e: &AudioEngine, id| e.ambient_sounds().find(|(i, _)| *i == id).unwrap().1.clone();
    assert_eq!(get(&e, a).counter, 120 + 5);
    assert!(get(&e, a).voice.is_some());
    let lv = get(&e, b).voice.unwrap();
    // Looping ambient: the next update re-evaluates instead of restarting.
    e.update_ambient_sounds(1, &mut |_| 0, &|_| None);
    assert_eq!(get(&e, b).voice, Some(lv));
    assert_eq!(calls, vec![60]);
    e.free_ambient_sounds();
    assert_eq!(e.ambient_sounds().count(), 0);
    assert!(!e.is_playing(lv));
}

#[test]
fn unpause_drops_loop_flag() {
    let mut e = engine(vec![sample(2, 22050, true, &sine(22050, 500.0, 220))]);
    let v = e.play(2, Play::default()).unwrap();
    e.pause_audio();
    e.render_offline(10000);
    assert!(!e.is_playing(v));
    assert_eq!(e.active_voices(), 1);
    e.unpause_audio();
    assert!(e.is_playing(v));
    e.render_offline(1000);
    assert!(!e.is_playing(v), "BUG(original): resumed without DSBPLAY_LOOPING");
}

fn music_dir(tag: &str, track: u32, frames: usize, rate: u32) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("f2-audio-test-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut pcm = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let s = ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 8000.0) as i16;
        pcm.push(s);
        pcm.push(-s);
    }
    std::fs::write(dir.join(format!("track{track:02}.wav")), wav::encode_wav(2, rate, &pcm)).unwrap();
    dir
}

#[test]
fn music_loop_and_play_once() {
    let dir = music_dir("loop", track::GARDEN, 4410, 44100);
    let mut e = AudioEngine::null(44100);
    e.set_music_dir(&dir);
    e.prepare_song(song::WORLDID_GARDEN, true);
    assert_eq!(e.cd_track(), track::GARDEN as i32);
    let out = e.render_offline(4410 * 3 + 100);
    assert!(e.is_song_playing());
    assert_eq!(e.music_track(), Some(track::GARDEN));
    // Stereo content (R = -L) survives, and the loop is gapless (audible after 3 loops).
    assert!(out[out.len() - 200..].iter().any(|s| s.abs() > 0.1));
    let i = out.len() - 100;
    assert!((out[i] + out[i + 1]).abs() < 1e-3);

    // City uses the Super Retro track; play-once ends and clears cdTrack (LoopSong).
    std::fs::copy(dir.join("track02.wav"), dir.join("track08.wav")).unwrap();
    e.prepare_song(song::WORLDID_CITY, false);
    assert_eq!(e.cd_track(), -1);
    assert_eq!(e.music_track(), Some(track::SUPERRETRO));
    e.render_offline(4000);
    assert!(e.is_song_playing());
    e.render_offline(500);
    assert!(!e.is_song_playing());
    assert_eq!(e.music_track(), None);

    // Missing track: silent but cdTrack still set, like a failed MCI play.
    e.play_music(track::GAMEOVER, false);
    assert!(e.is_song_playing());
    e.stop_music();
    assert!(!e.is_song_playing());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn music_resamples_and_volume() {
    let dir = music_dir("rs", track::FRONTEND, 44100, 44100);
    let mut e = AudioEngine::null(48000);
    e.set_music_dir(&dir);
    e.play_music(track::FRONTEND, false);
    let out = e.render_offline(24000);
    // Half a second of output consumes ~half the source (read-ahead is one 4096-frame chunk).
    let pos = e.music_position().unwrap();
    assert!((22050..=22050 + 4096).contains(&pos), "{pos}");
    let z = crossings(out.iter().step_by(2).copied());
    assert!((436..=444).contains(&z), "{z}");
    e.set_music_volume(50);
    let a = e.render_offline(2000);
    let peak = a.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak < 0.13 && peak > 0.1, "{peak}");
    e.set_master_volume(0.0);
    assert!(e.render_offline(100).iter().all(|&s| s == 0.0));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn muted_skip_advances_time() {
    let mut e = engine(vec![sample(1, 22050, false, &sine(22050, 500.0, 22050))]);
    let v = e.play(1, Play::default()).unwrap();
    e.skip(44000);
    assert!(e.is_playing(v));
    e.skip(200);
    assert!(!e.is_playing(v));
}
