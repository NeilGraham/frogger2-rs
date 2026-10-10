//! Full-screen Bink playback (`StartVideoPlayback`, `FMVPLAY_MODE`), skippable.

use f2_audio::AudioEngine;
use f2_engine::{Blend, Quad2D, Renderer, TexHandle};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

/// Decoded audio track: interleaved PCM, channels, sample rate.
type AudioResult = Result<(Vec<i16>, u16, u32), String>;

/// Playback clock for FMVs: advances with wall time and is slewed towards the audio
/// playhead, so the video moves smoothly even when the audio position is only updated
/// coarsely (per device callback, or per decode chunk) and stays locked to the sound.
#[derive(Clone, Debug, Default)]
pub struct VideoClock {
    t: f64,
    last_audio: Option<f64>,
    /// Wall time since `last_audio` last changed.
    since: f64,
    /// Average step between successive audio readings (the device callback period). The
    /// playhead counts frames already handed to the device, which are heard about one
    /// callback later, so the audible position is estimated as `reading - lag + since`.
    lag: f64,
}

impl VideoClock {
    /// Beyond this error (seconds) the clock jumps to the audio instead of slewing.
    const SNAP: f64 = 0.5;
    /// Longest extrapolation past an unchanged audio reading (audio stalled or ended).
    const MAX_EXTRAP: f64 = 0.25;
    /// Readings that move further than this (s) are seeks/stalls, not callback steps.
    const MAX_LAG: f64 = 0.1;
    /// Proportional correction gain (1/s); the correction is limited to ±50% of `dt`, so
    /// the clock always runs forward at 0.5x..1.5x wall speed.
    const GAIN: f64 = 4.0;

    pub fn new(start: f64) -> VideoClock {
        VideoClock { t: start, ..Default::default() }
    }

    pub fn time(&self) -> f64 {
        self.t
    }

    /// Estimated audio output latency (one callback period), seconds.
    pub fn lag(&self) -> f64 {
        self.lag
    }

    /// Advances by `dt` seconds of wall time; `audio` is the audio playhead in seconds
    /// (None when there is no audio clock).
    pub fn advance(&mut self, dt: f64, audio: Option<f64>) -> f64 {
        self.t += dt;
        match audio {
            Some(a) => {
                if self.last_audio != Some(a) {
                    if let Some(step) = self.last_audio.map(|p| a - p).filter(|s| *s > 0.0 && *s <= Self::MAX_LAG) {
                        self.lag += (step - self.lag) * 0.1;
                    }
                    self.last_audio = Some(a);
                    self.since = 0.0;
                } else {
                    self.since += dt;
                }
                let est = a - self.lag + self.since.min(Self::MAX_EXTRAP);
                let err = est - self.t;
                if err.abs() > Self::SNAP {
                    self.t = est;
                } else {
                    self.t += (err * (dt * Self::GAIN).min(1.0)).clamp(-0.5 * dt, 0.5 * dt);
                }
            }
            None => self.last_audio = None,
        }
        self.t
    }
}

/// Presentation statistics (`F2_VIDEO_STATS=1`): distinct frames shown and A/V drift.
#[derive(Default)]
struct Stats {
    start: Option<Instant>,
    /// Wall time (s since start) of each newly presented frame.
    shown: Vec<f64>,
    /// Video clock minus audio playhead, per update (s).
    drift: Vec<f64>,
    /// Estimated audio callback period (s), last value.
    lag: f64,
    redraws: u32,
}

