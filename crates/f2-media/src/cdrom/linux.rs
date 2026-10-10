//! Linux optical drives: `/sys/block/sr*`, `/proc/mounts`, and the `linux/cdrom.h`
//! ioctls. Every `unsafe` block of the Linux path is in this file.

use std::fs::File;
use std::io;
use std::os::raw::{c_int, c_ulong, c_void};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use super::{looks_like_frogger, CdReader, DriveInfo, DriveKind, Toc, TocTrack};

extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

const CDROMREADTOCHDR: c_ulong = 0x5305;
const CDROMREADTOCENTRY: c_ulong = 0x5306;
const CDROMREADAUDIO: c_ulong = 0x530e;
const CDROM_SELECT_SPEED: c_ulong = 0x5322;
const CDROM_DRIVE_STATUS: c_ulong = 0x5326;
const CDS_DISC_OK: c_int = 4;
const CDROM_LBA: u8 = 0x01;
const CDROM_LEADOUT: u8 = 0xaa;
const O_NONBLOCK: i32 = 0o4000;

#[repr(C)]
#[derive(Default)]
struct TocHdr {
    trk0: u8,
    trk1: u8,
}

#[repr(C)]
#[derive(Default)]
struct TocEntry {
    track: u8,
    /// adr (low nibble) / ctrl (high nibble) bitfields.
    adr_ctrl: u8,
    format: u8,
    /// `union cdrom_addr` (LBA form).
    lba: c_int,
    datamode: u8,
}

#[repr(C)]
struct ReadAudio {
    lba: c_int,
    addr_format: u8,
    nframes: c_int,
    buf: *mut u8,
}

pub struct LinuxDrive {
    file: File,
    device: String,
}

/// Safe wrapper: an ioctl whose argument is a pointer to `arg`.
fn ioctl_ptr<T>(f: &File, req: c_ulong, arg: &mut T) -> io::Result<c_int> {
    // SAFETY: `arg` is a live, exclusively borrowed repr(C) value of the type the kernel
    // expects for `req`; the fd stays open for the call.
    let r = unsafe { ioctl(f.as_raw_fd(), req, arg as *mut T as *mut c_void) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

/// Safe wrapper: an ioctl with an integer argument.
fn ioctl_int(f: &File, req: c_ulong, arg: c_int) -> io::Result<c_int> {
    // SAFETY: `req` takes an integer argument by value.
    let r = unsafe { ioctl(f.as_raw_fd(), req, arg as c_ulong) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

fn open_dev(device: &str) -> io::Result<File> {
    std::fs::OpenOptions::new().read(true).custom_flags(O_NONBLOCK).open(device)
}

pub fn open(device: &str) -> io::Result<LinuxDrive> {
    Ok(LinuxDrive { file: open_dev(device)?, device: device.to_string() })
}

impl CdReader for LinuxDrive {
    fn read_toc(&mut self) -> io::Result<Toc> {
        let mut h = TocHdr::default();
        ioctl_ptr(&self.file, CDROMREADTOCHDR, &mut h)?;
        let mut toc = Toc::default();
        for n in (h.trk0..=h.trk1).chain([CDROM_LEADOUT]) {
            let mut e = TocEntry { track: n, format: CDROM_LBA, ..Default::default() };
            ioctl_ptr(&self.file, CDROMREADTOCENTRY, &mut e)?;
            if n == CDROM_LEADOUT {
                toc.leadout = e.lba as u32;
            } else {
                toc.tracks.push(TocTrack { number: n, lba: e.lba as u32, audio: (e.adr_ctrl >> 4) & 0x04 == 0 });
            }
        }
        Ok(toc)
    }

    fn read_cdda(&mut self, lba: u32, count: u32, out: &mut [u8]) -> io::Result<()> {
        let need = count as usize * 2352;
        if out.len() < need {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "buffer too small"));
        }
        let mut ra = ReadAudio { lba: lba as c_int, addr_format: CDROM_LBA, nframes: count as c_int, buf: out.as_mut_ptr() };
        // `ra.buf` points at `need` writable bytes for the duration of the call.
        ioctl_ptr(&self.file, CDROMREADAUDIO, &mut ra).map(|_| ())
    }

    fn max_batch(&self) -> u32 {
        25
    }

    fn slow_down(&mut self) {
        let _ = ioctl_int(&self.file, CDROM_SELECT_SPEED, 4);
    }

    fn describe(&self) -> String {
        self.device.clone()
    }
}

fn mount_point(dev: &Path) -> Option<PathBuf> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    let canon = std::fs::canonicalize(dev).ok()?;
    mounts.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        let (d, m) = (w.next()?, w.next()?);
        (std::fs::canonicalize(d).ok()? == canon).then(|| PathBuf::from(m.replace("\\040", " ")))
    })
}

/// Volume id from the ISO9660 primary volume descriptor (sector 16).
fn volume_label(f: &File) -> Option<String> {
    let mut pvd = [0u8; 2048];
    f.read_exact_at(&mut pvd, 16 * 2048).ok()?;
    (&pvd[1..6] == b"CD001").then(|| String::from_utf8_lossy(&pvd[40..72]).trim().to_string())
}

pub fn list_drives() -> Vec<DriveInfo> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/sys/block") else { return out };
    let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("sr")).collect();
    names.sort();
    for n in names {
        let dev = PathBuf::from(format!("/dev/{n}"));
        let sys = |k: &str| std::fs::read_to_string(format!("/sys/block/{n}/device/{k}")).map(|s| s.trim().to_string()).unwrap_or_default();
        let model = format!("{} {}", sys("vendor"), sys("model")).trim().to_string();
        let file = open_dev(&dev.to_string_lossy()).ok();
        let media = file.as_ref().and_then(|f| ioctl_int(f, CDROM_DRIVE_STATUS, 0).ok()) == Some(CDS_DISC_OK);
        let label = if media { file.as_ref().and_then(volume_label) } else { None };
        let root = mount_point(&dev);
        let frogger = media && looks_like_frogger(root.as_deref(), label.as_deref());
        out.push(DriveInfo { device: dev.to_string_lossy().into_owned(), root, label, media, frogger, kind: DriveKind::Physical, model });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_layouts_match_linux_cdrom_h() {
        assert_eq!(std::mem::size_of::<TocHdr>(), 2);
        assert_eq!(std::mem::size_of::<TocEntry>(), 12);
        assert_eq!(std::mem::offset_of!(TocEntry, lba), 4);
        if cfg!(target_pointer_width = "64") {
            assert_eq!(std::mem::size_of::<ReadAudio>(), 24);
            assert_eq!(std::mem::offset_of!(ReadAudio, buf), 16);
        }
        assert_eq!(std::mem::offset_of!(ReadAudio, nframes), 8);
    }

    #[test]
    fn listing_without_drives_is_empty_or_sane() {
        for d in list_drives() {
            assert!(d.device.starts_with("/dev/sr"));
        }
    }
}
