//! Bit-exact port of the VADPCM codec used by libdragon's `audioconv64`.
//!
//! Upstream: the VADPCM encoder/decoder by Dietrich Epp, as vendored (with libdragon
//! extensions: rectangular dither, residual range) in libdragon's
//! `tools/audioconv64/vadpcm/codec/{encode.c,predictor.c,autocorr.c,decode.c,random.h}`.
//!
//! ```text
//! Copyright 2022-2023 Dietrich Epp.
//! This file is part of VADPCM. VADPCM is licensed under the terms of the
//! Mozilla Public License, version 2.0.
//! ```
//!
//! This file is a translation of that MPL-2.0 code and is therefore itself covered by the
//! Mozilla Public License, version 2.0 (file-level copyleft); a copy of the license is at
//! <https://mozilla.org/MPL/2.0/>. The upstream source is available from
//! <https://github.com/depp/skelly64> and in the libdragon tree.
//!
//! The upstream object is compiled with `-O2 -ffp-contract=off -fno-fast-math`, so every
//! floating-point operation below keeps the C types (`f32` for `float`, `f64` for `double`)
//! and the C evaluation order, with no fused multiply-add. `lrint` (default rounding mode)
//! is [`f64::round_ties_even`]. Integer arithmetic that could overflow `int` in C uses
//! wrapping operations (what the compiled x86-64 code does).

/// Samples in a VADPCM frame (`kVADPCMFrameSampleCount`).
pub const FRAME_SAMPLES: usize = 16;
/// Bytes in an encoded VADPCM frame (`kVADPCMFrameByteSize`).
pub const FRAME_BYTES: usize = 9;
/// Predictor order used by the encoder (`kVADPCMEncodeOrder`).
pub const ENCODE_ORDER: usize = 2;
/// Maximum predictor count (`kVADPCMMaxPredictorCount`).
pub const MAX_PREDICTORS: usize = 16;
/// Iterations for predictor assignment (`kVADPCMIterations`).
const ITERATIONS: usize = 20;

/// `struct vadpcm_vector`: eight 16-bit samples / codebook taps.
pub type Vector = [i16; 8];

/// `vadpcm_dither`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dither {
    /// `kVADPCMDitherNone`.
    None,
    /// `kVADPCMDitherRectangular`.
    Rectangular,
}

/// `struct vadpcm_params`.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub predictor_count: usize,
    pub dither: Dither,
    pub min_residual: i32,
    pub max_residual: i32,
}

/// `struct vadpcm_stats`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub signal_mean_square: f64,
    pub error_mean_square: f64,
}

/// `vadpcm_error` (non-success values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// `kVADPCMErrInvalidData`.
    InvalidData,
    /// `kVADPCMErrInvalidParams`.
    InvalidParams,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::InvalidData => "invalid_data",
            Error::InvalidParams => "invalid_params",
        })
    }
}

impl std::error::Error for Error {}

type Corr = [f32; 6];

// ---------------------------------------------------------------------------------------------
// random.h
// ---------------------------------------------------------------------------------------------

/// `vadpcm_rng`.
#[inline]
fn rng(state: u32) -> u32 {
    state.wrapping_mul(0xd9f5).wrapping_add(0x6487_ed51)
}

// ---------------------------------------------------------------------------------------------
// autocorr.c
// ---------------------------------------------------------------------------------------------

/// `vadpcm_autocorr`: autocorrelation matrix of each frame (history carries across frames).
fn autocorr(frame_count: usize, src: &[i16]) -> Vec<Corr> {
    let (mut x0, mut x1) = (0.0f32, 0.0f32);
    let mut x2: f32;
    let mut corr = Vec::with_capacity(frame_count);
    for frame in 0..frame_count {
        let mut m = [0.0f32; 6];
        for i in 0..FRAME_SAMPLES {
            x2 = x1;
            x1 = x0;
            x0 = f32::from(src[frame * FRAME_SAMPLES + i]) * (1.0f32 / 32768.0f32);
            m[0] += x0 * x0;
            m[1] += x1 * x0;
            m[2] += x1 * x1;
            m[3] += x2 * x0;
            m[4] += x2 * x1;
            m[5] += x2 * x2;
        }
        corr.push(m);
    }
    corr
}

