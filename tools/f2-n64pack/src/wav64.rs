//! Pure-Rust `.wav64` writer for VADPCM, byte-identical to libdragon's
//! `audioconv64 --wav-compress vadpcm,huffman=false [--wav-loop true]`.
//!
//! Port of `wav_convert` / `wav64_write` (format 1) / `wav_rotate_loop_at_end` from libdragon's
//! `tools/audioconv64/conv_wav64.cpp` (libdragon variant at `~/opt/libdragon-3a`), plus the
//! big-endian writers and placeholders of `tools/common/binout.c`. Those files are by Giovanni
//! Bajo and released into the public domain (Unlicense). The codec itself is in
//! [`crate::vadpcm`].
//!
//! Scope: 16-bit PCM input without `smpl`/`cue` chunks, no resampling, no `--wav-mono`, no
//! seek points, 4-bit residuals, no Huffman stage. The runtime decodes this exact layout
//! (header version 10, block-planar stereo with B = 128, loop head taps, skip points).

use crate::vadpcm::{self, Dither, Params, Vector, ENCODE_ORDER, FRAME_BYTES, FRAME_SAMPLES};

/// 16-bit PCM input (interleaved), as audioconv64 holds it after reading a WAV.
#[derive(Clone, Debug)]
pub struct Pcm {
    pub samples: Vec<i16>,
    pub channels: u16,
    pub rate: u32,
}

/// Options of the `audioconv64` command line that apply on this path.
#[derive(Clone, Copy, Debug, Default)]
pub struct Wav64Opts {
    /// `--wav-loop true`
    pub looping: bool,
}

/// `kPREDICTORS` in `wav64_write`.
const PREDICTORS: usize = 4;
/// `HUFF_CONTEXT_LEN` (`HUFF_CONTEXTS * 24`, `huff_vadpcm.c`).
const HUFF_CONTEXT_LEN: usize = 3 * 24;
/// `flag_wav_compress_vadpcm_bits` default.
const VADPCM_BITS: i32 = 4;
/// Frames per block of the block-planar stereo layout.
const BLOCK_FRAMES: usize = 128;
/// Bytes of per-mixer-channel decoder state (`state_size` placeholder).
const STATE_SIZE: u32 = 48;

/// The subset of `wav_data_t` that this path uses.
struct WavData {
    samples: Vec<i16>,
    cnt: usize,
    channels: usize,
    sample_rate: u32,
    looping: bool,
    loop_offset: usize,
    loop_end: usize,
    skip_points: Vec<usize>,
}

/// Big-endian writers of `binout.c`.
fn w8(o: &mut Vec<u8>, v: u8) {
    o.push(v);
}
fn w16(o: &mut Vec<u8>, v: u16) {
    o.extend_from_slice(&v.to_be_bytes());
}
fn w32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_be_bytes());
}

/// Bytes of the .wav64 file `audioconv64 --wav-compress vadpcm,huffman=false [--wav-loop true] -o DIR in.wav` writes
/// for a 16-bit PCM WAV holding `pcm` (no smpl/cue chunks, no resampling, no --wav-mono).
///
/// A looped empty input is rejected by audioconv64 (its loop skip point lies past the end) and
/// no file is written; this returns an empty vector in that case.
pub fn vadpcm_wav64(pcm: &Pcm, opts: &Wav64Opts) -> Vec<u8> {
    let channels = usize::from(pcm.channels);
    assert!(channels > 0, "wav64: zero channels");
    let cnt = pcm.samples.len() / channels;
    // wav_convert: apply --wav-loop (no smpl chunk, so the loop starts at 0).
    let mut wav = WavData {
        samples: pcm.samples[..cnt * channels].to_vec(),
        cnt,
        channels,
        sample_rate: pcm.rate,
        looping: opts.looping,
        loop_offset: 0,
        loop_end: 0,
        skip_points: Vec::new(),
    };
    if wav.looping && wav.loop_end == 0 {
        wav.loop_end = wav.cnt;
    }
    wav64_write_vadpcm(&mut wav).unwrap_or_default()
}

