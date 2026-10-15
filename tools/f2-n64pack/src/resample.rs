//! Sample-rate conversion for the portable music path (`Ctx::portable`): a polyphase
//! windowed-sinc resampler with fixed taps, deterministic on every OS (filter table from the
//! pure-Rust `libm`, accumulation in `f64` in a fixed order, round half to even).
//!
//! Quality: 128 input taps per output sample, Blackman–Harris window, cut-off at 0.9 x the
//! lower Nyquist frequency (e.g. 10.8 kHz for 44.1 -> 24 kHz), plenty for music the N64
//! mixer plays at 32 kHz.

/// Taps per output sample (even).
pub const TAPS: usize = 128;
/// Cut-off as a fraction of the lower of the two Nyquist frequencies.
pub const CUTOFF: f64 = 0.9;

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Number of output frames for `frames` input frames: `ceil(frames * to / from)`.
pub fn output_len(frames: usize, from: u32, to: u32) -> usize {
    ((frames as u64 * to as u64).div_ceil(from as u64)) as usize
}

fn blackman_harris(x: f64) -> f64 {
    // x in [0, 1]
    let t = 2.0 * core::f64::consts::PI * x;
    0.35875 - 0.48829 * libm::cos(t) + 0.14128 * libm::cos(2.0 * t) - 0.01168 * libm::cos(3.0 * t)
}

/// Resamples interleaved 16-bit PCM with `channels` channels from `from` Hz to `to` Hz.
pub fn resample(pcm: &[i16], channels: usize, from: u32, to: u32) -> Vec<i16> {
    let channels = channels.max(1);
    let frames = pcm.len() / channels;
    if from == to || frames == 0 {
        return pcm[..frames * channels].to_vec();
    }
    let g = gcd(from as u64, to as u64);
    let (up, down) = ((to as u64 / g) as usize, (from as u64 / g) as usize);
    // Output frame n sits at input position n * down / up = (n * down) div up + phase / up.
    let half = TAPS / 2;
    // cut-off in cycles per input sample
    let fc = CUTOFF * (from.min(to) as f64) / (2.0 * from as f64);
    let mut table = vec![0f64; up * TAPS];
    for p in 0..up {
        let frac = p as f64 / up as f64;
        let mut sum = 0.0;
        for k in 0..TAPS {
            // input sample index offset relative to floor position: k - half + 1
            let x = (k as f64 - half as f64 + 1.0) - frac;
            let s = if x == 0.0 { 2.0 * fc } else { libm::sin(2.0 * core::f64::consts::PI * fc * x) / (core::f64::consts::PI * x) };
            let w = blackman_harris((x + half as f64) / TAPS as f64);
            table[p * TAPS + k] = s * w;
            sum += s * w;
        }
        // unity DC gain per phase
        for k in 0..TAPS {
            table[p * TAPS + k] /= sum;
        }
    }
    let out_frames = output_len(frames, from, to);
    let mut out = Vec::with_capacity(out_frames * channels);
    for n in 0..out_frames {
        let pos = n * down;
        let base = pos / up;
        let phase = pos % up;
        let taps = &table[phase * TAPS..(phase + 1) * TAPS];
        for c in 0..channels {
            let mut acc = 0.0f64;
            for (k, &t) in taps.iter().enumerate() {
                let i = base as isize + k as isize - half as isize + 1;
                // edges: hold the first / last sample
                let i = i.clamp(0, frames as isize - 1) as usize;
                acc += f64::from(pcm[i * channels + c]) * t;
            }
            out.push(acc.round_ties_even().clamp(-32768.0, 32767.0) as i16);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_and_identity() {
        assert_eq!(output_len(44100, 44100, 32000), 32000);
        assert_eq!(output_len(1, 44100, 32000), 1);
        let x: Vec<i16> = (0..100).map(|i| i * 3).collect();
        assert_eq!(resample(&x, 1, 22050, 22050), x);
    }

    #[test]
    fn dc_and_sine_survive() {
        let dc = vec![1000i16; 2 * 4410];
        let r = resample(&dc, 2, 44100, 24000);
        assert_eq!(r.len(), 2 * 2400);
        assert!(r[200..r.len() - 200].iter().all(|&s| (s - 1000).abs() <= 1));
        // 1 kHz sine: amplitude kept, frequency kept (zero crossings)
        let s: Vec<i16> = (0..44100).map(|i| (10000.0 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 44100.0).sin()) as i16).collect();
        let r = resample(&s, 1, 44100, 32000);
        let peak = r[1000..31000].iter().map(|&v| v.unsigned_abs()).max().unwrap();
        assert!((9900..=10100).contains(&peak), "{peak}");
        let crossings = r[1000..31000].windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        assert!((1870..=1880).contains(&crossings), "{crossings}");
    }

    #[test]
    fn deterministic() {
        let s: Vec<i16> = (0..5000).map(|i| ((i * 7919) % 65536 - 32768) as i16).collect();
        assert_eq!(resample(&s, 2, 44100, 22050), resample(&s, 2, 44100, 22050));
    }
}
