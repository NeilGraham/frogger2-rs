//! Sound effects (deduplicated `.wus` samples) and music → libdragon `wav64`.
//!
//! * [`pack_sfx`]: parses the 19 banks, dedupes identical samples across banks, encodes each unique
//!   sample (VADPCM, or Opus where a documented rule says it is clearly smaller), writes `sfx/NNNN.wav64`
//!   plus `sfx/index.bin` ([`index`] documents the format).
//! * [`pack_music`]: `music/track02..12.wav` → `music/trackNN.wav64`, ULC stereo at `kbps` (ABR).
//!   Phase 3I measured libdragon's Opus at ~14 ms of VR4300 time per 60 Hz tick for one stereo
//!   track in ares (a third of the CPU); ULC ("simple and fast transform codec") costs
//!   ~2 ms at the same bitrate, so the music is ULC. [`pack_music_opus`] stays for comparison.
//!
//! ## Codec rule for sound effects
//! Everything is encoded as VADPCM (4.5 bit/sample, near-free RSP cost, exact loop support). A sample is
//! *also* tried as Opus when it is mono and at least [`OPUS_MIN_SECS`] long (one-shots and
//! whole-sample loops alike: libdragon's Opus wav64 supports terminal full-length loops);
//! Opus is kept only if its file is at most [`OPUS_MAX_RATIO`] of the VADPCM file. Short effects (the
//! ones that overlap heavily: hops, splats, pickups) therefore stay VADPCM, and the RSP only ever
//! decodes Opus for long voice lines / ambience loops (a handful of simultaneous voices at most; the
//! 6.4 MB of >= 2 s looped ambience was 60% of the all-VADPCM size, which is why loops qualify). Opus
//! bitrate is [`SFX_OPUS_KBPS`] mono (libdragon: `3000 + resample_rate * channels` bps; Opus output is
//! always 48 kHz). Source rates are never changed (22050 stays 22050; 11050 is kept as is, the mixer
//! resamples at playback).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use f2_formats::wus::{Sample, SoundBank};

use crate::tools::{file_key, fnv64, par_map, run};
use crate::{Class, ClassReport, Ctx, PackedFile, Result};

pub mod index;
use index::{BankRec, Codec, EntryRec, SampleRec, SfxIndex};

/// Minimum duration for an Opus attempt on a sound effect.
pub const OPUS_MIN_SECS: f64 = 2.0;
/// Keep Opus only when `opus_bytes <= OPUS_MAX_RATIO * vadpcm_bytes`.
pub const OPUS_MAX_RATIO: f64 = 0.6;
/// Opus bitrate for sound effects (mono).
pub const SFX_OPUS_KBPS: u32 = 32;

/// `--wav-resample` value that yields `kbps` for Opus: libdragon uses `3000 + rate * channels` bps.
/// audioconv64 accepts at most 48000, so stereo tops out at 99 kbps (mono 51 kbps); higher requests clamp.
pub fn opus_resample_for(kbps: u32, channels: u32) -> u32 {
    ((kbps * 1000).saturating_sub(3000) / channels.max(1)).clamp(1, 48000)
}

// ---------------------------------------------------------------------------------------------
// Dedupe
// ---------------------------------------------------------------------------------------------

type DedupeKey = (u64, usize, u32, bool, u16, u16);

/// Cross-bank deduplication of samples: identical PCM + rate + loop flag (+ channels/bits).
#[derive(Default)]
pub struct Dedupe {
    /// Unique samples in first-seen order.
    pub unique: Vec<Sample>,
    /// Number of bank entries referencing each unique sample.
    pub refs: Vec<u32>,
    by_key: HashMap<DedupeKey, Vec<usize>>,
    /// uid -> unique sample ids it maps to over all banks.
    uid_ids: HashMap<u32, Vec<usize>>,
    /// Total PCM bytes over all bank entries (before dedupe).
    pub total_pcm_bytes: u64,
}

impl Dedupe {
    pub fn new() -> Dedupe {
        Dedupe::default()
    }

    /// Adds one sample, returns its unique id.
    pub fn add(&mut self, s: &Sample) -> usize {
        self.total_pcm_bytes += s.pcm.len() as u64;
        let key = (fnv64(&s.pcm), s.pcm.len(), s.sample_rate, s.looped, s.channels, s.bits);
        let slot = self.by_key.entry(key).or_default();
        let id = match slot.iter().copied().find(|&i| self.unique[i].pcm == s.pcm) {
            Some(i) => i,
            None => {
                self.unique.push(s.clone());
                self.refs.push(0);
                slot.push(self.unique.len() - 1);
                self.unique.len() - 1
            }
        };
        self.refs[id] += 1;
        let ids = self.uid_ids.entry(s.uid).or_default();
        if !ids.contains(&id) {
            ids.push(id);
        }
        id
    }

