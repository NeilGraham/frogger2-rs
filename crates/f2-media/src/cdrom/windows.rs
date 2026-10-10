//! Windows optical drives: drive letters, `IOCTL_CDROM_READ_TOC`, `IOCTL_CDROM_RAW_READ`
//! (CDDA), `IOCTL_STORAGE_QUERY_PROPERTY` (model / virtual bus) on `\\.\X:`. Minimal
//! hand-written kernel32 FFI; every `unsafe` block of the Windows path is in this file.
//! The image behind a virtual drive (Explorer "Mount") comes from
//! `Get-Volume -DriveLetter X | Get-DiskImage` (PowerShell, no console window).

use std::ffi::c_void;
use std::io;
use std::path::PathBuf;

use super::{looks_like_frogger, CdReader, DriveInfo, DriveKind, Toc};

type Handle = *mut c_void;
type Bool = i32;

#[link(name = "kernel32")]
extern "system" {
    fn GetLogicalDrives() -> u32;
    fn GetDriveTypeW(root: *const u16) -> u32;
    fn GetVolumeInformationW(
        root: *const u16,
        name: *mut u16,
        name_len: u32,
        serial: *mut u32,
        max_comp: *mut u32,
        flags: *mut u32,
        fs: *mut u16,
        fs_len: u32,
    ) -> Bool;
    fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *mut c_void, disposition: u32, flags: u32, template: Handle) -> Handle;
    fn DeviceIoControl(h: Handle, code: u32, inb: *const c_void, in_len: u32, outb: *mut c_void, out_len: u32, returned: *mut u32, ov: *mut c_void) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn SetErrorMode(mode: u32) -> u32;
}

const DRIVE_CDROM: u32 = 5;
const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_READ_WRITE: u32 = 3;
const OPEN_EXISTING: u32 = 3;
const SEM_FAILCRITICALERRORS: u32 = 1;
const IOCTL_CDROM_READ_TOC: u32 = 0x0002_4000;
const IOCTL_CDROM_RAW_READ: u32 = 0x0002_403e;
const IOCTL_CDROM_SET_SPEED: u32 = 0x0002_4060;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002d_1400;
const IOCTL_STORAGE_CHECK_VERIFY2: u32 = 0x002d_0800;
const CDDA: u32 = 2;
const BUS_TYPE_VIRTUAL: u32 = 0x0e;
const BUS_TYPE_FILE_BACKED_VIRTUAL: u32 = 0x0f;

#[repr(C)]
struct RawReadInfo {
    disk_offset: i64,
    sector_count: u32,
    track_mode: u32,
}

#[repr(C)]
struct SetSpeed {
    request_type: u32,
    read_speed: u16,
    write_speed: u16,
    rotation: u32,
}

