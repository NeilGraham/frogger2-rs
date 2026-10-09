//! Audio: sample banks, positional voices and music streaming.
//!
//! Replaces DirectSound + MCI CD audio from `PC/pcaudio.c` / `PC/dx_sound.cpp`:
//! - samples come from `.wus` banks ([`f2_formats::wus::SoundBank`]) and are looked up by uid
//!   (`UpdateCRC` of the lowercase name, [`f2_formats::crc`]);
//! - DirectSound volume/pan/frequency are reproduced with the original `bytetoDB` table and
//!   DirectSound's dB/pan law, mixed in software ([`mixer`]);
//! - positional sounds use the original attenuation (`PlaySample`, `UpdateLoopingSample`) and the
//!   ambient-sound scheduler (`AddAmbientSound`, `UpdateAmbientSounds`);
//! - CD tracks are ripped WAVs (`music/trackNN.wav`) with `PrepareSong`/`LoopSong` semantics.
//!
//! Output is pluggable ([`AudioOutput`]): [`CpalOutput`] for a real device, [`NullOutput`] for
//! tests, tools and muted parity runs (time then advances only via
//! [`AudioEngine::render_offline`] / [`AudioEngine::skip`]).

pub mod flac;
pub mod mixer;
pub mod output;
pub mod tables;
pub mod wav;

pub use mixer::{Mixer, VoiceId};
#[cfg(feature = "cpal")]
pub use output::CpalOutput;
pub use output::{AudioOutput, NullOutput};
pub use tables::{song, track};

use f2_formats::wus::SoundBank;
use mixer::Voice;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use tables::*;

/// A decoded sample (`SAMPLE`, pcaudio.h:94).
#[derive(Clone, Debug)]
pub struct SampleData {
    pub uid: u32,
    /// Native rate (`DSBFREQUENCY_ORIGINAL`).
    pub rate: u32,
    /// `SFXFLAGS_LOOP` from the bank.
    pub looped: bool,
    /// Mono 16-bit PCM (stereo banks are downmixed).
    pub pcm: Arc<[i16]>,
}

impl SampleData {
    pub fn from_wus(s: &f2_formats::wus::Sample) -> SampleData {
        let raw = s.pcm_i16();
        let pcm: Vec<i16> = if s.channels > 1 {
            raw.chunks_exact(s.channels as usize).map(|c| (c.iter().map(|&x| x as i32).sum::<i32>() / c.len() as i32) as i16).collect()
        } else {
            raw
        };
        SampleData { uid: s.uid, rate: s.sample_rate.max(1), looped: s.looped, pcm: pcm.into() }
    }
}

/// Parameters of [`AudioEngine::play`] — the arguments of `PlaySample(sample, pos, radius, volume, pitch)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Play {
    /// 0..=255 (`SAMPLE_VOLUME` = 255); scaled by the sfx volume (`globalSoundVol`).
    pub volume: i32,
    /// -1 = native rate, otherwise `pitch * PITCH_STEP` Hz.
    pub pitch: i16,
    /// Pan -255 (left) ..= 255 (right) for non-positional sounds (the original always uses 0).
    pub pan: i32,
    /// Override the bank's loop flag (`None` = as the bank says, like the original).
    pub looped: Option<bool>,
    /// World position (`SVECTOR`, world units ×10 as the game passes it) for attenuation/pan.
    pub pos: Option<[i16; 3]>,
    /// Attenuation radius; 0 = `DEFAULT_SFX_DIST`.
    pub radius: i32,
}

impl Default for Play {
    fn default() -> Self {
        Play { volume: SAMPLE_VOLUME, pitch: -1, pan: 0, looped: None, pos: None, radius: 0 }
    }
}

/// Listener state the original reads from globals: `currCamSource` (fixed-point 20.12
/// `FVECTOR`) and the view matrix `vMatrix` (row-vector convention of `guMtxXFMF`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub cam_source: [i32; 3],
    pub view: [[f32; 4]; 4],
}

