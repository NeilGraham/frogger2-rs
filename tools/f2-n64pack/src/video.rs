//! FMV: the 14 Bink movies → libdragon video player format (+ `wav64` audio track).
//!
//! [`pack_fmv`] writes finished files under `video/` in `ctx.out` (`NAME.h264` + `NAME.wav64`, the audio
//! sitting next to the video because `fmv_play()` looks for `<video>.wav64`) whose total size is at most
//! `budget` (best quality that fits, capped at the decoder's limits).
//!
//! ## Choices (measured, see `compare_codecs` and the notes `pack_fmv` returns)
//! * **Codec: H.264.** Measured with `compare_codecs` (SSIM/PSNR against the Bink source scaled to 320x240):
//!   `05s` H.264 348 kB = SSIM 0.952 / 33.7 dB vs MPEG-1 1349 kB = 0.928 / 33.5 dB (and 0.860 / 30.1 dB at
//!   MPEG-1's 388 kB floor); `12s` H.264 1689 kB = 0.938 / 35.2 dB vs MPEG-1 3111 kB = 0.899 / 34.8 dB, and
//!   MPEG-1 needs 5539 kB for 0.942; on the noisy `09s` MPEG-1 cannot even encode at its 200 kbps floor
//!   ("bitrate too low"). H.264 is 2.5-4x smaller at equal quality. MPEG-1 decodes cheaper, but cannot hit
//!   the FMV budget at comparable quality.
//! * **Decoder cap:** videoconv64 limits H.264 to `300 * 24 / fps` kbps (= 288 kbps at 25 fps) `maxrate`,
//!   the N64 decoder's throughput limit; never raised here. Hence the maximum-quality FMV is bounded
//!   (15.96 MB of video for 472 s, 18.3 MB with audio) and a larger budget cannot buy more.
//! * **Resolution:** 320x240 (width 320, 4:3). 256x192 is the fallback rung for budgets below the 320 CRF-36
//!   size. The RDP/VI shows 320x240 natively; wider is pointless (display) and costs decode time.
//!   Decode RAM: libdragon allocates `decode_ahead_pics` (8 with the Expansion Pak, 4 without) YUV 4:2:0
//!   pictures, 115 kB each at 320x240 -> about 1 MB, plus a 64 kB bitstream buffer and the H.264 DPB.
//! * **Quality knob:** videoconv64 `-q 0..100` maps to H.264 CRF 36..14 (sqrt curve); the search ladder is
//!   one rung per distinct CRF (22), searched by bisection per width. Widths are tried 320 first,
//!   then 256 only when even 320 at CRF 36 exceeds the budget (sizes are not monotone across widths).
//! * **Audio:** Opus, mono 32 kbps (stereo `blitzs` 64 kbps), `wav64` loaded whole by `fmv_play`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::audio::{audioconv, opus_resample_for};
use crate::tools::{file_key, par_map, run};
use crate::{Class, ClassReport, Ctx, PackedFile, Result};

/// The 14 retail movies (file stems of `video/*.bik`).
pub const MOVIES: [&str; 14] = ["01s", "02s", "03s", "04s", "05s", "06s", "07s", "08s", "09s", "10s", "11s", "12s", "blitzs", "hasbros"];

pub const AUDIO_KBPS_MONO: u32 = 32;
pub const AUDIO_KBPS_STEREO: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Mpeg1,
}

impl VideoCodec {
    pub fn name(self) -> &'static str {
        match self {
            VideoCodec::H264 => "h264",
            VideoCodec::Mpeg1 => "mpeg1",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            VideoCodec::H264 => "h264",
            VideoCodec::Mpeg1 => "m1v",
        }
    }
}

/// videoconv64's `quality_to_h264_crf` (`vconv_encode_h264.cpp`).
pub fn h264_crf(q: u32) -> u32 {
    let x = f64::from(100 - q.min(100)) / 100.0;
    let crf = 14.0 + 22.0 * x.powf(0.5);
    (crf + 0.5) as u32
}

/// One rung of the quality ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rung {
    pub width: u32,
    pub q: u32,
}

impl Rung {
    pub fn label(&self) -> String {
        format!("h264 {}x{} q{} (crf {})", self.width, self.width * 3 / 4, self.q, h264_crf(self.q))
    }
}

/// Widths tried in order of preference; the next one is used only when even the lowest-quality rung
/// of the previous one exceeds the budget (sizes are not monotone across widths).
pub const WIDTHS: [u32; 2] = [320, 256];

/// Quality ladder for one width, worst to best: one rung per distinct CRF (22 rungs).
pub fn ladder(width: u32) -> Vec<Rung> {
    let mut v: Vec<Rung> = Vec::new();
    for q in 0..=100 {
        if v.last().is_none_or(|l| h264_crf(l.q) != h264_crf(q)) {
            v.push(Rung { width, q });
        }
    }
    v
}