// ---------------------------------------------------------------------------------------------
// predictor.h / predictor.c
// ---------------------------------------------------------------------------------------------

/// `vadpcm_eval`: square error for an autocorrelation matrix and coefficients (float).
#[inline]
fn eval(corr: &Corr, coeff: &[f32; 2]) -> f32 {
    corr[0] + corr[2] * coeff[0] * coeff[0] + corr[5] * coeff[1] * coeff[1] + 2.0f32 * (corr[4] * coeff[0] * coeff[1] - corr[1] * coeff[0] - corr[3] * coeff[1])
}

/// `vadpcm_eval_solved`.
#[inline]
fn eval_solved(corr: &[f64; 6], coeff: &[f64; 2]) -> f64 {
    corr[0] - corr[1] * coeff[0] - corr[3] * coeff[1]
}

/// `vadpcm_best_error`.
fn best_error(corr: &[Corr]) -> Vec<f32> {
    corr.iter()
        .map(|c| {
            let fcorr: [f64; 6] = std::array::from_fn(|i| f64::from(c[i]));
            let mut coeff = [0.0f64; 2];
            if solve_stable(&fcorr, &mut coeff) == 0 {
                eval_solved(&fcorr, &coeff) as f32
            } else {
                let fcoeff = [coeff[0] as f32, coeff[1] as f32];
                eval(c, &fcoeff)
            }
        })
        .collect()
}

/// `vadpcm_meancorrs`: mean autocorrelation per predictor (frames out of range ignored).
fn meancorrs(predictor_count: usize, corr: &[Corr], predictors: &[u8], pcorr: &mut [[f64; 6]; MAX_PREDICTORS], count: &mut [i32; MAX_PREDICTORS]) {
    for i in 0..predictor_count {
        count[i] = 0;
        pcorr[i] = [0.0; 6];
    }
    for (c, &p) in corr.iter().zip(predictors) {
        let p = usize::from(p);
        if p < predictor_count {
            count[p] += 1;
            for j in 0..6 {
                pcorr[p][j] += f64::from(c[j]);
            }
        }
    }
    for i in 0..predictor_count {
        if count[i] > 0 {
            let a = 1.0f64 / f64::from(count[i]);
            for p in pcorr[i].iter_mut() {
                *p *= a;
            }
        }
    }
}

/// `vadpcm_solve`: unconstrained least-squares predictor coefficients.
fn solve(corr: &[f64; 6], coeff: &mut [f64; 2]) {
    let rel_epsilon = 1.0e-5f64;
    coeff[0] = 0.0;
    coeff[1] = 0.0;

    let mut max = corr[0];
    if corr[2] > max {
        max = corr[2];
    }
    if corr[5] > max {
        max = corr[5];
    }
    let epsilon = max * rel_epsilon;

    let mut a = corr[2];
    let b = corr[4];
    let mut c = corr[5];
    let mut x = corr[1];
    let mut y = corr[3];

    let pivot = usize::from(c > a);
    if pivot != 0 {
        std::mem::swap(&mut a, &mut c);
        std::mem::swap(&mut x, &mut y);
    }

    if a <= epsilon {
        return;
    }
    let a1 = 1.0f64 / a;
    let b1 = b * a1;
    let x1 = x * a1;

    let c2 = c - b1 * b;
    let y2 = y - x1 * b;

    if c2.abs() <= epsilon {
        coeff[pivot] = x1;
        return;
    }
    let y3 = y2 / c2;

    let x4 = x1 - y3 * b1;

    coeff[pivot] = x4;
    coeff[1 - pivot] = y3;
}