    /// Adds a whole bank; returns `(uid, unique id)` per sample in bank order.
    pub fn add_bank(&mut self, b: &SoundBank) -> Vec<(u32, usize)> {
        b.samples.iter().map(|s| (s.uid, self.add(s))).collect()
    }

    /// uids that map to more than one distinct audio across (or within) banks.
    pub fn conflicting_uids(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.uid_ids.iter().filter(|(_, ids)| ids.len() > 1).map(|(u, _)| *u).collect();
        v.sort_unstable();
        v
    }

    pub fn unique_pcm_bytes(&self) -> u64 {
        self.unique.iter().map(|s| s.pcm.len() as u64).sum()
    }
}

/// Loads every `game/sfx/*.wus` (sorted by file name) and dedupes it.
/// A bank's name and its `(uid, unique sample id)` entries in bank order.
pub type BankEntries = (String, Vec<(u32, usize)>);

pub fn load_banks(ctx: &Ctx) -> Result<(Vec<BankEntries>, Dedupe)> {
    let dir = ctx.game().join("sfx");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "wus"))
        .collect();
    files.sort();
    let mut dd = Dedupe::new();
    let mut banks = Vec::new();
    for f in files {
        let data = std::fs::read(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        let bank = SoundBank::parse(&data).map_err(|e| format!("{}: {e}", f.display()))?;
        let name = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        banks.push((name, dd.add_bank(&bank)));
    }
    Ok((banks, dd))
}

// ---------------------------------------------------------------------------------------------
// Encoding helpers
// ---------------------------------------------------------------------------------------------

/// RIFF/WAVE bytes for 16-bit PCM.
pub fn wav16(rate: u32, channels: u16, pcm: &[i16]) -> Vec<u8> {
    let data_len = pcm.len() as u32 * 2;
    let mut o = Vec::with_capacity(44 + data_len as usize);
    o.extend_from_slice(b"RIFF");
    o.extend_from_slice(&(36 + data_len).to_le_bytes());
    o.extend_from_slice(b"WAVEfmt ");
    o.extend_from_slice(&16u32.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&channels.to_le_bytes());
    o.extend_from_slice(&rate.to_le_bytes());
    o.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    o.extend_from_slice(&(channels * 2).to_le_bytes());
    o.extend_from_slice(&16u16.to_le_bytes());
    o.extend_from_slice(b"data");
    o.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        o.extend_from_slice(&s.to_le_bytes());
    }
    o
}

/// Runs `audioconv64 <flags> -o out_dir input`, returns the produced `.wav64` path.
pub fn audioconv(ctx: &Ctx, input: &Path, out_dir: &Path, flags: &[String]) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let mut c = Command::new(ctx.tool("audioconv64"));
    c.args(flags).arg("-o").arg(out_dir).arg(input);
    run(&mut c)?;
    let stem = input.file_stem().ok_or("audioconv: no stem")?;
    let out = out_dir.join(stem).with_extension("wav64");
    if out.is_file() {
        Ok(out)
    } else {
        Err(format!("audioconv64 did not produce {}", out.display()))
    }
}

fn sample_key(s: &Sample) -> String {
    format!("{:016x}-{}-{}-{}", fnv64(&s.pcm), s.pcm.len(), s.sample_rate, u8::from(s.looped))
}

/// Encodes one unique sample with the given codec; cached by content.
fn encode_sample(ctx: &Ctx, s: &Sample, codec: Codec) -> Result<Vec<u8>> {
    if ctx.portable {
        // In process, the same bytes as `audioconv64 --wav-compress vadpcm,huffman=false`.
        return match codec {
            Codec::Vadpcm => Ok(crate::wav64::vadpcm_wav64(
                &crate::wav64::Pcm { samples: s.pcm_i16(), channels: s.channels, rate: s.sample_rate },
                &crate::wav64::Wav64Opts { looping: s.looped },
            )),
            Codec::Opus => Err("Opus has no pure-Rust encoder (portable pack)".into()),
        };
    }
    let cache = ctx.cache_dir("sfx").map_err(|e| e.to_string())?;
    let tag = match codec {
        Codec::Vadpcm => "vadpcm-nohuff".to_string(),
        Codec::Opus => format!("opus{SFX_OPUS_KBPS}"),
    };
    let key = sample_key(s);
    let cached = cache.join(format!("{key}-{tag}.wav64"));
    if let Ok(b) = std::fs::read(&cached) {
        return Ok(b);
    }
    let tmp = cache.join(format!("tmp-{key}-{tag}"));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let wav_path = tmp.join("in.wav");
    std::fs::write(&wav_path, wav16(s.sample_rate, s.channels, &s.pcm_i16())).map_err(|e| e.to_string())?;
    let mut flags: Vec<String> = Vec::new();
    match codec {
        // No Huffman stage: it decodes on the CPU (~10 % of a frame with a few voices,
        // research/n64-performance.md §2G); plain VADPCM decodes on the RSP.
        Codec::Vadpcm => flags.extend(["--wav-compress".into(), "vadpcm,huffman=false".into()]),
        Codec::Opus => {
            flags.extend(["--wav-compress".into(), "3".into(), "--wav-resample".into(), opus_resample_for(SFX_OPUS_KBPS, u32::from(s.channels)).to_string()])
        }
    }
    if s.looped {
        flags.extend(["--wav-loop".into(), "true".into()]);
    }
    let out = audioconv(ctx, &wav_path, &tmp.join("out"), &flags)?;
    let bytes = std::fs::read(&out).map_err(|e| e.to_string())?;
    let part = cache.join(format!("{key}-{tag}.part{}", std::process::id()));
    std::fs::write(&part, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&part, &cached).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(bytes)
}