impl Default for Listener {
    fn default() -> Self {
        let mut view = [[0.0; 4]; 4];
        for (i, r) in view.iter_mut().enumerate() {
            r[i] = 1.0;
        }
        Listener { cam_source: [0; 3], view }
    }
}

/// Result of the positional computation shared by `PlaySample` and `UpdateLoopingSample`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Positional {
    /// Distance from the camera (`MagnitudeS(diff)/4096.0`).
    pub dist: f32,
    /// Attenuation radius used.
    pub att: f32,
    /// Game pan -255..=255.
    pub pan: i32,
}

/// Distance / pan of a sound at `pos` (pcaudio.c:323-347 / 474-498).
pub fn positional(pos: [i16; 3], radius: i32, l: &Listener) -> Positional {
    let att = if radius != 0 { radius as f32 } else { DEFAULT_SFX_DIST as f32 };
    // SubVectorSSF: short result of short - (fixed >> 12).
    let d: [i32; 3] = std::array::from_fn(|i| (pos[i] as i32).wrapping_sub(l.cam_source[i] >> 12) as i16 as i32);
    // MagnitudeS: long sum of squares (wraps), Fsqrt of it as unsigned.
    let sum = d.iter().fold(0i32, |a, &x| a.wrapping_add(x.wrapping_mul(x)));
    let dist = fsqrt(sum as u32) as f32 / 4096.0;
    // Pan: transform pos/10 by the view matrix and take x / |scr|.
    let p = [(pos[0] as f64 / 10.0) as f32, (pos[1] as f64 / 10.0) as f32, (pos[2] as f64 / 10.0) as f32];
    let m = &l.view;
    let sx = m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0];
    let sy = m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1];
    let sz = m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2];
    let sd = (sx * sx + sy * sy + sz * sz).sqrt();
    let pan = if sd != 0.0 { ((sx * 255.0) / sd) as i32 } else { 0 };
    Positional { dist, att, pan }
}

/// One `AMBIENT_SOUND` (pcaudio.h:138).
#[derive(Clone, Debug)]
pub struct AmbientSound {
    pub uid: u32,
    pub pos: [i16; 3],
    pub radius: i32,
    pub volume: i32,
    pub pitch: i16,
    /// Pre-multiplied by 60 like the original.
    pub freq: i32,
    pub rand_freq: i32,
    /// Opaque id of an actor to follow; resolved by the caller in `update_ambient_sounds`.
    pub follow: Option<u32>,
    pub counter: i32,
    /// `lpdsBuffer`: the instance last started.
    pub voice: Option<VoiceId>,
}

/// Handle of an ambient sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AmbientId(pub u64);

/// The audio engine. Not thread-safe by itself; the device thread only touches the mixer.
pub struct AudioEngine {
    mixer: Arc<Mutex<Mixer>>,
    output: Box<dyn AudioOutput>,
    samples: HashMap<u32, SampleData>,
    voice_banks: [HashMap<u32, SampleData>; 4],
    listener: Listener,
    /// `globalSoundVol` 0..=100.
    sfx_volume: i32,
    music_volume: i32,
    music_dir: Option<PathBuf>,
    /// Head-first like the original linked list (newest first).
    ambients: Vec<(AmbientId, AmbientSound)>,
    next_voice: u64,
    next_ambient: u64,
}

impl AudioEngine {
    /// Creates an engine on `output` and starts it. If a real-time output fails to start,
    /// falls back to a silent offline mixer at the same rate.
    pub fn new(mut output: Box<dyn AudioOutput>) -> AudioEngine {
        let mixer = Arc::new(Mutex::new(Mixer::new(output.sample_rate())));
        if let Err(e) = output.start(mixer.clone()) {
            eprintln!("f2-audio: output failed to start ({e}); continuing muted");
            output = Box::new(NullOutput::new(output.sample_rate()));
        }
        AudioEngine {
            mixer,
            output,
            samples: HashMap::new(),
            voice_banks: Default::default(),
            listener: Listener::default(),
            sfx_volume: MAX_SOUND_VOL,
            music_volume: MAX_SOUND_VOL,
            music_dir: None,
            ambients: Vec::new(),
            next_voice: 1,
            next_ambient: 1,
        }
    }