/// `vadpcm_eval_at`: `vadpcm_eval` in double precision.
#[inline]
fn eval_at(corr: &[f64; 6], c0: f64, c1: f64) -> f64 {
    corr[0] + corr[2] * c0 * c0 + corr[5] * c1 * c1 + 2.0f64 * (corr[4] * c0 * c1 - corr[1] * c0 - corr[3] * c1)
}

/// `vadpcm_solve_edge`: minimise the error along the segment (x0,y0)-(x1,y1).
#[allow(clippy::too_many_arguments)]
fn solve_edge(corr: &[f64; 6], x0: f64, y0: f64, x1: f64, y1: f64, best_error: &mut f64, coeff: &mut [f64; 2]) {
    let vx = x1 - x0;
    let vy = y1 - y0;
    let apx = corr[2] * x0 + corr[4] * y0;
    let apy = corr[4] * x0 + corr[5] * y0;
    let num = vx * (corr[1] - apx) + vy * (corr[3] - apy);
    let den = vx * (corr[2] * vx + corr[4] * vy) + vy * (corr[4] * vx + corr[5] * vy);
    let t = if den > 0.0 {
        (num / den).clamp(0.0, 1.0)
    } else if num > 0.0 {
        1.0
    } else {
        0.0
    };
    let c0 = x0 + t * vx;
    let c1 = y0 + t * vy;
    let error = eval_at(corr, c0, c1);
    if error < *best_error {
        *best_error = error;
        coeff[0] = c0;
        coeff[1] = c1;
    }
}

/// `vadpcm_solve_stable`: best coefficients within the stability triangle. Returns 1 when
/// the constraint was active.
fn solve_stable(corr: &[f64; 6], coeff: &mut [f64; 2]) -> i32 {
    solve(corr, coeff);
    if coeff[1] >= -1.0 && coeff[0] + coeff[1] <= 1.0 && coeff[1] - coeff[0] <= 1.0 {
        return 0;
    }
    let mut best_error = f64::MAX;
    coeff[0] = 0.0;
    coeff[1] = 0.0;
    solve_edge(corr, -2.0, -1.0, 2.0, -1.0, &mut best_error, coeff);
    solve_edge(corr, 2.0, -1.0, 0.0, 1.0, &mut best_error, coeff);
    solve_edge(corr, 0.0, 1.0, -2.0, -1.0, &mut best_error, coeff);
    1
}

/// `vadpcm_refine_predictors`: reassign frames to the best active predictor. Returns the
/// index of an unused predictor, or the active count when all are used.
fn refine_predictors(predictor_count: usize, corr: &[Corr], error: &mut [f32], predictors: &mut [u8]) -> usize {
    let mut pcorr = [[0.0f64; 6]; MAX_PREDICTORS];
    let mut count = [0i32; MAX_PREDICTORS];
    meancorrs(predictor_count, corr, predictors, &mut pcorr, &mut count);

    let mut coeff = [[0.0f32; 2]; MAX_PREDICTORS];
    let mut active_count = 0usize;
    for i in 0..predictor_count {
        if count[i] > 0 {
            let mut dcoeff = [0.0f64; 2];
            solve_stable(&pcorr[i], &mut dcoeff);
            for j in 0..2 {
                coeff[active_count][j] = dcoeff[j] as f32;
            }
            active_count += 1;
        }
    }

    let mut count2 = [0i32; MAX_PREDICTORS];
    for (frame, c) in corr.iter().enumerate() {
        let mut fpredictor = 0usize;
        let mut ferror = 0.0f32;
        for (i, co) in coeff.iter().enumerate().take(active_count) {
            let e = eval(c, co);
            if i == 0 || e < ferror {
                fpredictor = i;
                ferror = e;
            }
        }
        predictors[frame] = fpredictor as u8;
        error[frame] = ferror;
        count2[fpredictor] += 1;
    }
    (0..active_count).find(|&i| count2[i] == 0).unwrap_or(active_count)
}

