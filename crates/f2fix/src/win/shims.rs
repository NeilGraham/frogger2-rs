//! Import-table shims of `frogger2.exe` (and `binkw32.dll`'s CreateFileA).
//!
//! | area | APIs | answer |
//! |---|---|---|
//! | registry (Main.cpp `GetRegistryInformation`, controll.cpp) | `RegOpenKeyExA`, `RegCreateKeyExA`, `RegQueryValueExA`, `RegSetValueExA`, `RegCloseKey` | `HKLM\Software\Hasbro Interactive\Frogger2` is virtual: values from `f2fix.ini`, writes dropped; the real registry is never touched for this key |
//! | CD (Main.cpp `FindFrogger2CD`, video.cpp) | `GetLogicalDriveStringsA`, `GetDriveTypeA`, `GetVolumeInformationA`, `CreateFileA`, `FindFirstFileA` | one virtual CD-ROM drive `Q:\` labelled `FROGGER2`; paths on it are redirected to `cd.root` |
//! | CD audio (pcaudio.c) | `mciSendCommandA` | every command succeeds silently (fake device id) |
//! | system mixer | `mixerSetControlDetails` | dropped (the game would change the owner's mixer volumes) |
//! | window (mdxWindows.cpp, mdxDDraw.cpp) | `RegisterClassA`, `CreateWindowExA`, `ShowWindow`, `SetWindowPos` | off-screen tool window that never activates; deactivation messages are filtered so `DrawLoop` keeps drawing |
//! | modal UI | `MessageBoxA`, `DialogBoxParamA` | never shown: logged and answered with the cancel/no default |
//! | DirectDraw | `DirectDrawCreateEx` | vtable hooks for the framebuffer dump (`dd`) |
//! | DirectSound | `DirectSoundCreate` (ordinal 1) | waits for the session mute first |
//! | Bink (video.cpp `StartVideoPlayback`) | `_BinkOpen@8` | `fmv.skip`: fails, so every FMV is skipped (Bink plays in real time) |

use super::iat::{self, orig, Import};
use super::log::flog;
use super::{audiomute, cfg, dd, ffi};
use crate::cfg::{to_ansi, WindowMode};
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;

const HKLM: usize = 0x8000_0002;
const FAKE_KEY: usize = 0x4632_0001;
const REG_KEY: &[u8] = b"Software\\Hasbro Interactive\\Frogger2";
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_INVALID_PARAMETER: i32 = 87;
const ERROR_MORE_DATA: i32 = 234;
const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;
const DRIVE_CDROM: u32 = 5;

macro_rules! origs {
    ($($n:ident),* $(,)?) => { $(static $n: AtomicUsize = AtomicUsize::new(0);)* };
}
origs!(
    O_REG_OPEN, O_REG_CREATE, O_REG_QUERY, O_REG_SET, O_REG_CLOSE, O_DRIVES, O_DRIVE_TYPE, O_VOLINFO,
    O_CREATE_FILE, O_BINK_CREATE_FILE, O_FIND_FIRST, O_MCI, O_MIXER_SET, O_REGISTER_CLASS,
    O_CREATE_WINDOW, O_SHOW_WINDOW, O_SET_WINDOW_POS, O_MESSAGE_BOX, O_DIALOG_BOX, O_DD_CREATE_EX,
    O_DS_CREATE, O_BINK_OPEN,
);

macro_rules! imp {
    ($dll:literal, $name:literal, $hook:ident, $orig:ident) => {
        Import { dll: $dll, name: $name, ord: 0, hook: $hook as *const () as usize, orig: &$orig }
    };
}

