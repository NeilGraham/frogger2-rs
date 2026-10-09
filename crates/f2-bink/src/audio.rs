//! Bink audio decoder (RDFT variant only, the one used by Frogger 2; the DCT variant is
//! rejected as unsupported).
//!
//! Close translation of FFmpeg `libavcodec/binkaudio.c` (LGPL-2.1-or-later,
//! Copyright (c) 2007-2011 Peter Ross, Copyright (c) 2009 Daniel Verkamp).
//! The inverse transforms are computed with a plain radix-2 FFT in `f64`, so output differs
//! from FFmpeg's single-precision transforms only by rounding noise.

use crate::bits::BitReader;
use crate::tables::WMA_CRITICAL_FREQS;
use crate::Error;

const MAX_CHANNELS: usize = 2;
const RLE_LENGTH_TAB: [usize; 16] = [2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 32, 64];

pub(crate) struct AudioDecoder {
    version_b: bool,
    use_dct: bool,
    first: bool,
    /// Number of separately coded channels (1 for RDFT: samples are already interleaved).
    channels: usize,
    frame_len: usize,
    overlap_len: usize,
    num_bands: usize,
    root: f32,
    bands: [usize; 26],
    previous: Vec<Vec<f32>>,
    quant_table: [f32; 96],
    fft: Fft,
}

impl AudioDecoder {
    pub(crate) fn new(sample_rate: u32, channels: u32, use_dct: bool, codec_tag: u32) -> Result<Self, Error> {
        if use_dct {
            return Err(Error::Unsupported("Bink audio DCT variant".into()));
        }
        let max_channels = MAX_CHANNELS as u32;
        if channels < 1 || channels > max_channels {
            return Err(Error::Unsupported(format!("{channels} audio channels")));
        }
        let mut frame_len_bits: u32 = if sample_rate < 22050 {
            9
        } else if sample_rate < 44100 {
            10
        } else {
            11
        };
        let version_b = (codec_tag >> 24) as u8 == b'b';
        let mut sample_rate = sample_rate as u64;
        let dec_channels;
        if !use_dct {
            sample_rate *= channels as u64;
            dec_channels = 1;
            if !version_b {
                frame_len_bits += 31 - channels.leading_zeros();
            }
        } else {
            dec_channels = channels as usize;
        }
        let frame_len = 1usize << frame_len_bits;
        let overlap_len = frame_len / 16;
        let sample_rate_half = sample_rate.div_ceil(2);
        let root = if !use_dct { 2.0 / ((frame_len as f64).sqrt() * 32768.0) } else { frame_len as f64 / ((frame_len as f64).sqrt() * 32768.0) } as f32;
        let mut quant_table = [0f32; 96];
        for (i, q) in quant_table.iter_mut().enumerate() {
            *q = (i as f32 * 0.152_891_65_f32).exp() * root;
        }
        let mut num_bands = 1;
        while num_bands < 25 {
            if sample_rate_half <= WMA_CRITICAL_FREQS[num_bands - 1] as u64 {
                break;
            }
            num_bands += 1;
        }
        let mut bands = [0usize; 26];
        bands[0] = 2;
        for i in 1..num_bands {
            bands[i] = ((WMA_CRITICAL_FREQS[i - 1] as u64 * frame_len as u64 / sample_rate_half) & !1) as usize;
        }
        bands[num_bands] = frame_len;
        let fft_len = if use_dct { frame_len * 2 } else { frame_len };
        Ok(AudioDecoder {
            version_b,
            use_dct,
            first: true,
            channels: dec_channels,
            frame_len,
            overlap_len,
            num_bands,
            root,
            bands,
            previous: vec![vec![0.0; overlap_len]; dec_channels],
            quant_table,
            fft: Fft::new(fft_len),
        })
    }

    pub(crate) fn reset(&mut self) {
        self.first = true;
    }

    /// Decode one audio packet (including its 4-byte size prefix), appending interleaved
    /// samples to `out`.
    pub(crate) fn decode_packet(&mut self, pkt: &[u8], out: &mut Vec<f32>) -> Result<(), Error> {
        if pkt.len() < 4 {
            return Err(Error::InvalidData("audio packet too small".into()));
        }
        let mut gb = BitReader::new(pkt);
        gb.skip(32);
        let mut blocks: Vec<Vec<f32>> = vec![vec![0.0; self.frame_len]; self.channels];
        while gb.left() > 0 {
            let mut ch_offset = 0;
            while ch_offset < self.channels {
                let n = MAX_CHANNELS.min(self.channels - ch_offset);
                self.decode_block(&mut gb, &mut blocks, n, ch_offset)?;
                ch_offset += MAX_CHANNELS;
                gb.align32();
            }
            let block_size = self.frame_len - self.overlap_len;
            if self.channels == 1 {
                out.extend_from_slice(&blocks[0][..block_size]);
            } else {
                for i in 0..block_size {
                    for b in &blocks {
                        out.push(b[i]);
                    }
                }
            }
        }
        Ok(())
    }

