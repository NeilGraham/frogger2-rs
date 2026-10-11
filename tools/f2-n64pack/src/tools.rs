//! Running external tools (libdragon, ffmpeg) and hashing for caches.

use std::path::Path;
use std::process::Command;

use crate::Result;

/// Runs a command, failing with its stderr on a non-zero exit.
pub fn run(cmd: &mut Command) -> Result<std::process::Output> {
    let out = cmd.output().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if !out.status.success() {
        return Err(format!(
            "{:?} {:?} failed ({}): {}",
            cmd.get_program(),
            cmd.get_args().collect::<Vec<_>>(),
            out.status,
            String::from_utf8_lossy(&out.stderr).lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
        ));
    }
    Ok(out)
}

/// FNV-1a 64 (cache keys and dedupe; not cryptographic).
pub fn fnv64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Cache key of a file (size + mtime + path) combined with encoder parameters.
pub fn file_key(p: &Path, params: &str) -> String {
    let md = std::fs::metadata(p).ok();
    let size = md.as_ref().map_or(0, |m| m.len());
    let mtime = md.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    format!("{:016x}", fnv64(format!("{}|{size}|{mtime}|{params}", p.display()).as_bytes()))
}

/// Runs `f` over `items` with up to `jobs` threads, preserving order.
pub fn par_map<T: Sync, R: Send>(items: &[T], jobs: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let jobs = jobs.max(1).min(items.len().max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<R>>> = items.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= items.len() {
                    break;
                }
                let r = f(&items[i]);
                *results[i].lock().unwrap() = Some(r);
            });
        }
    });
    results.into_iter().map(|m| m.into_inner().unwrap().unwrap()).collect()
}
