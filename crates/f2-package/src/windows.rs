//! Export for Windows (docs/EXPORT.md "Windows"): a folder `<out>/<file stem>/` holding
//! `<file stem>.exe` (the running app's `frogger.exe` with its icon and version strings
//! rebranded), the licenses and the game data at `game-data/<rendition>/`, which the exe
//! finds next to itself (`frogger::data::Data::locate`).
//!
//! The exe is edited with `editpe` (pure Rust PE resource editor): every icon group is
//! replaced by one multi-size icon made from the branding icon, and the version info's
//! `ProductName`/`FileDescription` (Explorer's "File description", Task Manager's name) are
//! set to the title. The code is untouched.

use crate::util;
use crate::{Image, Job};
use editpe::constants::{RT_GROUP_ICON, RT_ICON};
use editpe::{ResourceEntryName, VersionInfo, VersionStringTable};
use std::path::{Path, PathBuf};

/// Icon sizes in the exe's icon (Explorer picks the best for each view and DPI).
pub(crate) const ICON_SIZES: [u32; 10] = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

/// Files copied from the runtime folder besides the exe (when present).
const DOCS: [&str; 4] = ["NOTICE", "README.md", "README-windows.txt", "licenses"];

pub(crate) fn export(job: &Job) -> Result<PathBuf, String> {
    let exe_src = runtime_exe(job.runtime)?;
    let stem = job.branding.file_stem();
    let dir = job.out_dir.join(&stem);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    (job.log)(&format!("Branding {}", exe_src.display()));
    let exe = std::fs::read(&exe_src).map_err(|e| format!("{}: {e}", exe_src.display()))?;
    let branded = rebrand_exe(&exe, &job.branding.title, &job.branding.version, &job.branding.icon)?;
    let exe_out = dir.join(format!("{stem}.exe"));
    std::fs::write(&exe_out, branded).map_err(|e| format!("{}: {e}", exe_out.display()))?;

    let runtime_dir = exe_src.parent().unwrap_or(Path::new("."));
    for d in DOCS {
        let p = runtime_dir.join(d);
        if p.exists() {
            copy_tree(&p, &dir.join(d))?;
        }
    }
    copy_data(job, &dir.join("game-data"))?;
    Ok(dir)
}

/// `frogger.exe` in the runtime (the folder holding it, or the exe itself).
fn runtime_exe(runtime: &Path) -> Result<PathBuf, String> {
    if runtime.is_file() {
        return Ok(runtime.to_path_buf());
    }
    let exe = runtime.join("frogger.exe");
    if exe.is_file() {
        Ok(exe)
    } else {
        Err(format!("{} not found (the Windows export is made from the Windows build of this app)", exe.display()))
    }
}

/// `exe` with its icons replaced by `icon` and its version strings naming `title`.
pub(crate) fn rebrand_exe(exe: &[u8], title: &str, version: &str, icon: &Image) -> Result<Vec<u8>, String> {
    let err = |e: &dyn std::fmt::Display| format!("editing the exe's resources: {e}");
    let mut image = editpe::Image::parse(exe).map_err(|e| err(&e))?;
    let mut res = image.resource_directory().cloned().unwrap_or_default();
    // Every icon and icon group goes: the shell uses the first group, which must be ours.
    for t in [RT_ICON, RT_GROUP_ICON] {
        res.root_mut().remove(ResourceEntryName::ID(t as u32));
    }
    let ico = encode_ico(icon, &ICON_SIZES);
    res.set_main_icon(ico.as_slice()).map_err(|e| err(&e))?;

    let mut info = res.get_version_info().map_err(|e| err(&e))?.unwrap_or_else(|| new_version_info(version));
    if info.strings.is_empty() {
        info.strings.push(VersionStringTable { key: "040904B0".into(), strings: Default::default() });
    }
    for table in &mut info.strings {
        for key in ["ProductName", "FileDescription"] {
            table.strings.insert(key.into(), title.into());
        }
    }
    res.set_version_info(&info).map_err(|e| err(&e))?;
    image.set_resource_directory(res).map_err(|e| err(&e))?;
    Ok(image.data().to_vec())
}

/// Version info for an exe that has none (US English, Unicode).
fn new_version_info(version: &str) -> VersionInfo {
    let mut info = VersionInfo::default();
    let mut table = VersionStringTable { key: "040904B0".into(), strings: Default::default() };
    table.strings.insert("ProductVersion".into(), version.into());
    table.strings.insert("FileVersion".into(), version.into());
    info.strings.push(table);
    info.vars.push(editpe::types::VersionU16 { major: 0x0409, minor: 0x04B0 });
    info
}