/// `vadpcm_worst_frame`: frame with the highest error relative to its best case.
fn worst_frame(best_error: &[f32], error: &[f32]) -> usize {
    let mut best_improvement = error[0] - best_error[0];
    let mut best_index = 0usize;
    for frame in 1..error.len() {
        let improvement = error[frame] - best_error[frame];
        if improvement > best_improvement {
            best_improvement = improvement;
            best_index = frame;
        }
    }
    best_index
}

/// `vadpcm_assign_predictors`.
fn assign_predictors(predictor_count: usize, corr: &[Corr], predictors: &mut [u8]) {
    predictors.fill(0);
    if predictor_count <= 1 {
        return;
    }
    let best = best_error(corr);
    // Uninitialised (malloc) in C, but only read after the first refinement writes it.
    let mut error = vec![0.0f32; corr.len()];
    let mut unassigned = predictor_count;
    let mut active_count = 1usize;
    for _ in 0..ITERATIONS {
        if unassigned < predictor_count {
            let worst = worst_frame(&best, &error);
            predictors[worst] = unassigned as u8;
            if unassigned >= active_count {
                active_count = unassigned + 1;
            }
        }
        unassigned = refine_predictors(active_count, corr, &mut error, predictors);
    }
}

// ---------------------------------------------------------------------------------------------
// encode.c
// ---------------------------------------------------------------------------------------------

/// `vadpcm_make_vectors`: codebook vectors for one predictor.
fn make_vectors(coeff: &[f64; 2], vectors: &mut [Vector]) {
    let scale = f64::from(1i32 << 11);
    for (i, vec) in vectors.iter_mut().enumerate().take(2) {
        let (mut x1, mut x2) = (0.0f64, 0.0f64);
        if i == 0 {
            x2 = scale;
        } else {
            x1 = scale;
        }
        for v in vec.iter_mut() {
            let x = coeff[0] * x1 + coeff[1] * x2;
            let value: i32 = if x > f64::from(0x7fff) {
                0x7fff
            } else if x < f64::from(-0x8000) {
                -0x8000
            } else {
                x.round_ties_even() as i32
            };
            *v = value as i16;
            x2 = x1;
            x1 = x;
        }
    }
}

/// `vadpcm_make_codebook`.
fn make_codebook(predictor_count: usize, corr: &[Corr], predictors: &[u8], codebook: &mut [Vector]) {
    let mut pcorr = [[0.0f64; 6]; MAX_PREDICTORS];
    let mut count = [0i32; MAX_PREDICTORS];
    meancorrs(predictor_count, corr, predictors, &mut pcorr, &mut count);
    for i in 0..predictor_count {
        let cb = &mut codebook[2 * i..2 * i + 2];
        if count[i] > 0 {
            let mut coeff = [0.0f64; 2];
            solve_stable(&pcorr[i], &mut coeff);
            make_vectors(&coeff, cb);
        } else {
            cb.fill([0; 8]);
        }
    }
}

/// `vadpcm_getshift`.
fn getshift(mut min: i32, mut max: i32, min_residual: i32, max_residual: i32) -> i32 {
    let mut shift = 0;
    while shift < 12 && (min < min_residual || max_residual < max) {
        min >>= 1;
        max >>= 1;
        shift += 1;
    }
    shift
}

/// `struct vadpcm_encoder_state`.
#[derive(Clone, Copy, Debug, Default)]
pub struct EncoderState {
    pub data: [i16; 2],
    pub rng: u32,
}