fn src_path(ctx: &Ctx, name: &str) -> PathBuf {
    ctx.rendition.join("video").join(format!("{name}.bik"))
}

fn atomic_write(dst: &Path, data: &[u8]) -> Result<()> {
    let part = dst.with_extension(format!("part{}", std::process::id()));
    std::fs::write(&part, data).map_err(|e| format!("{}: {e}", part.display()))?;
    std::fs::rename(&part, dst).map_err(|e| e.to_string())
}

/// x264 reference frames. libdragon's H.264 decoder allocates max_dec_frame_buffering + 1
/// pictures (115 KiB each at 320x240); x264's "slower" preset means 8 refs, ~1 MiB of RDRAM
/// during a video, which a story FMV started from the book did not have (out of memory in
/// h264bsdInitDpb). 1 ref: 2 pictures.
pub const H264_REFS: u32 = 1;

/// Encodes one movie's video stream (cached). Returns the cached file path.
pub fn encode_video(ctx: &Ctx, name: &str, codec: VideoCodec, width: u32, q: u32) -> Result<PathBuf> {
    let src = src_path(ctx, name);
    let cache = ctx.cache_dir("fmv").map_err(|e| e.to_string())?;
    let params = match codec {
        VideoCodec::H264 => format!("h264-w{width}-crf{}-quick{}-ref{H264_REFS}", h264_crf(q), u8::from(ctx.quick)),
        VideoCodec::Mpeg1 => format!("mpeg1-w{width}-q{q}-quick{}", u8::from(ctx.quick)),
    };
    let cached = cache.join(format!("{name}-{}.{}", file_key(&src, &params), codec.ext()));
    if cached.is_file() {
        return Ok(cached);
    }
    let tmp = cache.join(format!("tmp-{name}-{}-{}", std::process::id(), params));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let mut c = Command::new(ctx.tool("videoconv64"));
    c.env("N64_INST", std::path::absolute(ctx.tools.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?);
    c.args(["-c", codec.name(), "-w", &width.to_string(), "-q", &q.to_string(), "--no-audio", "--no-progress"]);
    // Our videoconv64 (tools/n64/libdragon-3a.patch) passes this on to x264 as `ref=N`.
    c.env("VCONV64_H264_REF", H264_REFS.to_string());
    if ctx.quick {
        c.arg("-Q");
    }
    // Own working directory: the MPEG-1 path runs two-pass ffmpeg whose log files land in the cwd,
    // which clashes between parallel encodes.
    let tmp_abs = std::path::absolute(&tmp).map_err(|e| e.to_string())?;
    let src_abs = std::path::absolute(&src).map_err(|e| e.to_string())?;
    c.current_dir(&tmp_abs).arg("-o").arg(&tmp_abs).arg(&src_abs);
    run(&mut c)?;
    let out = tmp.join(format!("{name}.{}", codec.ext()));
    let data = std::fs::read(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    atomic_write(&cached, &data)?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(cached)
}

/// Encodes one movie's audio track (Opus, cached); returns the `.wav64` bytes.
pub fn encode_audio(ctx: &Ctx, name: &str) -> Result<Vec<u8>> {
    encode_audio_at(ctx, name, (AUDIO_KBPS_MONO, AUDIO_KBPS_STEREO))
}

/// [`encode_audio`] at explicit (mono, stereo) Opus bitrates.
pub fn encode_audio_at(ctx: &Ctx, name: &str, kbps: (u32, u32)) -> Result<Vec<u8>> {
    let src = src_path(ctx, name);
    let cache = ctx.cache_dir("fmv").map_err(|e| e.to_string())?;
    let mut p = Command::new("ffprobe");
    p.args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=channels", "-of", "csv=p=0"]).arg(&src);
    let channels: u32 = String::from_utf8_lossy(&run(&mut p)?.stdout).trim().parse().map_err(|_| format!("{name}: no audio stream"))?;
    let kbps = if channels >= 2 { kbps.1 } else { kbps.0 };
    let params = format!("opus{kbps}-ch{channels}");
    let cached = cache.join(format!("{name}-{}.wav64", file_key(&src, &params)));
    if let Ok(b) = std::fs::read(&cached) {
        return Ok(b);
    }
    let tmp = cache.join(format!("tmp-{name}-audio-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let wav = tmp.join(format!("{name}.wav"));
    let mut f = Command::new("ffmpeg");
    f.args(["-v", "error", "-y", "-i"]).arg(&src).args(["-vn", "-c:a", "pcm_s16le"]).arg(&wav);
    run(&mut f)?;
    let flags: Vec<String> = ["--wav-compress", "3", "--wav-resample", &opus_resample_for(kbps, channels).to_string()].iter().map(|s| s.to_string()).collect();
    let out = audioconv(ctx, &wav, &tmp.join("out"), &flags)?;
    let data = std::fs::read(&out).map_err(|e| e.to_string())?;
    atomic_write(&cached, &data)?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(data)
}

/// Total video bytes of all movies at a rung (encodes, cached).
fn video_sizes(ctx: &Ctx, r: Rung) -> Result<Vec<u64>> {
    let res = par_map(&MOVIES, ctx.jobs, |m| {
        encode_video(ctx, m, VideoCodec::H264, r.width, r.q).and_then(|p| std::fs::metadata(&p).map(|m| m.len()).map_err(|e| e.to_string()))
    });
    res.into_iter().collect()
}

/// Encodes everything and picks the best rung whose total fits `budget`.
pub fn pack_fmv(ctx: &Ctx, budget: u64) -> Result<ClassReport> {
    pack_fmv_with(ctx, budget, (AUDIO_KBPS_MONO, AUDIO_KBPS_STEREO))
}

/// [`pack_fmv`] with explicit (mono, stereo) Opus bitrates for the audio tracks.
pub fn pack_fmv_with(ctx: &Ctx, budget: u64, audio_kbps: (u32, u32)) -> Result<ClassReport> {
    if !ctx.rendition.join("video").is_dir() {
        return Err(format!("no video dir under {}", ctx.rendition.display()));
    }
    let audio: Vec<Result<Vec<u8>>> = par_map(&MOVIES, ctx.jobs, |m| encode_audio_at(ctx, m, audio_kbps));
    let mut audio_bytes = Vec::new();
    for (m, a) in MOVIES.iter().zip(audio) {
        audio_bytes.push(a.map_err(|e| format!("{m} audio: {e}"))?);
    }
    let audio_total: u64 = audio_bytes.iter().map(|a| a.len() as u64).sum();

    let total_at = |r: Rung| -> Result<u64> { Ok(video_sizes(ctx, r)?.iter().sum::<u64>() + audio_total) };
    let mut chosen: Option<(Rung, usize, usize)> = None;
    let mut smallest = u64::MAX;
    for &w in &WIDTHS {
        let l = ladder(w);
        let top = l.len() - 1;
        if budget == u64::MAX || total_at(l[top])? <= budget {
            chosen = Some((l[top], top, l.len()));
            break;
        }
        let floor = total_at(l[0])?;
        smallest = smallest.min(floor);
        if floor > budget {
            continue;
        }
        // Binary search for the highest rung that fits (sizes grow with the rung index).
        let (mut lo, mut hi) = (0usize, top); // lo fits, hi does not
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if total_at(l[mid])? <= budget {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        chosen = Some((l[lo], lo, l.len()));
        break;
    }
    let (rung, idx, nrungs) =
        chosen.ok_or_else(|| format!("FMV budget {budget} bytes is below the smallest encode ({smallest} bytes, incl. {audio_total} audio)"))?;
    let sizes = video_sizes(ctx, rung)?;

    let mut rep = ClassReport::default();
    let mut total = 0u64;
    for (i, name) in MOVIES.iter().enumerate() {
        let v = encode_video(ctx, name, VideoCodec::H264, rung.width, rung.q)?;
        let vb = std::fs::read(&v).map_err(|e| e.to_string())?;
        let vrel = format!("video/{name}.h264");
        std::fs::write(ctx.out_path(&vrel).map_err(|e| e.to_string())?, &vb).map_err(|e| e.to_string())?;
        let arel = format!("video/{name}.wav64");
        std::fs::write(ctx.out_path(&arel).map_err(|e| e.to_string())?, &audio_bytes[i]).map_err(|e| e.to_string())?;
        let src_bytes = std::fs::metadata(src_path(ctx, name)).map(|m| m.len()).unwrap_or(0);
        total += sizes[i] + audio_bytes[i].len() as u64;
        rep.files.push(PackedFile { path: vrel, class: Class::Fmv, bytes: sizes[i], source_bytes: src_bytes, format: rung.label(), notes: vec![] });
        rep.files.push(PackedFile { path: arel, class: Class::Fmv, bytes: audio_bytes[i].len() as u64, source_bytes: 0, format: "opus".into(), notes: vec![] });
    }
    rep.notes.push(format!(
        "fmv: {} at rung {}/{} of its width -> {total} bytes (video {} + audio {audio_total}), budget {}",
        rung.label(),
        idx + 1,
        nrungs,
        total - audio_total,
        if budget == u64::MAX { "unlimited".to_string() } else { budget.to_string() }
    ));
    rep.notes.push(
        "fmv: codec h264 (see video.rs docs: 2.5-4x smaller than MPEG-1 at equal SSIM); decoder cap maxrate 288 kbps at 25 fps (videoconv64) is never exceeded"
            .into(),
    );
    rep.notes.push(format!("fmv: audio Opus mono {} kbps (blitzs stereo {}); wav64 next to each video", audio_kbps.0, audio_kbps.1));
    Ok(rep)
}

/// Total FMV bytes (video + audio) at a few quality levels, for the budget table. Encodes (cached).
pub fn fmv_size_table(ctx: &Ctx, qs: &[u32]) -> Result<Vec<(String, u64)>> {
    let audio: u64 = par_map(&MOVIES, ctx.jobs, |m| encode_audio(ctx, m)).into_iter().collect::<Result<Vec<_>>>()?.iter().map(|a| a.len() as u64).sum();
    let mut out = Vec::new();
    for &q in qs {
        let r = Rung { width: 320, q };
        out.push((r.label(), video_sizes(ctx, r)?.iter().sum::<u64>() + audio));
    }
    Ok(out)
}

/// Mean SSIM (All) and PSNR of `encoded` against the source movie scaled to the encoded size.
pub fn quality_vs_source(src: &Path, encoded: &Path) -> Result<(f64, f64)> {
    let mut p = Command::new("ffprobe");
    p.args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0:s=x"]).arg(encoded);
    let dims = String::from_utf8_lossy(&run(&mut p)?.stdout).trim().to_string();
    let mut it = dims.split('x').filter(|x| !x.is_empty());
    let (w, h) = (it.next().ok_or_else(|| format!("bad dims {dims}"))?, it.next().ok_or_else(|| format!("bad dims {dims}"))?);
    let measure = |metric: &str, key: &str| -> Result<f64> {
        let fc = format!("[1:v]scale={w}:{h}:flags=lanczos,format=yuv420p[r];[0:v]format=yuv420p[d];[d][r]{metric}");
        let mut c = Command::new("ffmpeg");
        c.args(["-v", "info", "-i"]).arg(encoded).arg("-i").arg(src).args(["-filter_complex", &fc, "-f", "null", "-"]);
        let o = run(&mut c)?;
        let text = String::from_utf8_lossy(&o.stderr).into_owned();
        let line = text.lines().rev().find(|l| l.contains(key)).ok_or("metric line not found")?;
        let v = line.split(key).nth(1).and_then(|r| r.split_whitespace().next()).ok_or("metric parse")?;
        v.trim_end_matches(')').parse::<f64>().map_err(|e| e.to_string())
    };
    Ok((measure("ssim", "All:")?, measure("psnr", "average:")?))
}

/// One codec-comparison probe result.
pub struct Probe {
    pub label: String,
    pub bytes: u64,
    pub ssim: f64,
    pub psnr: f64,
}

/// H.264 vs MPEG-1 quality probe on one movie (320 wide). A failed encode (e.g. MPEG-1 q0 on noisy
/// movies: "bitrate too low for this video") is returned as `Err` for that label.
pub fn compare_codecs(ctx: &Ctx, name: &str) -> Vec<(String, Result<Probe>)> {
    let src = src_path(ctx, name);
    let jobs: Vec<(VideoCodec, u32)> =
        vec![(VideoCodec::H264, 30), (VideoCodec::H264, 60), (VideoCodec::H264, 100), (VideoCodec::Mpeg1, 0), (VideoCodec::Mpeg1, 40), (VideoCodec::Mpeg1, 70)];
    par_map(&jobs, ctx.jobs, |&(c, q)| {
        let label = format!("{} q{q}", c.name());
        let r = (|| -> Result<Probe> {
            let p = encode_video(ctx, name, c, 320, q)?;
            let (ssim, psnr) = quality_vs_source(&src, &p)?;
            Ok(Probe { label: label.clone(), bytes: std::fs::metadata(&p).map_err(|e| e.to_string())?.len(), ssim, psnr })
        })();
        (label, r)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crf_curve_matches_videoconv64() {
        assert_eq!(h264_crf(100), 14);
        assert_eq!(h264_crf(0), 36);
        assert_eq!(h264_crf(60), 28);
    }

    #[test]
    fn ladder_is_monotone_with_unique_crfs() {
        let l = ladder(320);
        assert_eq!(l.len(), 22); // crf 15 is skipped by the sqrt curve
        for w in l.windows(2) {
            assert!(h264_crf(w[0].q) > h264_crf(w[1].q));
        }
        assert_eq!(l[0], Rung { width: 320, q: 0 });
        assert_eq!(l.last().unwrap().q, 100);
        assert_eq!(ladder(256)[0].width, 256);
    }
}
