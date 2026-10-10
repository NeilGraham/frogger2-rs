//! Windows only: attaches a Frogger 2 `.iso` as a virtual DVD drive (the API behind
//! Explorer's "Mount" / `Mount-DiskImage`), then checks drive enumeration, the IOCTL
//! TOC/raw-read path and both drive import paths (via the backing image, and directly
//! from the drive's file system). The image is detached when the test ends.
//!
//! `F2_TEST_ISO=C:\path\FROGGER2.iso cargo test --release -p f2-import --test windows_mount -- --ignored --nocapture`
#![cfg(windows)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use f2_import::{import_opened, open_source, Options, Source};
use f2_media::cdrom::{self, DriveKind};

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);
#[repr(C)]
struct StorageType {
    device_id: u32,
    vendor: Guid,
}
#[repr(C)]
struct OpenParams {
    version: u32,
    rw_depth: u32,
}
#[repr(C)]
struct AttachParams {
    version: u32,
    reserved: u32,
}

#[link(name = "virtdisk")]
extern "system" {
    fn OpenVirtualDisk(t: *const StorageType, path: *const u16, access: u32, flags: u32, params: *const OpenParams, h: *mut *mut c_void) -> u32;
    fn AttachVirtualDisk(h: *mut c_void, sd: *mut c_void, flags: u32, provider_flags: u32, params: *const AttachParams, ov: *mut c_void) -> u32;
    fn DetachVirtualDisk(h: *mut c_void, flags: u32, provider_flags: u32) -> u32;
}
#[link(name = "kernel32")]
extern "system" {
    fn CloseHandle(h: *mut c_void) -> i32;
}

/// Attached ISO; detached + closed on drop.
struct Mounted(*mut c_void);

impl Mounted {
    fn attach(iso: &std::path::Path) -> Result<Mounted, u32> {
        let path: Vec<u16> = iso.as_os_str().to_string_lossy().encode_utf16().chain([0]).collect();
        // VIRTUAL_STORAGE_TYPE_DEVICE_ISO, VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT.
        let t = StorageType { device_id: 1, vendor: Guid(0xec984aec, 0xa0f9, 0x47e9, [0x90, 0x1f, 0x71, 0x41, 0x5a, 0x66, 0x34, 0x5b]) };
        let op = OpenParams { version: 1, rw_depth: 0 };
        let mut h = std::ptr::null_mut();
        // SAFETY: valid pointers for the duration of the call; VIRTUAL_DISK_ACCESS_READ, OPEN_VIRTUAL_DISK_FLAG_NONE.
        let r = unsafe { OpenVirtualDisk(&t, path.as_ptr(), 0x000d_0000, 0, &op, &mut h) };
        if r != 0 {
            return Err(r);
        }
        let ap = AttachParams { version: 1, reserved: 0 };
        // SAFETY: `h` is an open virtual disk handle; ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY.
        let r = unsafe { AttachVirtualDisk(h, std::ptr::null_mut(), 1, 0, &ap, std::ptr::null_mut()) };
        if r != 0 {
            // SAFETY: `h` is open.
            unsafe { CloseHandle(h) };
            return Err(r);
        }
        Ok(Mounted(h))
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        // SAFETY: `self.0` is an attached virtual disk handle.
        unsafe {
            DetachVirtualDisk(self.0, 0, 0);
            CloseHandle(self.0);
        }
    }
}

#[test]
#[ignore = "attaches an ISO as a virtual drive; needs F2_TEST_ISO"]
fn mounted_iso_imports() {
    let Some(iso) = std::env::var_os("F2_TEST_ISO").map(PathBuf::from) else {
        eprintln!("skipping: set F2_TEST_ISO");
        return;
    };
    let before: Vec<String> = cdrom::list_drives().into_iter().map(|d| d.device).collect();
    let _m = Mounted::attach(&iso).unwrap_or_else(|e| panic!("AttachVirtualDisk failed: Win32 error {e}"));
    let t0 = Instant::now();
    let drive = loop {
        if let Some(d) = cdrom::list_drives().into_iter().find(|d| d.frogger && !before.contains(&d.device)) {
            break d;
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "mounted drive did not appear");
        std::thread::sleep(Duration::from_millis(500));
    };
    eprintln!("mounted: {}", drive.summary());
    assert_eq!(drive.label.as_deref(), Some("FROGGER2"));
    assert!(matches!(drive.kind, DriveKind::Virtual { .. }), "virtual drive expected");
    let image = drive.image().cloned();
    eprintln!("backing image: {image:?}");

    // IOCTL path: TOC of a data-only disc; CD-DA reads of a data sector must fail cleanly.
    let mut r = cdrom::open_drive(&drive.device).expect("open drive");
    let toc = r.read_toc().expect("IOCTL_CDROM_READ_TOC");
    eprintln!("TOC: {toc:?}");
    assert_eq!(toc.tracks.len(), 1);
    assert!(!toc.tracks[0].audio);
    assert!(toc.leadout > 90_000);
    let mut buf = vec![0u8; 2352 * 2];
    let raw = r.read_cdda(16, 2, &mut buf);
    eprintln!("IOCTL_CDROM_RAW_READ (CDDA) on a data sector: {raw:?}");
    drop(r);

    let tmp = std::env::temp_dir().join(format!("f2-winmount-{}", std::process::id()));
    // 1. As the importer does it: a virtual drive imports from its backing image when found.
    let opened = open_source(Source::Drive(drive.clone()), &drive.device).unwrap();
    eprintln!("source kind via drive: {}", opened.kind);
    let m1 = import_opened(opened, &tmp.join("a"), &Options::default()).unwrap();
    // 2. Directly from the drive's file system (as for a physical drive).
    let mut direct = drive.clone();
    direct.kind = DriveKind::Physical;
    let opened = open_source(Source::Drive(direct), &drive.device).unwrap();
    assert_eq!(opened.kind, "drive");
    let m2 = import_opened(opened, &tmp.join("b"), &Options::default()).unwrap();
    for m in [&m1, &m2] {
        assert_eq!(m.rendition, "usa");
        assert!(m.verified);
        assert_eq!(m.videos.len(), 14);
        assert!(m.music_tracks.is_empty());
        assert!(m.music_note.is_some());
    }
    assert_eq!(m1.files, m2.files, "image and drive file-system imports differ");
    if image.is_some() {
        assert_eq!(m1.source_kind, "iso");
    }
    eprintln!("PASS mounted ISO: {} files, {} videos, music: {} ({})", m1.files.len(), m1.videos.len(), m1.music_source, m1.music_note.as_deref().unwrap_or(""));
    std::fs::remove_dir_all(&tmp).ok();
}