/// `vadpcm_encode_data`: encode with a fixed predictor assignment and codebook.
#[allow(clippy::too_many_arguments)]
pub fn vadpcm_encode_data(
    frame_count: usize,
    dest: &mut [u8],
    src: &[i16],
    predictors: &[u8],
    codebook: &[Vector],
    dither: Dither,
    min_residual: i32,
    max_residual: i32,
    stats: &mut Stats,
    encoder_state: &mut EncoderState,
) {
    let mut rng_state = encoder_state.rng;
    let mut state = [0i32; 4];
    state[0] = i32::from(encoder_state.data[0]);
    state[1] = i32::from(encoder_state.data[1]);
    stats.signal_mean_square = 0.0;
    stats.error_mean_square = 0.0;
    for frame in 0..frame_count {
        let predictor = usize::from(predictors[frame]);
        let pvec = &codebook[2 * predictor..2 * predictor + 2];
        let p0: [i32; 8] = std::array::from_fn(|i| i32::from(pvec[0][i]));
        let p1: [i32; 8] = std::array::from_fn(|i| i32::from(pvec[1][i]));
        let fsrc: [i32; 16] = std::array::from_fn(|i| i32::from(src[frame * 16 + i]));
        let mut accumulator = [0i32; 8];

        let mut sum_square = 0.0f64;
        for &v in &fsrc {
            let value = f64::from(v);
            sum_square += value * value;
        }
        stats.signal_mean_square += sum_square;

        // Residual at full precision, to find the scaling factor.
        state[2] = fsrc[6];
        state[3] = fsrc[7];
        let mut min = 0i32;
        let mut max = 0i32;
        for vector in 0..2 {
            let s0 = state[vector * 2];
            let s1 = state[vector * 2 + 1];
            for i in 0..8 {
                accumulator[i] = (fsrc[vector * 8 + i].wrapping_mul(1 << 11)).wrapping_sub(s0.wrapping_mul(p0[i])).wrapping_sub(s1.wrapping_mul(p1[i]));
            }
            for i in 0..8 {
                let s = accumulator[i] >> 11;
                if s < min {
                    min = s;
                }
                if s > max {
                    max = s;
                }
                for j in 0..7 - i {
                    accumulator[i + 1 + j] = accumulator[i + 1 + j].wrapping_sub(s.wrapping_mul(p1[j]));
                }
            }
        }
        let shift = getshift(min, max, min_residual, max_residual);

        // Try three shift values and keep the one with the lowest error.
        let mut best_error = 0.0f64;
        let min_shift = if shift > 0 { shift - 1 } else { 0 };
        let max_shift = if shift < 12 { shift + 1 } else { 12 };
        let init_state = rng_state;
        for shift in min_shift..=max_shift {
            rng_state = init_state;
            let mut fout = [0u8; 8];
            let mut error = 0.0f64;
            let round_bias = if shift > 0 { 1i32 << (shift - 1) } else { 0 };
            let mut s0 = state[0];
            let mut s1 = state[1];
            for vector in 0..2 {
                for i in 0..8 {
                    accumulator[i] = s0.wrapping_mul(p0[i]).wrapping_add(s1.wrapping_mul(p1[i]));
                }
                for i in 0..8 {
                    let s = fsrc[vector * 8 + i];
                    let a = accumulator[i] >> 11;
                    let bias = match dither {
                        Dither::Rectangular => {
                            let b = ((rng_state >> 16) >> (16 - shift)) as i32;
                            rng_state = rng(rng_state);
                            b
                        }
                        Dither::None => round_bias,
                    };
                    let mut r = (s.wrapping_sub(a).wrapping_add(bias)) >> shift;
                    if r > max_residual {
                        r = max_residual;
                    } else if r < min_residual {
                        r = min_residual;
                    }
                    accumulator[i] = r;
                    let mut sout = r.wrapping_mul(1 << shift);
                    for j in 0..7 - i {
                        accumulator[i + 1 + j] = accumulator[i + 1 + j].wrapping_add(sout.wrapping_mul(p1[j]));
                    }
                    sout = sout.wrapping_add(a);
                    sout = sout.clamp(-0x8000, 0x7fff);
                    s0 = s1;
                    s1 = sout;
                    let serror = f64::from(s.wrapping_sub(sout));
                    error += serror * serror;
                }
                for i in 0..4 {
                    fout[vector * 4 + i] = (((accumulator[2 * i] & 15) << 4) | (accumulator[2 * i + 1] & 15)) as u8;
                }
            }
            if shift == min_shift || error < best_error {
                dest[frame * 9] = ((shift << 4) as usize | predictor) as u8;
                dest[frame * 9 + 1..frame * 9 + 9].copy_from_slice(&fout);
                state[2] = s0;
                state[3] = s1;
                best_error = error;
            }
        }
        state[0] = state[2];
        state[1] = state[3];
        stats.error_mean_square += best_error;
    }
    *encoder_state = EncoderState { data: [state[0] as i16, state[1] as i16], rng: rng_state };
    let factor = 1.0f64 / ((frame_count * FRAME_SAMPLES) as f64 * (32768.0f64 * 32768.0f64));
    stats.signal_mean_square *= factor;
    stats.error_mean_square *= factor;
}