    /// A muted engine (no device) at `rate` Hz.
    pub fn null(rate: u32) -> AudioEngine {
        AudioEngine::new(Box::new(NullOutput::new(rate)))
    }

    /// The default device if one can be opened, else muted.
    pub fn with_default_output() -> AudioEngine {
        #[cfg(feature = "cpal")]
        match CpalOutput::new() {
            Ok(o) => return AudioEngine::new(Box::new(o)),
            Err(e) => eprintln!("f2-audio: {e}; continuing muted"),
        }
        AudioEngine::null(44100)
    }

    pub fn sample_rate(&self) -> u32 {
        self.output.sample_rate()
    }

    pub fn is_realtime(&self) -> bool {
        self.output.is_realtime()
    }

    fn mixer(&self) -> MutexGuard<'_, Mixer> {
        self.mixer.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---- samples -----------------------------------------------------------------------

    /// Port of `LoadSfxSet` (pcaudio.c:156). Later loads shadow earlier uids, as `FindSample`
    /// walks the head-inserted list.
    ///
    /// Note: the original reads the sample count as `(int)stak[0]` — a signed char — so a bank
    /// with >127 samples would load none; no retail bank is that large.
    pub fn load_bank(&mut self, bank: &SoundBank) {
        for s in &bank.samples {
            let d = SampleData::from_wus(s);
            self.samples.insert(d.uid, d);
        }
    }

    /// Port of `InitVoices` (pcaudio.c:513) for one player: that frog's voice bank.
    pub fn load_voice_bank(&mut self, player: usize, bank: &SoundBank) {
        if let Some(m) = self.voice_banks.get_mut(player) {
            for s in &bank.samples {
                let d = SampleData::from_wus(s);
                m.insert(d.uid, d);
            }
        }
    }

    /// Port of `FreeSampleList` (pcaudio.c:1350): stops everything and drops all samples.
    pub fn free_samples(&mut self) {
        self.mixer().voices.clear();
        self.samples.clear();
        for m in &mut self.voice_banks {
            m.clear();
        }
    }

    /// `FindSample` (pcaudio.c:1225).
    pub fn find_sample(&self, uid: u32) -> Option<&SampleData> {
        if uid == 0 {
            return None;
        }
        self.samples.get(&uid)
    }

    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    // ---- voices ------------------------------------------------------------------------

    /// Port of `PlaySample` (pcaudio.c:296) on a sample from the loaded banks.
    /// Returns `None` where the original returns FALSE (no sample, zero volume, out of range).
    pub fn play(&mut self, uid: u32, p: Play) -> Option<VoiceId> {
        let s = self.find_sample(uid)?.clone();
        self.play_sample(&s, p)
    }

    /// `PlayVoice(pl, sound)` (pcaudio.h:210): non-positional, `SAMPLE_VOLUME`, native pitch.
    pub fn play_voice(&mut self, player: usize, uid: u32) -> Option<VoiceId> {
        let s = self.voice_banks.get(player)?.get(&uid)?.clone();
        self.play_sample(&s, Play::default())
    }

    fn play_sample(&mut self, s: &SampleData, p: Play) -> Option<VoiceId> {
        let mut vol = p.volume.wrapping_mul(self.sfx_volume) / MAX_SOUND_VOL;
        if vol == 0 {
            return None;
        }
        let mut pan = p.pan;
        if let Some(pos) = p.pos {
            let q = positional(pos, p.radius, &self.listener);
            if q.dist > q.att {
                return None;
            }
            vol = (vol as f32 * ((q.att - q.dist) / q.att)) as i32;
            pan = q.pan;
        }
        let id = VoiceId(self.next_voice);
        self.next_voice += 1;
        let looped = p.looped.unwrap_or(s.looped);
        let mut v = Voice::new(id, s.uid, s.pcm.clone(), s.rate, looped);
        v.freq = pitch_to_freq(p.pitch, s.rate, s.rate);
        v.set_volume_pan(byte_to_db(vol), ds_pan(pan));
        self.mixer().voices.push(v);
        Some(id)
    }

