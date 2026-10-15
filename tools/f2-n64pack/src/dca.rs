//! libdragon compressed-asset container (`DCA5`) as written by `mkasset -c 2` (aPLib).
//!
//! Mirrors `tools/common/assetcomp.c` `asset_compress_mem` (case 2), `src/asset.c`
//! `asset_winsize_to_flags` and `tools/common/binout.c` (`wleb128u`, `walign`) of libdragon, so
//! the output is byte-identical to `mkasset -c 2 -w <KiB> -o OUTDIR in`.
//!
//! Layout: `"DCA5"`, flags byte (`winsize flag | algo << 4`), LEB128 `cmp_size`, LEB128
//! `dec_size`, LEB128 in-place margin, zero padding to a 2-byte boundary, aPLib payload.
//!
//! Note: `mkasset` reads its input with `asset_load`, so an input that is itself a `DCA` asset
//! would be decompressed first. Callers here always pass raw (uncompressed) data.

use crate::apultra;

/// `ASSET_FLAG_ALGO_SHIFT`
const ASSET_FLAG_ALGO_SHIFT: u8 = 4;

/// `asset_winsize_to_flags` — `None` for an unsupported window size.
pub fn winsize_to_flags(winsize: usize) -> Option<u8> {
    Some(match winsize {
        0x4000 => 0x00,  // 16K
        0x2000 => 0x01,  // 8K
        0x1000 => 0x02,  // 4K
        0x800 => 0x03,   // 2K
        0x8000 => 0x04,  // 32K
        0x10000 => 0x05, // 64K
        0x20000 => 0x06, // 128K
        0x40000 => 0x07, // 256K
        _ => return None,
    })
}

/// `wleb128u`
fn wleb128u(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v & 0x7f) as u8 | 0x80);
        v >>= 7;
    }
    out.push((v & 0x7f) as u8);
}