/// `vadpcm_encode`: encode `frame_count` frames of `src` into `dest`
/// (`frame_count * FRAME_BYTES` bytes), filling `codebook`
/// (`predictor_count * ENCODE_ORDER` vectors).
pub fn vadpcm_encode(
    params: &Params,
    codebook: &mut [Vector],
    frame_count: usize,
    dest: &mut [u8],
    src: &[i16],
    stats: Option<&mut Stats>,
) -> Result<(), Error> {
    let predictor_count = params.predictor_count;
    let (min_residual, max_residual) = (params.min_residual, params.max_residual);
    if !(1..=MAX_PREDICTORS).contains(&predictor_count) {
        return Err(Error::InvalidParams);
    }
    if min_residual < -8 || max_residual > 7 || min_residual > max_residual {
        return Err(Error::InvalidParams);
    }
    if frame_count == 0 {
        codebook[..ENCODE_ORDER * predictor_count].fill([0; 8]);
        if let Some(s) = stats {
            *s = Stats::default();
        }
        return Ok(());
    }
    let corr = autocorr(frame_count, src);
    let mut predictors = vec![0u8; frame_count];
    assign_predictors(predictor_count, &corr, &mut predictors);
    make_codebook(predictor_count, &corr, &predictors, codebook);
    let mut stats_buf = Stats::default();
    let mut encoder_state = EncoderState::default();
    vadpcm_encode_data(
        frame_count,
        dest,
        src,
        &predictors,
        codebook,
        params.dither,
        min_residual,
        max_residual,
        stats.unwrap_or(&mut stats_buf),
        &mut encoder_state,
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// decode.c
// ---------------------------------------------------------------------------------------------

/// `vadpcm_ext4`: sign-extend a 4-bit value.
#[inline]
fn ext4(x: i32) -> i32 {
    if x > 7 {
        x - 16
    } else {
        x
    }
}

/// `vadpcm_clamp16`.
#[inline]
fn clamp16(x: i32) -> i32 {
    if !(-0x8000..=0x7fff).contains(&x) {
        (x >> 31) ^ 0x7fff
    } else {
        x
    }
}

/// `vadpcm_decode`: decode `frame_count` frames from `src` into `dest`
/// (`frame_count * FRAME_SAMPLES` samples), updating the decoder `state`.
pub fn vadpcm_decode(
    predictor_count: usize,
    order: usize,
    codebook: &[Vector],
    state: &mut Vector,
    frame_count: usize,
    dest: &mut [i16],
    src: &[u8],
) -> Result<(), Error> {
    for frame in 0..frame_count {
        let fin = &src[FRAME_BYTES * frame..FRAME_BYTES * (frame + 1)];
        let control = i32::from(fin[0]);
        let scaling = control >> 4;
        let predictor_index = (control & 15) as usize;
        if predictor_index >= predictor_count {
            return Err(Error::InvalidData);
        }
        let predictor = &codebook[order * predictor_index..order * (predictor_index + 1)];

        for vector in 0..2 {
            let mut accumulator = [0i32; 8];
            for k in 0..order {
                let sample = i32::from(state[8 - order + k]);
                for i in 0..8 {
                    accumulator[i] = accumulator[i].wrapping_add(sample.wrapping_mul(i32::from(predictor[k][i])));
                }
            }

            let mut residuals = [0i32; 8];
            for i in 0..4 {
                let byte = i32::from(fin[1 + 4 * vector + i]);
                residuals[2 * i] = ext4(byte >> 4);
                residuals[2 * i + 1] = ext4(byte & 15);
            }

            let v = &predictor[order - 1];
            for k in 0..8 {
                let residual = residuals[k].wrapping_mul(1 << scaling);
                accumulator[k] = accumulator[k].wrapping_add(residual.wrapping_mul(1 << 11));
                for i in 0..7 - k {
                    accumulator[k + 1 + i] = accumulator[k + 1 + i].wrapping_add(residual.wrapping_mul(i32::from(v[i])));
                }
            }

            for i in 0..8 {
                let sample = clamp16(accumulator[i] >> 11) as i16;
                dest[FRAME_SAMPLES * frame + 8 * vector + i] = sample;
                state[i] = sample;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        Params { predictor_count: 4, dither: Dither::Rectangular, min_residual: -8, max_residual: 7 }
    }

    fn roundtrip(src: &[i16]) -> (Vec<i16>, f64) {
        let n = src.len() / FRAME_SAMPLES;
        let mut cb = vec![[0i16; 8]; 8];
        let mut dest = vec![0u8; n * FRAME_BYTES];
        let mut stats = Stats::default();
        vadpcm_encode(&params(), &mut cb, n, &mut dest, src, Some(&mut stats)).unwrap();
        let mut out = vec![0i16; n * FRAME_SAMPLES];
        let mut st = [0i16; 8];
        vadpcm_decode(4, 2, &cb, &mut st, n, &mut out, &dest).unwrap();
        (out, stats.error_mean_square)
    }

    #[test]
    fn roundtrip_sine_small_error() {
        let src: Vec<i16> = (0..4096).map(|i| ((i as f64 * 0.05).sin() * 12000.0) as i16).collect();
        let (out, ems) = roundtrip(&src);
        let mut err = 0.0f64;
        let mut sig = 0.0f64;
        for (a, b) in src.iter().zip(&out) {
            let d = f64::from(*a) - f64::from(*b);
            err += d * d;
            sig += f64::from(*a) * f64::from(*a);
        }
        let snr = 10.0 * (sig / err).log10();
        assert!(snr > 30.0, "snr {snr}");
        // The encoder's own error estimate is the decoder's actual error.
        let measured = err / (src.len() as f64 * 32768.0 * 32768.0);
        assert!((measured - ems).abs() <= 1e-12 + measured * 1e-9, "{measured} vs {ems}");
    }

    #[test]
    fn silence_and_empty() {
        let (out, _) = roundtrip(&[0i16; 64]);
        assert!(out.iter().all(|&s| s == 0));
        let mut cb = vec![[1i16; 8]; 8];
        vadpcm_encode(&params(), &mut cb, 0, &mut [], &[], None).unwrap();
        assert!(cb.iter().all(|v| *v == [0; 8]));
    }

    #[test]
    fn invalid_params_and_data() {
        let mut p = params();
        p.max_residual = 8;
        assert_eq!(vadpcm_encode(&p, &mut [[0; 8]; 8], 0, &mut [], &[], None), Err(Error::InvalidParams));
        let mut st = [0i16; 8];
        let mut out = [0i16; 16];
        assert_eq!(vadpcm_decode(4, 2, &[[0; 8]; 8], &mut st, 1, &mut out, &[0x05, 0, 0, 0, 0, 0, 0, 0, 0]), Err(Error::InvalidData));
    }

    #[test]
    fn stable_solution_is_in_triangle() {
        // An unstable unconstrained solution gets projected onto the triangle.
        let corr = [1.0, 0.99, 1.0, 0.99, 0.0, 1.0];
        let mut c = [0.0; 2];
        solve_stable(&corr, &mut c);
        assert!(c[1] >= -1.0 && c[0] + c[1] <= 1.0 + 1e-12 && c[1] - c[0] <= 1.0 + 1e-12);
    }
}
