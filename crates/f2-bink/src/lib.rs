//! Bink 1 video and audio decoder (pure Rust).
//!
//! Decodes the Bink 1 files shipped with Frogger 2 (`BIKi`, 640x480 @ 25 fps, Bink audio
//! RDFT variant). Supports Bink 1 revisions 'c' to 'k' (including alpha planes, which are
//! decoded but not exposed); the old revision 'b' video and the DCT audio variant are rejected
//! with [`Error::Unsupported`].
//!
//! # License
//!
//! The container parser, video decoder and audio decoder are close translations of FFmpeg's
//! `libavformat/bink.c`, `libavcodec/bink.c`, `libavcodec/binkdsp.c`,
//! `libavcodec/binkaudio.c` and the tables of `libavcodec/binkdata.h`. FFmpeg is licensed
//! under the LGPL-2.1-or-later, so this crate is too (see `Cargo.toml`).
//!
//! # Example
//!
//! ```no_run
//! let mut bink = f2_bink::BinkFile::open(std::fs::read("01s.bik").unwrap()).unwrap();
//! while let Some(frame) = bink.decode_next() {
//!     let frame = frame.unwrap();
//!     let _rgba = frame.to_rgba8();
//! }
//! ```
#![forbid(unsafe_code)]

mod audio;
mod bits;
mod tables;
mod video;

use std::fmt;

/// Decoder errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file is not a Bink 1 file or its header is malformed.
    InvalidHeader(String),
    /// Corrupt frame data.
    InvalidData(String),
    /// A valid but unsupported feature.
    Unsupported(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidHeader(s) => write!(f, "invalid Bink header: {s}"),
            Error::InvalidData(s) => write!(f, "invalid Bink data: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported Bink feature: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// Audio track description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioTrackInfo {
    pub sample_rate: u32,
    pub channels: u32,
}

/// A decoded YUV 4:2:0 picture (planes cropped to the visible size).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Yuv420 {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    /// Bytes per row of `y` (= `width`).
    pub y_stride: usize,
    /// Bytes per row of `u` and `v` (= `(width + 1) / 2`).
    pub uv_stride: usize,
}

/// One decoded frame plus the audio stored alongside it.
#[derive(Debug, Clone)]
pub struct Frame {
    pub index: u32,
    pub yuv: Yuv420,
    /// Per audio track, interleaved `f32` samples (nominal range -1..1).
    pub audio: Vec<Vec<f32>>,
}

impl Frame {
    /// Convert to RGBA8 using BT.601 limited-range coefficients (like FFmpeg's default
    /// swscale conversion; chroma is nearest-neighbour upsampled, so results may differ from
    /// swscale by a few levels).
    pub fn to_rgba8(&self) -> Vec<u8> {
        let y4 = &self.yuv;
        let (w, h) = (y4.width as usize, y4.height as usize);
        let mut out = Vec::with_capacity(w * h * 4);
        let clip = |v: i32| (v >> 8).clamp(0, 255) as u8;
        for row in 0..h {
            for col in 0..w {
                let c = (y4.y[row * y4.y_stride + col] as i32 - 16) * 298;
                let ci = (row / 2) * y4.uv_stride + col / 2;
                let d = y4.u[ci] as i32 - 128;
                let e = y4.v[ci] as i32 - 128;
                out.push(clip(c + 409 * e + 128));
                out.push(clip(c - 100 * d - 208 * e + 128));
                out.push(clip(c + 516 * d + 128));
                out.push(255);
            }
        }
        out
    }
}

struct FrameIndex {
    pos: usize,
    size: usize,
    #[allow(dead_code)]
    keyframe: bool,
}

/// An opened Bink 1 file.
pub struct BinkFile {
    data: Vec<u8>,
    width: u32,
    height: u32,
    fps: (u32, u32),
    tracks: Vec<AudioTrackInfo>,
    index: Vec<FrameIndex>,
    next: usize,
    video: video::VideoDecoder,
    audio: Vec<audio::AudioDecoder>,
}

const BINK_AUD_STEREO: u16 = 0x2000;
const BINK_AUD_USEDCT: u16 = 0x1000;

struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}

impl Rd<'_> {
    fn u32(&mut self) -> Result<u32, Error> {
        let b = self.d.get(self.p..self.p + 4).ok_or_else(|| Error::InvalidHeader("truncated".into()))?;
        self.p += 4;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u16(&mut self) -> Result<u16, Error> {
        let b = self.d.get(self.p..self.p + 2).ok_or_else(|| Error::InvalidHeader("truncated".into()))?;
        self.p += 2;
        Ok(u16::from_le_bytes(b.try_into().unwrap()))
    }
}