fn sample_len(s: &Sample) -> u32 {
    (s.pcm.len() / (usize::from(s.bits / 8).max(1) * usize::from(s.channels).max(1))) as u32
}

fn opus_candidate(s: &Sample) -> bool {
    s.channels == 1 && f64::from(sample_len(s)) / f64::from(s.sample_rate.max(1)) >= OPUS_MIN_SECS
}

/// Encoded result for one unique sample.
struct Encoded {
    codec: Codec,
    bytes: Vec<u8>,
    vadpcm_bytes: usize,
    opus_bytes: Option<usize>,
}

/// Sound-effect codec policy (the budget's fill step can turn Opus off).
#[derive(Clone, Copy, Debug)]
pub struct SfxOpts {
    /// Keep Opus when `opus <= ratio * vadpcm`; 0 = all VADPCM.
    pub opus_max_ratio: f64,
}

impl Default for SfxOpts {
    fn default() -> Self {
        SfxOpts { opus_max_ratio: OPUS_MAX_RATIO }
    }
}

fn encode_best(ctx: &Ctx, s: &Sample, opts: &SfxOpts) -> Result<Encoded> {
    let v = encode_sample(ctx, s, Codec::Vadpcm)?;
    let vadpcm_bytes = v.len();
    if opus_candidate(s) && !(ctx.portable && opts.opus_max_ratio == 0.0) {
        let o = encode_sample(ctx, s, Codec::Opus)?;
        let opus_bytes = Some(o.len());
        if o.len() as f64 <= opts.opus_max_ratio * v.len() as f64 {
            return Ok(Encoded { codec: Codec::Opus, bytes: o, vadpcm_bytes, opus_bytes });
        }
        return Ok(Encoded { codec: Codec::Vadpcm, bytes: v, vadpcm_bytes, opus_bytes });
    }
    Ok(Encoded { codec: Codec::Vadpcm, bytes: v, vadpcm_bytes, opus_bytes: None })
}

// ---------------------------------------------------------------------------------------------
// Sound effects
// ---------------------------------------------------------------------------------------------

/// Deduplicates and encodes every sample of the 19 sound banks; writes the index.
pub fn pack_sfx(ctx: &Ctx) -> Result<ClassReport> {
    pack_sfx_with(ctx, &SfxOpts::default())
}