fn exe_table() -> [Import; 21] {
    [
        imp!(b"ADVAPI32.dll", b"RegOpenKeyExA", h_reg_open, O_REG_OPEN),
        imp!(b"ADVAPI32.dll", b"RegCreateKeyExA", h_reg_create, O_REG_CREATE),
        imp!(b"ADVAPI32.dll", b"RegQueryValueExA", h_reg_query, O_REG_QUERY),
        imp!(b"ADVAPI32.dll", b"RegSetValueExA", h_reg_set, O_REG_SET),
        imp!(b"ADVAPI32.dll", b"RegCloseKey", h_reg_close, O_REG_CLOSE),
        imp!(b"KERNEL32.dll", b"GetLogicalDriveStringsA", h_drives, O_DRIVES),
        imp!(b"KERNEL32.dll", b"GetDriveTypeA", h_drive_type, O_DRIVE_TYPE),
        imp!(b"KERNEL32.dll", b"GetVolumeInformationA", h_volinfo, O_VOLINFO),
        imp!(b"KERNEL32.dll", b"CreateFileA", h_create_file, O_CREATE_FILE),
        imp!(b"KERNEL32.dll", b"FindFirstFileA", h_find_first, O_FIND_FIRST),
        imp!(b"WINMM.dll", b"mciSendCommandA", h_mci, O_MCI),
        imp!(b"WINMM.dll", b"mixerSetControlDetails", h_mixer_set, O_MIXER_SET),
        imp!(b"USER32.dll", b"RegisterClassA", h_register_class, O_REGISTER_CLASS),
        imp!(b"USER32.dll", b"CreateWindowExA", h_create_window, O_CREATE_WINDOW),
        imp!(b"USER32.dll", b"ShowWindow", h_show_window, O_SHOW_WINDOW),
        imp!(b"USER32.dll", b"SetWindowPos", h_set_window_pos, O_SET_WINDOW_POS),
        imp!(b"USER32.dll", b"MessageBoxA", h_message_box, O_MESSAGE_BOX),
        imp!(b"USER32.dll", b"DialogBoxParamA", h_dialog_box, O_DIALOG_BOX),
        imp!(b"DDRAW.dll", b"DirectDrawCreateEx", h_dd_create_ex, O_DD_CREATE_EX),
        Import { dll: b"DSOUND.dll", name: b"", ord: 1, hook: h_ds_create as *const () as usize, orig: &O_DS_CREATE },
        imp!(b"binkw32.dll", b"_BinkOpen@8", h_bink_open, O_BINK_OPEN),
    ]
}

fn bink_table() -> [Import; 1] {
    [imp!(b"KERNEL32.dll", b"CreateFileA", h_bink_create_file, O_BINK_CREATE_FILE)]
}

/// Patch the exe's and binkw32's import tables (idempotent).
pub fn install(when: &str) {
    // SAFETY: loaded images; GetModuleHandleA loads nothing.
    unsafe {
        let exe = ffi::GetModuleHandleA(core::ptr::null()) as usize;
        let (p, n) = iat::patch_imports(exe, &exe_table());
        flog!("shims[{}]: exe {:#x}: {} imports hooked, {} not yet bound", when, exe, p, n);
        let bink = ffi::GetModuleHandleA(c"binkw32.dll".as_ptr().cast()) as usize;
        if bink != 0 {
            let (p, n) = iat::patch_imports(bink, &bink_table());
            flog!("shims[{}]: binkw32 {:#x}: {} imports hooked, {} not yet bound", when, bink, p, n);
        } else {
            flog!("shims[{}]: binkw32.dll not loaded yet", when);
        }
    }
}

