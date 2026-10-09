//! Pluggable audio outputs: a real device through cpal, or nothing (offline/muted).

use crate::mixer::Mixer;
use std::sync::{Arc, Mutex};

/// Where the mixer's output goes. Real-time outputs pull from the shared mixer on their own
/// thread; offline outputs leave time advancement to [`crate::AudioEngine::render_offline`] /
/// [`crate::AudioEngine::skip`].
pub trait AudioOutput {
    /// Device sample rate (the mixer resamples everything to this).
    fn sample_rate(&self) -> u32;
    /// Starts pulling audio from `mixer`. Called once by the engine.
    fn start(&mut self, mixer: Arc<Mutex<Mixer>>) -> Result<(), String>;
    /// `true` when a device thread drives the mixer (offline rendering is then unavailable).
    fn is_realtime(&self) -> bool;
}

/// No device: used for tests, headless tools and muted background parity runs. Voices and
/// music still advance deterministically when the caller renders or skips frames.
pub struct NullOutput {
    rate: u32,
}

impl NullOutput {
    pub fn new(rate: u32) -> NullOutput {
        NullOutput { rate }
    }
}

impl Default for NullOutput {
    fn default() -> Self {
        NullOutput::new(44100)
    }
}

impl AudioOutput for NullOutput {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn start(&mut self, _mixer: Arc<Mutex<Mixer>>) -> Result<(), String> {
        Ok(())
    }
    fn is_realtime(&self) -> bool {
        false
    }
}

#[cfg(feature = "cpal")]
pub use self::cpal_out::CpalOutput;

#[cfg(feature = "cpal")]
mod cpal_out {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample};

    /// The default output device of the default host (PulseAudio/ALSA on Linux/WSLg).
    pub struct CpalOutput {
        device: cpal::Device,
        config: cpal::StreamConfig,
        format: SampleFormat,
        stream: Option<cpal::Stream>,
    }

    impl CpalOutput {
        pub fn new() -> Result<CpalOutput, String> {
            let host = cpal::default_host();
            let device = host.default_output_device().ok_or("no default audio output device")?;
            let sup = device.default_output_config().map_err(|e| e.to_string())?;
            let format = sup.sample_format();
            let config: cpal::StreamConfig = sup.into();
            Ok(CpalOutput { device, config, format, stream: None })
        }

        fn build<T: SizedSample + FromSample<f32>>(&self, mixer: Arc<Mutex<Mixer>>) -> Result<cpal::Stream, String> {
            let channels = self.config.channels as usize;
            let mut buf: Vec<f32> = Vec::new();
            self.device
                .build_output_stream(
                    &self.config,
                    move |data: &mut [T], _| {
                        let frames = data.len() / channels.max(1);
                        buf.resize(frames * 2, 0.0);
                        match mixer.lock() {
                            Ok(mut m) => m.render(&mut buf),
                            Err(_) => buf.iter_mut().for_each(|s| *s = 0.0),
                        }
                        for (f, frame) in data.chunks_mut(channels.max(1)).enumerate() {
                            for (c, s) in frame.iter_mut().enumerate() {
                                let v = match (channels, c) {
                                    (1, _) => 0.5 * (buf[2 * f] + buf[2 * f + 1]),
                                    (_, 0) => buf[2 * f],
                                    (_, 1) => buf[2 * f + 1],
                                    _ => 0.0,
                                };
                                *s = T::from_sample(v.clamp(-1.0, 1.0));
                            }
                        }
                    },
                    |e| eprintln!("f2-audio: output stream error: {e}"),
                    None,
                )
                .map_err(|e| e.to_string())
        }
    }

    impl AudioOutput for CpalOutput {
        fn sample_rate(&self) -> u32 {
            self.config.sample_rate.0
        }

        fn start(&mut self, mixer: Arc<Mutex<Mixer>>) -> Result<(), String> {
            let stream = match self.format {
                SampleFormat::F32 => self.build::<f32>(mixer)?,
                SampleFormat::I16 => self.build::<i16>(mixer)?,
                SampleFormat::U16 => self.build::<u16>(mixer)?,
                SampleFormat::I32 => self.build::<i32>(mixer)?,
                f => return Err(format!("unsupported sample format {f:?}")),
            };
            stream.play().map_err(|e| e.to_string())?;
            self.stream = Some(stream);
            Ok(())
        }

        fn is_realtime(&self) -> bool {
            true
        }
    }
}
