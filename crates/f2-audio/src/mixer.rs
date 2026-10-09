//! Software mixer: replaces the DirectSound secondary buffers and the MCI CD player.
//!
//! Output is interleaved stereo `f32` at the device rate. Voices are mono 16-bit PCM resampled
//! with linear interpolation from their current DirectSound "frequency"; music is a streamed
//! 16-bit WAV, resampled the same way. Everything is deterministic: time only advances when
//! [`Mixer::render`] is called.

use crate::tables::{db_to_gain, pan_gains};
use crate::wav::WavStream;
use std::sync::Arc;

/// Handle of a playing voice (a duplicated DirectSound buffer in the original).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VoiceId(pub u64);

pub(crate) struct Voice {
    pub id: VoiceId,
    pub uid: u32,
    pub data: Arc<[i16]>,
    /// Current playback rate in Hz (`SetFrequency`).
    pub freq: u32,
    pub pos: f64,
    pub looped: bool,
    /// DirectSound volume / pan, hundredths of dB.
    pub vol_db: i32,
    pub pan_ds: i32,
    pub paused: bool,
    gl: f32,
    gr: f32,
}

impl Voice {
    pub fn new(id: VoiceId, uid: u32, data: Arc<[i16]>, freq: u32, looped: bool) -> Voice {
        let mut v = Voice { id, uid, data, freq, pos: 0.0, looped, vol_db: 0, pan_ds: 0, paused: false, gl: 1.0, gr: 1.0 };
        v.recompute();
        v
    }

    /// `SetVolume` + `SetPan`. A volume above 0 dB is rejected by DirectSound and leaves the
    /// previous volume in place (matters for the `bytetoDB[238]` typo).
    pub fn set_volume_pan(&mut self, vol_db: i32, pan_ds: i32) {
        if (-10000..=0).contains(&vol_db) {
            self.vol_db = vol_db;
        }
        if (-10000..=10000).contains(&pan_ds) {
            self.pan_ds = pan_ds;
        }
        self.recompute();
    }

    fn recompute(&mut self) {
        let g = db_to_gain(self.vol_db);
        let (l, r) = pan_gains(self.pan_ds);
        self.gl = g * l;
        self.gr = g * r;
    }
}

struct Music {
    stream: WavStream,
    chunk: Vec<f32>,
    chunk_frames: usize,
    chunk_idx: usize,
    a: [f32; 2],
    b: [f32; 2],
    frac: f64,
    track: u32,
}

impl Music {
    fn new(stream: WavStream, track: u32) -> Music {
        let mut m = Music { stream, chunk: vec![0.0; 4096 * 2], chunk_frames: 0, chunk_idx: 0, a: [0.0; 2], b: [0.0; 2], frac: 0.0, track };
        if let Some(f) = m.next_frame(false) {
            m.a = f;
        }
        m.b = m.next_frame(false).unwrap_or(m.a);
        m
    }

    /// Next source frame; `rewind` restarts the track at end of data (gapless loop).
    fn next_frame(&mut self, rewind: bool) -> Option<[f32; 2]> {
        if self.chunk_idx >= self.chunk_frames {
            self.chunk_idx = 0;
            self.chunk_frames = self.stream.read_stereo(&mut self.chunk).unwrap_or(0);
            if self.chunk_frames == 0 {
                if !rewind || self.stream.frames == 0 || self.stream.rewind().is_err() {
                    return None;
                }
                self.chunk_frames = self.stream.read_stereo(&mut self.chunk).unwrap_or(0);
                if self.chunk_frames == 0 {
                    return None;
                }
            }
        }
        let i = self.chunk_idx * 2;
        self.chunk_idx += 1;
        Some([self.chunk[i], self.chunk[i + 1]])
    }
}

pub struct Mixer {
    rate: u32,
    pub(crate) voices: Vec<Voice>,
    music: Option<Music>,
    music_paused: bool,
    /// `cdTrack` (pcaudio.c:76): 0 = not playing, -1 = playing once, >0 = looping this track.
    pub(crate) cd_track: i32,
    pub(crate) master: f32,
    pub(crate) music_gain: f32,
    scratch: Vec<f32>,
}

