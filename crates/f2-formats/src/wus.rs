//! Sound banks `sfx/*.wus` (`LoadSfxSet`, `PC/pcaudio.c:156`; builder `soundbanker`).
//!
//! ```text
//! u32 numSamples            // the game only reads the low byte as signed char
//! numSamples × { u32 pcmLength; u32 uid = crc(lowercase name without ".wav");
//!                u32 loop;  WAVEFORMATEX (18 bytes, packed; cbSize is junk); u8 pcm[pcmLength] }
//! ```

use crate::prelude::*;

use crate::read::{Reader, Result};

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct Sample {
    pub uid: u32,
    pub looped: bool,
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits: u16,
    pub pcm: Vec<u8>,
}

impl Sample {
    /// Signed 16-bit samples (the retail banks are all 16-bit PCM).
    pub fn pcm_i16(&self) -> Vec<i16> {
        match self.bits {
            16 => self.pcm.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect(),
            8 => self.pcm.iter().map(|&b| ((b as i16) - 128) << 8).collect(),
            _ => Vec::new(),
        }
    }

    /// A standalone RIFF/WAVE file.
    pub fn to_wav(&self) -> Vec<u8> {
        let mut h = Vec::with_capacity(44 + self.pcm.len());
        let block = self.channels * self.bits / 8;
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&(36 + self.pcm.len() as u32).to_le_bytes());
        h.extend_from_slice(b"WAVEfmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        h.extend_from_slice(&self.format_tag.to_le_bytes());
        h.extend_from_slice(&self.channels.to_le_bytes());
        h.extend_from_slice(&self.sample_rate.to_le_bytes());
        h.extend_from_slice(&(self.sample_rate * u32::from(block)).to_le_bytes());
        h.extend_from_slice(&block.to_le_bytes());
        h.extend_from_slice(&self.bits.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&(self.pcm.len() as u32).to_le_bytes());
        h.extend_from_slice(&self.pcm);
        h
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct SoundBank {
    pub samples: Vec<Sample>,
}

impl SoundBank {
    pub fn parse(data: &[u8]) -> Result<SoundBank> {
        let mut r = Reader::new("wus", data);
        let n = r.u32()?;
        if n > 4096 {
            return Err(r.err(format!("implausible sample count {n}")));
        }
        let mut samples = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let len = r.u32()? as usize;
            let uid = r.u32()?;
            let looped = r.u32()? != 0;
            let format_tag = r.u16()?;
            let channels = r.u16()?;
            let sample_rate = r.u32()?;
            let _avg = r.u32()?;
            let _block = r.u16()?;
            let bits = r.u16()?;
            let _cb = r.u16()?;
            let pcm = r.bytes(len)?.to_vec();
            samples.push(Sample { uid, looped, format_tag, channels, sample_rate, bits, pcm });
        }
        if !r.is_empty() {
            return Err(r.err(format!("{} trailing bytes", r.remaining())));
        }
        Ok(SoundBank { samples })
    }

    pub fn find(&self, uid: u32) -> Option<&Sample> {
        self.samples.iter().find(|s| s.uid == uid)
    }
}