/// Bytes of the file `mkasset -c 2 -w <winsize/1024> -o OUT in` writes for `data`
/// (`winsize` e.g. `256 * 1024`; 0 selects mkasset's aPLib default of 256 KiB).
///
/// # Panics
/// If `winsize` is not one of the sizes libdragon supports (2..256 KiB, powers of two).
pub fn mkasset_aplib(data: &[u8], winsize: usize) -> Vec<u8> {
    assert!(winsize == 0 || winsize_to_flags(winsize).is_some(), "unsupported window size: {winsize}");
    let sz = data.len();
    let mut winsize = winsize;

    // asset_compress_mem: silently shrink the window for small files.
    if winsize != 0 {
        while sz < winsize && winsize > 2 * 1024 {
            winsize /= 2;
        }
    }
    // case 2 (aplib): default window.
    if winsize == 0 {
        winsize = 256 * 1024;
        while sz < winsize && winsize > 2 * 1024 {
            winsize /= 2;
        }
    }

    let (payload, stats) = apultra::compress(data, winsize);
    let cmp_size = payload.len() as i64;
    // FIXME (upstream): in rare cases, apultra returns a negative margin; clamp to zero.
    let inplace_margin = (stats.safe_dist as i64 + cmp_size - sz as i64).max(0);

    let mut out = Vec::with_capacity(payload.len() + 16);
    out.extend_from_slice(b"DCA5");
    out.push(winsize_to_flags(winsize).unwrap() | (2 << ASSET_FLAG_ALGO_SHIFT));
    wleb128u(&mut out, cmp_size as u64);
    wleb128u(&mut out, sz as u64);
    wleb128u(&mut out, inplace_margin as u64);
    // walign(out, 2)
    while out.len() % 2 != 0 {
        out.push(0);
    }
    out.extend_from_slice(&payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn read_leb(b: &[u8], p: &mut usize) -> u64 {
        let mut v = 0u64;
        let mut sh = 0;
        loop {
            let x = b[*p];
            *p += 1;
            v |= ((x & 0x7f) as u64) << sh;
            sh += 7;
            if x & 0x80 == 0 {
                return v;
            }
        }
    }

    /// Parses a DCA5 aPLib asset and decodes it.
    fn unpack(asset: &[u8]) -> Vec<u8> {
        assert_eq!(&asset[..4], b"DCA5");
        assert_eq!(asset[4] >> 4, 2);
        let mut p = 5;
        let cmp = read_leb(asset, &mut p) as usize;
        let dec = read_leb(asset, &mut p) as usize;
        let _margin = read_leb(asset, &mut p);
        p += p % 2;
        assert_eq!(asset.len(), p + cmp);
        let d = apultra::decompress(&asset[p..]).unwrap();
        assert_eq!(d.len(), dec);
        d
    }

    #[test]
    fn dca_header_and_roundtrip() {
        let a = mkasset_aplib(&[], 256 * 1024);
        assert_eq!(a, b"DCA5\x23\x00\x00\x00");
        let a = mkasset_aplib(b"a", 256 * 1024);
        assert_eq!(a, b"DCA5\x23\x03\x01\x02\x61\xc0\x00");
        for n in [1usize, 100, 2047, 2048, 2049, 5000, 70000, 300_000] {
            let data: Vec<u8> = (0..n).map(|i| ((i * 7) ^ (i >> 5)) as u8).collect();
            let a = mkasset_aplib(&data, 256 * 1024);
            assert_eq!(unpack(&a), data);
        }
        // window shrinking: 5000 bytes -> 4K window (flag 2)
        assert_eq!(mkasset_aplib(&[1u8; 5000], 256 * 1024)[4], 0x22);
    }

    fn mkasset_path() -> Option<PathBuf> {
        let mut cands = Vec::new();
        if let Ok(p) = std::env::var("N64_INST") {
            cands.push(Path::new(&p).join("bin/mkasset"));
        }
        if let Ok(h) = std::env::var("HOME") {
            cands.push(Path::new(&h).join("opt/n64root-3a/opt/libdragon/bin/mkasset"));
        }
        cands.into_iter().find(|p| p.is_file())
    }

    /// Runs the real `mkasset -c 2 -w 256` on `data` and returns its output file.
    fn run_mkasset(tool: &Path, dir: &Path, name: &str, data: &[u8]) -> Vec<u8> {
        let inp = dir.join("in").join(name);
        let outdir = dir.join("out");
        std::fs::create_dir_all(inp.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&outdir).unwrap();
        std::fs::write(&inp, data).unwrap();
        let st = Command::new(tool).args(["-c", "2", "-w", "256", "-o"]).arg(&outdir).arg(&inp).output().unwrap();
        assert!(st.status.success(), "mkasset failed: {}", String::from_utf8_lossy(&st.stderr));
        let o = std::fs::read(outdir.join(name)).unwrap();
        std::fs::remove_file(&inp).ok();
        std::fs::remove_file(outdir.join(name)).ok();
        o
    }

    fn compare_files(tool: &Path, dir: &Path, files: &[(String, Vec<u8>)]) -> usize {
        let mut bad = Vec::new();
        for (name, data) in files {
            let want = run_mkasset(tool, dir, "f.bin", data);
            let got = mkasset_aplib(data, 256 * 1024);
            if want != got {
                eprintln!("MISMATCH {name} (len {}): tool {} bytes, ours {} bytes", data.len(), want.len(), got.len());
                bad.push(name.clone());
            }
        }
        assert!(bad.is_empty(), "{} mismatches: {bad:?}", bad.len());
        files.len()
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("f2-dca-parity-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Parity against the real `mkasset` on synthetic inputs (skips if the tool is missing).
    #[test]
    fn dca_parity_mkasset_synthetic() {
        let Some(tool) = mkasset_path() else {
            eprintln!("skipping: mkasset not found ($N64_INST/bin/mkasset)");
            return;
        };
        let dir = scratch("synth");
        let mut files = Vec::new();
        let sizes = [0usize, 1, 2, 3, 7, 16, 100, 1000, 1023, 1024, 1025, 2047, 2049, 4097, 10_000, 65_537, 100_000, 262_144, 262_145, 400_000];
        for (i, &n) in sizes.iter().enumerate() {
            for kind in 0..6u32 {
                files.push((format!("k{kind}-n{n}"), crate::apultra::tests_support::synth(kind, n, (i as u64) * 31 + kind as u64)));
            }
        }
        files.push(("zeros-300k".into(), vec![0u8; 300_000]));
        files.push(("ff-5000".into(), vec![0xffu8; 5000]));
        let n = compare_files(&tool, &dir, &files);
        std::fs::remove_dir_all(&dir).ok();
        eprintln!("byte-identical: {n} synthetic inputs");
    }

    /// Multi-block (> 1 MiB) parity; slow-ish, run with `--ignored`.
    #[test]
    #[ignore]
    fn dca_parity_mkasset_multiblock() {
        let Some(tool) = mkasset_path() else {
            eprintln!("skipping: mkasset not found");
            return;
        };
        let dir = scratch("multi");
        let mut files = Vec::new();
        for (i, &n) in [1_048_575usize, 1_048_576, 1_048_577, 1_500_000, 2_200_000, 3_145_728].iter().enumerate() {
            for kind in [0u32, 2, 3, 4, 5] {
                files.push((format!("k{kind}-n{n}"), crate::apultra::tests_support::synth(kind, n, i as u64 + 1000)));
            }
        }
        let n = compare_files(&tool, &dir, &files);
        std::fs::remove_dir_all(&dir).ok();
        eprintln!("byte-identical: {n} multi-block inputs");
    }

    /// Parity on arbitrary files: every regular file under the directories listed in
    /// `F2_DCA_PARITY_DIRS` (colon-separated), default `target/release/deps` of this workspace.
    /// Run with `--ignored`.
    #[test]
    #[ignore]
    fn dca_parity_mkasset_files() {
        let Some(tool) = mkasset_path() else {
            eprintln!("skipping: mkasset not found");
            return;
        };
        let dirs = std::env::var("F2_DCA_PARITY_DIRS").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/release/deps").to_string());
        let max_len: usize = std::env::var("F2_DCA_PARITY_MAX").ok().and_then(|s| s.parse().ok()).unwrap_or(4 << 20);
        let mut paths = Vec::new();
        for d in dirs.split(':') {
            let Ok(rd) = std::fs::read_dir(d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_file() {
                    paths.push(p);
                }
            }
        }
        paths.sort();
        let dir = scratch("files");
        let mut total = 0;
        for chunk in paths.chunks(16) {
            let files: Vec<(String, Vec<u8>)> = chunk
                .iter()
                .filter_map(|p| {
                    let d = std::fs::read(p).ok()?;
                    // mkasset would decompress an input that is already a DCA asset.
                    (d.len() <= max_len && !d.starts_with(b"DCA")).then(|| (p.display().to_string(), d))
                })
                .collect();
            total += compare_files(&tool, &dir, &files);
        }
        std::fs::remove_dir_all(&dir).ok();
        eprintln!("byte-identical: {total} files");
    }
}