    /// Stops one instance.
    pub fn stop(&mut self, id: VoiceId) {
        self.mixer().voices.retain(|v| v.id != id);
    }

    /// Port of `StopSample` (pcaudio.c:389): stops every instance of `uid`.
    pub fn stop_sample(&mut self, uid: u32) -> bool {
        let mut m = self.mixer();
        let before = m.voices.len();
        m.voices.retain(|v| v.uid != uid);
        m.voices.len() != before
    }

    pub fn stop_all(&mut self) {
        self.mixer().voices.clear();
    }

    pub fn is_playing(&self, id: VoiceId) -> bool {
        self.mixer().voices.iter().any(|v| v.id == id && !v.paused)
    }

    pub fn active_voices(&self) -> usize {
        self.mixer().voices.len()
    }

    /// Port of `UpdateLoopingSample` (pcaudio.c:464): re-evaluates volume, pan and pitch of a
    /// playing instance. `pos` of `None` (or the origin) means non-positional.
    pub fn update_voice(&mut self, id: VoiceId, volume: i32, pitch: i16, pos: Option<[i16; 3]>, radius: i32) -> bool {
        let mut vol = volume.wrapping_mul(self.sfx_volume) / MAX_SOUND_VOL;
        let mut pan = 0;
        if let Some(pos) = pos.filter(|p| *p != [0; 3]) {
            let q = positional(pos, radius, &self.listener);
            if q.dist > q.att {
                vol = 0;
            }
            vol = (vol as f32 * ((q.att - q.dist) / q.att)) as i32;
            pan = q.pan;
        }
        let mut m = self.mixer();
        let Some(v) = m.voice_mut(id) else { return false };
        let orig = self.samples.get(&v.uid).map(|s| s.rate).unwrap_or(v.freq);
        v.freq = pitch_to_freq(pitch, orig, v.freq);
        v.set_volume_pan(byte_to_db(vol), ds_pan(pan));
        true
    }

    pub fn set_listener(&mut self, l: Listener) {
        self.listener = l;
    }

    pub fn listener(&self) -> &Listener {
        &self.listener
    }

    // ---- ambient sounds ----------------------------------------------------------------

    /// Port of `AddAmbientSound` (pcaudio.c:640). `freq`/`rand_freq` are in seconds.
    #[allow(clippy::too_many_arguments)]
    pub fn add_ambient_sound(
        &mut self,
        uid: u32,
        pos: Option<[i16; 3]>,
        radius: i32,
        volume: i32,
        pitch: i16,
        freq: i32,
        rand_freq: i32,
        follow: Option<u32>,
    ) -> Option<AmbientId> {
        self.find_sample(uid)?;
        let id = AmbientId(self.next_ambient);
        self.next_ambient += 1;
        let a = AmbientSound {
            uid,
            pos: pos.unwrap_or([0; 3]),
            radius,
            volume,
            pitch,
            freq: freq.wrapping_mul(60),
            rand_freq: rand_freq.wrapping_mul(60),
            follow,
            counter: 0,
            voice: None,
        };
        self.ambients.insert(0, (id, a));
        Some(id)
    }

    /// Port of `SubAmbientSound` (pcaudio.c:1415). Like the original, a playing instance is
    /// left alone.
    pub fn sub_ambient_sound(&mut self, id: AmbientId) {
        self.ambients.retain(|(i, _)| *i != id);
    }

    pub fn ambient_sounds(&self) -> impl Iterator<Item = (AmbientId, &AmbientSound)> {
        self.ambients.iter().map(|(i, a)| (*i, a))
    }