/// [`pack_sfx`] with an explicit codec policy.
pub fn pack_sfx_with(ctx: &Ctx, opts: &SfxOpts) -> Result<ClassReport> {
    let (banks, dd) = load_banks(ctx)?;
    let ids: Vec<usize> = (0..dd.unique.len()).collect();
    let encoded: Vec<Result<Encoded>> = par_map(&ids, ctx.jobs, |&i| encode_best(ctx, &dd.unique[i], opts));
    let mut enc = Vec::with_capacity(encoded.len());
    for (i, e) in encoded.into_iter().enumerate() {
        enc.push(e.map_err(|e| format!("sfx sample {i}: {e}"))?);
    }

    let mut idx = SfxIndex::default();
    let mut rep = ClassReport::default();
    for (name, ents) in &banks {
        idx.banks.push(BankRec { name: name.clone(), first: idx.entries.len() as u32, count: ents.len() as u32 });
        idx.entries.extend(ents.iter().map(|&(uid, id)| EntryRec { uid, sample: id as u16 }));
    }
    let mut per_codec: HashMap<&'static str, (usize, u64)> = HashMap::new();
    let (mut vad_all, mut opus_cmp_v, mut opus_cmp_o) = (0u64, 0u64, 0u64);
    for (i, (s, e)) in dd.unique.iter().zip(&enc).enumerate() {
        idx.samples.push(SampleRec {
            src_rate: s.sample_rate,
            src_len: sample_len(s),
            file_bytes: e.bytes.len() as u32,
            codec: e.codec,
            looped: s.looped,
            channels: s.channels as u8,
        });
        let path = format!("sfx/{i:04}.wav64");
        std::fs::write(ctx.out_path(&path).map_err(|e| e.to_string())?, &e.bytes).map_err(|e| e.to_string())?;
        let c = per_codec.entry(e.codec.name()).or_default();
        c.0 += 1;
        c.1 += e.bytes.len() as u64;
        vad_all += e.vadpcm_bytes as u64;
        if let Some(o) = e.opus_bytes {
            opus_cmp_v += e.vadpcm_bytes as u64;
            opus_cmp_o += o as u64;
        }
        let secs = f64::from(sample_len(s)) / f64::from(s.sample_rate.max(1));
        rep.files.push(PackedFile {
            path,
            class: Class::Sfx,
            bytes: e.bytes.len() as u64,
            source_bytes: s.pcm.len() as u64,
            format: format!("{} {}Hz{}", e.codec.name(), s.sample_rate, if s.looped { " loop" } else { "" }),
            notes: vec![format!("{secs:.2}s, {} refs", dd.refs[i])],
        });
    }
    let index_bytes = idx.write();
    std::fs::write(ctx.out_path("sfx/index.bin").map_err(|e| e.to_string())?, &index_bytes).map_err(|e| e.to_string())?;
    rep.files.push(PackedFile {
        path: "sfx/index.bin".into(),
        class: Class::Sfx,
        bytes: index_bytes.len() as u64,
        source_bytes: 0,
        format: "F2SX index v1".into(),
        notes: vec![],
    });

    let total: u64 = enc.iter().map(|e| e.bytes.len() as u64).sum();
    let nentries: usize = banks.iter().map(|b| b.1.len()).sum();
    rep.notes.push(format!(
        "sfx: {} banks, {} bank entries -> {} unique samples (PCM {} -> {} bytes, dedupe saves {} bytes = {:.1}%)",
        banks.len(),
        nentries,
        dd.unique.len(),
        dd.total_pcm_bytes,
        dd.unique_pcm_bytes(),
        dd.total_pcm_bytes - dd.unique_pcm_bytes(),
        100.0 * (dd.total_pcm_bytes - dd.unique_pcm_bytes()) as f64 / dd.total_pcm_bytes.max(1) as f64
    ));
    let conflicts = dd.conflicting_uids();
    rep.notes.push(format!(
        "sfx: {} uid(s) map to different audio in different banks (kept per bank in the index){}",
        conflicts.len(),
        if conflicts.is_empty() { String::new() } else { format!(": {}", conflicts.iter().take(8).map(|u| format!("{u:08x}")).collect::<Vec<_>>().join(" ")) }
    ));
    let mut codecs: Vec<_> = per_codec.into_iter().collect();
    codecs.sort();
    for (n, (cnt, b)) in codecs {
        rep.notes.push(format!("sfx codec {n}: {cnt} samples, {b} bytes"));
    }
    rep.notes.push(format!("sfx: total {total} bytes (+{} index) vs all-VADPCM {vad_all}; Opus candidates (>= {OPUS_MIN_SECS}s mono): VADPCM {opus_cmp_v} vs Opus {opus_cmp_o} bytes; rule: Opus if <= {} x VADPCM, {SFX_OPUS_KBPS} kbps", index_bytes.len(), opts.opus_max_ratio));
    let loops = dd.unique.iter().filter(|s| s.looped).count();
    let opus_loops = dd.unique.iter().zip(&enc).filter(|(s, e)| s.looped && e.codec == Codec::Opus).count();
    rep.notes.push(format!("sfx: {loops} looped samples (whole-sample loops), {opus_loops} of them Opus"));
    Ok(rep)
}

// ---------------------------------------------------------------------------------------------
// Music
// ---------------------------------------------------------------------------------------------