impl Mixer {
    pub fn new(rate: u32) -> Mixer {
        Mixer { rate: rate.max(1), voices: Vec::new(), music: None, music_paused: false, cd_track: 0, master: 1.0, music_gain: 1.0, scratch: Vec::new() }
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub(crate) fn voice_mut(&mut self, id: VoiceId) -> Option<&mut Voice> {
        self.voices.iter_mut().find(|v| v.id == id)
    }

    pub(crate) fn start_music(&mut self, stream: WavStream, track: u32) {
        self.music = Some(Music::new(stream, track));
        self.music_paused = false;
    }

    pub(crate) fn stop_music(&mut self) {
        self.music = None;
        self.music_paused = false;
    }

    pub(crate) fn music_track(&self) -> Option<u32> {
        self.music.as_ref().map(|m| m.track)
    }

    pub(crate) fn set_music_paused(&mut self, p: bool) {
        self.music_paused = p;
    }

    /// Source frames consumed by the current track (tests/diagnostics).
    pub fn music_position(&self) -> Option<u64> {
        self.music.as_ref().map(|m| m.stream.position())
    }

    /// Mixes `out.len() / 2` stereo frames into `out` (overwriting it).
    pub fn render(&mut self, out: &mut [f32]) {
        out.iter_mut().for_each(|s| *s = 0.0);
        let frames = out.len() / 2;
        let rate = self.rate as f64;

        for v in self.voices.iter_mut() {
            if v.paused {
                continue;
            }
            let len = v.data.len();
            if len == 0 {
                v.looped = false;
                v.pos = 0.0;
                continue;
            }
            let step = v.freq as f64 / rate;
            for f in 0..frames {
                if v.pos >= len as f64 {
                    if v.looped {
                        v.pos %= len as f64;
                    } else {
                        break;
                    }
                }
                let i = v.pos as usize;
                let t = (v.pos - i as f64) as f32;
                let s0 = v.data[i] as f32;
                let s1 = if i + 1 < len {
                    v.data[i + 1] as f32
                } else if v.looped {
                    v.data[0] as f32
                } else {
                    0.0
                };
                let s = (s0 + (s1 - s0) * t) / 32768.0;
                out[2 * f] += s * v.gl;
                out[2 * f + 1] += s * v.gr;
                v.pos += step;
            }
        }
        self.voices.retain(|v| v.paused || v.looped || (v.pos < v.data.len() as f64));

        if let (Some(m), false) = (self.music.as_mut(), self.music_paused) {
            let step = m.stream.sample_rate as f64 / rate;
            let looping = self.cd_track > 0;
            let g = self.music_gain;
            let mut ended = false;
            for f in 0..frames {
                let t = m.frac as f32;
                out[2 * f] += (m.a[0] + (m.b[0] - m.a[0]) * t) * g;
                out[2 * f + 1] += (m.a[1] + (m.b[1] - m.a[1]) * t) * g;
                m.frac += step;
                while m.frac >= 1.0 {
                    m.frac -= 1.0;
                    m.a = m.b;
                    match m.next_frame(looping) {
                        Some(n) => m.b = n,
                        None => {
                            ended = true;
                            break;
                        }
                    }
                }
                if ended {
                    break;
                }
            }
            if ended {
                // MCI "play complete" notification -> LoopSong (pcaudio.c:979). Looping
                // tracks never get here (rewound gaplessly above).
                self.music = None;
                self.cd_track = 0;
            }
        }

        if self.master != 1.0 {
            out.iter_mut().for_each(|s| *s *= self.master);
        }
    }

    /// Advances time by `frames` without producing output (muted/headless runs).
    pub fn skip(&mut self, frames: usize) {
        let mut scratch = std::mem::take(&mut self.scratch);
        let mut left = frames;
        while left > 0 {
            let n = left.min(1024);
            scratch.resize(n * 2, 0.0);
            self.render(&mut scratch);
            left -= n;
        }
        self.scratch = scratch;
    }
}