    /// Port of `UpdateAmbientSounds` (pcaudio.c:680), called once per game frame.
    /// `random(n)` must be the game's `Random` (it consumes the game RNG); `follow_pos`
    /// returns the followed actor's `SVECTOR` position (already ×10).
    pub fn update_ambient_sounds(&mut self, act_frame_count: i32, random: &mut dyn FnMut(i32) -> i32, follow_pos: &dyn Fn(u32) -> Option<[i16; 3]>) {
        for i in 0..self.ambients.len() {
            let (_, a) = &mut self.ambients[i];
            if act_frame_count < a.counter {
                continue;
            }
            if let Some(p) = a.follow.and_then(follow_pos) {
                a.pos = p;
            }
            let a = a.clone();
            let looped = self.find_sample(a.uid).map(|s| s.looped).unwrap_or(false);
            let voice = if let (true, Some(v)) = (looped, a.voice) {
                self.update_voice(v, a.volume, a.pitch, Some(a.pos), a.radius);
                Some(v)
            } else {
                // BUG(original): `PlaySample` gets `&amb->pos` even when the computed `pos` is
                // NULL, so a sound "without a source" is attenuated relative to the origin.
                self.play(a.uid, Play { volume: a.volume, pitch: a.pitch, pos: Some(a.pos), radius: a.radius, ..Play::default() })
            };
            let r = if a.rand_freq != 0 { random(a.rand_freq) } else { 0 };
            let e = &mut self.ambients[i].1;
            e.voice = voice;
            e.counter = act_frame_count.wrapping_add(a.freq).wrapping_add(r);
        }
    }

    /// Port of `FreeAmbientSoundList` (pcaudio.c:1427): stops ambient instances and every
    /// looping voice, then empties the list.
    pub fn free_ambient_sounds(&mut self) {
        let ids: Vec<VoiceId> = self.ambients.iter().filter_map(|(_, a)| a.voice).collect();
        self.ambients.clear();
        let mut m = self.mixer();
        m.voices.retain(|v| !ids.contains(&v.id) && !(v.looped && !v.paused));
    }

    // ---- pause -------------------------------------------------------------------------

    /// Port of `PauseAudio` (pcaudio.c:742).
    pub fn pause_audio(&mut self) {
        for (_, a) in &mut self.ambients {
            a.voice = None;
        }
        let mut m = self.mixer();
        for v in &mut m.voices {
            v.paused = true;
        }
        m.set_music_paused(true);
    }

    /// Port of `UnPauseAudio` (pcaudio.c:770).
    ///
    /// BUG(original): buffers are resumed with `Play(0,0,0)`, i.e. without `DSBPLAY_LOOPING`,
    /// so a paused loop plays to its end once and stops (while the ambient scheduler starts a
    /// fresh instance). Reproduced.
    pub fn unpause_audio(&mut self) {
        for (_, a) in &mut self.ambients {
            a.voice = None;
        }
        let mut m = self.mixer();
        for v in &mut m.voices {
            if v.paused {
                v.paused = false;
                v.looped = false;
            }
        }
        m.set_music_paused(false);
    }

    // ---- music -------------------------------------------------------------------------

    /// Directory holding `trackNN.wav` (e.g. `<data root>/usa/music`).
    pub fn set_music_dir(&mut self, dir: impl Into<PathBuf>) {
        self.music_dir = Some(dir.into());
    }

    pub fn music_path(&self, track: u32) -> Option<PathBuf> {
        let d = self.music_dir.as_ref()?;
        // Prefer WAV, then FLAC; fall back to the WAV name when neither exists.
        let wav = d.join(format!("track{track:02}.wav"));
        if wav.exists() {
            return Some(wav);
        }
        let flac = d.join(format!("track{track:02}.flac"));
        Some(if flac.exists() { flac } else { wav })
    }

    /// Port of `PrepareSong` (pcaudio.c:949): picks the world's track and plays it.
    pub fn prepare_song(&mut self, world: i16, looped: bool) {
        if let Some(t) = song_track(world) {
            self.play_music(t, looped);
        }
    }