/// `(rate, channels, bits, frames)` of a PCM WAV file.
pub fn wav_info(p: &Path) -> Result<(u32, u16, u16, u64)> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut hdr = [0u8; 12];
    f.read_exact(&mut hdr).map_err(|e| e.to_string())?;
    if &hdr[..4] != b"RIFF" || &hdr[8..12] != b"WAVE" {
        return Err(format!("{}: not a RIFF/WAVE", p.display()));
    }
    let (mut rate, mut ch, mut bits) = (0u32, 0u16, 0u16);
    loop {
        let mut ck = [0u8; 8];
        f.read_exact(&mut ck).map_err(|_| format!("{}: no data chunk", p.display()))?;
        let len = u32::from_le_bytes([ck[4], ck[5], ck[6], ck[7]]) as u64;
        if &ck[..4] == b"fmt " {
            let mut b = vec![0u8; len as usize];
            f.read_exact(&mut b).map_err(|e| e.to_string())?;
            ch = u16::from_le_bytes([b[2], b[3]]);
            rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
            bits = u16::from_le_bytes([b[14], b[15]]);
        } else if &ck[..4] == b"data" {
            let frame = u64::from(ch) * u64::from(bits / 8).max(1);
            return Ok((rate, ch, bits, len / frame.max(1)));
        } else {
            std::io::Seek::seek(&mut f, std::io::SeekFrom::Current(len as i64 + (len & 1) as i64)).map_err(|e| e.to_string())?;
        }
    }
}

/// The music track files `music/track02..12.wav` that exist.
pub fn music_tracks(ctx: &Ctx) -> Vec<(u32, PathBuf)> {
    (2..=12).map(|n| (n, ctx.rendition.join("music").join(format!("track{n:02}.wav")))).filter(|(_, p)| p.is_file()).collect()
}

/// Total music duration in seconds.
pub fn music_seconds(ctx: &Ctx) -> f64 {
    music_tracks(ctx).iter().filter_map(|(_, p)| wav_info(p).ok()).map(|(r, _, _, n)| n as f64 / f64::from(r.max(1))).sum()
}

/// Opus VBR overshoots the nominal bitrate at low rates and undershoots at high ones. Measured
/// actual/nominal size ratios for the 11 retail tracks (kbps, ratio); linearly interpolated.
const MUSIC_VBR_RATIO: [(f64, f64); 5] = [(32.0, 1.0246), (48.0, 1.0271), (64.0, 1.0084), (96.0, 0.9809), (99.0, 0.983)];

/// Predicted total size of the music at `kbps`: `seconds * kbps * 125` times the measured VBR ratio
/// ([`MUSIC_VBR_RATIO`]) plus a small per-file header ([`MUSIC_FILE_OVERHEAD`] bytes). [`pack_music`]
/// reports predicted vs actual. Requests above 99 kbps clamp to 99 (audioconv64's `--wav-resample` cap).
pub fn predict_music_bytes(ctx: &Ctx, kbps: u32) -> u64 {
    let n = music_tracks(ctx).len() as u64;
    let k = f64::from(kbps).clamp(MUSIC_VBR_RATIO[0].0, 99.0);
    let ratio = MUSIC_VBR_RATIO.windows(2).find(|w| k <= w[1].0).map_or(1.0, |w| w[0].1 + (w[1].1 - w[0].1) * (k - w[0].0) / (w[1].0 - w[0].0));
    (music_seconds(ctx) * k * 125.0 * ratio) as u64 + n * MUSIC_FILE_OVERHEAD
}

/// Approximate wav64 header + Opus decoder-state bytes per file.
pub const MUSIC_FILE_OVERHEAD: u64 = 64;
/// Music sample rate: the N64 mixer's output rate (`audio_init(32000, ..)` in main.c). The
/// 44.1 kHz sources would lose everything above 16 kHz at playback anyway; decoding ULC at
/// 32 kHz costs the VR4300 ~27 % less (no samples the mixer only throws away) and spends the
/// bitrate on audible content.
pub const MUSIC_RATE: u32 = 32000;

