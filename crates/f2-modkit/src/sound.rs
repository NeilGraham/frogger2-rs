//! WAV / FLAC → `f2_formats::wus::Sample` (16-bit mono PCM).
//!
//! Any PCM WAV (8/16/24/32-bit integer, 32/64-bit float, WAVE_FORMAT_EXTENSIBLE,
//! any channel count and rate) or FLAC is decoded, mixed down to mono and
//! converted to signed 16-bit. The rate is kept when it is one the retail
//! banks use ([`NATIVE_RATES`]); otherwise it is resampled (linear) to the
//! replaced sample's rate, or to 22050 Hz for new samples. Pitch changes in
//! the game set an absolute playback frequency, so matching the replaced
//! sample's rate keeps pitched effects sounding right.
//!
//! Looping: a `smpl` chunk with at least one loop marks the sample looped;
//! otherwise the replaced sample's flag is kept (new samples don't loop).

use f2_formats::crc::crc;
use f2_formats::wus::{Sample, SoundBank};

use crate::Error;

/// Sample rates the retail sound banks use.
pub const NATIVE_RATES: [u32; 4] = [11025, 11050, 22050, 44100];
/// Rate for new samples whose source rate is not native.
pub const DEFAULT_RATE: u32 = 22050;

/// Decoded audio: mono f32 in [-1, 1].
#[derive(Clone, Debug)]
pub struct Pcm {
    pub rate: u32,
    pub mono: Vec<f32>,
    /// `smpl` chunk with a loop present.
    pub looped: Option<bool>,
}

fn bad(m: impl Into<String>) -> Error {
    Error::new(format!("wav: {}", m.into()))
}

/// Parses a RIFF/WAVE file.
pub fn decode_wav(data: &[u8]) -> Result<Pcm, Error> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(bad("not a RIFF/WAVE file"));
    }
    let mut o = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut pcm: Option<&[u8]> = None;
    let mut looped = None;
    while o + 8 <= data.len() {
        let id = &data[o..o + 4];
        let len = u32::from_le_bytes(data[o + 4..o + 8].try_into().unwrap()) as usize;
        let body = &data[o + 8..(o + 8).saturating_add(len).min(data.len())];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(bad("short fmt chunk"));
                }
                let mut tag = u16::from_le_bytes([body[0], body[1]]);
                if tag == 0xFFFE && body.len() >= 26 {
                    // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID starts with the real tag.
                    tag = u16::from_le_bytes([body[24], body[25]]);
                }
                let ch = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes([body[14], body[15]]);
                fmt = Some((tag, ch, rate, bits));
            }
            b"data" => pcm = Some(body),
            b"smpl" if body.len() >= 36 => {
                looped = Some(u32::from_le_bytes(body[28..32].try_into().unwrap()) > 0);
            }
            _ => {}
        }
        o += 8 + len + (len & 1);
    }
    let (tag, ch, rate, bits) = fmt.ok_or_else(|| bad("no fmt chunk"))?;
    let pcm = pcm.ok_or_else(|| bad("no data chunk"))?;
    if ch == 0 || rate == 0 {
        return Err(bad("zero channels or rate"));
    }
    let bps = usize::from(bits).div_ceil(8);
    let frame = bps * usize::from(ch);
    type Conv = Box<dyn Fn(&[u8]) -> f32>;
    let conv: Conv = match (tag, bits) {
        (1, 8) => Box::new(|b: &[u8]| (f32::from(b[0]) - 128.0) / 128.0),
        (1, 16) => Box::new(|b: &[u8]| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0),
        (1, 24) => Box::new(|b: &[u8]| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0),
        (1, 32) => Box::new(|b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0),
        (3, 32) => Box::new(|b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        (3, 64) => Box::new(|b: &[u8]| f64::from_le_bytes(b[0..8].try_into().unwrap()) as f32),
        _ => return Err(bad(format!("unsupported format tag {tag} / {bits}-bit (use PCM or float)"))),
    };
    let mono = pcm.chunks_exact(frame).map(|f| f.chunks_exact(bps).map(&conv).sum::<f32>() / f32::from(ch)).collect();
    Ok(Pcm { rate, mono, looped })
}

/// Decodes a FLAC file (via `f2-audio`'s decoder).
pub fn decode_flac(data: &[u8]) -> Result<Pcm, Error> {
    let mut s = f2_audio::wav::WavStream::from_flac_reader(Box::new(std::io::Cursor::new(data.to_vec()))).map_err(|e| Error::new(format!("flac: {e}")))?;
    let mut mono = Vec::with_capacity(s.frames as usize);
    let mut buf = vec![0f32; 8192];
    loop {
        let n = s.read_stereo(&mut buf).map_err(|e| Error::new(format!("flac: {e}")))?;
        if n == 0 {
            break;
        }
        // Mono sources are duplicated by read_stereo, so averaging is exact for them.
        mono.extend(buf[..2 * n].as_chunks::<2>().0.iter().map(|c| (c[0] + c[1]) * 0.5));
    }
    Ok(Pcm { rate: s.sample_rate, mono, looped: None })
}

