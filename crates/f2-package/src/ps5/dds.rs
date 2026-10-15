//! BC7 DDS artwork for PS5 titles (`sce_sys/icon0.dds`, `pic0.dds`, `pic1.dds`): a small BC7
//! encoder (mode 6 only: one subset, RGBA endpoints with p-bits, 4-bit indices: plenty for
//! launcher art) and the DX10 DDS header in the exact form `tools/ps5/build.sh` writes with
//! bc7enc_rdo (single level, BC7_UNORM, mip count 1, straight alpha), which the PS5 shell and
//! LibProsperoPkg accept.

use crate::Image;

const WEIGHTS: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// A DDS file (DX10 header, BC7_UNORM) of `img` (width and height multiples of 4).
pub(crate) fn bc7_dds(img: &Image) -> Vec<u8> {
    assert!(img.width.is_multiple_of(4) && img.height.is_multiple_of(4), "BC7 needs whole 4x4 blocks");
    let (w, h) = (img.width, img.height);
    let mut out = vec![0u8; 148];
    let mut put = |o: usize, v: u32| out[o..o + 4].copy_from_slice(&v.to_le_bytes());
    put(4, 124);
    put(8, 0x000A_1007); // caps | height | width | pixel format | mip count | linear size
    put(12, h);
    put(16, w);
    put(20, w * h); // linear size: one byte per pixel
    put(24, 1); // depth
    put(28, 1); // mip count
    put(76, 32); // pixel format size
    put(80, 4); // FOURCC
    put(108, 0x1000); // texture
    put(128, 98); // DXGI_FORMAT_BC7_UNORM
    put(132, 3); // texture 2D
    put(140, 1); // array size
    put(144, 1); // straight alpha
    out[0..4].copy_from_slice(b"DDS ");
    out[84..88].copy_from_slice(b"DX10");
    out.reserve((w * h) as usize);
    // blocks in parallel bands of rows
    let rows = (h / 4) as usize;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16);
    let per = rows.div_ceil(threads);
    let bands: Vec<Vec<u8>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|t| {
                s.spawn(move || {
                    let mut v = Vec::new();
                    for by in (t * per..((t + 1) * per).min(rows)).map(|r| r as u32) {
                        for bx in 0..w / 4 {
                            let mut px = [[0u8; 4]; 16];
                            for (i, p) in px.iter_mut().enumerate() {
                                let (x, y) = (bx * 4 + i as u32 % 4, by * 4 + i as u32 / 4);
                                let o = ((y * w + x) * 4) as usize;
                                p.copy_from_slice(&img.rgba[o..o + 4]);
                            }
                            v.extend(encode_block(&px));
                        }
                    }
                    v
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for b in bands {
        out.extend(b);
    }
    out
}

fn interp(e0: [u32; 4], e1: [u32; 4], i: usize) -> [u32; 4] {
    let w = WEIGHTS[i];
    std::array::from_fn(|c| ((64 - w) * e0[c] + w * e1[c] + 32) >> 6)
}

fn err(a: [u32; 4], b: [u8; 4]) -> u32 {
    (0..4).map(|c| (a[c] as i32 - b[c] as i32).pow(2) as u32).sum()
}

/// 7-bit endpoint + p-bit closest to `v` (one p-bit for all four channels; opaque stays
/// exactly 255).
fn quantize(v: [f32; 4]) -> ([u32; 4], u32) {
    let mut best = ([0; 4], 0, f32::MAX);
    for p in if v[3] > 254.0 { 1..2u32 } else { 0..2u32 } {
        let q: [u32; 4] = std::array::from_fn(|c| ((v[c] - p as f32) / 2.0).round().clamp(0.0, 127.0) as u32);
        let e: f32 = (0..4).map(|c| ((q[c] * 2 + p) as f32 - v[c]).powi(2)).sum();
        if e < best.2 {
            best = (q, p, e);
        }
    }
    (best.0, best.1)
}

/// One mode-6 block of 16 RGBA pixels (row-major).
pub(crate) fn encode_block(px: &[[u8; 4]; 16]) -> [u8; 16] {
    // principal axis by power iteration on the covariance
    let mean: [f32; 4] = std::array::from_fn(|c| px.iter().map(|p| p[c] as f32).sum::<f32>() / 16.0);
    let mut cov = [[0f32; 4]; 4];
    for p in px {
        let d: [f32; 4] = std::array::from_fn(|c| p[c] as f32 - mean[c]);
        for i in 0..4 {
            for j in 0..4 {
                cov[i][j] += d[i] * d[j];
            }
        }
    }
    // start from the channel with the largest variance (a fixed start can be orthogonal to the
    // spread, e.g. red rising while blue falls)
    let k = (0..4).max_by(|&a, &b| cov[a][a].total_cmp(&cov[b][b])).unwrap();
    let mut axis = [0f32; 4];
    axis[k] = 1.0;
    for _ in 0..8 {
        let n: [f32; 4] = std::array::from_fn(|i| (0..4).map(|j| cov[i][j] * axis[j]).sum());
        let len = n.iter().map(|x| x * x).sum::<f32>().sqrt();
        if len < 1e-6 {
            break;
        }
        axis = n.map(|x| x / len);
    }
    let proj = |p: &[u8; 4]| (0..4).map(|c| (p[c] as f32 - mean[c]) * axis[c]).sum::<f32>();
    let (lo, hi) = px.iter().map(proj).fold((f32::MAX, f32::MIN), |(a, b), t| (a.min(t), b.max(t)));
    let ep = |t: f32| -> [f32; 4] { std::array::from_fn(|c| (mean[c] + axis[c] * t).clamp(0.0, 255.0)) };
    let (q0, p0) = quantize(ep(lo));
    let (q1, p1) = quantize(ep(hi));
    let e0 = q0.map(|q| q * 2 + p0);
    let e1 = q1.map(|q| q * 2 + p1);
    let palette: [[u32; 4]; 16] = std::array::from_fn(|i| interp(e0, e1, i));
    let mut idx = [0usize; 16];
    for (k, p) in px.iter().enumerate() {
        idx[k] = (0..16).min_by_key(|&i| err(palette[i], *p)).unwrap();
    }
    // the anchor (pixel 0) index must have its top bit clear: swap the endpoints if not
    let (q0, p0, q1, p1) = if idx[0] >= 8 {
        for i in idx.iter_mut() {
            *i = 15 - *i;
        }
        (q1, p1, q0, p0)
    } else {
        (q0, p0, q1, p1)
    };
    let mut bits = 1u128 << 6; // mode 6
    let mut pos = 7;
    let mut push = |v: u128, n: u32| {
        bits |= v << pos;
        pos += n;
    };
    for c in 0..4 {
        push(q0[c] as u128, 7);
        push(q1[c] as u128, 7);
    }
    push(p0 as u128, 1);
    push(p1 as u128, 1);
    for (k, &i) in idx.iter().enumerate() {
        push(i as u128, if k == 0 { 3 } else { 4 });
    }
    bits.to_le_bytes()
}

/// Decodes a mode-6 block (tests).
#[cfg(test)]
pub(crate) fn decode_block(b: &[u8; 16]) -> [[u8; 4]; 16] {
    let bits = u128::from_le_bytes(*b);
    assert_eq!(bits & 0x7F, 1 << 6, "mode 6");
    let mut pos = 7;
    let mut take = |n: u32| {
        let v = (bits >> pos) & ((1u128 << n) - 1);
        pos += n;
        v as u32
    };
    let mut q = [[0u32; 4]; 2];
    let [q0, q1] = &mut q;
    for (a, b) in q0.iter_mut().zip(q1.iter_mut()) {
        (*a, *b) = (take(7), take(7));
    }
    let (p0, p1) = (take(1), take(1));
    let e0 = q[0].map(|v| v * 2 + p0);
    let e1 = q[1].map(|v| v * 2 + p1);
    std::array::from_fn(|k| {
        let i = take(if k == 0 { 3 } else { 4 }) as usize;
        interp(e0, e1, i).map(|v| v as u8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_round_trip_closely() {
        let mut worst = 0u32;
        for seed in 0..200u32 {
            let px: [[u8; 4]; 16] = std::array::from_fn(|i| {
                let t = (i as u32 * 37 + seed * 101) % 256;
                [(t * 3 % 256) as u8, (255 - t) as u8, (seed * 7 % 256) as u8, 255]
            });
            let gradient = seed % 2 == 0;
            let px = if gradient { std::array::from_fn(|i| [(i * 16) as u8, (seed % 256) as u8, (255 - i * 16) as u8, 255]) } else { px };
            let d = decode_block(&encode_block(&px));
            for (a, b) in d.iter().zip(&px) {
                assert_eq!(a[3], 255, "opaque stays opaque");
                if gradient {
                    worst = worst.max((0..3).map(|c| (a[c] as i32 - b[c] as i32).unsigned_abs()).max().unwrap());
                }
            }
        }
        assert!(worst <= 6, "gradient error {worst}");
    }

    /// An independent decoder (bcdec) reads the blocks as this encoder means them, at a good
    /// quality on a smooth picture.
    #[test]
    fn independent_decoder_agrees() {
        let (w, h) = (64u32, 32u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let n = ((x * 7919 + y * 104729) % 13) as f32;
                let r = 128.0 + 100.0 * (x as f32 / 9.0).sin() + n;
                let g = 128.0 + 90.0 * (y as f32 / 7.0).cos();
                let b = (x + y) as f32 * 2.5;
                rgba.extend([r, g, b].map(|v| v.clamp(0.0, 255.0) as u8));
                rgba.push(255);
            }
        }
        let img = Image { width: w, height: h, rgba };
        let d = bc7_dds(&img);
        let (mut se, mut n) = (0f64, 0f64);
        for (k, blk) in d[148..].as_chunks::<16>().0.iter().enumerate() {
            let mut out = [0u8; 64];
            bcdec_rs::bc7(blk, &mut out, 16);
            let mine = decode_block(blk);
            let (bx, by) = (k as u32 % (w / 4), k as u32 / (w / 4));
            for i in 0..16u32 {
                assert_eq!(out[(i * 4) as usize..(i * 4 + 4) as usize], mine[i as usize], "block {k} pixel {i}");
                let o = (((by * 4 + i / 4) * w + bx * 4 + i % 4) * 4) as usize;
                for c in 0..3 {
                    se += (out[(i * 4) as usize + c] as f64 - img.rgba[o + c] as f64).powi(2);
                    n += 1.0;
                }
                assert_eq!(out[(i * 4 + 3) as usize], 255);
            }
        }
        let psnr = 10.0 * (255.0f64 * 255.0 / (se / n)).log10();
        assert!(psnr > 34.0, "PSNR {psnr:.1} dB");
    }

    #[test]
    fn dds_header() {
        let img = Image { width: 8, height: 4, rgba: vec![200; 8 * 4 * 4] };
        let d = bc7_dds(&img);
        assert_eq!(d.len(), 148 + 2 * 16);
        assert_eq!(&d[..4], b"DDS ");
        assert_eq!(&d[84..88], b"DX10");
        let blk: [u8; 16] = d[148..164].try_into().unwrap();
        assert!(decode_block(&blk).iter().all(|p| p.iter().all(|&v| (198..=202).contains(&v))));
    }
}
