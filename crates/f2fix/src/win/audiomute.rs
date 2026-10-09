//! Mute this process's audio session (`audio.mute=1`, the default).
//!
//! Adapted from theme-park-world-recomp `crates/tpwfix/src/win/audiomute.rs`
//! (MIT, same owner). The game's sound code (DirectSound, Bink's DirectSound
//! and waveOut streams) runs unchanged; only the Windows session volume of
//! this one process is muted, like the per-app mute in the volume mixer.
//! Nothing system-wide is touched. A dedicated MTA thread re-asserts the
//! mute periodically; `DirectSoundCreate` (hooked in `shims`) waits until the
//! first mute succeeded, so no sound can play before it.

use super::ffi;
use super::log::flog;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

/// Set once the session mute succeeded (or muting is off / impossible).
static READY: AtomicBool = AtomicBool::new(false);

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

const CLSID_MM_DEVICE_ENUMERATOR: Guid = Guid(
    0xbcde_0395,
    0xe52f,
    0x467c,
    [0x8e, 0x3d, 0xc4, 0x57, 0x92, 0x91, 0x69, 0x2e],
);
const IID_IMM_DEVICE_ENUMERATOR: Guid = Guid(
    0xa956_64d2,
    0x9614,
    0x4f35,
    [0xa7, 0x46, 0xde, 0x8d, 0xb6, 0x36, 0x17, 0xe6],
);
const IID_IAUDIO_SESSION_MANAGER: Guid = Guid(
    0xbfa9_71f1,
    0x4d5e,
    0x40bb,
    [0x93, 0x5e, 0x96, 0x70, 0x39, 0xbf, 0xbe, 0xe4],
);

const CLSCTX_INPROC_SERVER: u32 = 1;
const CLSCTX_ALL: u32 = 0x17;
const E_RENDER: u32 = 0;
const E_CONSOLE: u32 = 0;

type FnCoInit = unsafe extern "system" fn(*mut c_void, u32) -> i32;
type FnCoCreate =
    unsafe extern "system" fn(*const Guid, *mut c_void, u32, *const Guid, *mut usize) -> i32;
type FnRelease = unsafe extern "system" fn(usize) -> u32;
type FnGetDefault = unsafe extern "system" fn(usize, u32, u32, *mut usize) -> i32;
type FnActivate =
    unsafe extern "system" fn(usize, *const Guid, u32, *mut c_void, *mut usize) -> i32;
type FnGetSimpleVol = unsafe extern "system" fn(usize, *const Guid, u32, *mut usize) -> i32;
type FnSetMute = unsafe extern "system" fn(usize, i32, *const Guid) -> i32;
type FnGetMute = unsafe extern "system" fn(usize, *mut i32) -> i32;

/// COM vtable slot `i` of interface pointer `p`.
unsafe fn slot(p: usize, i: usize) -> usize {
    let vt = *(p as *const *const usize);
    *vt.add(i)
}

unsafe fn release(p: usize) {
    if p != 0 {
        core::mem::transmute::<usize, FnRelease>(slot(p, 2))(p);
    }
}