    fn decode_block(&mut self, gb: &mut BitReader, out: &mut [Vec<f32>], channels: usize, ch_offset: usize) -> Result<(), Error> {
        let incomplete = || Error::InvalidData("incomplete audio packet".into());
        let mut coeffs = vec![0f32; self.frame_len + 2];
        let mut quant = [0f32; 25];
        if self.use_dct {
            gb.skip(2);
        }
        for ch in 0..channels {
            if self.version_b {
                if gb.left() < 64 {
                    return Err(incomplete());
                }
                coeffs[0] = f32::from_bits(gb.get(32)) * self.root;
                coeffs[1] = f32::from_bits(gb.get(32)) * self.root;
            } else {
                if gb.left() < 58 {
                    return Err(incomplete());
                }
                coeffs[0] = get_float(gb) * self.root;
                coeffs[1] = get_float(gb) * self.root;
            }
            if gb.left() < (self.num_bands * 8) as isize {
                return Err(incomplete());
            }
            for q in quant.iter_mut().take(self.num_bands) {
                let v = gb.get(8) as usize;
                *q = self.quant_table[v.min(95)];
            }
            let mut k = 0;
            let mut q = quant[0];
            let mut i = 2;
            while i < self.frame_len {
                let mut j = if self.version_b {
                    i + 16
                } else if gb.bit() {
                    i + RLE_LENGTH_TAB[gb.get(4) as usize] * 8
                } else {
                    i + 8
                };
                j = j.min(self.frame_len);
                let width = gb.get(4);
                if width == 0 {
                    coeffs[i..j].fill(0.0);
                    i = j;
                    while self.bands[k] < i {
                        q = quant[k];
                        k += 1;
                    }
                } else {
                    while i < j {
                        if self.bands[k] == i {
                            q = quant[k];
                            k += 1;
                        }
                        let coeff = gb.get(width);
                        coeffs[i] = if coeff != 0 {
                            if gb.bit() {
                                -q * coeff as f32
                            } else {
                                q * coeff as f32
                            }
                        } else {
                            0.0
                        };
                        i += 1;
                    }
                }
            }
            let dst = &mut out[ch + ch_offset];
            if self.use_dct {
                unreachable!("DCT variant rejected in AudioDecoder::new");
            } else {
                // Packed real spectrum: [re0, re(N/2), re1, im1, re2, im2, ...].
                self.fft.inverse_rdft(&coeffs[..self.frame_len], dst);
            }
        }
        for ch in 0..channels {
            let c = ch + ch_offset;
            let count = (self.overlap_len * channels) as f32;
            if !self.first {
                let mut j = ch;
                for (o, &p) in out[c].iter_mut().zip(&self.previous[c]) {
                    *o = (p * (count - j as f32) + *o * j as f32) / count;
                    j += channels;
                }
            }
            let fl = self.frame_len;
            self.previous[c].copy_from_slice(&out[c][fl - self.overlap_len..fl]);
        }
        self.first = false;
        Ok(())
    }
}

fn get_float(gb: &mut BitReader) -> f32 {
    let power = gb.get(5) as i32;
    let mant = gb.get(23) as f32;
    let f = mant * 2f32.powi(power - 23);
    if gb.bit() {
        -f
    } else {
        f
    }
}

/// Minimal iterative radix-2 complex FFT in `f64`.
struct Fft {
    n: usize,
    /// e^{+2πik/n}
    twiddle: Vec<(f64, f64)>,
    rev: Vec<usize>,
    buf: Vec<(f64, f64)>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let bits = n.trailing_zeros();
        let twiddle = (0..n / 2)
            .map(|k| {
                let a = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
                (a.cos(), a.sin())
            })
            .collect();
        let rev = (0..n).map(|i| if bits == 0 { 0 } else { i.reverse_bits() >> (usize::BITS - bits) }).collect();
        Fft { n, twiddle, rev, buf: vec![(0.0, 0.0); n] }
    }

    /// In-place inverse (positive exponent) unscaled DFT of `self.buf`.
    fn run_inverse(&mut self) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i];
            if i < j {
                self.buf.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = self.twiddle[k * step];
                    let (ar, ai) = self.buf[start + k];
                    let (br, bi) = self.buf[start + k + len / 2];
                    let tr = br * wr - bi * wi;
                    let ti = br * wi + bi * wr;
                    self.buf[start + k] = (ar + tr, ai + ti);
                    self.buf[start + k + len / 2] = (ar - tr, ai - ti);
                }
            }
            len <<= 1;
        }
    }

    /// Inverse real DFT of a packed spectrum of `n` floats (FFmpeg's legacy `IDFT_C2R` layout,
    /// as used by Bink audio before FFmpeg's sign flip for `av_tx`), output scaled by 1/2.
    fn inverse_rdft(&mut self, coeffs: &[f32], out: &mut [f32]) {
        let n = self.n;
        let half = n / 2;
        self.buf[0] = (coeffs[0] as f64, 0.0);
        self.buf[half] = (coeffs[1] as f64, 0.0);
        for k in 1..half {
            let re = coeffs[2 * k] as f64;
            let im = RDFT_IM_SIGN * coeffs[2 * k + 1] as f64;
            self.buf[k] = (re, im);
            self.buf[n - k] = (re, -im);
        }
        self.run_inverse();
        for (o, v) in out.iter_mut().zip(&self.buf) {
            *o = (v.0 * RDFT_SCALE) as f32;
        }
    }
}

const RDFT_IM_SIGN: f64 = -1.0;
const RDFT_SCALE: f64 = 0.5;
