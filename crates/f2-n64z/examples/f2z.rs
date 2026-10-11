//! `f2z bench <level> <files...>`: compresses each file (no dictionary, no transform),
//! verifies the round trip and prints totals and decode speed.
//! `f2z c <in> <out>` / `f2z d <in> <out>`: single file (no dictionary).

use f2_n64z::enc::{compress, Options};
use f2_n64z::{header, Decoder};
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("c") => {
            let d = std::fs::read(&a[1]).unwrap();
            std::fs::write(&a[2], compress(&d, &[], None, &Options::default())).unwrap();
        }
        Some("d") => {
            let z = std::fs::read(&a[1]).unwrap();
            let h = header(&z).unwrap();
            let mut out = vec![0u8; h.raw_size as usize];
            let mut tmp = vec![0u8; h.raw_size as usize];
            let mut dec = Box::new(Decoder::new());
            dec.decode(&z, None, &mut out, &mut tmp).unwrap();
            std::fs::write(&a[2], out).unwrap();
        }
        Some("bench") => {
            let lvl: u32 = a[1].parse().unwrap();
            let opt = Options::level(lvl);
            let (mut raw, mut comp) = (0usize, 0usize);
            let mut dec_t = 0f64;
            let t0 = Instant::now();
            let mut dec = Box::new(Decoder::new());
            let mut files = Vec::new();
            for f in &a[2..] {
                let p = std::path::Path::new(f);
                if p.is_dir() {
                    let mut v: Vec<_> = std::fs::read_dir(p).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_file()).collect();
                    v.sort();
                    files.extend(v.into_iter().map(|p| p.display().to_string()));
                } else {
                    files.push(f.clone());
                }
            }
            for f in &files {
                let d = std::fs::read(f).unwrap();
                let z = compress(&d, &[], None, &opt);
                let mut out = vec![0u8; d.len()];
                let t = Instant::now();
                for _ in 0..5 {
                    dec.decode(&z, None, &mut out, &mut []).unwrap();
                }
                dec_t += t.elapsed().as_secs_f64() / 5.0;
                assert_eq!(out, d, "{f}");
                raw += d.len();
                comp += z.len();
            }
            println!(
                "files {} raw {raw} f2z {comp} ratio {:.3} enc {:.1}s dec {:.1} MB/s",
                files.len(),
                comp as f64 / raw as f64,
                t0.elapsed().as_secs_f64(),
                raw as f64 / dec_t / 1e6
            );
        }
        _ => eprintln!("usage: f2z bench <level> <files...> | c <in> <out> | d <in> <out>"),
    }
}
