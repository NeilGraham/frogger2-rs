//! Export for Linux (docs/EXPORT.md "Linux").
//!
//! - Running from an AppImage (the runtime is the `.AppImage` file): `<out>/<file stem>.AppImage`,
//!   the same AppImage runtime (the ELF stub before the squashfs, its update information and
//!   signature cleared) followed by a new squashfs (pure Rust, `backhand`, gzip: the type 2
//!   runtime reads zlib, zstd and xz) holding the old AppDir with its `.desktop` file and
//!   icons replaced by the branded ones and the game data at `usr/share/game-data/<rendition>/`
//!   (found from `$APPDIR`/the exe by `frogger::data::Data::locate`).
//! - Running from a plain folder (tarball, developer build): `<out>/<file stem>/` with the
//!   `frogger` binary, the licenses, a `.desktop` file, the icon and `game-data/<rendition>/`.

use crate::util;
use crate::{Branding, Job};
use backhand::compression::{CompressionAction, CompressionOptions, Compressor, DefaultCompressor, Gzip};
use backhand::kind::Kind;
use backhand::{BackhandError, FilesystemCompressor, FilesystemReader, FilesystemWriter, InnerNode, NodeHeader, SuperBlock};
use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Files copied from a runtime folder besides the binary (when present).
const DOCS: [&str; 3] = ["NOTICE", "README.md", "licenses"];

pub(crate) fn export(job: &Job) -> Result<PathBuf, String> {
    if job.runtime.is_file() {
        appimage(job)
    } else {
        folder(job)
    }
}

/// Desktop/icon id of the exported app: "frogger-2-swampys-revenge".
pub(crate) fn app_id(title: &str) -> String {
    let mut id = String::new();
    for c in title.chars().filter(|c| !matches!(c, '\'' | '\u{2019}')) {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    id.trim_end_matches('-').to_string()
}

/// The `.desktop` entry of the exported app.
pub(crate) fn desktop_entry(b: &Branding, id: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={title}\n\
         Comment={title}, on the Swampy's Revenge Recomp engine (your own copy of the game)\n\
         Exec=frogger\n\
         Icon={id}\n\
         Categories=Game;ArcadeGame;\n\
         Terminal=false\n\
         StartupNotify=true\n\
         StartupWMClass=frogger\n\
         X-AppImage-Version={version}\n",
        title = b.title.replace('\n', " "),
        version = b.version,
    )
}

// ---------------------------------------------------------------------------------------
// Folder

fn folder(job: &Job) -> Result<PathBuf, String> {
    let bin = job.runtime.join("frogger");
    if !bin.is_file() {
        return Err(format!("{} not found (the Linux export is made from the Linux build of this app)", bin.display()));
    }
    let dir = job.out_dir.join(job.branding.file_stem());
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let id = app_id(&job.branding.title);
    std::fs::copy(&bin, dir.join("frogger")).map_err(|e| format!("{}: {e}", bin.display()))?;
    for d in DOCS {
        let p = job.runtime.join(d);
        if p.is_dir() {
            for (rel, f) in util::walk(&p).map_err(|e| format!("{}: {e}", p.display()))? {
                let to = dir.join(d).join(rel);
                std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| format!("{}: {e}", to.display()))?;
                std::fs::copy(&f, &to).map_err(|e| format!("{}: {e}", f.display()))?;
            }
        } else if p.is_file() {
            std::fs::copy(&p, dir.join(d)).map_err(|e| format!("{}: {e}", p.display()))?;
        }
    }
    std::fs::write(dir.join(format!("{id}.desktop")), desktop_entry(job.branding, &id)).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{id}.png")), util::png(&util::resize(&job.branding.icon, 256, 256))).map_err(|e| e.to_string())?;

    let name = job.rendition.file_name().ok_or("the rendition has no name")?;
    let files = util::rendition_files(job.rendition).map_err(|e| format!("{}: {e}", job.rendition.display()))?;
    log_data(job, &files);
    for (rel, src) in files {
        let to = dir.join("game-data").join(name).join(&rel);
        std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| format!("{}: {e}", to.display()))?;
        std::fs::copy(&src, &to).map_err(|e| format!("{} -> {}: {e}", src.display(), to.display()))?;
    }
    Ok(dir)
}

