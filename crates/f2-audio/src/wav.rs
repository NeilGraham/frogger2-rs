//! Minimal streaming RIFF/WAVE reader for ripped CD tracks (16-bit PCM, any channel count/rate).

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// A PCM WAV file read incrementally (the music tracks are ~10-50 MB each).
pub struct WavStream {
    reader: Box<dyn ReadSeek>,
    pub channels: u16,
    pub sample_rate: u32,
    data_start: u64,
    /// Total frames in the data chunk.
    pub frames: u64,
    pos: u64,
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

impl WavStream {
    pub fn open(path: &Path) -> io::Result<WavStream> {
        Self::from_reader(Box::new(BufReader::with_capacity(64 * 1024, File::open(path)?)))
    }

    pub fn from_reader(mut r: Box<dyn ReadSeek>) -> io::Result<WavStream> {
        let mut hdr = [0u8; 12];
        r.read_exact(&mut hdr)?;
        if &hdr[0..4] != b"RIFF" || &hdr[8..12] != b"WAVE" {
            return Err(bad("not a RIFF/WAVE file"));
        }
        let mut fmt: Option<(u16, u16, u32, u16)> = None;
        loop {
            let mut ch = [0u8; 8];
            r.read_exact(&mut ch)?;
            let len = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64;
            match &ch[0..4] {
                b"fmt " => {
                    let mut f = vec![0u8; len as usize];
                    r.read_exact(&mut f)?;
                    if f.len() < 16 {
                        return Err(bad("short fmt chunk"));
                    }
                    let tag = u16::from_le_bytes([f[0], f[1]]);
                    let chans = u16::from_le_bytes([f[2], f[3]]);
                    let rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
                    let bits = u16::from_le_bytes([f[14], f[15]]);
                    fmt = Some((tag, chans, rate, bits));
                    if len & 1 == 1 {
                        r.seek(SeekFrom::Current(1))?;
                    }
                }
                b"data" => {
                    let (tag, chans, rate, bits) = fmt.ok_or_else(|| bad("data before fmt"))?;
                    if tag != 1 || bits != 16 || chans == 0 || rate == 0 {
                        return Err(bad("only 16-bit PCM WAV is supported"));
                    }
                    let data_start = r.stream_position()?;
                    // Clamp to the real file size (rippers sometimes write 0xFFFFFFFF).
                    let end = r.seek(SeekFrom::End(0))?;
                    let len = len.min(end.saturating_sub(data_start));
                    r.seek(SeekFrom::Start(data_start))?;
                    return Ok(WavStream { reader: r, channels: chans, sample_rate: rate, data_start, frames: len / (2 * chans as u64), pos: 0 });
                }
                _ => {
                    r.seek(SeekFrom::Current((len + (len & 1)) as i64))?;
                }
            }
        }
    }

    pub fn position(&self) -> u64 {
        self.pos
    }

    pub fn rewind(&mut self) -> io::Result<()> {
        self.reader.seek(SeekFrom::Start(self.data_start))?;
        self.pos = 0;
        Ok(())
    }

    /// Reads up to `out.len() / 2` frames as stereo f32 (mono is duplicated, extra channels
    /// dropped). Returns the number of frames read; 0 means end of data.
    pub fn read_stereo(&mut self, out: &mut [f32]) -> io::Result<usize> {
        let ch = self.channels as usize;
        let want = ((out.len() / 2) as u64).min(self.frames - self.pos) as usize;
        if want == 0 {
            return Ok(0);
        }
        let mut raw = vec![0u8; want * ch * 2];
        self.reader.read_exact(&mut raw)?;
        for (i, f) in raw.chunks_exact(ch * 2).enumerate() {
            let l = i16::from_le_bytes([f[0], f[1]]) as f32 / 32768.0;
            let r = if ch > 1 { i16::from_le_bytes([f[2], f[3]]) as f32 / 32768.0 } else { l };
            out[2 * i] = l;
            out[2 * i + 1] = r;
        }
        self.pos += want as u64;
        Ok(want)
    }
}

/// Builds a 16-bit PCM WAV in memory (tests and tools).
pub fn encode_wav(channels: u16, rate: u32, samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    v.extend_from_slice(&(channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}