/// Encodes the music tracks as ULC stereo at `kbps` (average bitrate mode), looping flag set
/// (the runtime clears it for tracks the game plays once, `PrepareSong(.., FALSE)`).
pub fn pack_music(ctx: &Ctx, kbps: u32) -> Result<ClassReport> {
    let tracks = music_tracks(ctx);
    if tracks.is_empty() {
        return Err(format!("no music tracks under {}", ctx.rendition.join("music").display()));
    }
    let cache = ctx.cache_dir("music").map_err(|e| e.to_string())?;
    let results: Vec<Result<Vec<u8>>> = par_map(&tracks, ctx.jobs, |(n, p)| {
        let key = file_key(p, &format!("ulc-abr{kbps}-{MUSIC_RATE}hz-stereo-loop"));
        let cached = cache.join(format!("track{n:02}-{key}.wav64"));
        if let Ok(b) = std::fs::read(&cached) {
            return Ok(b);
        }
        let tmp = cache.join(format!("tmp-{n:02}-{key}"));
        let codec = format!("ulc,mode=abr,bitrate={kbps}");
        let rate = MUSIC_RATE.to_string();
        let flags: Vec<String> = ["--wav-compress", &codec, "--wav-resample", &rate, "--wav-loop", "true"].iter().map(|s| s.to_string()).collect();
        let out = audioconv(ctx, p, &tmp, &flags)?;
        let b = std::fs::read(&out).map_err(|e| e.to_string())?;
        std::fs::write(&cached, &b).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(b)
    });
    let mut rep = ClassReport::default();
    let mut total = 0u64;
    for ((n, p), r) in tracks.iter().zip(results) {
        let bytes = r.map_err(|e| format!("track{n:02}: {e}"))?;
        let rel = format!("music/track{n:02}.wav64");
        std::fs::write(ctx.out_path(&rel).map_err(|e| e.to_string())?, &bytes).map_err(|e| e.to_string())?;
        total += bytes.len() as u64;
        let (rate, ch, _, frames) = wav_info(p)?;
        rep.files.push(PackedFile {
            path: rel,
            class: Class::Music,
            bytes: bytes.len() as u64,
            source_bytes: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
            format: format!("ulc abr {kbps}kbps {MUSIC_RATE} Hz stereo loop"),
            notes: vec![format!("{:.1}s {rate}Hz x{ch}", frames as f64 / f64::from(rate.max(1)))],
        });
    }
    rep.notes.push(format!("music: {} tracks, {:.1}s, ULC stereo ABR {kbps} kbps at {MUSIC_RATE} Hz: {total} bytes", tracks.len(), music_seconds(ctx)));
    Ok(rep)
}

// ---------------------------------------------------------------------------------------------
// Portable music (Ctx::portable): VADPCM
// ---------------------------------------------------------------------------------------------

/// Sample rates tried for the portable (VADPCM) music, best first: the highest whose files
/// fit the space left is used. ULC (libdragon's transform codec) has no Rust encoder; VADPCM
/// costs ~2.3x the bytes of ULC at 128 kbps for the same rate but decodes on the RSP.
pub const MUSIC_VADPCM_RATES: [u32; 8] = [32000, 28000, 24000, 22050, 20000, 18000, 16000, 11025];

/// Reads a PCM WAV (8 or 16 bits) as interleaved 16-bit samples (8-bit expanded like
/// dr_wav's `s16` reader): `(samples, channels, rate)`.
pub fn read_wav16(p: &Path) -> Result<(Vec<i16>, u16, u32)> {
    let b = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(format!("{}: not a RIFF/WAVE", p.display()));
    }
    let (mut fmt, mut ch, mut rate, mut bits) = (0u16, 0u16, 0u32, 0u16);
    let mut at = 12usize;
    while at + 8 <= b.len() {
        let id = &b[at..at + 4];
        let len = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &b[at + 8..(at + 8 + len).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            fmt = u16::from_le_bytes([body[0], body[1]]);
            ch = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            if fmt != 1 || ch == 0 || !(bits == 8 || bits == 16) {
                return Err(format!("{}: unsupported WAV (format {fmt}, {ch} channels, {bits} bits): 8/16-bit PCM only", p.display()));
            }
            let frame = usize::from(ch) * usize::from(bits / 8);
            let body = &body[..body.len() / frame * frame];
            let s: Vec<i16> = if bits == 16 {
                body.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect()
            } else {
                body.iter().map(|&v| (i16::from(v) - 128) << 8).collect()
            };
            return Ok((s, ch, rate));
        }
        at += 8 + len + (len & 1);
    }
    Err(format!("{}: no data chunk", p.display()))
}

/// Size of a VADPCM wav64 (looped, no Huffman) of `frames` frames x `channels`.
pub fn vadpcm_wav64_bytes(frames: u64, channels: u64) -> u64 {
    // wav64 header 32 + VADPCM header 20 + Huffman context 72 + 4, codebooks 136 per channel,
    // one skip point (the loop) 8 + its state 16 per channel, then 9-byte frames of 16 samples
    // (the length padded to 32 samples).
    let data = frames.div_ceil(32) * 32 / 16 * 9 * channels;
    128 + 136 * channels + 8 + 16 * channels + data
}

/// The rate the portable music uses for a track recorded at `src_rate`: `rate`, never above
/// the source rate.
fn music_rate(src_rate: u32, rate: u32) -> u32 {
    rate.min(src_rate)
}

/// Predicted total size of the portable music at `rate` (exact up to the WAV parsing).
pub fn predict_music_vadpcm_bytes(ctx: &Ctx, rate: u32) -> u64 {
    music_tracks(ctx)
        .iter()
        .filter_map(|(_, p)| wav_info(p).ok())
        .map(|(r, ch, _, n)| vadpcm_wav64_bytes(crate::resample::output_len(n as usize, r, music_rate(r, rate)) as u64, u64::from(ch)))
        .sum()
}

