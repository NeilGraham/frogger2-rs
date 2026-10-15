//! Byte-for-byte parity of the pure-Rust VADPCM wav64 writer against libdragon's `audioconv64`.
//!
//! `cargo run --release -p f2-n64pack --example vadpcm_parity -- [synthetic] [sfx DIR] [music WAV...]`
//!
//! * `synthetic`: the synthetic corpus of the unit test.
//! * `sfx DIR`: every unique sample of `DIR/*.wus` (looped samples with `--wav-loop true`).
//! * `music WAV`: a 16-bit PCM WAV (re-written without extra chunks), looped.
//!
//! Scratch files go to `$TMPDIR` (or /tmp) and are deleted. Dev tool; reads game data read-only.
use f2_n64pack::wav64::{parity, vadpcm_wav64, Pcm, Wav64Opts};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vadpcm-parity-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Minimal 16-bit PCM WAV reader (fmt + data chunks).
fn read_wav16(p: &Path) -> Pcm {
    let b = std::fs::read(p).unwrap();
    assert_eq!(&b[..4], b"RIFF");
    let mut o = 12;
    let (mut ch, mut rate, mut bits, mut data) = (0u16, 0u32, 0u16, None);
    while o + 8 <= b.len() {
        let id = &b[o..o + 4];
        let len = u32::from_le_bytes(b[o + 4..o + 8].try_into().unwrap()) as usize;
        let body = &b[o + 8..(o + 8 + len).min(b.len())];
        if id == b"fmt " {
            ch = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            data = Some(body.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect::<Vec<i16>>());
        }
        o += 8 + len + (len & 1);
    }
    assert_eq!(bits, 16, "{}: not 16-bit", p.display());
    Pcm { samples: data.expect("data chunk"), channels: ch, rate }
}

fn main() {
    let tool = parity::find_tool().expect("audioconv64 not found ($N64_INST/bin or ~/opt/n64root-3a)");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "synthetic" => {
                let t = Instant::now();
                let (ok, total) = parity::run_synthetic(&tool);
                println!("synthetic: {ok}/{total} byte-identical ({:.1}s)", t.elapsed().as_secs_f64());
            }
            "sfx" => {
                i += 1;
                sfx(&tool, Path::new(&args[i]));
            }
            "music" => {
                i += 1;
                music(&tool, Path::new(&args[i]));
            }
            a => panic!("unknown argument {a}"),
        }
        i += 1;
    }
}

fn sfx(tool: &Path, dir: &Path) {
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "wus")).collect();
    files.sort();
    let mut seen = HashSet::new();
    let mut work: Vec<(Pcm, bool)> = Vec::new();
    let mut entries = 0;
    for f in &files {
        let bank = f2_formats::wus::SoundBank::parse(&std::fs::read(f).unwrap()).unwrap();
        for s in bank.samples {
            entries += 1;
            if seen.insert((s.pcm.clone(), s.channels, s.sample_rate, s.bits, s.looped)) {
                work.push((Pcm { samples: s.pcm_i16(), channels: s.channels, rate: s.sample_rate }, s.looped));
            }
        }
    }
    let ok = AtomicUsize::new(0);
    let next = AtomicUsize::new(0);
    let port_ns = AtomicUsize::new(0);
    let tool_ns = AtomicUsize::new(0);
    let t = Instant::now();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|sc| {
        for w in 0..threads {
            let (work, ok, next, port_ns, tool_ns) = (&work, &ok, &next, &port_ns, &tool_ns);
            sc.spawn(move || {
                let d = scratch(&format!("sfx{w}"));
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some((pcm, looped)) = work.get(k) else { break };
                    let opts = Wav64Opts { looping: *looped };
                    let t0 = Instant::now();
                    let want = parity::tool_wav64(tool, &d, &format!("s{k}"), pcm, &opts).unwrap_or_default();
                    let t1 = Instant::now();
                    let got = vadpcm_wav64(pcm, &opts);
                    let t2 = Instant::now();
                    tool_ns.fetch_add((t1 - t0).as_nanos() as usize, Ordering::Relaxed);
                    port_ns.fetch_add((t2 - t1).as_nanos() as usize, Ordering::Relaxed);
                    if want == got {
                        ok.fetch_add(1, Ordering::Relaxed);
                    } else {
                        let first = want.iter().zip(&got).position(|(a, b)| a != b).unwrap_or(want.len().min(got.len()));
                        eprintln!(
                            "MISMATCH sample {k} ({} ch, {} Hz, {} frames, loop {looped}): tool {} port {} bytes, first diff {first}",
                            pcm.channels,
                            pcm.rate,
                            pcm.samples.len() / usize::from(pcm.channels),
                            want.len(),
                            got.len()
                        );
                    }
                }
                let _ = std::fs::remove_dir_all(&d);
            });
        }
    });
    println!(
        "sfx: {}/{} unique samples byte-identical ({entries} bank entries, {} banks); wall {:.1}s, cpu: tool {:.1}s, port {:.1}s",
        ok.load(Ordering::Relaxed),
        work.len(),
        files.len(),
        t.elapsed().as_secs_f64(),
        Duration::from_nanos(tool_ns.load(Ordering::Relaxed) as u64).as_secs_f64(),
        Duration::from_nanos(port_ns.load(Ordering::Relaxed) as u64).as_secs_f64(),
    );
}

fn music(tool: &Path, wav: &Path) {
    let pcm = read_wav16(wav);
    let d = scratch("music");
    let opts = Wav64Opts { looping: true };
    let t0 = Instant::now();
    let want = parity::tool_wav64(tool, &d, "music", &pcm, &opts).unwrap_or_default();
    let t1 = Instant::now();
    let got = vadpcm_wav64(&pcm, &opts);
    let t2 = Instant::now();
    let _ = std::fs::remove_dir_all(&d);
    let same = want == got;
    println!(
        "music {}: {} ({} ch, {} Hz, {} frames, {} bytes); tool {:.1}s, port {:.1}s",
        wav.file_name().unwrap().to_string_lossy(),
        if same { "byte-identical" } else { "MISMATCH" },
        pcm.channels,
        pcm.rate,
        pcm.samples.len() / usize::from(pcm.channels),
        want.len(),
        (t1 - t0).as_secs_f64(),
        (t2 - t1).as_secs_f64()
    );
    if !same {
        let first = want.iter().zip(&got).position(|(a, b)| a != b).unwrap_or(want.len().min(got.len()));
        println!("  tool {} port {} bytes, first diff {first}", want.len(), got.len());
    }
}