/// Decodes by extension (`wav`, `flac`).
pub fn decode_audio(ext: &str, data: &[u8]) -> Result<Pcm, Error> {
    match ext.to_ascii_lowercase().as_str() {
        "wav" => decode_wav(data),
        "flac" => decode_flac(data),
        "ogg" => Err(Error::new("ogg: Ogg Vorbis is not supported yet; convert to .wav or .flac")),
        e => Err(Error::new(format!("unsupported audio extension .{e}"))),
    }
}

/// Linear resampling.
pub fn resample(src: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || src.is_empty() {
        return src.to_vec();
    }
    let n = ((src.len() as u64 * u64::from(to)) / u64::from(from)).max(1) as usize;
    let step = f64::from(from) / f64::from(to);
    (0..n)
        .map(|i| {
            let p = i as f64 * step;
            let k = p as usize;
            let t = (p - k as f64) as f32;
            let a = src[k.min(src.len() - 1)];
            let b = src[(k + 1).min(src.len() - 1)];
            a + (b - a) * t
        })
        .collect()
}

/// Sample uid for an override file stem: 8 hex digits are a raw uid (as
/// `f2-export` names samples), anything else is `crc(lowercase name)`.
pub fn sample_uid(name: &str) -> u32 {
    if name.len() == 8 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
        return u32::from_str_radix(name, 16).unwrap();
    }
    crc(&name.to_ascii_lowercase())
}

/// Builds a bank sample from decoded audio. `replacing` is the retail sample
/// with the same uid, if any.
pub fn sample_from_pcm(uid: u32, pcm: &Pcm, replacing: Option<&Sample>) -> Sample {
    let rate = if NATIVE_RATES.contains(&pcm.rate) { pcm.rate } else { replacing.map(|s| s.sample_rate).filter(|r| *r > 0).unwrap_or(DEFAULT_RATE) };
    let data = resample(&pcm.mono, pcm.rate, rate);
    let pcm_bytes = data.iter().flat_map(|&v| ((v.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes()).collect();
    Sample {
        uid,
        looped: pcm.looped.unwrap_or_else(|| replacing.is_some_and(|s| s.looped)),
        format_tag: 1,
        channels: 1,
        sample_rate: rate,
        bits: 16,
        pcm: pcm_bytes,
    }
}

/// Serialises a bank in `.wus` layout ([`SoundBank::parse`] inverse).
pub fn write_soundbank(bank: &SoundBank) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&(bank.samples.len() as u32).to_le_bytes());
    for s in &bank.samples {
        let block = s.channels * s.bits / 8;
        v.extend_from_slice(&(s.pcm.len() as u32).to_le_bytes());
        v.extend_from_slice(&s.uid.to_le_bytes());
        v.extend_from_slice(&u32::from(s.looped).to_le_bytes());
        v.extend_from_slice(&s.format_tag.to_le_bytes());
        v.extend_from_slice(&s.channels.to_le_bytes());
        v.extend_from_slice(&s.sample_rate.to_le_bytes());
        v.extend_from_slice(&(s.sample_rate * u32::from(block)).to_le_bytes());
        v.extend_from_slice(&block.to_le_bytes());
        v.extend_from_slice(&s.bits.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(&s.pcm);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(tag: u16, ch: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&ch.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * u32::from(ch * bits / 8)).to_le_bytes());
        v.extend_from_slice(&(ch * bits / 8).to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn stereo_8bit_downmix() {
        let p = decode_wav(&wav(1, 2, 11025, 8, &[255, 1, 128, 128])).unwrap();
        assert_eq!(p.rate, 11025);
        assert_eq!(p.mono.len(), 2);
        assert!(p.mono[0].abs() < 0.01 && p.mono[1] == 0.0);
        let s = sample_from_pcm(sample_uid("HopOnGrass"), &p, None);
        assert_eq!((s.channels, s.bits, s.sample_rate, s.pcm.len()), (1, 16, 11025, 4));
        assert_eq!(s.uid, crc("hopongrass"));
    }

    #[test]
    fn resamples_odd_rates_to_replaced_rate() {
        let data: Vec<u8> = (0..48000i32).flat_map(|i| (((i % 100) * 300) as i16).to_le_bytes()).collect();
        let p = decode_wav(&wav(1, 1, 48000, 16, &data)).unwrap();
        let old = Sample { uid: 1, looped: true, format_tag: 1, channels: 1, sample_rate: 11025, bits: 16, pcm: vec![] };
        let s = sample_from_pcm(1, &p, Some(&old));
        assert_eq!(s.sample_rate, 11025);
        assert_eq!(s.pcm.len(), 11025 * 2);
        assert!(s.looped);
        assert_eq!(sample_uid("00a1b2c3"), 0x00a1b2c3);
        let b = SoundBank { samples: vec![s] };
        let p = SoundBank::parse(&write_soundbank(&b)).unwrap();
        assert_eq!(p.samples[0].pcm, b.samples[0].pcm);
    }

    #[test]
    fn float_and_rejects() {
        let p = decode_wav(&wav(3, 1, 22050, 32, &0.5f32.to_le_bytes())).unwrap();
        assert_eq!(p.mono, vec![0.5]);
        assert!(decode_wav(&wav(2, 1, 22050, 4, &[0])).is_err());
        assert!(decode_wav(b"nope").is_err());
        assert!(decode_audio("ogg", b"").is_err());
    }
}