/// A `.ico` with `icon` at each of `sizes`: 32-bit DIBs below 256 px (what every shell
/// API reads), PNG at 256 px (Vista+).
pub(crate) fn encode_ico(icon: &Image, sizes: &[u32]) -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = sizes
        .iter()
        .map(|&s| {
            let img = util::resize(icon, s, s);
            (s, if s >= 256 { util::png(&img) } else { dib(&img) })
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (s, data) in &images {
        let b = (*s % 256) as u8;
        out.extend_from_slice(&[b, b, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend_from_slice(&data);
    }
    out
}

/// An icon image as a 32-bit BI_RGB DIB: header, BGRA rows bottom-up, empty AND mask.
fn dib(img: &Image) -> Vec<u8> {
    let (w, h) = (img.width, img.height);
    let mask_row = w.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + (w * h * 4 + mask_row * h) as usize);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(2 * h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(w * h * 4 + mask_row * h).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for y in (0..h).rev() {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let p = &img.rgba[i..i + 4];
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    out.resize(out.len() + (mask_row * h) as usize, 0);
    out
}

/// Copies the rendition's game files to `<dest>/<rendition name>/`.
fn copy_data(job: &Job, dest: &Path) -> Result<(), String> {
    let name = job.rendition.file_name().ok_or("the rendition has no name")?;
    let dest = dest.join(name);
    let files = util::rendition_files(job.rendition).map_err(|e| format!("{}: {e}", job.rendition.display()))?;
    let total: u64 = files.iter().filter_map(|(_, p)| p.metadata().ok()).map(|m| m.len()).sum();
    (job.log)(&format!("Copying the game data ({} files, {} MB)", files.len(), total / 1_000_000));
    for (rel, src) in files {
        let to = dest.join(&rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::copy(&src, &to).map_err(|e| format!("{} -> {}: {e}", src.display(), to.display()))?;
    }
    Ok(())
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    if src.is_dir() {
        for (rel, p) in util::walk(src).map_err(|e| format!("{}: {e}", src.display()))? {
            let to = dst.join(rel);
            std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| format!("{}: {e}", to.display()))?;
            std::fs::copy(&p, &to).map_err(|e| format!("{}: {e}", p.display()))?;
        }
    } else {
        std::fs::copy(src, dst).map_err(|e| format!("{}: {e}", src.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid PE32+ exe: headers + one `.text` section (`ret`).
    fn tiny_exe() -> Vec<u8> {
        let mut f = vec![0u8; 0x400];
        f[0..2].copy_from_slice(b"MZ");
        f[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        let pe = 0x80;
        f[pe..pe + 4].copy_from_slice(b"PE\0\0");
        let coff = pe + 4;
        let w16 = |f: &mut Vec<u8>, o: usize, v: u16| f[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let w32 = |f: &mut Vec<u8>, o: usize, v: u32| f[o..o + 4].copy_from_slice(&v.to_le_bytes());
        w16(&mut f, coff, 0x8664);
        w16(&mut f, coff + 2, 1); // sections
        w16(&mut f, coff + 16, 240); // optional header size
        w16(&mut f, coff + 18, 0x22); // executable, large address aware
        let opt = coff + 20;
        w16(&mut f, opt, 0x20b);
        w32(&mut f, opt + 4, 0x200); // SizeOfCode
        w32(&mut f, opt + 16, 0x1000); // entry
        w32(&mut f, opt + 20, 0x1000); // BaseOfCode
        f[opt + 24..opt + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
        w32(&mut f, opt + 32, 0x1000); // SectionAlignment
        w32(&mut f, opt + 36, 0x200); // FileAlignment
        w16(&mut f, opt + 40, 6); // OS version
        w16(&mut f, opt + 48, 6); // subsystem version
        w32(&mut f, opt + 56, 0x2000); // SizeOfImage
        w32(&mut f, opt + 60, 0x400); // SizeOfHeaders
        w16(&mut f, opt + 68, 2); // GUI
        w16(&mut f, opt + 70, 0x8160);
        f[opt + 72..opt + 80].copy_from_slice(&0x10_0000u64.to_le_bytes());
        f[opt + 80..opt + 88].copy_from_slice(&0x1000u64.to_le_bytes());
        f[opt + 88..opt + 96].copy_from_slice(&0x10_0000u64.to_le_bytes());
        f[opt + 96..opt + 104].copy_from_slice(&0x1000u64.to_le_bytes());
        w32(&mut f, opt + 108, 16); // NumberOfRvaAndSizes
        let sec = opt + 240;
        f[sec..sec + 8].copy_from_slice(b".text\0\0\0");
        w32(&mut f, sec + 8, 1);
        w32(&mut f, sec + 12, 0x1000);
        w32(&mut f, sec + 16, 0x200);
        w32(&mut f, sec + 20, 0x200);
        w32(&mut f, sec + 36, 0x6000_0020);
        f[0x200] = 0xc3;
        f.truncate(0x400);
        f
    }

    fn icon() -> Image {
        let mut rgba = Vec::new();
        for y in 0..64u32 {
            for x in 0..64u32 {
                rgba.extend_from_slice(&[x as u8 * 4, y as u8 * 4, 128, 255]);
            }
        }
        Image { width: 64, height: 64, rgba }
    }

    #[test]
    fn ico_layout() {
        let ico = encode_ico(&icon(), &[16, 256]);
        assert_eq!(&ico[0..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(ico[6], 16);
        assert_eq!(ico[22], 0, "256 is stored as 0");
        let off = u32::from_le_bytes(ico[6 + 12..6 + 16].try_into().unwrap()) as usize;
        assert_eq!(u32::from_le_bytes(ico[off..off + 4].try_into().unwrap()), 40, "DIB header");
        let off = u32::from_le_bytes(ico[22 + 12..22 + 16].try_into().unwrap()) as usize;
        assert_eq!(&ico[off + 1..off + 4], b"PNG");
    }

    #[test]
    fn rebrands_an_exe() {
        let exe = tiny_exe();
        editpe::Image::parse(exe.as_slice()).expect("test exe parses");
        let out = rebrand_exe(&exe, "Frogger 2: Swampy's Revenge", "0.1.0", &icon()).unwrap();
        let img = editpe::Image::parse(out.as_slice()).unwrap();
        let res = img.resource_directory().expect("resources");
        let main = res.get_main_icon().unwrap().expect("icon");
        assert!(!main.is_empty());
        let groups = res.root().get(ResourceEntryName::ID(RT_GROUP_ICON as u32)).unwrap().as_table().unwrap();
        assert_eq!(groups.entries().len(), 1);
        let info = res.get_version_info().unwrap().unwrap();
        assert_eq!(info.strings[0].strings["ProductName"], "Frogger 2: Swampy's Revenge");
        assert_eq!(info.strings[0].strings["FileDescription"], "Frogger 2: Swampy's Revenge");
        // Rebranding a rebranded exe replaces, not adds.
        let again = rebrand_exe(&out, "X", "0.1.0", &icon()).unwrap();
        let img = editpe::Image::parse(again.as_slice()).unwrap();
        let res = img.resource_directory().unwrap();
        let icons = res.root().get(ResourceEntryName::ID(RT_ICON as u32)).unwrap().as_table().unwrap();
        assert_eq!(icons.entries().len(), ICON_SIZES.len());
        assert_eq!(res.get_version_info().unwrap().unwrap().strings[0].strings["ProductName"], "X");
    }

    #[test]
    fn exports_a_folder() {
        let tmp = std::env::temp_dir().join(format!("f2pkg-win-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let rt = tmp.join("rt");
        let rend = tmp.join("data/usa");
        std::fs::create_dir_all(rt.join("licenses")).unwrap();
        std::fs::write(rt.join("frogger.exe"), tiny_exe()).unwrap();
        std::fs::write(rt.join("licenses/LICENSE-MIT"), "mit").unwrap();
        std::fs::write(rt.join("NOTICE"), "n").unwrap();
        std::fs::create_dir_all(rend.join("game/maps")).unwrap();
        std::fs::create_dir_all(rend.join("oracle")).unwrap();
        std::fs::write(rend.join("game/maps/a.fla"), "a").unwrap();
        std::fs::write(rend.join("manifest.json"), "{}").unwrap();
        std::fs::write(rend.join("oracle/frogger2.exe"), "x").unwrap();
        let b = crate::Branding {
            title: "Frogger 2: Swampy's Revenge".into(),
            short_title: "Frogger 2".into(),
            art: icon(),
            icon: icon(),
            version: "0.1.0".into(),
        };
        let out = tmp.join("out");
        let job = Job { target: crate::Target::Windows, rendition: &rend, runtime: &rt, out_dir: &out, branding: &b, keys_dir: None, log: &|_| {} };
        let dir = crate::export(&job).unwrap();
        assert_eq!(dir, out.join("Frogger 2 - Swampy's Revenge"));
        assert!(dir.join("Frogger 2 - Swampy's Revenge.exe").is_file());
        assert!(dir.join("licenses/LICENSE-MIT").is_file());
        assert!(dir.join("NOTICE").is_file());
        assert!(dir.join("game-data/usa/game/maps/a.fla").is_file());
        assert!(dir.join("game-data/usa/manifest.json").is_file());
        assert!(!dir.join("game-data/usa/oracle").exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
