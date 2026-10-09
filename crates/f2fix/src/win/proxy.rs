//! `dinput.dll` proxy exports, forwarded at runtime to the real system DLL,
//! plus the DirectInput 7 vtable hooks (`input.virtual`).
//!
//! Adapted from theme-park-world-recomp `crates/tpwfix/src/win/proxy.rs`
//! (MIT, same owner). `frogger2.exe` imports only `DirectInputCreateEx`
//! (controll.cpp `InitInputDevices`: DirectInput 7, keyboard
//! `NONEXCLUSIVE|BACKGROUND`, joypads `EXCLUSIVE|FOREGROUND`).
//!
//! With `input.virtual=1` (default):
//! * `EnumDevices(DIDEVTYPE_JOYSTICK)` reports no device, so the owner's
//!   gamepads are never opened, let alone acquired exclusively;
//! * every device's cooperative level is forced to `BACKGROUND|NONEXCLUSIVE`;
//! * `GetDeviceState`/`GetDeviceData` return "nothing pressed", so the
//!   owner's typing in other windows never reaches the game (the keyboard
//!   is opened in background mode by the original).

use super::iat::{self, orig};
use super::log::flog;
use super::{cfg, ffi};
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::os::windows::ffi::OsStrExt;
use std::sync::OnceLock;

const E_FAIL: i32 = 0x8000_4005u32 as i32;
const S_FALSE: i32 = 1;

static REAL: OnceLock<usize> = OnceLock::new();

/// HMODULE of the real dinput.dll (0 if it failed to load). Never call it
/// under the loader lock.
pub fn real_module() -> usize {
    *REAL.get_or_init(|| {
        let mut buf = [0u16; 512];
        // SAFETY: valid buffer.
        let n = unsafe { ffi::GetSystemDirectoryW(buf.as_mut_ptr(), 512) } as usize;
        let mut p = std::path::PathBuf::from(std::ffi::OsString::from(String::from_utf16_lossy(&buf[..n])));
        p.push("dinput.dll");
        let w: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: NUL-terminated path.
        let m = unsafe { ffi::LoadLibraryW(w.as_ptr()) } as usize;
        if m == 0 {
            flog!("proxy: LoadLibrary({}) FAILED, error {}", p.display(), unsafe { ffi::GetLastError() });
        } else {
            flog!("proxy: real {} at {:#010x}", p.display(), m);
        }
        m
    })
}

fn resolve(cache: &AtomicUsize, name: &'static [u8]) -> usize {
    let p = cache.load(Ordering::Acquire);
    if p != 0 {
        return p;
    }
    let m = real_module();
    if m == 0 {
        return 0;
    }
    // SAFETY: valid module, NUL-terminated name.
    let p = unsafe { ffi::GetProcAddress(m as ffi::HMODULE, name.as_ptr()) } as usize;
    cache.store(p, Ordering::Release);
    p
}

type CreateFn = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void, *mut c_void) -> i32;
type CreateExFn = unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void, *mut c_void) -> i32;
type GetClassObjectFn = unsafe extern "system" fn(*const c_void, *const c_void, *mut *mut c_void) -> i32;

/// # Safety
/// DirectInputCreateA's contract.
#[no_mangle]
pub unsafe extern "system" fn DirectInputCreateA(hinst: *mut c_void, version: u32, out: *mut *mut c_void, outer: *mut c_void) -> i32 {
    static F: AtomicUsize = AtomicUsize::new(0);
    let p = resolve(&F, b"DirectInputCreateA\0");
    let hr = if p == 0 { E_FAIL } else { core::mem::transmute::<usize, CreateFn>(p)(hinst, version, out, outer) };
    flog!("proxy: DirectInputCreateA({:#x}) = {:#010x}", version, hr);
    hr
}

/// # Safety
/// DirectInputCreateW's contract.
#[no_mangle]
pub unsafe extern "system" fn DirectInputCreateW(hinst: *mut c_void, version: u32, out: *mut *mut c_void, outer: *mut c_void) -> i32 {
    static F: AtomicUsize = AtomicUsize::new(0);
    let p = resolve(&F, b"DirectInputCreateW\0");
    let hr = if p == 0 { E_FAIL } else { core::mem::transmute::<usize, CreateFn>(p)(hinst, version, out, outer) };
    flog!("proxy: DirectInputCreateW({:#x}) = {:#010x}", version, hr);
    hr
}