/// `wav_rotate_loop_at_end`: insert `ncopy` samples of the loop prefix at the loop end,
/// shifting the release tail forward and continuing the loop up to the next frame boundary.
fn wav_rotate_loop_at_end(wav: &mut WavData, ncopy: usize) {
    let ch = wav.channels;
    let loop_end = if wav.loop_end != 0 { wav.loop_end } else { wav.cnt };
    let loop_len = loop_end - wav.loop_offset;
    let tail = wav.cnt - loop_end;
    assert!(ncopy > 0 && loop_len > 0);

    let end = loop_end + ncopy;
    let over = ((end + 3).div_ceil(FRAME_SAMPLES) * FRAME_SAMPLES - end).min(tail);

    wav.samples.resize((wav.cnt + ncopy) * ch, 0);
    wav.samples.copy_within(loop_end * ch..(loop_end + tail) * ch, end * ch);
    for i in 0..ncopy + over {
        let src = wav.loop_offset + (i % loop_len);
        for c in 0..ch {
            wav.samples[(loop_end + i) * ch + c] = wav.samples[src * ch + c];
        }
    }
    wav.cnt += ncopy;
    wav.loop_offset += ncopy;
    wav.loop_end = end;
}

/// `wav64_write` with `format == 1` (VADPCM, Huffman off). Returns `None` where audioconv64
/// fails (and removes the output file).
fn wav64_write_vadpcm(wav: &mut WavData) -> Option<Vec<u8>> {
    let mut out = Vec::new();

    // Terminal loop if none was set explicitly.
    if wav.looping && wav.loop_end == 0 {
        wav.loop_end = wav.cnt;
    }
    let loop_end = if wav.looping { wav.loop_end } else { 0 };
    if loop_end < wav.loop_offset || loop_end > wav.cnt {
        // "invalid loop" warning: loop disabled.
        wav.looping = false;
        wav.loop_end = 0;
    }

    // case 1: align loopStart to 32 samples by rotating the prefix into the loop end.
    const VADPCM_LOOP_ALIGN: usize = 32;
    if wav.looping && !wav.loop_offset.is_multiple_of(VADPCM_LOOP_ALIGN) {
        wav_rotate_loop_at_end(wav, VADPCM_LOOP_ALIGN - wav.loop_offset % VADPCM_LOOP_ALIGN);
    }
    let bits_per_sample = 16u8;

    // Recompute after alignment. On disk, 0 means terminal (loop_end == len).
    let loop_end = if wav.looping {
        if wav.loop_end != 0 {
            wav.loop_end
        } else {
            wav.cnt
        }
    } else {
        0
    };
    let loop_len = if wav.looping { loop_end - wav.loop_offset } else { 0 };
    let loop_end_disk = if wav.looping && loop_end < wav.cnt { loop_end } else { 0 };

    out.extend_from_slice(b"WV64");
    w8(&mut out, 10); // version
    w8(&mut out, 1); // format
    w8(&mut out, wav.channels as u8);
    w8(&mut out, bits_per_sample);
    w32(&mut out, wav.sample_rate);
    w32(&mut out, wav.cnt as u32); // len (before padding)
    w32(&mut out, loop_len as u32);
    w32(&mut out, loop_end_disk as u32);
    let samples_placeholder = out.len();
    w32(&mut out, 0); // samples offset, set below
    w32(&mut out, STATE_SIZE); // state_size placeholder, resolved to 48

    // Pad cnt to a multiple of 32 samples, continuing the waveform.
    let ch = wav.channels;
    let align = FRAME_SAMPLES * 2;
    if !wav.cnt.is_multiple_of(align) {
        let newcnt = wav.cnt.div_ceil(align) * align;
        wav.samples.resize(newcnt * ch, 0);
        for i in wav.cnt..newcnt {
            let src = if loop_len > 0 { wav.loop_offset + (i - wav.cnt) % loop_len } else { wav.cnt - 1 };
            for c in 0..ch {
                wav.samples[i * ch + c] = wav.samples[src * ch + c];
            }
        }
        wav.cnt = newcnt;
    }

    let nframes = wav.cnt / FRAME_SAMPLES;
    let mut codebook = vec![[0i16; 8]; PREDICTORS * ENCODE_ORDER * ch];
    let parms = Params {
        predictor_count: PREDICTORS,
        dither: Dither::Rectangular,
        min_residual: -(1 << (VADPCM_BITS - 1)),
        max_residual: (1 << (VADPCM_BITS - 1)) - 1,
    };
    let mut dest = vec![0u8; nframes * FRAME_BYTES * ch];

    let mut skip_points: Vec<usize> = wav.skip_points.clone();
    if wav.looping {
        skip_points.push(wav.loop_offset);
    }
    skip_points.sort_unstable();
    for sp in &mut skip_points {
        *sp = sp.div_ceil(FRAME_SAMPLES) * FRAME_SAMPLES;
        if *sp >= wav.cnt {
            // "invalid skip point": conversion fails, output removed.
            return None;
        }
    }

    let mut skip_state: Vec<[Vector; 2]> = vec![[[0; 8]; 2]; skip_points.len()];
    let mut loop_head = [[0i16; 3]; 2];
    let loop_start_aligned = if wav.looping { Some(wav.loop_offset.div_ceil(FRAME_SAMPLES) * FRAME_SAMPLES) } else { None };

    let mut schan = vec![0i16; wav.cnt];
    for i in 0..ch {
        let mut destchan = vec![0u8; nframes * FRAME_BYTES];
        for (j, s) in schan.iter_mut().enumerate() {
            *s = wav.samples[i + j * ch];
        }
        let cb = &mut codebook[PREDICTORS * ENCODE_ORDER * i..PREDICTORS * ENCODE_ORDER * (i + 1)];
        vadpcm::vadpcm_encode(&parms, cb, nframes, &mut destchan, &schan, None).ok()?;
        let cb = &codebook[PREDICTORS * ENCODE_ORDER * i..PREDICTORS * ENCODE_ORDER * (i + 1)];
        if !skip_points.is_empty() {
            // Decoder states at all skip points in one forward pass.
            let mut st: Vector = [0; 8];
            let mut cur_frame = 0usize;
            for (j, &sp) in skip_points.iter().enumerate() {
                let target_frame = sp / FRAME_SAMPLES;
                if target_frame > cur_frame {
                    let run = target_frame - cur_frame;
                    vadpcm::vadpcm_decode(PREDICTORS, ENCODE_ORDER, cb, &mut st, run, &mut schan, &destchan[cur_frame * FRAME_BYTES..])
                        .expect("valid predictor");
                }
                skip_state[j][i] = st;
                // Loop taps: the first three samples of the frame the loop resumes on.
                if Some(sp) == loop_start_aligned && target_frame < nframes {
                    let mut st_head = st;
                    let mut head = [0i16; FRAME_SAMPLES];
                    vadpcm::vadpcm_decode(PREDICTORS, ENCODE_ORDER, cb, &mut st_head, 1, &mut head, &destchan[target_frame * FRAME_BYTES..])
                        .expect("valid predictor");
                    loop_head[i].copy_from_slice(&head[..3]);
                }
                cur_frame = target_frame;
            }
        }

        // Block-planar layout: per block of B frames, all L then all R (mono unchanged).
        for j in 0..nframes {
            let dstj = if ch == 1 {
                j
            } else {
                let block = j / BLOCK_FRAMES;
                let off = j % BLOCK_FRAMES;
                let nblocks = nframes.div_ceil(BLOCK_FRAMES);
                let bs = if block == nblocks - 1 { nframes - block * BLOCK_FRAMES } else { BLOCK_FRAMES };
                block * BLOCK_FRAMES * ch + i * bs + off
            };
            dest[dstj * FRAME_BYTES..(dstj + 1) * FRAME_BYTES].copy_from_slice(&destchan[j * FRAME_BYTES..(j + 1) * FRAME_BYTES]);
        }
    }

    let flags = 0u16; // no Huffman, not resident
    let codebook_stride = PREDICTORS * ENCODE_ORDER * 16 + 8;
    let codebook_bytes = (codebook_stride * ch) as u32;
    let nskip = skip_points.len();
    w8(&mut out, PREDICTORS as u8);
    w8(&mut out, ENCODE_ORDER as u8);
    w16(&mut out, flags);
    w16(&mut out, nskip as u16);
    w8(&mut out, VADPCM_BITS as u8);
    w8(&mut out, 0); // attack_frames
    w32(&mut out, 0); // huff_tbl_ptr
    w32(&mut out, if nskip > 0 { codebook_bytes } else { 0 }); // skip_points_ptr
    w32(&mut out, if nskip > 0 { codebook_bytes + nskip as u32 * 8 } else { 0 }); // skip_states_ptr
    out.extend_from_slice(&[0u8; HUFF_CONTEXT_LEN]); // Huffman context (unused, zero)
    w32(&mut out, 0); // padding
    for c in 0..ch {
        for v in &codebook[PREDICTORS * ENCODE_ORDER * c..PREDICTORS * ENCODE_ORDER * (c + 1)] {
            for &x in v {
                w16(&mut out, x as u16);
            }
        }
        for &x in &loop_head[c] {
            w16(&mut out, x as u16);
        }
        w16(&mut out, 0); // padding
    }
    for &sp in &skip_points {
        w32(&mut out, sp as u32);
        w32(&mut out, 0); // bit position (Huffman only)
    }
    for st in &skip_state {
        for v in st.iter().take(ch) {
            for &x in v {
                w16(&mut out, x as u16);
            }
        }
    }

    let samples_off = out.len() as u32;
    out[samples_placeholder..samples_placeholder + 4].copy_from_slice(&samples_off.to_be_bytes());
    out.extend_from_slice(&dest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be32(b: &[u8], o: usize) -> u32 {
        u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
    }

    fn sine(n: usize, ch: u16, f: f64) -> Vec<i16> {
        (0..n * usize::from(ch)).map(|i| (((i / usize::from(ch)) as f64 * f + (i % usize::from(ch)) as f64).sin() * 9000.0) as i16).collect()
    }

    #[test]
    fn header_mono_unlooped() {
        let pcm = Pcm { samples: sine(100, 1, 0.07), channels: 1, rate: 22050 };
        let b = vadpcm_wav64(&pcm, &Wav64Opts { looping: false });
        assert_eq!(&b[..4], b"WV64");
        assert_eq!(&b[4..8], &[10, 1, 1, 16]);
        assert_eq!(be32(&b, 8), 22050);
        assert_eq!(be32(&b, 12), 100); // len before padding
        assert_eq!(be32(&b, 16), 0); // loop_len
        assert_eq!(be32(&b, 20), 0); // loop_end
        let samples = be32(&b, 24) as usize;
        assert_eq!(be32(&b, 28), 48);
        assert_eq!(&b[32..34], &[4, 2]);
        assert_eq!(&b[34..36], &[0, 0]); // flags
        assert_eq!(&b[36..38], &[0, 0]); // no skip points
        assert_eq!(b[38], 4);
        assert_eq!(samples, 32 + 20 + HUFF_CONTEXT_LEN + 4 + 136);
        // 100 samples pad to 128: 8 frames.
        assert_eq!(b.len(), samples + 8 * FRAME_BYTES);
    }

    #[test]
    fn header_stereo_looped() {
        let pcm = Pcm { samples: sine(3000, 2, 0.03), channels: 2, rate: 44100 };
        let b = vadpcm_wav64(&pcm, &Wav64Opts { looping: true });
        assert_eq!(b[6], 2);
        assert_eq!(be32(&b, 12), 3000);
        assert_eq!(be32(&b, 16), 3000); // loop_len
        assert_eq!(be32(&b, 20), 0); // terminal loop
        assert_eq!(&b[36..38], &[0, 1]); // one skip point (the loop start)
        let cb_bytes = 2 * 136;
        assert_eq!(be32(&b, 44), cb_bytes as u32);
        assert_eq!(be32(&b, 48), cb_bytes as u32 + 8);
        let samples = be32(&b, 24) as usize;
        assert_eq!(samples, 128 + cb_bytes + 8 + 2 * 16);
        let nframes = 3008 / 16;
        assert_eq!(b.len(), samples + nframes * FRAME_BYTES * 2);
        // Loop head taps of channel 0 match the first decoded samples.
        let cb0 = 128;
        let head = [i16::from_be_bytes([b[cb0 + 128], b[cb0 + 129]]), i16::from_be_bytes([b[cb0 + 130], b[cb0 + 131]])];
        let mut codebook = [[0i16; 8]; 8];
        for (k, v) in codebook.iter_mut().enumerate() {
            for (j, x) in v.iter_mut().enumerate() {
                let o = cb0 + k * 16 + j * 2;
                *x = i16::from_be_bytes([b[o], b[o + 1]]);
            }
        }
        let mut st = [0i16; 8];
        let mut out = [0i16; 16];
        vadpcm::vadpcm_decode(4, 2, &codebook, &mut st, 1, &mut out, &b[samples..]).unwrap();
        assert_eq!(head, [out[0], out[1]]);
    }

    #[test]
    fn empty_inputs() {
        let pcm = Pcm { samples: vec![], channels: 1, rate: 22050 };
        let b = vadpcm_wav64(&pcm, &Wav64Opts { looping: false });
        assert_eq!(be32(&b, 12), 0);
        assert_eq!(b.len(), be32(&b, 24) as usize);
        assert!(vadpcm_wav64(&pcm, &Wav64Opts { looping: true }).is_empty());
    }

    #[test]
    fn rotation_aligns_loop() {
        let mut wav = WavData {
            samples: (0..200).collect(),
            cnt: 200,
            channels: 1,
            sample_rate: 8000,
            looping: true,
            loop_offset: 10,
            loop_end: 90,
            skip_points: vec![],
        };
        wav_rotate_loop_at_end(&mut wav, 22);
        assert_eq!((wav.cnt, wav.loop_offset, wav.loop_end), (222, 32, 112));
        assert_eq!(wav.samples[90], 10);
        assert_eq!(wav.samples[111], 31);
        // The loop continues over the shifted tail up to the frame boundary past end + 3.
        assert_eq!(wav.samples[112], 32);
        assert_eq!(wav.samples[127], 47);
        assert_eq!(wav.samples[128], 106);
        assert_eq!(wav.samples[221], 199);
    }

    /// Parity with the real tool on synthetic inputs (skipped without audioconv64).
    #[test]
    fn parity_with_audioconv64_synthetic() {
        let Some(tool) = parity::find_tool() else {
            eprintln!("audioconv64 not found: skipping VADPCM parity test");
            return;
        };
        let (ok, total) = parity::run_synthetic(&tool);
        assert_eq!(ok, total, "{ok}/{total} synthetic wav64 files byte-identical");
        eprintln!("synthetic parity: {ok}/{total} byte-identical");
    }
}

/// Helpers for byte-for-byte comparison against the real `audioconv64` (tests and the
/// `vadpcm_parity` example).
#[doc(hidden)]
pub mod parity {
    use super::{vadpcm_wav64, Pcm, Wav64Opts};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// `$N64_INST/bin/audioconv64`, else `~/opt/n64root-3a/opt/libdragon/bin/audioconv64`.
    pub fn find_tool() -> Option<PathBuf> {
        let mut cands = Vec::new();
        if let Some(n) = std::env::var_os("N64_INST") {
            cands.push(PathBuf::from(n).join("bin/audioconv64"));
        }
        if let Some(h) = std::env::var_os("HOME") {
            cands.push(PathBuf::from(h).join("opt/n64root-3a/opt/libdragon/bin/audioconv64"));
        }
        cands.into_iter().find(|p| p.is_file())
    }

    /// Runs the tool on `pcm` in `dir`; `None` when it writes no file.
    pub fn tool_wav64(tool: &Path, dir: &Path, name: &str, pcm: &Pcm, opts: &Wav64Opts) -> Option<Vec<u8>> {
        let input = dir.join(format!("{name}.wav"));
        let outdir = dir.join("out");
        std::fs::create_dir_all(&outdir).unwrap();
        std::fs::write(&input, crate::audio::wav16(pcm.rate, pcm.channels, &pcm.samples)).unwrap();
        let out = outdir.join(format!("{name}.wav64"));
        let _ = std::fs::remove_file(&out);
        let mut c = Command::new(tool);
        c.args(["--wav-compress", "vadpcm,huffman=false"]);
        if opts.looping {
            c.args(["--wav-loop", "true"]);
        }
        let _ = c.arg("-o").arg(&outdir).arg(&input).output().unwrap();
        let _ = std::fs::remove_file(&input);
        let r = std::fs::read(&out).ok();
        let _ = std::fs::remove_file(&out);
        r
    }

    /// Compares one input; prints the first difference on mismatch.
    pub fn compare(tool: &Path, dir: &Path, name: &str, pcm: &Pcm, opts: &Wav64Opts) -> bool {
        let want = tool_wav64(tool, dir, name, pcm, opts);
        if want.is_none() {
            eprintln!("note: audioconv64 wrote no file for {name}");
        }
        let want = want.unwrap_or_default();
        let got = vadpcm_wav64(pcm, opts);
        if want == got {
            return true;
        }
        let first = want.iter().zip(&got).position(|(a, b)| a != b).unwrap_or(want.len().min(got.len()));
        eprintln!("MISMATCH {name}: tool {} bytes, port {} bytes, first diff at {first}", want.len(), got.len());
        false
    }

    /// Deterministic xorshift noise.
    fn noise(seed: u32, n: usize, amp: f64) -> Vec<i16> {
        let mut x = seed | 1;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                ((f64::from(x) / f64::from(u32::MAX) * 2.0 - 1.0) * amp) as i16
            })
            .collect()
    }

    /// Synthetic corpus: returns (identical, total).
    pub fn run_synthetic(tool: &Path) -> (usize, usize) {
        let dir = std::env::temp_dir().join(format!("f2-vadpcm-parity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cases: Vec<(String, Vec<i16>, u16)> = Vec::new();
        for &n in &[0usize, 1, 15, 16, 17, 31, 32, 33, 100, 2047, 2048, 2049, 4111, 10000] {
            for ch in [1u16, 2] {
                let len = n * usize::from(ch);
                let sine: Vec<i16> =
                    (0..len).map(|i| (((i / usize::from(ch)) as f64 * (0.02 + 0.05 * (i % usize::from(ch)) as f64)).sin() * 20000.0) as i16).collect();
                cases.push((format!("sine{n}c{ch}"), sine, ch));
                cases.push((format!("noise{n}c{ch}"), noise(n as u32 * 7 + u32::from(ch), len, 30000.0), ch));
                if n >= 100 {
                    cases.push((format!("silence{n}c{ch}"), vec![0; len], ch));
                    let clip: Vec<i16> = (0..len).map(|i| (((i / usize::from(ch)) as f64 * 0.01).sin() * 80000.0).clamp(-32768.0, 32767.0) as i16).collect();
                    cases.push((format!("clip{n}c{ch}"), clip, ch));
                    let square: Vec<i16> = (0..len).map(|i| if (i / usize::from(ch) / 37) % 2 == 0 { 32767 } else { -32768 }).collect();
                    cases.push((format!("square{n}c{ch}"), square, ch));
                    // Chirp + low-level noise mix.
                    let nz = noise(n as u32, len, 300.0);
                    let chirp: Vec<i16> = (0..len)
                        .map(|i| {
                            let t = (i / usize::from(ch)) as f64;
                            ((t * t * 1e-5).sin() * 12000.0) as i16 + nz[i]
                        })
                        .collect();
                    cases.push((format!("chirp{n}c{ch}"), chirp, ch));
                }
            }
        }
        let rates = [11025u32, 22050, 32000, 44100];
        let (mut ok, mut total) = (0, 0);
        for (k, (name, samples, ch)) in cases.into_iter().enumerate() {
            let pcm = Pcm { samples, channels: ch, rate: rates[k % rates.len()] };
            for looping in [false, true] {
                total += 1;
                if compare(tool, &dir, &format!("{name}l{}", u8::from(looping)), &pcm, &Wav64Opts { looping }) {
                    ok += 1;
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        (ok, total)
    }
}