fn log_data(job: &Job, files: &[(String, PathBuf)]) {
    let total: u64 = files.iter().filter_map(|(_, p)| p.metadata().ok()).map(|m| m.len()).sum();
    (job.log)(&format!("Adding the game data ({} files, {} MB)", files.len(), total / 1_000_000));
}

// ---------------------------------------------------------------------------------------
// AppImage

/// Reads the squashfs of the source AppImage (zstd through `ruzstd`, the rest through
/// backhand's pure-Rust codecs) and writes gzip.
struct Codec;
static CODEC: Codec = Codec;

impl CompressionAction for Codec {
    type Error = BackhandError;
    type Compressor = Compressor;
    type FilesystemCompressor = FilesystemCompressor;
    type SuperBlock = SuperBlock;

    fn decompress(&self, bytes: &[u8], out: &mut Vec<u8>, c: Compressor) -> Result<(), BackhandError> {
        if c == Compressor::Zstd {
            let mut src = bytes;
            let mut dec = ruzstd::decoding::StreamingDecoder::new(&mut src).map_err(|_| BackhandError::CorruptedOrInvalidSquashfs)?;
            dec.read_to_end(out).map_err(|_| BackhandError::CorruptedOrInvalidSquashfs)?;
            return Ok(());
        }
        DefaultCompressor.decompress(bytes, out, c)
    }

    fn compress(&self, bytes: &[u8], fc: FilesystemCompressor, block_size: u32) -> Result<Vec<u8>, BackhandError> {
        DefaultCompressor.compress(bytes, fc, block_size)
    }

    fn compression_options(&self, sb: &mut SuperBlock, kind: &Kind, fc: FilesystemCompressor) -> Result<Option<Vec<u8>>, BackhandError> {
        DefaultCompressor.compression_options(sb, kind, fc)
    }
}

/// An ELF file's size from its header (the end of the section header table, which the
/// AppImage runtime places last): where an AppImage's squashfs starts.
pub(crate) fn elf_size(elf: &[u8]) -> Option<u64> {
    let h = ElfHeader::parse(elf)?;
    Some(h.shoff + h.shentsize as u64 * h.shnum as u64)
}

struct ElfHeader {
    is64: bool,
    shoff: u64,
    shentsize: u16,
    shnum: u16,
    shstrndx: u16,
}

impl ElfHeader {
    fn parse(b: &[u8]) -> Option<ElfHeader> {
        if b.len() < 0x40 || &b[0..4] != b"\x7fELF" || b[5] != 1 {
            return None;
        }
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        match b[4] {
            1 => Some(ElfHeader { is64: false, shoff: u32_at(0x20) as u64, shentsize: u16_at(0x2e), shnum: u16_at(0x30), shstrndx: u16_at(0x32) }),
            2 => Some(ElfHeader { is64: true, shoff: u64_at(0x28), shentsize: u16_at(0x3a), shnum: u16_at(0x3c), shstrndx: u16_at(0x3e) }),
            _ => None,
        }
    }

    /// (name, file offset, size) of each section.
    fn sections(&self, b: &[u8]) -> Vec<(String, usize, usize)> {
        let sh = |i: u16| -> Option<(u32, usize, usize)> {
            let o = (self.shoff + i as u64 * self.shentsize as u64) as usize;
            let e = b.get(o..o + self.shentsize as usize)?;
            let name = u32::from_le_bytes(e[0..4].try_into().unwrap());
            Some(if self.is64 {
                (name, u64::from_le_bytes(e[0x18..0x20].try_into().unwrap()) as usize, u64::from_le_bytes(e[0x20..0x28].try_into().unwrap()) as usize)
            } else {
                (name, u32::from_le_bytes(e[0x10..0x14].try_into().unwrap()) as usize, u32::from_le_bytes(e[0x14..0x18].try_into().unwrap()) as usize)
            })
        };
        let Some((_, stroff, strsize)) = sh(self.shstrndx) else { return Vec::new() };
        let Some(strtab) = b.get(stroff..stroff + strsize) else { return Vec::new() };
        (0..self.shnum)
            .filter_map(sh)
            .map(|(n, off, size)| {
                let name =
                    strtab.get(n as usize..).and_then(|s| s.split(|&c| c == 0).next()).map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default();
                (name, off, size)
            })
            .collect()
    }
}