pub struct VideoPlayer {
    pub name: String,
    bink: f2_bink::BinkFile,
    /// The latest decoded frame (RGBA), uploaded at draw time.
    pending: Option<(u32, u32, Vec<u8>)>,
    image: Option<TexHandle>,
    decoded: u32,
    pub finished: bool,
    /// Playback position (seconds): wall time slewed to the audio playhead.
    clock: VideoClock,
    /// Latest audio playhead (seconds into the video), for the stats.
    audio_t: Option<f64>,
    stats: Option<Stats>,
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
            clock: VideoClock::new(0.0),
            audio_t: None,
            stats: std::env::var_os("F2_VIDEO_STATS").map(|_| Stats::default()),
            wait: 0.0,
            audio_rx,
            audio_rate: None,
            audio_offset: 0,
        })
    }

    pub fn update(&mut self, dt: f64, audio: &mut AudioEngine) {
        if let Some(rx) = &self.audio_rx {
            match rx.try_recv() {
                Ok(Ok((pcm, channels, rate))) => {
                    self.audio_rx = None;
                    // If the video already started (audio decode took over 3 s), start the
                    // sound at the same point rather than from the beginning.
                    let offset = (self.clock.time() * rate as f64) as u64;
                    let skip = (offset as usize).saturating_mul(channels.max(1) as usize).min(pcm.len());
                    let wav = f2_audio::wav::encode_wav(channels, rate, &pcm[skip..]);
                    if let Ok(s) = f2_audio::wav::WavStream::from_reader(Box::new(std::io::Cursor::new(wav))) {
                        audio.play_music_stream(Some(s), 99, false);
                        self.audio_rate = Some(rate);
                        self.audio_offset = offset;
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
        if self.finished {
            return;
        }
        // The audio playhead is the master clock; it moves per device callback (and used to
        // be read per 4096-frame chunk, ~5 Hz for 22 kHz tracks), so the video clock runs on
        // wall time and is slewed towards it.
        self.audio_t = match (self.audio_rate, audio.is_realtime(), audio.music_playhead()) {
            (Some(rate), true, Some(pos)) => Some((pos + self.audio_offset as f64) / rate as f64),
            _ => None,
        };
        let t = self.clock.advance(dt, self.audio_t);
        if let (Some(st), Some(a)) = (&mut self.stats, self.audio_t) {
            st.drift.push(t - a);
            st.lag = self.clock.lag();
        }
        let target = (t * self.fps()) as u32 + 1;
        if self.decoded < target {
            self.decode((target - self.decoded).min(8));
        }
    }

    fn fps(&self) -> f64 {
        let (num, den) = self.bink.fps();
        if den > 0 && num > 0 {
            num as f64 / den as f64
        } else {
            25.0
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
        if let Some(st) = &mut self.stats {
            let start = *st.start.get_or_insert_with(Instant::now);
            st.redraws += 1;
            if self.pending.is_some() {
                st.shown.push(start.elapsed().as_secs_f64());
            }
        }
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

    /// Prints the `F2_VIDEO_STATS` summary: distinct frames per second over the first 5 s
    /// window (from 0.5 s, past the start-up hold) and A/V drift.
    fn report_stats(&self) {
        let Some(st) = &self.stats else { return };
        let total = st.shown.last().copied().unwrap_or(0.0);
        let win = |a: f64, b: f64| st.shown.iter().filter(|&&t| t >= a && t < b).count();
        let w5 = win(0.5, 5.5);
        let gaps: Vec<f64> = st.shown.windows(2).map(|w| w[1] - w[0]).collect();
        let max_gap = gaps.iter().skip(1).fold(0.0f64, |m, &g| m.max(g));
        let (mean, max) = if st.drift.is_empty() {
            (f64::NAN, f64::NAN)
        } else {
            let n = st.drift.len() as f64;
            (st.drift.iter().sum::<f64>() / n, st.drift.iter().fold(0.0f64, |m, d| m.max(d.abs())))
        };
        eprintln!(
            "frogger: video-stats {}: {} distinct frames in {:.2}s ({} redraws); 0.5-5.5s window: {} frames = {:.1} fps; max gap {:.0} ms; A/V drift (video clock - audio playhead) mean {:+.1} ms, max |{:.1}| ms over {} samples; audio callback period ~{:.1} ms (heard ~1 period after the playhead)",
            self.name,
            st.shown.len(),
            total,
            st.redraws,
            w5,
            w5 as f64 / 5.0,
            max_gap * 1e3,
            mean * 1e3,
            max * 1e3,
            st.drift.len(),
            st.lag * 1e3
        );
    }

    /// Frees the frame texture.
    pub fn release(&mut self, r: &mut Renderer) {
        self.report_stats();
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
    Ok((pcm, info.channels as u16, info.sample_rate))
}

#[cfg(test)]
mod tests {
    use super::VideoClock;

    /// Drives a clock at `hz` display refresh for `secs` against an audio playhead that is
    /// only updated every `step` seconds (quantised to the step, like the device callback or
    /// the old 4096-frame chunk read position). Returns (distinct 25 fps frames shown in
    /// 0.5..5.5 s, max |clock - true time| after 1 s, clock monotonic).
    fn simulate(hz: f64, step: f64, secs: f64) -> (usize, f64, bool) {
        let mut c = VideoClock::new(0.0);
        let dt = 1.0 / hz;
        let (mut now, mut prev_t, mut prev_frame) = (0.0f64, 0.0f64, u32::MAX);
        let (mut frames, mut max_err, mut mono) = (0usize, 0.0f64, true);
        while now < secs {
            now += dt;
            let reading = (now / step).floor() * step;
            let t = c.advance(dt, Some(reading));
            mono &= t >= prev_t;
            prev_t = t;
            let frame = (t * 25.0) as u32;
            if frame != prev_frame && (0.5..5.5).contains(&now) {
                frames += 1;
            }
            prev_frame = frame;
            if now > 1.0 {
                max_err = max_err.max((t - now).abs());
            }
        }
        (frames, max_err, mono)
    }

    #[test]
    fn chunky_audio_clock_gives_full_frame_rate() {
        // 22050 Hz track read in 4096-frame chunks: the position moves in 186 ms steps,
        // which used to show ~5 distinct frames per second.
        let chunk = 4096.0 / 22050.0;
        for hz in [60.0, 144.0, 165.0, 240.0] {
            let (frames, err, mono) = simulate(hz, chunk, 6.0);
            assert!((123..=127).contains(&frames), "{hz} Hz: {frames} frames in 5 s");
            assert!(err < 0.03, "{hz} Hz: max error {err}");
            assert!(mono, "{hz} Hz: clock went backwards");
        }
        // The legacy clock (position used as-is) on the same input: ~5.4 fps.
        let mut shown = std::collections::BTreeSet::new();
        for i in 0..(144 * 5) {
            let now = 0.5 + i as f64 / 144.0;
            shown.insert((((now / chunk).floor() * chunk) * 25.0) as u32);
        }
        assert!(shown.len() <= 28, "{}", shown.len());
    }

    #[test]
    fn callback_audio_clock_compensates_latency() {
        // 10 ms device callbacks: the playhead is one period ahead of what is heard; the
        // clock settles on reading - period (+ time since the reading).
        let (frames, _, mono) = simulate(144.0, 0.01, 6.0);
        assert!((123..=127).contains(&frames), "{frames}");
        assert!(mono);
        let mut c = VideoClock::new(0.0);
        let mut now = 0.0f64;
        for _ in 0..(144 * 4) {
            now += 1.0 / 144.0;
            c.advance(1.0 / 144.0, Some((now / 0.01).floor() * 0.01));
        }
        assert!((c.lag() - 0.01).abs() < 0.002, "lag {}", c.lag());
        assert!((c.time() - (now - 0.01)).abs() < 0.012, "clock {} wall {now}", c.time());
    }

    #[test]
    fn clock_follows_wall_time_without_audio_and_snaps_to_late_audio() {
        let mut c = VideoClock::new(0.0);
        for _ in 0..60 {
            c.advance(1.0 / 60.0, None);
        }
        assert!((c.time() - 1.0).abs() < 1e-9);
        // Audio appears 2 s further on (stall/seek): jump rather than crawl.
        c.advance(1.0 / 60.0, Some(3.0));
        assert!((c.time() - 3.0).abs() < 1e-9, "{}", c.time());
        // A small offset is slewed out within two seconds, never moving backwards.
        let mut c = VideoClock::new(0.2);
        let (mut a, mut prev) = (0.0, 0.2);
        for _ in 0..120 {
            a += 1.0 / 60.0;
            let t = c.advance(1.0 / 60.0, Some(a));
            assert!(t >= prev);
            prev = t;
        }
        assert!((c.time() - a).abs() < 0.03, "{} vs {a}", c.time());
    }
}