    /// `playCDTrack` + the `cdTrack` bookkeeping of `PrepareSong`. A missing/unreadable file
    /// behaves like a failed MCI play (silence) but still sets `cdTrack`, as the original
    /// ignores `playCDTrack`'s result.
    pub fn play_music(&mut self, track: u32, looped: bool) {
        let stream = self.music_path(track).and_then(|p| match wav::WavStream::open_any(&p) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("f2-audio: couldn't play cd track {track} ({}): {e}", p.display());
                None
            }
        });
        self.play_music_stream(stream, track, looped);
    }

    /// Plays an already-opened stream as `track` (mods, tests).
    pub fn play_music_stream(&mut self, stream: Option<wav::WavStream>, track: u32, looped: bool) {
        let mut m = self.mixer();
        match stream {
            Some(s) => m.start_music(s, track),
            None => m.stop_music(),
        }
        m.cd_track = if looped { track as i32 } else { -1 };
    }

    /// Port of `LoopSong` (pcaudio.c:979). The mixer calls this logic itself at end of track
    /// (the MCI notification); exposed for completeness.
    pub fn loop_song(&mut self) {
        let t = self.mixer().cd_track;
        if t > 0 {
            self.play_music(t as u32, true);
        } else {
            self.mixer().cd_track = 0;
        }
    }

    /// Port of `StopSong` (pcaudio.c:988).
    pub fn stop_music(&mut self) {
        let mut m = self.mixer();
        m.stop_music();
        m.cd_track = 0;
    }

    /// Port of `IsSongPlaying` (pcaudio.c:995).
    pub fn is_song_playing(&self) -> bool {
        self.mixer().cd_track != 0
    }

    /// Raw `cdTrack` value (0 idle, -1 play-once, >0 looping track).
    pub fn cd_track(&self) -> i32 {
        self.mixer().cd_track
    }

    /// Track whose audio is currently streaming.
    pub fn music_track(&self) -> Option<u32> {
        self.mixer().music_track()
    }

    pub fn music_position(&self) -> Option<u64> {
        self.mixer().music_position()
    }

    // ---- volumes -----------------------------------------------------------------------

    /// Extra output gain (0.0..=1.0), not in the original.
    pub fn set_master_volume(&mut self, v: f32) {
        self.mixer().master = v.clamp(0.0, 1.0);
    }

    /// `globalMusicVol` 0..=100, applied linearly like `SetCDVolume(vol*65535/100)`.
    pub fn set_music_volume(&mut self, v: i32) {
        self.music_volume = v.clamp(0, MAX_SOUND_VOL);
        self.mixer().music_gain = self.music_volume as f32 / MAX_SOUND_VOL as f32;
    }

    /// `globalSoundVol` 0..=100. Like the original it only affects sounds started (or
    /// updated) afterwards.
    pub fn set_sfx_volume(&mut self, v: i32) {
        self.sfx_volume = v.clamp(0, MAX_SOUND_VOL);
    }

    pub fn music_volume(&self) -> i32 {
        self.music_volume
    }

    pub fn sfx_volume(&self) -> i32 {
        self.sfx_volume
    }

    // ---- offline -----------------------------------------------------------------------

    /// Renders `frames` stereo frames (interleaved L/R) and advances time. With a real-time
    /// output this returns silence and does not touch the device's mixer timeline.
    pub fn render_offline(&mut self, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        if !self.output.is_realtime() {
            self.mixer().render(&mut out);
        }
        out
    }

    /// Advances time by `frames` without output (muted runs). No-op on real-time outputs.
    pub fn skip(&mut self, frames: usize) {
        if !self.output.is_realtime() {
            self.mixer().skip(frames);
        }
    }
}

/// Loads a `.wus` file from disk.
pub fn load_bank_file(path: &Path) -> Result<SoundBank, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    SoundBank::parse(&data).map_err(|e| format!("{}: {e}", path.display()))
}