/// Encodes the music tracks as VADPCM at `rate` (resampled with [`crate::resample`]), looping
/// flag set (the runtime clears it for tracks played once), in process.
pub fn pack_music_vadpcm(ctx: &Ctx, rate: u32) -> Result<ClassReport> {
    let tracks = music_tracks(ctx);
    if tracks.is_empty() {
        return Err(format!("no music tracks under {}", ctx.rendition.join("music").display()));
    }
    type Track = (Vec<u8>, u32, u16, u32);
    let results: Vec<Result<Track>> = par_map(&tracks, ctx.jobs.min(4), |(_, p)| {
        let (pcm, ch, src) = read_wav16(p)?;
        let to = music_rate(src, rate);
        let pcm = crate::resample::resample(&pcm, usize::from(ch), src, to);
        let w = crate::wav64::vadpcm_wav64(&crate::wav64::Pcm { samples: pcm, channels: ch, rate: to }, &crate::wav64::Wav64Opts { looping: true });
        Ok((w, src, ch, to))
    });
    let mut rep = ClassReport::default();
    let mut total = 0u64;
    for ((n, p), r) in tracks.iter().zip(results) {
        let (bytes, src, ch, to) = r.map_err(|e| format!("track{n:02}: {e}"))?;
        let rel = format!("music/track{n:02}.wav64");
        std::fs::write(ctx.out_path(&rel).map_err(|e| e.to_string())?, &bytes).map_err(|e| e.to_string())?;
        total += bytes.len() as u64;
        rep.files.push(PackedFile {
            path: rel,
            class: Class::Music,
            bytes: bytes.len() as u64,
            source_bytes: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
            format: format!("vadpcm {to} Hz x{ch} loop"),
            notes: vec![format!("from {src} Hz")],
        });
    }
    rep.notes.push(format!("music: {} tracks, {:.1}s, VADPCM at {rate} Hz (portable pack): {total} bytes", tracks.len(), music_seconds(ctx)));
    Ok(rep)
}