/// The AppImage runtime with its update information (which would update the export to the
/// plain recomp release) and its signature (of the old payload) blanked.
fn clean_runtime(mut stub: Vec<u8>) -> Vec<u8> {
    if let Some(h) = ElfHeader::parse(&stub) {
        for (name, off, size) in h.sections(&stub) {
            if matches!(name.as_str(), ".upd_info" | ".sha256_sig" | ".sig_key") && off + size <= stub.len() {
                stub[off..off + size].fill(0);
            }
        }
    }
    stub
}

/// Paths of the source AppDir that the export replaces (relative, `/`-separated).
fn replaced(rel: &str, old_icon: Option<&str>) -> bool {
    let top = !rel.contains('/');
    let stem = rel.rsplit_once('.').map_or(rel, |(s, _)| s);
    (top && (rel.ends_with(".desktop") || rel == ".DirIcon" || old_icon.is_some_and(|i| stem == i && rel != stem)))
        || rel == "usr/share/game-data"
        || rel.starts_with("usr/share/game-data/")
        || rel == "usr/share/icons"
        || rel.starts_with("usr/share/icons/")
        || rel.starts_with("usr/share/applications/")
        || rel.starts_with("usr/share/metainfo/")
}

fn appimage(job: &Job) -> Result<PathBuf, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", job.runtime.display());
    let mut file = std::fs::File::open(job.runtime).map_err(|e| err(&e))?;
    let mut head = vec![0u8; 0x40];
    file.read_exact(&mut head).map_err(|e| err(&e))?;
    let offset = elf_size(&head).ok_or_else(|| err(&"not an AppImage (no ELF header)"))?;
    let mut stub = vec![0u8; offset as usize];
    file.seek(SeekFrom::Start(0)).map_err(|e| err(&e))?;
    file.read_exact(&mut stub).map_err(|e| err(&e))?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).map_err(|e| err(&e))?;
    if &magic != b"hsqs" {
        return Err(err(&"not a type 2 AppImage (no squashfs after the runtime)"));
    }
    let stub = clean_runtime(stub);

    (job.log)(&format!("Reading {}", job.runtime.display()));
    let src = std::io::BufReader::new(file);
    let reader = FilesystemReader::from_reader_with_offset_and_kind(src, offset, Kind::new_v4(&CODEC)).map_err(|e| err(&e))?;

    // The old desktop entry's icon name (its files at the AppDir root are replaced).
    let mut old_icon = None;
    for node in reader.files() {
        let rel = rel_path(&node.fullpath);
        if !rel.contains('/') && rel.ends_with(".desktop") {
            if let InnerNode::File(f) = &node.inner {
                let mut text = String::new();
                let _ = reader.file(f).reader().read_to_string(&mut text);
                old_icon = text.lines().find_map(|l| l.strip_prefix("Icon=")).map(|s| s.trim().to_string());
            }
        }
    }

    let mut w = FilesystemWriter::default();
    w.set_kind(Kind::new_v4(&CODEC));
    let gzip = Gzip { compression_level: 6, window_size: 15, strategies: 0 };
    w.set_compressor(FilesystemCompressor::new(Compressor::Gzip, Some(CompressionOptions::Gzip(gzip))).map_err(|e| err(&e))?);
    // The level is only for the encoder: backhand stores gzip options in a layout that
    // squashfs-tools rejects (and the defaults need none).
    w.set_emit_compression_options(false);
    w.set_block_size(reader.block_size);
    w.set_time(reader.mod_time);
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    let root = reader.files().find(|n| rel_path(&n.fullpath).is_empty()).map(|n| n.header);
    if let Some(h) = root {
        w.set_root_mode(h.permissions);
    }
    for node in reader.files() {
        let rel = rel_path(&node.fullpath);
        if rel.is_empty() || replaced(&rel, old_icon.as_deref()) {
            continue;
        }
        let res = match &node.inner {
            InnerNode::Dir(_) => {
                dirs.insert(rel.clone());
                w.push_dir(&rel, node.header)
            }
            InnerNode::File(f) => w.push_file(reader.file(f).reader(), &rel, node.header),
            InnerNode::Symlink(s) => w.push_symlink(&s.link, &rel, node.header),
            _ => Ok(()), // devices, pipes, sockets: none in an AppDir
        };
        res.map_err(|e| format!("{rel}: {e}"))?;
    }

    let id = app_id(&job.branding.title);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32);
    let file_hdr = NodeHeader::new(0o644, 0, 0, now);
    let dir_hdr = NodeHeader::new(0o755, 0, 0, now);
    let mut add = |w: &mut FilesystemWriter, rel: String, data: Vec<u8>| -> Result<(), String> {
        ensure_parent(w, &mut dirs, &rel, dir_hdr)?;
        w.push_file(std::io::Cursor::new(data), &rel, file_hdr).map_err(|e| format!("{rel}: {e}"))
    };
    let desktop = desktop_entry(job.branding, &id).into_bytes();
    let png256 = util::png(&util::resize(&job.branding.icon, 256, 256));
    let png512 = util::png(&util::resize(&job.branding.icon, 512, 512));
    add(&mut w, format!("{id}.desktop"), desktop.clone())?;
    add(&mut w, format!("usr/share/applications/{id}.desktop"), desktop)?;
    add(&mut w, format!("{id}.png"), png256.clone())?;
    add(&mut w, ".DirIcon".into(), png256.clone())?;
    add(&mut w, format!("usr/share/icons/hicolor/256x256/apps/{id}.png"), png256)?;
    add(&mut w, format!("usr/share/icons/hicolor/512x512/apps/{id}.png"), png512)?;

    let name = job.rendition.file_name().ok_or("the rendition has no name")?.to_string_lossy().into_owned();
    let files = util::rendition_files(job.rendition).map_err(|e| format!("{}: {e}", job.rendition.display()))?;
    log_data(job, &files);
    for (rel, src) in files {
        let rel = format!("usr/share/game-data/{name}/{rel}");
        ensure_parent(&mut w, &mut dirs, &rel, dir_hdr)?;
        w.push_file_from_path(src, &rel, file_hdr).map_err(|e| format!("{rel}: {e}"))?;
    }

    let out = job.out_dir.join(format!("{}.AppImage", job.branding.file_stem()));
    (job.log)(&format!("Writing {}", out.display()));
    let werr = |e: &dyn std::fmt::Display| format!("{}: {e}", out.display());
    let mut f = std::fs::File::create(&out).map_err(|e| werr(&e))?;
    f.write_all(&stub).map_err(|e| werr(&e))?;
    let mut bw = std::io::BufWriter::new(f);
    w.write_with_offset(&mut bw, stub.len() as u64).map_err(|e| werr(&e))?;
    let f = bw.into_inner().map_err(|e| werr(&e))?;
    f.sync_all().map_err(|e| werr(&e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755)).map_err(|e| werr(&e))?;
    }
    Ok(out)
}