/// # Safety
/// DirectInputCreateEx's contract.
#[no_mangle]
pub unsafe extern "system" fn DirectInputCreateEx(hinst: *mut c_void, version: u32, riid: *const c_void, out: *mut *mut c_void, outer: *mut c_void) -> i32 {
    static F: AtomicUsize = AtomicUsize::new(0);
    let p = resolve(&F, b"DirectInputCreateEx\0");
    let hr = if p == 0 { E_FAIL } else { core::mem::transmute::<usize, CreateExFn>(p)(hinst, version, riid, out, outer) };
    flog!("proxy: DirectInputCreateEx({:#x}) = {:#010x}", version, hr);
    if hr >= 0 && !out.is_null() && !(*out).is_null() && cfg().input_virtual {
        hook_di(*out as usize);
    }
    hr
}

/// # Safety
/// COM DllGetClassObject contract.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const c_void, iid: *const c_void, out: *mut *mut c_void) -> i32 {
    static F: AtomicUsize = AtomicUsize::new(0);
    let p = resolve(&F, b"DllGetClassObject\0");
    if p == 0 {
        return E_FAIL;
    }
    core::mem::transmute::<usize, GetClassObjectFn>(p)(clsid, iid, out)
}

/// The proxy is never unloaded (it hosts hooks).
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> i32 {
    S_FALSE
}

// ---- DirectInput 7 vtable hooks -------------------------------------------

const DIDEVTYPE_JOYSTICK: u32 = 4;
const DISCL_NONEXCLUSIVE: u32 = 2;
const DISCL_BACKGROUND: u32 = 8;

static O_ENUM: AtomicUsize = AtomicUsize::new(0);
static O_CREATE_EX: AtomicUsize = AtomicUsize::new(0);
static O_COOP: AtomicUsize = AtomicUsize::new(0);
static O_STATE: AtomicUsize = AtomicUsize::new(0);
static O_DATA: AtomicUsize = AtomicUsize::new(0);

type EnumFn = unsafe extern "system" fn(usize, u32, usize, usize, u32) -> i32;
type CreateDevExFn = unsafe extern "system" fn(usize, usize, usize, *mut usize, usize) -> i32;
type CoopFn = unsafe extern "system" fn(usize, usize, u32) -> i32;

unsafe fn hook_di(di: usize) {
    // IDirectInput7A: 4 EnumDevices, 9 CreateDeviceEx.
    let a = iat::patch_vtbl(di, 4, h_enum_devices as *const () as usize, &O_ENUM);
    let b = iat::patch_vtbl(di, 9, h_create_device_ex as *const () as usize, &O_CREATE_EX);
    flog!("input: IDirectInput7 hooks {} {} (virtual input: no joysticks, no key state)", a, b);
}

unsafe extern "system" fn h_enum_devices(this: usize, ty: u32, cb: usize, r: usize, fl: u32) -> i32 {
    if ty & 0xff == DIDEVTYPE_JOYSTICK {
        flog!("input: EnumDevices(joystick) -> none (input.virtual)");
        return 0;
    }
    orig!(O_ENUM, EnumFn)(this, ty, cb, r, fl)
}

unsafe extern "system" fn h_create_device_ex(this: usize, guid: usize, iid: usize, out: *mut usize, outer: usize) -> i32 {
    let hr = orig!(O_CREATE_EX, CreateDevExFn)(this, guid, iid, out, outer);
    if hr >= 0 && !out.is_null() && *out != 0 {
        // IDirectInputDevice7A: 9 GetDeviceState, 10 GetDeviceData, 13 SetCooperativeLevel.
        let d = *out;
        iat::patch_vtbl(d, 9, h_get_state as *const () as usize, &O_STATE);
        iat::patch_vtbl(d, 10, h_get_data as *const () as usize, &O_DATA);
        iat::patch_vtbl(d, 13, h_set_coop as *const () as usize, &O_COOP);
        flog!("input: CreateDeviceEx -> {:#x}; device hooks installed", d);
    }
    hr
}

unsafe extern "system" fn h_set_coop(this: usize, hwnd: usize, flags: u32) -> i32 {
    let forced = DISCL_BACKGROUND | DISCL_NONEXCLUSIVE;
    let hr = orig!(O_COOP, CoopFn)(this, hwnd, forced);
    flog!("input: SetCooperativeLevel({:#x}) forced to {:#x} = {:#010x}", flags, forced, hr);
    hr
}

unsafe extern "system" fn h_get_state(this: usize, cb: u32, data: *mut u8) -> i32 {
    let _ = this;
    if !data.is_null() {
        core::ptr::write_bytes(data, 0, cb as usize);
    }
    0
}

unsafe extern "system" fn h_get_data(this: usize, cbo: u32, rgdod: usize, n: *mut u32, fl: u32) -> i32 {
    let _ = (this, cbo, rgdod, fl);
    if !n.is_null() {
        *n = 0;
    }
    0
}