unsafe fn cstr<'a>(p: *const u8) -> &'a [u8] {
    if p.is_null() {
        return b"";
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(p, n)
}

fn show(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// Throttle per-call-site logging.
fn first(counter: &AtomicU32, n: u32) -> bool {
    counter.fetch_add(1, Ordering::Relaxed) < n
}

// ---- registry ---------------------------------------------------------------

/// InstallDir: `reg.install_dir`, else the exe's directory; with a trailing `\`.
pub fn install_dir() -> &'static [u8] {
    static D: OnceLock<Vec<u8>> = OnceLock::new();
    D.get_or_init(|| {
        let c = cfg();
        let mut v = if c.install_dir.is_empty() {
            to_ansi(&super::run_dir().to_string_lossy())
        } else {
            to_ansi(&c.install_dir)
        };
        if v.last() != Some(&b'\\') {
            v.push(b'\\');
        }
        v
    })
}

/// The primary display device's description (what the game's own device
/// enumeration lists first), so `DDrawInitObject` finds `VideoDevice` and
/// never opens its device dialog.
fn primary_device_desc() -> Vec<u8> {
    type CreateEx = unsafe extern "system" fn(usize, *mut usize, *const [u8; 16], usize) -> i32;
    type GetIdent = unsafe extern "system" fn(usize, *mut u8, u32) -> i32;
    type Release = unsafe extern "system" fn(usize) -> u32;
    const IID_IDIRECTDRAW7: [u8; 16] = [
        0xc0, 0x5e, 0xe6, 0x15, 0x9c, 0x3b, 0xd2, 0x11, 0xb9, 0x2f, 0x00, 0x60, 0x97, 0x97, 0xea, 0x5b,
    ];
    // SAFETY: ddraw.dll is a static import of the exe (loaded); documented
    // prototypes; the identifier buffer is larger than DDDEVICEIDENTIFIER2.
    unsafe {
        let m = ffi::GetModuleHandleA(c"ddraw.dll".as_ptr().cast());
        let f = ffi::GetProcAddress(m, c"DirectDrawCreateEx".as_ptr().cast()) as usize;
        if m.is_null() || f == 0 {
            flog!("reg: VideoDevice auto: ddraw.dll not available");
            return Vec::new();
        }
        let mut dd = 0usize;
        let hr = core::mem::transmute::<usize, CreateEx>(f)(0, &mut dd, &IID_IDIRECTDRAW7, 0);
        if hr < 0 || dd == 0 {
            flog!("reg: VideoDevice auto: DirectDrawCreateEx = {:#010x}", hr);
            return Vec::new();
        }
        let mut id = vec![0u8; 4096];
        let hr = core::mem::transmute::<usize, GetIdent>(iat::vslot(dd, 27))(dd, id.as_mut_ptr(), 0);
        core::mem::transmute::<usize, Release>(iat::vslot(dd, 2))(dd);
        if hr < 0 {
            flog!("reg: VideoDevice auto: GetDeviceIdentifier = {:#010x}", hr);
            return Vec::new();
        }
        // DDDEVICEIDENTIFIER2: szDriver[512], szDescription[512], ...
        let desc = &id[512..1024];
        let n = desc.iter().position(|&b| b == 0).unwrap_or(desc.len());
        flog!(
            "reg: VideoDevice auto: driver '{}' description '{}'",
            show(cstr(id.as_ptr())),
            show(&desc[..n])
        );
        desc[..n].to_vec()
    }
}

fn reg_value(name: &[u8]) -> Option<(u32, Vec<u8>)> {
    let c = cfg();
    let sz = |b: &[u8]| {
        let mut v = b.to_vec();
        v.push(0);
        Some((REG_SZ, v))
    };
    let lname = name.to_ascii_lowercase();
    match lname.as_slice() {
        b"installdir" => sz(install_dir()),
        b"resolution" => Some((REG_DWORD, c.resolution_dword().to_le_bytes().to_vec())),
        b"fullscreen" => Some((REG_DWORD, c.fullscreen.to_le_bytes().to_vec())),
        b"videodevice" => {
            if c.video_device.eq_ignore_ascii_case("auto") {
                static AUTO: OnceLock<Vec<u8>> = OnceLock::new();
                sz(AUTO.get_or_init(primary_device_desc))
            } else {
                sz(&to_ansi(&c.video_device))
            }
        }
        b"language" if !c.language.is_empty() => sz(&to_ansi(&c.language)),
        // controll.cpp names them "Control 1".."Control 4"; the retail exe asks
        // for "Control1".."Control4".
        b"control1" | b"control2" | b"control3" | b"control4" | b"control 1" | b"control 2"
        | b"control 3" | b"control 4"
            if !c.control.is_empty() =>
        {
            sz(&to_ansi(&c.control))
        }
        _ => None,
    }
}

fn is_game_key(hkey: usize, sub: &[u8]) -> bool {
    let sub = sub.strip_suffix(b"\\").unwrap_or(sub);
    hkey == HKLM && sub.eq_ignore_ascii_case(REG_KEY)
}

type RegOpenFn = unsafe extern "system" fn(usize, *const u8, u32, u32, *mut usize) -> i32;
unsafe extern "system" fn h_reg_open(hkey: usize, sub: *const u8, o: u32, sam: u32, out: *mut usize) -> i32 {
    if is_game_key(hkey, cstr(sub)) && !out.is_null() {
        *out = FAKE_KEY;
        flog!("reg: RegOpenKeyExA(HKLM\\{}) -> virtual key", show(cstr(sub)));
        return 0;
    }
    orig!(O_REG_OPEN, RegOpenFn)(hkey, sub, o, sam, out)
}

type RegCreateFn = unsafe extern "system" fn(usize, *const u8, u32, *const u8, u32, u32, usize, *mut usize, *mut u32) -> i32;
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn h_reg_create(
    hkey: usize,
    sub: *const u8,
    r: u32,
    class: *const u8,
    o: u32,
    sam: u32,
    sa: usize,
    out: *mut usize,
    disp: *mut u32,
) -> i32 {
    if is_game_key(hkey, cstr(sub)) && !out.is_null() {
        *out = FAKE_KEY;
        if !disp.is_null() {
            *disp = 2; // REG_OPENED_EXISTING_KEY
        }
        flog!("reg: RegCreateKeyExA(HKLM\\{}) -> virtual key (writes dropped)", show(cstr(sub)));
        return 0;
    }
    orig!(O_REG_CREATE, RegCreateFn)(hkey, sub, r, class, o, sam, sa, out, disp)
}

type RegQueryFn = unsafe extern "system" fn(usize, *const u8, *mut u32, *mut u32, *mut u8, *mut u32) -> i32;
unsafe extern "system" fn h_reg_query(
    hkey: usize,
    name: *const u8,
    r: *mut u32,
    ty: *mut u32,
    data: *mut u8,
    cb: *mut u32,
) -> i32 {
    if hkey != FAKE_KEY {
        return orig!(O_REG_QUERY, RegQueryFn)(hkey, name, r, ty, data, cb);
    }
    let n = cstr(name);
    let Some((t, v)) = reg_value(n) else {
        flog!("reg: query {} -> not found", show(n));
        return ERROR_FILE_NOT_FOUND;
    };
    let rc = if cb.is_null() {
        if data.is_null() { 0 } else { ERROR_INVALID_PARAMETER }
    } else if data.is_null() {
        *cb = v.len() as u32;
        0
    } else if (*cb as usize) < v.len() {
        *cb = v.len() as u32;
        ERROR_MORE_DATA
    } else {
        core::ptr::copy_nonoverlapping(v.as_ptr(), data, v.len());
        *cb = v.len() as u32;
        0
    };
    if rc == 0 && !ty.is_null() {
        *ty = t;
    }
    let shown = if t == REG_DWORD {
        format!("{:#x}", u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    } else {
        format!("'{}'", show(&v[..v.len() - 1]))
    };
    flog!("reg: query {} -> {} (rc {})", show(n), shown, rc);
    rc
}

type RegSetFn = unsafe extern "system" fn(usize, *const u8, u32, u32, *const u8, u32) -> i32;
unsafe extern "system" fn h_reg_set(hkey: usize, name: *const u8, r: u32, t: u32, d: *const u8, cb: u32) -> i32 {
    if hkey == FAKE_KEY {
        flog!("reg: set {} (type {}, {} bytes) dropped", show(cstr(name)), t, cb);
        return 0;
    }
    orig!(O_REG_SET, RegSetFn)(hkey, name, r, t, d, cb)
}

type RegCloseFn = unsafe extern "system" fn(usize) -> i32;
unsafe extern "system" fn h_reg_close(hkey: usize) -> i32 {
    if hkey == FAKE_KEY {
        return 0;
    }
    orig!(O_REG_CLOSE, RegCloseFn)(hkey)
}

// ---- virtual CD -------------------------------------------------------------

fn is_cd_path(p: &[u8]) -> bool {
    p.len() >= 2 && p[0].eq_ignore_ascii_case(&cfg().cd_drive) && p[1] == b':'
}

fn cd_root() -> &'static [u8] {
    static D: OnceLock<Vec<u8>> = OnceLock::new();
    D.get_or_init(|| {
        let c = cfg();
        let mut v = if c.cd_root.is_empty() { install_dir().to_vec() } else { to_ansi(&c.cd_root) };
        if v.last() != Some(&b'\\') {
            v.push(b'\\');
        }
        v
    })
}

/// `Q:\Video\x.bik` -> `<cd.root>\Video\x.bik` (NUL-terminated), else None.
unsafe fn map_cd(name: *const u8) -> Option<Vec<u8>> {
    let p = cstr(name);
    if !is_cd_path(p) {
        return None;
    }
    let mut rest = &p[2..];
    while let [b'\\' | b'/', r @ ..] = rest {
        rest = r;
    }
    let mut v = cd_root().to_vec();
    v.extend_from_slice(rest);
    v.push(0);
    Some(v)
}

unsafe extern "system" fn h_drives(n: u32, buf: *mut u8) -> u32 {
    let _ = O_DRIVES.load(Ordering::Relaxed);
    let s = [cfg().cd_drive, b':', b'\\', 0, 0];
    flog!("cd: GetLogicalDriveStringsA -> {}:\\ only (virtual CD)", cfg().cd_drive as char);
    if (n as usize) < s.len() || buf.is_null() {
        return s.len() as u32;
    }
    core::ptr::copy_nonoverlapping(s.as_ptr(), buf, s.len());
    4
}

type DriveTypeFn = unsafe extern "system" fn(*const u8) -> u32;
unsafe extern "system" fn h_drive_type(root: *const u8) -> u32 {
    if is_cd_path(cstr(root)) {
        return DRIVE_CDROM;
    }
    orig!(O_DRIVE_TYPE, DriveTypeFn)(root)
}

type VolInfoFn = unsafe extern "system" fn(*const u8, *mut u8, u32, *mut u32, *mut u32, *mut u32, *mut u8, u32) -> i32;
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn h_volinfo(
    root: *const u8,
    vn: *mut u8,
    vns: u32,
    serial: *mut u32,
    maxlen: *mut u32,
    flags: *mut u32,
    fsn: *mut u8,
    fsns: u32,
) -> i32 {
    if !is_cd_path(cstr(root)) {
        return orig!(O_VOLINFO, VolInfoFn)(root, vn, vns, serial, maxlen, flags, fsn, fsns);
    }
    let put = |dst: *mut u8, cap: u32, s: &[u8]| {
        if !dst.is_null() && cap as usize > s.len() {
            core::ptr::copy_nonoverlapping(s.as_ptr(), dst, s.len());
            *dst.add(s.len()) = 0;
        }
    };
    put(vn, vns, b"FROGGER2");
    put(fsn, fsns, b"CDFS");
    if !serial.is_null() {
        *serial = 0x0f20_2000;
    }
    if !maxlen.is_null() {
        *maxlen = 110;
    }
    if !flags.is_null() {
        *flags = 0x0008_0000; // FILE_READ_ONLY_VOLUME
    }
    flog!("cd: GetVolumeInformationA({}) -> FROGGER2 (maps to {})", show(cstr(root)), show(cd_root()));
    1
}

type CreateFileFn = unsafe extern "system" fn(*const u8, u32, u32, usize, u32, u32, usize) -> usize;
#[allow(clippy::too_many_arguments)]
unsafe fn create_file(o: &AtomicUsize, who: &str, name: *const u8, a: u32, s: u32, sa: usize, d: u32, f: u32, t: usize) -> usize {
    static N: AtomicU32 = AtomicU32::new(0);
    let real = orig!(o, CreateFileFn);
    match map_cd(name) {
        Some(m) => {
            let h = real(m.as_ptr(), a, s, sa, d, f, t);
            if first(&N, 64) {
                flog!("cd: {} CreateFileA({}) -> {} = {:#x}", who, show(cstr(name)), show(&m[..m.len() - 1]), h);
            }
            h
        }
        None => real(name, a, s, sa, d, f, t),
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn h_create_file(n: *const u8, a: u32, s: u32, sa: usize, d: u32, f: u32, t: usize) -> usize {
    create_file(&O_CREATE_FILE, "exe", n, a, s, sa, d, f, t)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn h_bink_create_file(n: *const u8, a: u32, s: u32, sa: usize, d: u32, f: u32, t: usize) -> usize {
    create_file(&O_BINK_CREATE_FILE, "binkw32", n, a, s, sa, d, f, t)
}

type FindFirstFn = unsafe extern "system" fn(*const u8, *mut c_void) -> usize;
unsafe extern "system" fn h_find_first(name: *const u8, data: *mut c_void) -> usize {
    match map_cd(name) {
        Some(m) => orig!(O_FIND_FIRST, FindFirstFn)(m.as_ptr(), data),
        None => orig!(O_FIND_FIRST, FindFirstFn)(name, data),
    }
}

// ---- winmm ------------------------------------------------------------------

const MCI_OPEN: u32 = 0x0803;
const MCI_STATUS: u32 = 0x0814;

unsafe extern "system" fn h_mci(id: u32, msg: u32, flags: usize, param: usize) -> u32 {
    static N: AtomicU32 = AtomicU32::new(0);
    let _ = O_MCI.load(Ordering::Relaxed);
    if first(&N, 32) {
        flog!("mci: mciSendCommandA(id {:#x}, msg {:#x}, flags {:#x}) -> 0 (CD audio stubbed)", id, msg, flags);
    }
    if param != 0 {
        match msg {
            // MCI_OPEN_PARMSA { dwCallback, wDeviceID, ... }
            MCI_OPEN => *((param + 4) as *mut u32) = 0x4632,
            // MCI_STATUS_PARMS { dwCallback, dwReturn, ... }
            MCI_STATUS => *((param + 4) as *mut u32) = 0,
            _ => {}
        }
    }
    0
}

unsafe extern "system" fn h_mixer_set(h: usize, d: usize, f: u32) -> u32 {
    static N: AtomicU32 = AtomicU32::new(0);
    let _ = O_MIXER_SET.load(Ordering::Relaxed);
    if first(&N, 8) {
        flog!("mixer: mixerSetControlDetails({:#x}, {:#x}, {:#x}) dropped (owner's mixer untouched)", h, d, f);
    }
    0
}

// ---- window -----------------------------------------------------------------

static GAME_WNDPROC: AtomicUsize = AtomicUsize::new(0);
const WS_EX_TOOLWINDOW: u32 = 0x80;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const WS_EX_TOPMOST: u32 = 0x8;
const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;
const SWP_NOMOVE: u32 = 0x2;
const SWP_NOZORDER: u32 = 0x4;
const SWP_NOACTIVATE: u32 = 0x10;
const SWP_SHOWWINDOW: u32 = 0x40;
const WM_ACTIVATE: u32 = 0x0006;
const WM_ACTIVATEAPP: u32 = 0x001c;

fn headless() -> bool {
    cfg().window != WindowMode::Off
}

fn offscreen_xy() -> (i32, i32) {
    // SAFETY: trivial user32 calls.
    unsafe { (ffi::GetSystemMetrics(ffi::SM_XVIRTUALSCREEN) - 12000, ffi::GetSystemMetrics(ffi::SM_YVIRTUALSCREEN)) }
}

type WndProc = unsafe extern "system" fn(usize, u32, usize, isize) -> isize;
/// The game's window procedure, minus deactivation: `WM_ACTIVATEAPP(FALSE)`
/// would clear `winActive` and stop `DrawLoop` (drawloop.cpp).
unsafe extern "system" fn wndproc_filter(h: usize, msg: u32, wp: usize, lp: isize) -> isize {
    if (msg == WM_ACTIVATEAPP && wp == 0) || (msg == WM_ACTIVATE && wp & 0xffff == 0) {
        return 0;
    }
    orig!(GAME_WNDPROC, WndProc)(h, msg, wp, lp)
}

type RegisterClassFn = unsafe extern "system" fn(*const [usize; 10]) -> u16;
unsafe extern "system" fn h_register_class(wc: *const [usize; 10]) -> u16 {
    let real = orig!(O_REGISTER_CLASS, RegisterClassFn);
    if !headless() || wc.is_null() || GAME_WNDPROC.load(Ordering::Acquire) != 0 {
        return real(wc);
    }
    let mut c = *wc;
    GAME_WNDPROC.store(c[1], Ordering::Release);
    c[1] = wndproc_filter as *const () as usize;
    flog!("window: RegisterClassA('{}'): deactivation filter installed", show(cstr(c[9] as *const u8)));
    real(&c)
}

type CreateWindowFn = unsafe extern "system" fn(u32, *const u8, *const u8, u32, i32, i32, i32, i32, usize, usize, usize, usize) -> usize;
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn h_create_window(
    ex: u32,
    cls: *const u8,
    name: *const u8,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: usize,
    menu: usize,
    inst: usize,
    param: usize,
) -> usize {
    let real = orig!(O_CREATE_WINDOW, CreateWindowFn);
    if !headless() || parent != 0 {
        return real(ex, cls, name, style, x, y, w, h, parent, menu, inst, param);
    }
    let ex2 = (ex | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) & !WS_EX_TOPMOST;
    let (ox, oy) = offscreen_xy();
    let hw = real(ex2, cls, name, style, ox, oy, w, h, parent, menu, inst, param);
    flog!(
        "window: CreateWindowExA('{}' {}x{} style {:#x} ex {:#x}) -> {:#x} at ({}, {}), ex {:#x}",
        show(cstr(name)), w, h, style, ex, hw, ox, oy, ex2
    );
    hw
}

type ShowWindowFn = unsafe extern "system" fn(usize, i32) -> i32;
unsafe extern "system" fn h_show_window(h: usize, cmd: i32) -> i32 {
    let real = orig!(O_SHOW_WINDOW, ShowWindowFn);
    if !headless() || cmd == SW_HIDE {
        return real(h, cmd);
    }
    if cfg().window == WindowMode::Hidden {
        flog!("window: ShowWindow({:#x}, {}) suppressed (hidden)", h, cmd);
        return 0;
    }
    flog!("window: ShowWindow({:#x}, {}) -> SW_SHOWNOACTIVATE", h, cmd);
    real(h, SW_SHOWNOACTIVATE)
}

type SetWindowPosFn = unsafe extern "system" fn(usize, usize, i32, i32, i32, i32, u32) -> i32;
unsafe extern "system" fn h_set_window_pos(h: usize, after: usize, x: i32, y: i32, cx: i32, cy: i32, fl: u32) -> i32 {
    let real = orig!(O_SET_WINDOW_POS, SetWindowPosFn);
    if !headless() {
        return real(h, after, x, y, cx, cy, fl);
    }
    let mut f = fl | SWP_NOACTIVATE | SWP_NOZORDER;
    if cfg().window == WindowMode::Hidden {
        f &= !SWP_SHOWWINDOW;
    }
    let (mut nx, mut ny) = (x, y);
    if fl & SWP_NOMOVE == 0 {
        (nx, ny) = offscreen_xy();
    }
    flog!("window: SetWindowPos({:#x}, {},{} {}x{} fl {:#x}) -> ({},{}) fl {:#x}", h, x, y, cx, cy, fl, nx, ny, f);
    real(h, 0, nx, ny, cx, cy, f)
}

type MessageBoxFn = unsafe extern "system" fn(usize, *const u8, *const u8, u32) -> i32;
unsafe extern "system" fn h_message_box(h: usize, text: *const u8, cap: *const u8, ty: u32) -> i32 {
    if !headless() {
        return orig!(O_MESSAGE_BOX, MessageBoxFn)(h, text, cap, ty);
    }
    // IDOK 1, IDCANCEL 2, IDABORT 3, IDNO 7.
    let ans = match ty & 0xf {
        0 => 1,
        2 => 3,
        4 => 7,
        _ => 2,
    };
    flog!("ui: MessageBoxA('{}': '{}', type {:#x}) suppressed -> {}", show(cstr(cap)), show(cstr(text)), ty, ans);
    ans
}

type DialogBoxFn = unsafe extern "system" fn(usize, usize, usize, usize, isize) -> isize;
unsafe extern "system" fn h_dialog_box(inst: usize, tmpl: usize, parent: usize, proc_: usize, p: isize) -> isize {
    if !headless() {
        return orig!(O_DIALOG_BOX, DialogBoxFn)(inst, tmpl, parent, proc_, p);
    }
    flog!(
        "ui: DialogBoxParamA(template {:#x}) suppressed -> 0 (cancel). The video-device dialog \
         (mdxDDraw.cpp DDrawInitObject) opens when VideoDevice matches no enumerated device; \
         set reg.video_device in f2fix.ini",
        tmpl
    );
    0
}

// ---- DirectX entry points ------------------------------------------------------

type DdCreateExFn = unsafe extern "system" fn(usize, *mut usize, usize, usize) -> i32;
unsafe extern "system" fn h_dd_create_ex(guid: usize, out: *mut usize, iid: usize, outer: usize) -> i32 {
    let hr = orig!(O_DD_CREATE_EX, DdCreateExFn)(guid, out, iid, outer);
    flog!("dd: DirectDrawCreateEx(guid {:#x}) = {:#010x}", guid, hr);
    if hr >= 0 && !out.is_null() && *out != 0 {
        dd::on_ddraw7(*out);
    }
    hr
}

type DsCreateFn = unsafe extern "system" fn(usize, *mut usize, usize) -> i32;
unsafe extern "system" fn h_ds_create(guid: usize, out: *mut usize, outer: usize) -> i32 {
    let ok = audiomute::wait_ready(5000);
    if !ok {
        // Never risk audible sound: the game runs without DirectSound.
        flog!("audio: session mute not confirmed after 5 s; DirectSoundCreate -> DSERR_NODRIVER");
        return 0x8878_0078u32 as i32;
    }
    let hr = orig!(O_DS_CREATE, DsCreateFn)(guid, out, outer);
    flog!("audio: DirectSoundCreate = {:#010x} (session mute {})", hr, if ok { "ready" } else { "NOT confirmed" });
    hr
}

// ---- Bink ---------------------------------------------------------------------

type BinkOpenFn = unsafe extern "system" fn(*const u8, u32) -> usize;

/// `fmv.skip`: `BinkOpen` fails, so video.cpp `StartVideoPlayback` returns 0
/// without playing (and `quitAllVideo` stays 0, so the next FMV is tried and
/// skipped the same way).
unsafe extern "system" fn h_bink_open(path: *const u8, flags: u32) -> usize {
    if cfg().fmv_skip {
        flog!("bink: BinkOpen({}) skipped (fmv.skip)", show(cstr(path)));
        return 0;
    }
    orig!(O_BINK_OPEN, BinkOpenFn)(path, flags)
}
