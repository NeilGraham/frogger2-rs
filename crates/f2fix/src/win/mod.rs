//! Windows side of f2fix: DllMain, settings, the init thread.
//!
//! Loader-lock discipline: `DllMain(PROCESS_ATTACH)`
//! only reads the ini, opens the log and patches import slots
//! (VirtualProtect + stores), then starts the init thread. It never loads a
//! library or waits. The init thread (which the OS starts after the loader
//! lock is released) loads the real dinput.dll and starts the audio mute and
//! the exit timer.

pub mod audiomute;
pub mod dd;
pub mod ffi;
pub mod iat;
pub mod log;
pub mod oracle;
pub mod proxy;
pub mod shims;

use crate::cfg::{self, Cfg};
use core::ffi::c_void;
use log::flog;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::OnceLock;

static CFG: OnceLock<Cfg> = OnceLock::new();
static RUN_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Settings (defaults until DllMain read the ini).
pub fn cfg() -> &'static Cfg {
    CFG.get_or_init(Cfg::default)
}

/// Directory of frogger2.exe: ini, log and dumps live here.
pub fn run_dir() -> &'static PathBuf {
    RUN_DIR.get_or_init(|| {
        let mut buf = [0u16; 1024];
        // SAFETY: valid buffer; NULL = the exe.
        let n = unsafe { ffi::GetModuleFileNameW(core::ptr::null_mut(), buf.as_mut_ptr(), 1024) };
        let p = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n as usize]));
        p.parent().map(|d| d.to_path_buf()).unwrap_or_default()
    })
}

/// Terminate the game now (exit.frames / exit.seconds).
pub fn terminate(why: &str) -> ! {
    flog!("f2fix: terminating the process ({})", why);
    // SAFETY: own process.
    unsafe {
        ffi::TerminateProcess(ffi::GetCurrentProcess(), 0);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// DLL entry point. Minimal and loader-lock safe. (Called only by the loader.)
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "system" fn DllMain(hinst: ffi::HMODULE, reason: u32, _r: *mut c_void) -> ffi::BOOL {
    if reason == ffi::DLL_PROCESS_ATTACH {
        // SAFETY: own module handle.
        unsafe { ffi::DisableThreadLibraryCalls(hinst) };
        attach();
    } else if reason == ffi::DLL_PROCESS_DETACH {
        flog!("f2fix: process detach");
    }
    1
}

fn attach() {
    let dir = run_dir();
    let log_ok = log::open(&dir.join("f2fix.log"));
    let ini = dir.join("f2fix.ini");
    let (c, errs, found) = match std::fs::read(&ini) {
        Ok(b) => {
            let (c, e) = cfg::parse(&cfg::decode(&b));
            (c, e, true)
        }
        Err(_) => (Cfg::default(), Vec::new(), false),
    };
    let _ = CFG.set(c);
    // SAFETY: trivial.
    let pid = unsafe { ffi::GetCurrentProcessId() };
    flog!("=== f2fix {} loaded, pid {}, run dir {} (log {}) ===", env!("CARGO_PKG_VERSION"), pid, dir.display(), if log_ok { "ok" } else { "FAILED" });
    flog!("f2fix: ini {} {}", ini.display(), if found { "read" } else { "absent (defaults)" });
    for e in &errs {
        flog!("f2fix: ini: {}", e);
    }
    flog!("f2fix: settings {:?}", cfg());
    shims::install("DllMain");
    oracle::install();
    // SAFETY: CreateThread with a valid start routine; the handle is closed.
    unsafe {
        let h = ffi::CreateThread(core::ptr::null_mut(), 0, init_thread, core::ptr::null_mut(), 0, core::ptr::null_mut());
        if h.is_null() {
            flog!("f2fix: CreateThread(init) FAILED");
        } else {
            ffi::CloseHandle(h);
        }
    }
}

unsafe extern "system" fn init_thread(_: *mut c_void) -> u32 {
    proxy::real_module();
    shims::install("init thread");
    audiomute::start();
    let secs = cfg().exit_seconds;
    if secs > 0 {
        std::thread::sleep(std::time::Duration::from_secs(secs as u64));
        flog!("f2fix: exit.seconds={} reached; requesting a final dump", secs);
        dd::request_final();
        // Give the game up to 5 s to present once more (the dump terminates).
        std::thread::sleep(std::time::Duration::from_secs(5));
        terminate("exit.seconds, no present for the final dump");
    }
    0
}