/// `/usr/bin/frogger` -> `usr/bin/frogger` (`""` for the root).
fn rel_path(p: &Path) -> String {
    p.components()
        .filter_map(|c| if let std::path::Component::Normal(s) = c { Some(s.to_string_lossy().into_owned()) } else { None })
        .collect::<Vec<_>>()
        .join("/")
}

/// Creates the missing parent directories of `rel`.
fn ensure_parent(w: &mut FilesystemWriter, dirs: &mut BTreeSet<String>, rel: &str, hdr: NodeHeader) -> Result<(), String> {
    let mut path = String::new();
    let parts: Vec<&str> = rel.split('/').collect();
    for part in &parts[..parts.len() - 1] {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(part);
        if dirs.insert(path.clone()) {
            w.push_dir(&path, hdr).map_err(|e| format!("{path}: {e}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Image;

    fn branding() -> Branding {
        let icon = Image { width: 2, height: 2, rgba: [200u8, 100, 50, 255].repeat(4) };
        Branding { title: "Frogger 2: Swampy's Revenge".into(), short_title: "Frogger 2".into(), art: icon.clone(), icon, version: "0.1.0".into() }
    }

    #[test]
    fn ids_and_entries() {
        assert_eq!(app_id("Frogger 2: Swampy's Revenge"), "frogger-2-swampys-revenge");
        let d = desktop_entry(&branding(), "frogger-2-swampys-revenge");
        assert!(d.contains("\nName=Frogger 2: Swampy's Revenge\n"));
        assert!(d.contains("\nIcon=frogger-2-swampys-revenge\n"));
    }

    #[test]
    fn replaced_paths() {
        let i = Some("swampys-revenge-recomp");
        assert!(replaced("swampys-revenge-recomp.desktop", i));
        assert!(replaced("swampys-revenge-recomp.svg", i));
        assert!(replaced(".DirIcon", i));
        assert!(replaced("usr/share/icons/hicolor/scalable/apps/x.svg", i));
        assert!(replaced("usr/share/game-data/usa/game/x", i));
        assert!(!replaced("AppRun", i));
        assert!(!replaced("usr/bin/frogger", i));
        assert!(!replaced("usr/share/licenses/x/NOTICE", i));
    }

    /// A fake 64-bit ELF "runtime": header, a `.upd_info` section, section headers.
    fn fake_runtime() -> Vec<u8> {
        let mut b = vec![0u8; 0x100];
        b[0..4].copy_from_slice(b"\x7fELF");
        b[4] = 2;
        b[5] = 1;
        let strtab = b"\0.shstrtab\0.upd_info\0";
        b[0x40..0x40 + strtab.len()].copy_from_slice(strtab);
        b[0x80..0x88].copy_from_slice(b"zsync|x\0");
        let shoff = 0x100u64;
        b[0x28..0x30].copy_from_slice(&shoff.to_le_bytes());
        b[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());
        b[0x3c..0x3e].copy_from_slice(&3u16.to_le_bytes());
        b[0x3e..0x40].copy_from_slice(&1u16.to_le_bytes());
        let mut sh = vec![0u8; 64 * 3];
        let mut set = |i: usize, name: u32, off: u64, size: u64| {
            sh[i * 64..i * 64 + 4].copy_from_slice(&name.to_le_bytes());
            sh[i * 64 + 0x18..i * 64 + 0x20].copy_from_slice(&off.to_le_bytes());
            sh[i * 64 + 0x20..i * 64 + 0x28].copy_from_slice(&size.to_le_bytes());
        };
        set(1, 1, 0x40, strtab.len() as u64);
        set(2, 11, 0x80, 8);
        b.extend_from_slice(&sh);
        b
    }

    #[test]
    fn runtime_stub() {
        let rt = fake_runtime();
        assert_eq!(elf_size(&rt), Some(rt.len() as u64));
        let clean = clean_runtime(rt.clone());
        assert_eq!(&clean[0x80..0x88], &[0; 8]);
        assert_eq!(&clean[0x40..0x50], &rt[0x40..0x50]);
    }

    /// Builds a fake AppImage (fake runtime + a squashfs AppDir like `package.sh`'s),
    /// exports it and reads the result back.
    #[test]
    fn exports_an_appimage() {
        let tmp = std::env::temp_dir().join(format!("f2pkg-linux-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let stub = fake_runtime();
        let src = tmp.join("SwampysRevengeRecomp-0.1.0-x86_64.AppImage");
        {
            let mut w = FilesystemWriter::default();
            w.set_kind(Kind::new_v4(&CODEC));
            w.set_compressor(FilesystemCompressor::new(Compressor::Gzip, None).unwrap());
            let h = |m| NodeHeader::new(m, 0, 0, 0);
            w.push_file(std::io::Cursor::new(b"#!/bin/sh\n".to_vec()), "AppRun", h(0o755)).unwrap();
            w.push_file(std::io::Cursor::new(b"[Desktop Entry]\nIcon=swampys-revenge-recomp\n".to_vec()), "swampys-revenge-recomp.desktop", h(0o644)).unwrap();
            w.push_file(std::io::Cursor::new(b"<svg/>".to_vec()), "swampys-revenge-recomp.svg", h(0o644)).unwrap();
            w.push_symlink("swampys-revenge-recomp.svg", ".DirIcon", h(0o777)).unwrap();
            w.push_dir_all("usr/bin", h(0o755)).unwrap();
            w.push_file(std::io::Cursor::new(vec![7u8; 300_000]), "usr/bin/frogger", h(0o755)).unwrap();
            w.push_dir_all("usr/share/icons/hicolor/scalable/apps", h(0o755)).unwrap();
            w.push_file(std::io::Cursor::new(b"<svg/>".to_vec()), "usr/share/icons/hicolor/scalable/apps/swampys-revenge-recomp.svg", h(0o644)).unwrap();
            let mut f = std::fs::File::create(&src).unwrap();
            f.write_all(&stub).unwrap();
            w.write_with_offset(&mut f, stub.len() as u64).unwrap();
        }
        let rend = tmp.join("data/usa");
        std::fs::create_dir_all(rend.join("game/maps")).unwrap();
        std::fs::write(rend.join("game/maps/a.fla"), vec![1u8; 200_000]).unwrap();
        std::fs::write(rend.join("manifest.json"), "{}").unwrap();
        let b = branding();
        let out = tmp.join("out");
        let job = Job { target: crate::Target::Linux, rendition: &rend, runtime: &src, out_dir: &out, branding: &b, log: &|_| {} };
        let res = crate::export(&job).unwrap();
        assert_eq!(res, out.join("Frogger 2 - Swampy's Revenge.AppImage"));

        let bytes = std::fs::read(&res).unwrap();
        assert_eq!(&bytes[..stub.len()], &clean_runtime(stub.clone())[..]);
        let r = FilesystemReader::from_reader_with_offset_and_kind(std::io::Cursor::new(bytes), stub.len() as u64, Kind::new_v4(&CODEC)).unwrap();
        let mut got = std::collections::BTreeMap::new();
        for n in r.files() {
            if let InnerNode::File(f) = &n.inner {
                let mut v = Vec::new();
                r.file(f).reader().read_to_end(&mut v).unwrap();
                got.insert(rel_path(&n.fullpath), (n.header.permissions, v));
            }
        }
        let names: Vec<&str> = got.keys().map(|s| s.as_str()).collect();
        assert_eq!(
            names,
            [
                ".DirIcon",
                "AppRun",
                "frogger-2-swampys-revenge.desktop",
                "frogger-2-swampys-revenge.png",
                "usr/bin/frogger",
                "usr/share/applications/frogger-2-swampys-revenge.desktop",
                "usr/share/game-data/usa/game/maps/a.fla",
                "usr/share/game-data/usa/manifest.json",
                "usr/share/icons/hicolor/256x256/apps/frogger-2-swampys-revenge.png",
                "usr/share/icons/hicolor/512x512/apps/frogger-2-swampys-revenge.png",
            ]
        );
        assert_eq!(got["usr/bin/frogger"], (0o755, vec![7u8; 300_000]));
        assert_eq!(got["usr/share/game-data/usa/game/maps/a.fla"].1, vec![1u8; 200_000]);
        assert!(String::from_utf8_lossy(&got["frogger-2-swampys-revenge.desktop"].1).contains("Name=Frogger 2: Swampy's Revenge"));
        assert_eq!(&got[".DirIcon"].1[1..4], b"PNG");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn exports_a_folder() {
        let tmp = std::env::temp_dir().join(format!("f2pkg-linuxdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let rt = tmp.join("rt");
        std::fs::create_dir_all(rt.join("licenses")).unwrap();
        std::fs::write(rt.join("frogger"), "bin").unwrap();
        std::fs::write(rt.join("f2-viewer"), "other").unwrap();
        std::fs::write(rt.join("licenses/LICENSE-MIT"), "mit").unwrap();
        let rend = tmp.join("data/eu");
        std::fs::create_dir_all(rend.join("video")).unwrap();
        std::fs::write(rend.join("video/intro.bik"), "v").unwrap();
        let b = branding();
        let out = tmp.join("out");
        let job = Job { target: crate::Target::Linux, rendition: &rend, runtime: &rt, out_dir: &out, branding: &b, log: &|_| {} };
        let dir = crate::export(&job).unwrap();
        assert_eq!(dir, out.join("Frogger 2 - Swampy's Revenge"));
        assert!(dir.join("frogger").is_file());
        assert!(!dir.join("f2-viewer").exists());
        assert!(dir.join("licenses/LICENSE-MIT").is_file());
        assert!(dir.join("frogger-2-swampys-revenge.desktop").is_file());
        assert!(dir.join("frogger-2-swampys-revenge.png").is_file());
        assert!(dir.join("game-data/eu/video/intro.bik").is_file());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
