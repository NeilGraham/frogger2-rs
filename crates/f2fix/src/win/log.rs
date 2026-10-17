//! `f2fix.log` in the run directory: one line per event, flushed per line.
//! Safe under the loader lock:
//! only CreateFileW/WriteFile underneath.

use super::ffi;
use std::fs::File;
use std::io::Write;
use std::sync::Mutex;

static LOG: Mutex<Option<File>> = Mutex::new(None);
static T0: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

pub fn open(path: &std::path::Path) -> bool {
    T0.get_or_init(|| unsafe { ffi::GetTickCount() });
    match std::fs::OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => {
            if let Ok(mut g) = LOG.lock() {
                *g = Some(f);
            }
            true
        }
        Err(_) => false,
    }
}

/// Milliseconds since the DLL loaded.
pub fn ms() -> u32 {
    // SAFETY: trivial kernel32 call.
    let now = unsafe { ffi::GetTickCount() };
    now.wrapping_sub(*T0.get_or_init(|| now))
}

pub fn line(args: core::fmt::Arguments<'_>) {
    // SAFETY: trivial kernel32 call.
    let tid = unsafe { ffi::GetCurrentThreadId() };
    let s = format!("[{:>8.3} t{}] {}\r\n", ms() as f64 / 1000.0, tid, args);
    if let Ok(mut g) = LOG.lock() {
        if let Some(f) = g.as_mut() {
            let _ = f.write_all(s.as_bytes());
        }
    }
}

macro_rules! flog {
    ($($t:tt)*) => { $crate::win::log::line(format_args!($($t)*)) };
}
pub(crate) use flog;