/// Encodes the music tracks at (about) `kbps` per stereo track, Opus, looping flag set (the runtime
/// clears it with `wav64_set_loop(w, false)` for tracks the game plays once, `PrepareSong(.., FALSE)`).
pub fn pack_music_opus(ctx: &Ctx, kbps: u32) -> Result<ClassReport> {
    let tracks = music_tracks(ctx);
    if tracks.is_empty() {
        return Err(format!("no music tracks under {}", ctx.rendition.join("music").display()));
    }
    let cache = ctx.cache_dir("music").map_err(|e| e.to_string())?;
    let resample = opus_resample_for(kbps, 2);
    let results: Vec<Result<Vec<u8>>> = par_map(&tracks, ctx.jobs, |(n, p)| {
        let key = file_key(p, &format!("opus-stereo-loop-r{resample}"));
        let cached = cache.join(format!("track{n:02}-{key}.wav64"));
        if let Ok(b) = std::fs::read(&cached) {
            return Ok(b);
        }
        let tmp = cache.join(format!("tmp-{n:02}-{key}"));
        let flags: Vec<String> = ["--wav-compress", "3", "--wav-resample", &resample.to_string(), "--wav-loop", "true"].iter().map(|s| s.to_string()).collect();
        let out = audioconv(ctx, p, &tmp, &flags)?;
        let b = std::fs::read(&out).map_err(|e| e.to_string())?;
        std::fs::write(&cached, &b).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(b)
    });
    let mut rep = ClassReport::default();
    let mut total = 0u64;
    for ((n, p), r) in tracks.iter().zip(results) {
        let bytes = r.map_err(|e| format!("track{n:02}: {e}"))?;
        let rel = format!("music/track{n:02}.wav64");
        std::fs::write(ctx.out_path(&rel).map_err(|e| e.to_string())?, &bytes).map_err(|e| e.to_string())?;
        total += bytes.len() as u64;
        let (rate, ch, _, frames) = wav_info(p)?;
        rep.files.push(PackedFile {
            path: rel,
            class: Class::Music,
            bytes: bytes.len() as u64,
            source_bytes: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
            format: format!("opus {kbps}kbps stereo loop"),
            notes: vec![format!("{:.1}s {rate}Hz x{ch}", frames as f64 / f64::from(rate.max(1)))],
        });
    }
    let secs = music_seconds(ctx);
    let pred = predict_music_bytes(ctx, kbps);
    rep.notes.push(format!(
        "music: {} tracks, {secs:.1}s, Opus stereo {kbps} kbps (--wav-resample {resample}): actual {total} bytes, predicted {pred} bytes ({:+.2}%)",
        tracks.len(),
        100.0 * (total as f64 - pred as f64) / pred.max(1) as f64
    ));
    let table: Vec<String> = [32u32, 48, 64, 96].iter().map(|&k| format!("{k} kbps = {:.2} MB", predict_music_bytes(ctx, k) as f64 / 1e6)).collect();
    rep.notes.push(format!("music size by bitrate (predicted, validated by real encodes): {}; 99 kbps = {:.2} MB is the maximum (audioconv64 caps --wav-resample at 48000, so 128 kbps stereo Opus is not available)", table.join(", "), predict_music_bytes(ctx, 99) as f64 / 1e6));
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smp(uid: u32, pcm: Vec<u8>, rate: u32, looped: bool) -> Sample {
        Sample { uid, looped, format_tag: 1, channels: 1, sample_rate: rate, bits: 16, pcm }
    }

    #[test]
    fn dedupe_identical_across_banks() {
        let mut d = Dedupe::new();
        let a = SoundBank { samples: vec![smp(1, vec![1, 2, 3, 4], 22050, false), smp(2, vec![9; 8], 22050, true)] };
        let b = SoundBank { samples: vec![smp(1, vec![1, 2, 3, 4], 22050, false), smp(3, vec![9; 8], 22050, false), smp(4, vec![1, 2, 3, 4], 11025, false)] };
        let ia = d.add_bank(&a);
        let ib = d.add_bank(&b);
        assert_eq!(ia[0].1, ib[0].1); // same audio, same rate, same loop
        assert_ne!(ia[1].1, ib[1].1); // same PCM but different loop flag
        assert_ne!(ib[0].1, ib[2].1); // same PCM, different rate
        assert_eq!(d.unique.len(), 4);
        assert_eq!(d.total_pcm_bytes, 4 + 8 + 4 + 8 + 4);
        assert_eq!(d.unique_pcm_bytes(), 4 + 8 + 8 + 4);
        assert_eq!(d.refs[ia[0].1], 2);
        assert!(d.conflicting_uids().is_empty());
    }

    #[test]
    fn dedupe_reports_uid_conflicts() {
        let mut d = Dedupe::new();
        d.add_bank(&SoundBank { samples: vec![smp(5, vec![1, 1], 22050, false)] });
        d.add_bank(&SoundBank { samples: vec![smp(5, vec![2, 2], 22050, false), smp(6, vec![1, 1], 22050, false)] });
        assert_eq!(d.conflicting_uids(), vec![5]);
    }

    #[test]
    fn opus_resample_math() {
        assert_eq!(opus_resample_for(64, 2), 30500);
        assert_eq!(opus_resample_for(32, 1), 29000);
        assert_eq!(3000 + opus_resample_for(48, 2) * 2, 48000);
    }

    #[test]
    fn wav16_header_parses() {
        let w = wav16(22050, 1, &[1, -2, 3]);
        let p = std::env::temp_dir().join(format!("f2n64pack-wav16-{}.wav", std::process::id()));
        std::fs::write(&p, &w).unwrap();
        assert_eq!(wav_info(&p).unwrap(), (22050, 1, 16, 3));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn codec_rule_candidates() {
        let long = smp(1, vec![0; 22050 * 2 * 3], 22050, false);
        assert!(opus_candidate(&long));
        assert!(opus_candidate(&smp(1, vec![0; 22050 * 2 * 3], 22050, true)));
        assert!(!opus_candidate(&smp(1, vec![0; 22050 * 2], 22050, false)));
    }

    /// Real data: parse all banks and report dedupe stats (no encoding). Skipped without data.
    #[test]
    fn real_banks_dedupe() {
        let Some(rend) = crate::ctx::find_rendition(Path::new(env!("CARGO_MANIFEST_DIR"))) else { return };
        let ctx =
            Ctx { rendition: rend, out: PathBuf::new(), cache: PathBuf::new(), tools: crate::ctx::default_tools(), jobs: 1, quick: true, portable: false };
        let (banks, dd) = load_banks(&ctx).unwrap();
        assert_eq!(banks.len(), 19);
        assert!(dd.unique.len() < banks.iter().map(|b| b.1.len()).sum::<usize>());
        assert!(dd.unique_pcm_bytes() < dd.total_pcm_bytes);
        assert!(dd.unique.len() < 65536);
        eprintln!(
            "banks {} unique {} pcm {} -> {} conflicts {}",
            banks.len(),
            dd.unique.len(),
            dd.total_pcm_bytes,
            dd.unique_pcm_bytes(),
            dd.conflicting_uids().len()
        );
    }
}