#[repr(C)]
struct PropertyQuery {
    property_id: u32,
    query_type: u32,
    additional: [u8; 4],
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// An open `\\.\X:` handle, closed on drop.
pub struct WinDrive {
    h: Handle,
    device: String,
}

// SAFETY: the handle is only used through &mut self / &self methods doing synchronous
// DeviceIoControl calls; Win32 handles may be used from any thread.
unsafe impl Send for WinDrive {}

impl Drop for WinDrive {
    fn drop(&mut self) {
        // SAFETY: `h` is a valid handle owned by this value.
        unsafe { CloseHandle(self.h) };
    }
}

/// `D`, `D:`, `D:\`, `\\.\D:` → `D`.
fn letter(device: &str) -> Option<char> {
    let s = device.trim_start_matches("\\\\.\\").trim_end_matches(['\\', '/']);
    let mut c = s.chars();
    let l = c.next()?.to_ascii_uppercase();
    (l.is_ascii_alphabetic() && matches!(c.as_str(), "" | ":")).then_some(l)
}

pub fn open(device: &str) -> io::Result<WinDrive> {
    let l = letter(device).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("not a drive letter: {device}")))?;
    let path = wide(&format!("\\\\.\\{l}:"));
    // SAFETY: `path` is a NUL-terminated UTF-16 string; other pointers are null as allowed.
    let h = unsafe { CreateFileW(path.as_ptr(), GENERIC_READ, FILE_SHARE_READ_WRITE, std::ptr::null_mut(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if h as isize == -1 || h.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(WinDrive { h, device: format!("{l}:") })
}

impl WinDrive {
    /// Safe wrapper around `DeviceIoControl` with byte buffers. Returns bytes written.
    fn ioctl(&self, code: u32, input: &[u8], output: &mut [u8]) -> io::Result<usize> {
        let mut ret = 0u32;
        // SAFETY: the buffers are valid for the given lengths for the duration of the
        // synchronous call; `ret` is a valid out pointer; no OVERLAPPED.
        let ok = unsafe {
            DeviceIoControl(
                self.h,
                code,
                if input.is_empty() { std::ptr::null() } else { input.as_ptr() as *const c_void },
                input.len() as u32,
                if output.is_empty() { std::ptr::null_mut() } else { output.as_mut_ptr() as *mut c_void },
                output.len() as u32,
                &mut ret,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret as usize)
        }
    }

    fn media_present(&self) -> bool {
        self.ioctl(IOCTL_STORAGE_CHECK_VERIFY2, &[], &mut []).is_ok()
    }

    /// (vendor + product, virtual bus?)
    fn model(&self) -> (String, bool) {
        let q = PropertyQuery { property_id: 0, query_type: 0, additional: [0; 4] };
        let qb = as_bytes(&q);
        let mut out = vec![0u8; 1024];
        let Ok(n) = self.ioctl(IOCTL_STORAGE_QUERY_PROPERTY, qb, &mut out) else { return (String::new(), false) };
        let out = &out[..n];
        let u32_at = |o: usize| out.get(o..o + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
        let s_at = |o: u32| -> String {
            let o = o as usize;
            if o == 0 || o >= out.len() {
                return String::new();
            }
            String::from_utf8_lossy(out[o..].split(|&c| c == 0).next().unwrap_or(&[])).trim().to_string()
        };
        let model = format!("{} {}", s_at(u32_at(12)), s_at(u32_at(16))).trim().to_string();
        let bus = u32_at(28);
        let virt = bus == BUS_TYPE_VIRTUAL || bus == BUS_TYPE_FILE_BACKED_VIRTUAL || model.to_ascii_lowercase().contains("virtual");
        (model, virt)
    }
}

fn as_bytes<T>(v: &T) -> &[u8] {
    // SAFETY: `T` is a plain repr(C) struct without padding-sensitive invariants; the slice
    // borrows `v` for its lifetime and is only read.
    unsafe { std::slice::from_raw_parts(v as *const T as *const u8, std::mem::size_of::<T>()) }
}

impl CdReader for WinDrive {
    fn read_toc(&mut self) -> io::Result<Toc> {
        let mut buf = vec![0u8; 804];
        let n = self.ioctl(IOCTL_CDROM_READ_TOC, &[], &mut buf)?;
        Toc::from_mmc(&buf[..n.max(4)], true)
    }

    fn read_cdda(&mut self, lba: u32, count: u32, out: &mut [u8]) -> io::Result<()> {
        let need = count as usize * 2352;
        if out.len() < need {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "buffer too small"));
        }
        let info = RawReadInfo { disk_offset: i64::from(lba) * 2048, sector_count: count, track_mode: CDDA };
        let n = self.ioctl(IOCTL_CDROM_RAW_READ, as_bytes(&info), &mut out[..need])?;
        if n < need {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("short raw read at {lba}: {n} of {need} bytes")));
        }
        Ok(())
    }

    fn max_batch(&self) -> u32 {
        20
    }

    fn slow_down(&mut self) {
        // 4x (706 KB/s); not every drive honours it.
        let s = SetSpeed { request_type: 0, read_speed: 706, write_speed: 0xffff, rotation: 0 };
        let _ = self.ioctl(IOCTL_CDROM_SET_SPEED, as_bytes(&s), &mut []);
    }

    fn describe(&self) -> String {
        self.device.clone()
    }
}

fn volume_label(root: &str) -> Option<String> {
    let r = wide(root);
    let mut name = [0u16; 261];
    // SAFETY: `r` is NUL-terminated; `name` is writable for its stated length; the other
    // optional out pointers are null.
    let ok = unsafe {
        GetVolumeInformationW(
            r.as_ptr(),
            name.as_mut_ptr(),
            name.len() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    if ok == 0 {
        return None;
    }
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..end]))
}

/// The image file behind a mounted virtual drive (Explorer "Mount" / `Mount-DiskImage`).
pub fn backing_image(letter: char) -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let cmd = format!("(Get-Volume -DriveLetter {letter} -ErrorAction Stop | Get-DiskImage -ErrorAction Stop).ImagePath");
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &cmd])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let p = PathBuf::from(s.lines().next()?.trim());
    (out.status.success() && p.is_file()).then_some(p)
}

pub fn list_drives() -> Vec<DriveInfo> {
    // Don't pop up "no disk in drive" dialogs.
    // SAFETY: plain flag setter.
    unsafe { SetErrorMode(SEM_FAILCRITICALERRORS) };
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let l = (b'A' + i as u8) as char;
        let root = format!("{l}:\\");
        let rw = wide(&root);
        // SAFETY: NUL-terminated root path.
        if unsafe { GetDriveTypeW(rw.as_ptr()) } != DRIVE_CDROM {
            continue;
        }
        let dev = open(&format!("{l}:")).ok();
        let (model, virt) = dev.as_ref().map(|d| d.model()).unwrap_or_default();
        let media = dev.as_ref().is_some_and(|d| d.media_present());
        let label = if media { volume_label(&root) } else { None };
        let rootp = PathBuf::from(&root);
        let frogger = media && looks_like_frogger(Some(&rootp), label.as_deref());
        let kind = if virt { DriveKind::Virtual { image: if media { backing_image(l) } else { None } } } else { DriveKind::Physical };
        out.push(DriveInfo { device: format!("{l}:"), root: media.then_some(rootp), label, media, frogger, kind, model });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_and_letters() {
        assert_eq!(std::mem::size_of::<RawReadInfo>(), 16);
        assert_eq!(std::mem::size_of::<SetSpeed>(), 12);
        assert_eq!(std::mem::size_of::<PropertyQuery>(), 12);
        assert_eq!(letter("d:"), Some('D'));
        assert_eq!(letter("\\\\.\\E:"), Some('E'));
        assert_eq!(letter("F:\\"), Some('F'));
        assert_eq!(letter("C:\\x"), None);
    }

    #[test]
    fn list_does_not_panic() {
        for d in list_drives() {
            eprintln!("{}", d.summary());
        }
    }
}
