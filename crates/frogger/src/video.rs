//! Full-screen Bink playback (`StartVideoPlayback`, `FMVPLAY_MODE`), skippable.

use f2_audio::AudioEngine;
use f2_engine::{Blend, Quad2D, Renderer, TexHandle};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// Decoded audio track: WAV bytes and sample rate.
type AudioResult = Result<(Vec<u8>, u32), String>;

pub struct VideoPlayer {
    pub name: String,
    bink: f2_bink::BinkFile,
    /// The latest decoded frame (RGBA), uploaded at draw time.
    pending: Option<(u32, u32, Vec<u8>)>,
    image: Option<TexHandle>,
    decoded: u32,
    pub finished: bool,
    /// Wall-clock playback position (seconds), used when audio is not on a real device.
    clock: f64,
    /// Time spent waiting for the audio decode before starting.
    wait: f64,
    audio_rx: Option<Receiver<AudioResult>>,
    audio_rate: Option<u32>,
    audio_offset: u64,
}

impl VideoPlayer {
    /// Opens `video/<name>.bik`; the audio track is decoded on a background thread.
    pub fn open(data: &crate::data::Data, name: &str, with_audio: bool) -> Result<VideoPlayer, String> {
        let file = format!("video/{}.bik", name.to_ascii_lowercase());
        let bytes = data.read(&file)?;
        let bink = f2_bink::BinkFile::open(bytes.clone()).map_err(|e| format!("{file}: {e}"))?;
        let audio_rx = (with_audio && !bink.audio_tracks().is_empty()).then(|| {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(decode_bink_audio(bytes));
            });
            rx
        });
        Ok(VideoPlayer {
            name: name.to_string(),
            bink,
            pending: None,
            image: None,
            decoded: 0,
            finished: false,
            clock: 0.0,
            wait: 0.0,
            audio_rx,
            audio_rate: None,
            audio_offset: 0,
        })
    }

    pub fn update(&mut self, dt: f64, audio: &mut AudioEngine) {
        if let Some(rx) = &self.audio_rx {
            match rx.try_recv() {
                Ok(Ok((wav, rate))) => {
                    self.audio_rx = None;
                    if let Ok(s) = f2_audio::wav::WavStream::from_reader(Box::new(std::io::Cursor::new(wav))) {
                        audio.play_music_stream(Some(s), 99, false);
                        self.audio_rate = Some(rate);
                        self.audio_offset = (self.clock * rate as f64) as u64;
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("frogger: video audio: {e}");
                    self.audio_rx = None;
                }
                Err(TryRecvError::Empty) => {
                    // Hold the first frame for up to 3 s while the audio decodes.
                    self.wait += dt;
                    if self.wait < 3.0 {
                        if self.decoded == 0 {
                            self.decode(1);
                        }
                        return;
                    }
                }
                Err(TryRecvError::Disconnected) => self.audio_rx = None,
            }
        }
        if !self.finished {
            self.clock += dt;
        }
        let (num, den) = self.bink.fps();
        let fps = if den > 0 && num > 0 { num as f64 / den as f64 } else { 25.0 };
        let t = match (self.audio_rate, audio.is_realtime(), audio.music_position()) {
            (Some(rate), true, Some(pos)) => (pos + self.audio_offset) as f64 / rate as f64,
            _ => self.clock,
        };
        let target = (t * fps) as u32 + 1;
        if self.decoded < target {
            self.decode((target - self.decoded).min(8));
        }
    }

    fn decode(&mut self, n: u32) {
        let mut last = None;
        for _ in 0..n {
            match self.bink.decode_next() {
                Some(Ok(f)) => {
                    self.decoded += 1;
                    last = Some(f);
                }
                Some(Err(e)) => {
                    eprintln!("frogger: {}: {e}", self.name);
                    self.finished = true;
                    break;
                }
                None => {
                    self.finished = true;
                    break;
                }
            }
        }
        if let Some(f) = last {
            self.pending = Some((f.yuv.width, f.yuv.height, f.to_rgba8()));
        }
    }

    /// Queues the current frame, letterboxed to the screen.
    pub fn draw(&mut self, r: &mut Renderer, w: f32, h: f32) {
        if let Some((iw, ih, rgba)) = self.pending.take() {
            match self.image {
                Some(h) => r.textures.replace_image(h, iw, ih, rgba),
                None => self.image = Some(r.textures.add_image(iw, ih, rgba)),
            }
        }
        let Some(img) = self.image else { return };
        let (vw, vh) = (self.bink.width() as f32, self.bink.height() as f32);
        let s = (w / vw).min(h / vh);
        let (dw, dh) = (vw * s, vh * s);
        r.draw_quad(Quad2D::rect((w - dw) / 2.0, (h - dh) / 2.0, dw, dh, [0.0, 0.0, 1.0, 1.0], Some(img), [255; 4], Blend::Semi));
    }

    /// Drops the uploaded image handle (after the renderer was recreated).
    pub fn forget_image(&mut self) {
        self.image = None;
    }

    /// Frees the frame texture.
    pub fn release(&mut self, r: &mut Renderer) {
        if let Some(h) = self.image.take() {
            r.textures.free_image(h);
        }
    }
}

/// Decodes a whole Bink file's first audio track to an in-memory WAV.
fn decode_bink_audio(bytes: Vec<u8>) -> AudioResult {
    let mut b = f2_bink::BinkFile::open(bytes).map_err(|e| e.to_string())?;
    let info = *b.audio_tracks().first().ok_or("no audio track")?;
    let mut pcm: Vec<i16> = Vec::new();
    while let Some(f) = b.decode_next() {
        let f = f.map_err(|e| e.to_string())?;
        if let Some(a) = f.audio.first() {
            pcm.extend(a.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
        }
    }
    Ok((f2_audio::wav::encode_wav(info.channels as u16, info.sample_rate, &pcm), info.sample_rate))
}