impl BinkFile {
    /// Parse the header and frame index (port of FFmpeg `libavformat/bink.c` `read_header`).
    pub fn open(data: Vec<u8>) -> Result<BinkFile, Error> {
        let hdr = |s: &str| Error::InvalidHeader(s.to_string());
        let mut r = Rd { d: &data, p: 0 };
        let codec_tag = r.u32()?;
        let sig = codec_tag & 0xFF_FFFF;
        let revision = (codec_tag >> 24) as u8;
        if sig == u32::from_le_bytes(*b"KB2\0") {
            return Err(Error::Unsupported("Bink 2".into()));
        }
        if sig != u32::from_le_bytes(*b"BIK\0") {
            return Err(hdr("bad signature"));
        }
        let file_size = r.u32()? as u64 + 8;
        let frames = r.u32()?;
        if frames > 1_000_000 {
            return Err(hdr("more than 1000000 frames"));
        }
        if r.u32()? as u64 > file_size {
            return Err(hdr("largest frame size greater than file size"));
        }
        r.u32()?;
        let width = r.u32()?;
        let height = r.u32()?;
        let fps_num = r.u32()?;
        let fps_den = r.u32()?;
        if fps_num == 0 || fps_den == 0 {
            return Err(hdr("invalid fps"));
        }
        if width == 0 || height == 0 || width > 7680 || height > 4800 {
            return Err(hdr("invalid dimensions"));
        }
        let flags = r.u32()?;
        let num_tracks = r.u32()?;
        if num_tracks > 256 {
            return Err(hdr("too many audio tracks"));
        }
        if revision == b'k' {
            r.u32()?;
        }
        let mut tracks = Vec::new();
        let mut audio = Vec::new();
        if num_tracks > 0 {
            r.p += 4 * num_tracks as usize;
            let mut info = Vec::new();
            for _ in 0..num_tracks {
                let sample_rate = r.u16()? as u32;
                let f = r.u16()?;
                let channels = if f & BINK_AUD_STEREO != 0 { 2 } else { 1 };
                info.push((sample_rate, channels, f & BINK_AUD_USEDCT != 0));
            }
            r.p += 4 * num_tracks as usize; // track ids
            for (sample_rate, channels, dct) in info {
                tracks.push(AudioTrackInfo { sample_rate, channels });
                audio.push(audio::AudioDecoder::new(sample_rate, channels, dct, codec_tag)?);
            }
        }
        let file_size = file_size.min(data.len() as u64) as usize;
        let mut index = Vec::with_capacity(frames as usize);
        let mut next_pos = r.u32()? as usize;
        let mut next_key = true;
        for i in 0..frames {
            let pos = next_pos & !1;
            let keyframe = next_key;
            if i == frames - 1 {
                next_pos = file_size;
                next_key = false;
            } else {
                next_pos = r.u32()? as usize;
                next_key = next_pos & 1 != 0;
            }
            let np = next_pos & !1;
            if np <= pos || np > data.len() {
                return Err(hdr("invalid frame index table"));
            }
            index.push(FrameIndex { pos, size: np - pos, keyframe });
        }
        let video = video::VideoDecoder::new(width, height, codec_tag, flags)?;
        Ok(BinkFile { data, width, height, fps: (fps_num, fps_den), tracks, index, next: 0, video, audio })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Frame rate as `(numerator, denominator)`.
    pub fn fps(&self) -> (u32, u32) {
        self.fps
    }

    pub fn frame_count(&self) -> u32 {
        self.index.len() as u32
    }

    pub fn audio_tracks(&self) -> &[AudioTrackInfo] {
        &self.tracks
    }

    /// Rewind to the first frame.
    pub fn seek_start(&mut self) {
        self.next = 0;
        self.video.reset();
        for a in &mut self.audio {
            a.reset();
        }
    }

    /// Decode the next frame; `None` after the last frame.
    pub fn decode_next(&mut self) -> Option<Result<Frame, Error>> {
        if self.next >= self.index.len() {
            return None;
        }
        let i = self.next;
        self.next += 1;
        Some(self.decode_frame(i))
    }

    fn decode_frame(&mut self, i: usize) -> Result<Frame, Error> {
        let FrameIndex { pos, size, .. } = self.index[i];
        let pkt = &self.data[pos..pos + size];
        let mut p = 0usize;
        let mut audio_out = vec![Vec::new(); self.audio.len()];
        for (t, dec) in self.audio.iter_mut().enumerate() {
            let len = pkt
                .get(p..p + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
                .ok_or_else(|| Error::InvalidData("truncated audio header".into()))?;
            p += 4;
            if len > pkt.len() - p {
                return Err(Error::InvalidData("audio size larger than packet".into()));
            }
            if len >= 4 {
                dec.decode_packet(&pkt[p..p + len], &mut audio_out[t])?;
            }
            p += len;
        }
        self.video.decode_frame(&pkt[p..])?;
        Ok(Frame { index: i as u32, yuv: self.yuv(), audio: audio_out })
    }

    fn yuv(&self) -> Yuv420 {
        let planes = self.video.output();
        let (w, h) = (self.width as usize, self.height as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let crop = |p: usize, pw: usize, ph: usize| {
            let s = planes.stride[p];
            let mut v = Vec::with_capacity(pw * ph);
            for row in 0..ph {
                v.extend_from_slice(&planes.data[p][row * s..row * s + pw]);
            }
            v
        };
        Yuv420 { width: self.width, height: self.height, y: crop(0, w, h), u: crop(1, cw, ch), v: crop(2, cw, ch), y_stride: w, uv_stride: cw }
    }
}