/// Mute the default render device's session of this process. Returns
/// Ok(was_already_muted) or the failing step and HRESULT.
unsafe fn mute_once(co_create: FnCoCreate) -> Result<bool, (&'static str, i32)> {
    let mut en = 0usize;
    let hr = co_create(
        &CLSID_MM_DEVICE_ENUMERATOR,
        core::ptr::null_mut(),
        CLSCTX_INPROC_SERVER,
        &IID_IMM_DEVICE_ENUMERATOR,
        &mut en,
    );
    if hr < 0 {
        return Err(("CoCreateInstance(MMDeviceEnumerator)", hr));
    }
    let mut dev = 0usize;
    let hr =
        core::mem::transmute::<usize, FnGetDefault>(slot(en, 4))(en, E_RENDER, E_CONSOLE, &mut dev);
    release(en);
    if hr < 0 {
        // No audio device at all: nothing can play, nothing to mute.
        return Err(("GetDefaultAudioEndpoint", hr));
    }
    let mut mgr = 0usize;
    let hr = core::mem::transmute::<usize, FnActivate>(slot(dev, 3))(
        dev,
        &IID_IAUDIO_SESSION_MANAGER,
        CLSCTX_ALL,
        core::ptr::null_mut(),
        &mut mgr,
    );
    release(dev);
    if hr < 0 {
        return Err(("IMMDevice::Activate(IAudioSessionManager)", hr));
    }
    let mut vol = 0usize;
    // GUID_NULL session, same process: the session DirectSound and waveOut
    // streams of this process join.
    let hr = core::mem::transmute::<usize, FnGetSimpleVol>(slot(mgr, 4))(
        mgr,
        core::ptr::null(),
        0,
        &mut vol,
    );
    release(mgr);
    if hr < 0 {
        return Err(("GetSimpleAudioVolume", hr));
    }
    let mut m = 0i32;
    core::mem::transmute::<usize, FnGetMute>(slot(vol, 6))(vol, &mut m);
    let hr = if m == 0 {
        core::mem::transmute::<usize, FnSetMute>(slot(vol, 5))(vol, 1, core::ptr::null())
    } else {
        0
    };
    release(vol);
    if hr < 0 {
        return Err(("ISimpleAudioVolume::SetMute", hr));
    }
    Ok(m != 0)
}

/// Block (up to `ms`) until the first mute attempt finished.
pub fn wait_ready(ms: u32) -> bool {
    let mut waited = 0;
    while !READY.load(Ordering::Acquire) && waited < ms {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 10;
    }
    READY.load(Ordering::Acquire)
}

/// Start the mute thread if `audio.mute` is on (init thread; may load ole32).
pub fn start() {
    if !super::cfg().audio_mute {
        flog!("audio: audio.mute=0; game audio is audible");
        READY.store(true, Ordering::Release);
        return;
    }
    // SAFETY: not under the loader lock (init thread); NUL-terminated names.
    let (ci, cc) = unsafe {
        let m = ffi::LoadLibraryA(c"ole32.dll".as_ptr().cast());
        if m.is_null() {
            (0, 0)
        } else {
            (
                ffi::GetProcAddress(m, c"CoInitializeEx".as_ptr().cast()) as usize,
                ffi::GetProcAddress(m, c"CoCreateInstance".as_ptr().cast()) as usize,
            )
        }
    };
    if ci == 0 || cc == 0 {
        flog!("audio: mute requested but ole32 is unavailable; game audio is NOT muted");
        READY.store(true, Ordering::Release);
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("f2fix-audiomute".into())
        .spawn(move || {
            // SAFETY: documented prototypes; our own thread and apartment.
            unsafe {
                let co_init = core::mem::transmute::<usize, FnCoInit>(ci);
                let co_create = core::mem::transmute::<usize, FnCoCreate>(cc);
                co_init(core::ptr::null_mut(), 0); // COINIT_MULTITHREADED
                let (mut logged_ok, mut logged_err) = (false, false);
                let mut i = 0u32;
                loop {
                    i = i.saturating_add(1);
                    match mute_once(co_create) {
                        Ok(already) => {
                            if !logged_ok {
                                flog!("audio: this process's audio session is muted (audio.mute=1)");
                                logged_ok = true;
                            } else if !already {
                                flog!("audio: session was unmuted (device change?); muted again");
                            }
                            READY.store(true, Ordering::Release);
                        }
                        Err((step, hr)) => {
                            if !logged_err {
                                flog!("audio: mute failed at {} = {:#010x} (retrying)", step, hr as u32);
                                logged_err = true;
                            }
                            // No render device at all: nothing can be heard.
                            if step == "GetDefaultAudioEndpoint" {
                                READY.store(true, Ordering::Release);
                            }
                        }
                    }
                    // Fast at start-up (sessions appear when sound opens), then slower.
                    std::thread::sleep(std::time::Duration::from_millis(if i < 60 { 250 } else { 2000 }));
                }
            }
        });
    if spawned.is_err() {
        flog!("audio: could not start the mute thread; game audio is NOT muted");
        READY.store(true, Ordering::Release);
    }
}
