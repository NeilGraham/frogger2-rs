//! The PS5 fake package (`.pkg`, installable with kstuff fPKG support on firmware 11.60 and
//! lower): a pure-Rust port of the path of LibProsperoPkg 1.2.0 that
//! `ffpfsc-pkg-tool build --deterministic` drives (LibProsperoPkg by drakmor / SvenGDK,
//! GPL-3.0, <https://github.com/drakmor/LibProsperoPkg>): an application package, PLAINTEXT_NOAUTH
//! debug image, Kraken level 7 in 256 KiB blocks (PFS compression v2), one PlayGo chunk,
//! passcode of zeros, build time 0. Its output is byte-identical to the tool's for the same
//! stage folder.
//!
//! Layout of the package: the FIH header (64 KiB), the outer PFS (`pfs_image.dat` = the inner
//! PFS image, and `naps_pkg_layout.dat`), the CNT (keys, digests, `param.json`, artwork, PlayGo
//! files) and the SI segment (a ZIP of `naps_meta_*.dat`, `playgo-chunk.dat` and the package's
//! CRC32C table). Not ported: GP5 projects, licenses (drm "standard"), additional content,
//! encrypted (native) images, the physical layout pass and block-dedup repair (refused with
//! an error when an image would need them), shuffle patterns.

pub(crate) mod cnt;
pub(crate) mod inner;
pub(crate) mod keys;
pub(crate) mod naps;
pub(crate) mod outer;
pub(crate) mod pfs;
pub(crate) mod si;

use super::fself;
use inner::{InputFile, Source};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// A file of the package's stage (what `build` would read from a stage folder): its path
/// relative to the app root (`eboot.bin`, `sce_sys/param.json`) and its bytes.
pub(crate) struct StageFile {
    pub path: String,
    pub len: u64,
    pub source: Source,
}

/// What `build` made.
pub(crate) struct Built {
    pub size: u64,
    pub content_version: String,
}

fn head(src: &Source) -> Result<Vec<u8>, String> {
    Ok(match src {
        Source::Bytes(b) => b.iter().take(64).copied().collect(),
        Source::Path(p) => {
            let mut h = vec![0u8; 64];
            let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let n = f.read(&mut h).map_err(|e| e.to_string())?;
            h.truncate(n);
            h
        }
    })
}

/// Writes the package of `stage` (content ID `content_id`) to `out`, using `temp` for the inner
/// image spool.
pub(crate) fn build(stage: Vec<StageFile>, content_id: &str, keys: &keys::Keyset, out: &Path, temp: &Path, log: &dyn Fn(&str)) -> Result<Built, String> {
    if content_id.len() != 36 {
        return Err(format!("content ID {content_id:?} is not 36 characters"));
    }
    let param_src = stage.iter().find(|f| f.path == "sce_sys/param.json").ok_or("the stage has no sce_sys/param.json")?.source.read()?;
    let param = cnt::publisher_param_json(&param_src)?;
    let (content_version, drm) = cnt::param_info(&param);
    if !drm.eq_ignore_ascii_case("free") {
        return Err(format!("applicationDrmType {drm:?} is not supported (only \"free\")"));
    }

    // The inner image's files: fake-signed executables, the generated sce_sys files.
    let eboot_has_version = match stage.iter().find(|f| f.path == "eboot.bin") {
        Some(f) => {
            let d = f.source.read()?;
            fself::is_elf(&d) && fself::has_sce_version(&d)
        }
        None => false,
    };
    let mut files: Vec<InputFile> = Vec::new();
    let mut media: Vec<(String, Vec<u8>)> = Vec::new();
    for f in &stage {
        let p = f.path.trim_start_matches('/');
        if let Some(name) = p.strip_prefix("sce_sys/") {
            if cnt::MEDIA.iter().any(|m| m.0 == name) {
                media.push((name.to_string(), f.source.read()?));
                continue;
            }
            if name == "param.json" || name == "pfs-region-hints.json" {
                continue;
            }
            if inner::excluded_from_inner(&format!("/{p}")) {
                return Err(format!("sce_sys/{name} is not supported in a package"));
            }
        }
        let converts =
            p == "eboot.bin" || p.to_ascii_lowercase().ends_with(".prx") || ["fakelib/", "sce_module/", "sce_sys/about/"].iter().any(|d| p.starts_with(d));
        if converts && fself::is_elf(&head(&f.source)?) {
            let elf = f.source.read()?;
            if p != "eboot.bin" && !fself::has_sce_version(&elf) && eboot_has_version {
                return Err(format!("{p} has no .sceversion section (borrowing eboot.bin's is not supported)"));
            }
            let s = fself::make_fself(&elf).map_err(|e| format!("{p}: {e}"))?;
            files.push(InputFile { path: format!("/{p}"), len: s.len() as i64, source: Source::Bytes(s) });
        } else {
            files.push(InputFile { path: format!("/{p}"), len: f.len as i64, source: f.source.clone() });
        }
    }
    let has = |files: &[InputFile], p: &str| files.iter().any(|f| f.path == p);
    if stage.iter().any(|f| f.path.starts_with("sce_sys/")) {
        if !has(&files, "/sce_sys/pfs-version.dat") {
            let v = cnt::pfs_content_version(&content_version).into_bytes();
            files.push(InputFile { path: "/sce_sys/pfs-version.dat".into(), len: v.len() as i64, source: Source::Bytes(v) });
        }
        if !has(&files, "/sce_sys/keystone") {
            let k = cnt::keystone(keys);
            files.push(InputFile { path: "/sce_sys/keystone".into(), len: k.len() as i64, source: Source::Bytes(k) });
        }
        if !has(&files, "/sce_sys/about/right.sprx") {
            files.push(InputFile {
                path: "/sce_sys/about/right.sprx".into(),
                len: keys.right_sprx.len() as i64,
                source: Source::Bytes(keys.right_sprx.clone()),
            });
        }
    }

    // Stage 1-2: inner image and NAPS layout.
    std::fs::create_dir_all(temp).map_err(|e| format!("{}: {e}", temp.display()))?;
    let spool = temp.join("pfs_image.dat");
    let result = (|| -> Result<Built, String> {
        log("Building the inner PFS image (Kraken compression)...");
        let img = inner::build(files, &[], &spool, log)?;
        let naps = naps::generate(&img)?;
        let seed = cnt::deterministic_seed(content_id);
        let image_key = cnt::pfs_image_key(content_id, &seed);

        // Stage 3: the outer PFS, at 0x10000 in the package.
        log("Writing the outer PFS...");
        let mut f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(out).map_err(|e| format!("{}: {e}", out.display()))?;
        let io = |e: std::io::Error| format!("{}: {e}", out.display());
        let outer_files = vec![
            outer::OuterFile {
                name: "pfs_image.dat",
                len: img.image_length as u64,
                size_compressed: img.ndblock as u64 * 65536,
                reader: Box::new(std::io::BufReader::new(std::fs::File::open(&spool).map_err(|e| format!("{}: {e}", spool.display()))?)),
            },
            outer::OuterFile { name: "naps_pkg_layout.dat", len: naps.len() as u64, size_compressed: naps.len() as u64, reader: Box::new(&naps[..]) },
        ];
        let o = outer::build(&mut f, 65536, outer_files)?;
        let mut sblock = vec![0u8; 65536];
        f.seek(SeekFrom::Start(65536 + o.superblock_index as u64 * 65536)).map_err(io)?;
        f.read_exact(&mut sblock).map_err(io)?;

        // Stage 4-5: FIH header, CNT, SI.
        let file_nodes: Vec<&inner::Node> = {
            let mut v: Vec<&inner::Node> = img.nodes.iter().filter(|n| !n.is_directory && n.parent_inode >= 0).collect();
            v.sort_by_key(|n| n.afid);
            v
        };
        let fih = si::fih_header(&si::FihInput {
            pfs_size: o.pfs_size,
            superblock_index: o.superblock_index as u64,
            game_digest: pfs::sha3(&sblock),
            naps_digest: pfs::sha3(&naps),
            naps_len: naps.len() as u64,
            meta_base_blocks: (img.meta_base_logical / 65536) as u64,
            content_version_high: cnt::content_version_high(&content_version),
            naps_file_count: (u64::from_le_bytes(naps[..8].try_into().unwrap()) & 0xFF_FFFF) as u32,
            app_file_count: file_nodes.iter().filter(|n| n.mode == 33133).count() as u32,
            sparse_afid_count: img.sparse_afid_holes.len() as u32,
            empty_file_count: img.empty_file_count as u32,
            ndblock: img.ndblock as u64,
        })?;
        log("Writing the package metadata...");
        let (cnt_bytes, chunk_dat) = cnt::build(
            keys,
            cnt::CntInput {
                content_id,
                param_json: param.clone(),
                media,
                pfs_size: o.pfs_size,
                image_digests: &o.image_digests,
                superblock: &sblock,
                pfs_image_key: image_key,
                inner_len: img.image_length as u64,
                file_paths: file_nodes.iter().map(|n| n.full_path.clone()).collect(),
                fih: &fih,
            },
        )?;
        f.seek(SeekFrom::Start(0)).map_err(io)?;
        f.write_all(&fih).map_err(io)?;
        f.seek(SeekFrom::Start(65536 + o.pfs_size)).map_err(io)?;
        f.write_all(&cnt_bytes).map_err(io)?;
        let mount = 65536 + o.pfs_size + cnt_bytes.len() as u64;

        log("Writing the PlayGo and NAPS tables...");
        let mut crc = Vec::with_capacity((mount / 65536) as usize * 4);
        f.seek(SeekFrom::Start(0)).map_err(io)?;
        let mut r = std::io::BufReader::with_capacity(1 << 20, &mut f);
        let mut blk = vec![0u8; 65536];
        let mut left = mount;
        while left > 0 {
            let n = left.min(65536) as usize;
            r.read_exact(&mut blk[..n]).map_err(io)?;
            crc.extend(si::crc32c(&blk[..n]).to_le_bytes());
            left -= n as u64;
        }
        drop(r);
        let mut content: Vec<(String, u64)> =
            file_nodes.iter().filter(|n| n.size != 0).map(|n| (n.full_path.trim_start_matches('/').to_string(), n.size as u64)).collect();
        content.push(("*PFSmetadata".into(), o.pfs_size));
        let m18 = si::meta18(keys, &img, &spool, &content, o.pfs_size, &o.image_digests, &image_key, &seed)?;
        let m300 = si::meta300(img.image_length as u64);
        let mut members = vec![("common/etc/naps_meta_18.dat".to_string(), m18)];
        for id in [300, 301, 302, 308] {
            members.push((format!("common/etc/naps_meta_{id}.dat"), m300.clone()));
        }
        members.push(("common/etc/playgo-chunk.dat".into(), chunk_dat));
        members.push((format!("config/{content_id}/playgo-chunk.crc"), crc));
        let zip = si::zip(&members);
        f.seek(SeekFrom::Start(mount)).map_err(io)?;
        f.write_all(&zip).map_err(io)?;
        let size = mount + zip.len() as u64;
        f.set_len(size).map_err(io)?;
        f.sync_all().map_err(io)?;
        Ok(Built { size, content_version: content_version.clone() })
    })();
    let _ = std::fs::remove_file(&spool);
    result
}

/// The stage files under `dir` (paths relative to it, `/`-separated).
pub(crate) fn stage_from_dir(dir: &Path) -> Result<Vec<StageFile>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))? {
            let p = e.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
                let len = std::fs::metadata(&p).map_err(|e| e.to_string())?.len();
                out.push(StageFile { path: rel, len, source: Source::Path(p) });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds the package of the stage folder `F2_PKG_STAGE` into `F2_PKG_OUT` with the real
    /// key files in `F2_PS5_KEYS_DIR` (compare with `ffpfsc-pkg-tool build --deterministic`).
    #[test]
    #[ignore]
    fn stage_vector() {
        let stage = std::path::PathBuf::from(std::env::var("F2_PKG_STAGE").unwrap());
        let out = std::path::PathBuf::from(std::env::var("F2_PKG_OUT").unwrap());
        let keys = keys::load(None, &|m| eprintln!("{m}")).unwrap();
        let tmp = out.with_extension("tmp");
        let b = build(stage_from_dir(&stage).unwrap(), "UP9000-PPSA99920_00-FROGGER2RUSTPORT", &keys, &out, &tmp, &|m| eprintln!("{m}")).unwrap();
        eprintln!("package {} bytes", b.size);
    }

    const SYNTH_CONTENT_ID: &str = "UP9000-PPSA99920_00-FROGGER2RUSTPORT";

    /// A small synthetic stage (no game data): param.json, artwork stand-ins, a non-ELF
    /// eboot.bin, data that compresses, data that does not, a repeated block and an empty file.
    fn synthetic_stage(dir: &Path) {
        let w = |rel: &str, b: &[u8]| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b).unwrap();
        };
        let mut x = 0x1234_5678u32;
        let mut noise = |n: usize| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (x >> 16) as u8
                })
                .collect()
        };
        w(
            "sce_sys/param.json",
            br#"{
  "ageLevel": {"default": 0},
  "applicationCategoryType": 0,
  "applicationDrmType": "free",
  "attribute": 0,
  "attribute2": 0,
  "attribute3": 524352,
  "conceptId": "99920",
  "contentBadgeType": 1,
  "contentId": "UP9000-PPSA99920_00-FROGGER2RUSTPORT",
  "contentVersion": "01.002.000",
  "downloadDataSize": 256,
  "localizedParameters": {"defaultLanguage": "en-US", "en-US": {"titleName": "Synthetic"}},
  "masterVersion": "01.00",
  "pubtools": {"creationDate": "2026-01-01 00:00:00", "toolVersion": "2.00"},
  "requiredSystemSoftwareVersion": "0x0000000000000000",
  "sdkVersion": "0x0000000000000000",
  "titleId": "PPSA99920",
  "versionFileUri": ""
}
"#,
        );
        let rgba = (0..512 * 512).flat_map(|i: u32| [(i % 512 / 2) as u8, (i / 512 / 2) as u8, 0x40, 0xFF]).collect();
        let icon = crate::Image { width: 512, height: 512, rgba };
        w("sce_sys/icon0.png", &crate::ps4::art::png_rgb(&icon));
        w("sce_sys/icon0.dds", &crate::ps5::dds::bc7_dds(&icon));
        w("eboot.bin", &noise(70_000));
        let text: Vec<u8> = (0..600_000u32).map(|i| b"frogger swampy lily pad "[(i % 24) as usize] ^ ((i / 997) as u8 & 1)).collect();
        w("data/game/text.bin", &text);
        w("data/game/noise.bin", &noise(400_000));
        let block = noise(262_144);
        w("data/game/maps/twice.bin", &[block.clone(), block].concat());
        w("data/game/maps/empty.bin", b"");
        w("data/manifest.json", b"{}");
    }

    /// SHA-256 of `ffpfsc-pkg-tool build STAGE OUT --content-id UP9000-PPSA99920_00-FROGGER2RUSTPORT
    /// --title-id PPSA99920 --title Synthetic --version 01.002.000 --deterministic` of the
    /// synthetic stage (the tool's output is the reference this port matches byte for byte).
    const SYNTH_SHA256: &str = "eef5b963fa9a0d6ab054e0798df8ad366d340f282756bb5b626fa409b1c9f9f8";

    /// Writes the synthetic stage to `F2_PKG_SYNTH_DIR` (to run the reference tool on it).
    #[test]
    #[ignore]
    fn write_synthetic_stage() {
        synthetic_stage(Path::new(&std::env::var("F2_PKG_SYNTH_DIR").unwrap()));
    }

    fn build_synthetic(keys: &keys::Keyset, name: &str) -> Vec<u8> {
        let root = std::env::temp_dir().join(format!("f2-ps5pkg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        synthetic_stage(&root.join("stage"));
        let out = root.join("out.pkg");
        build(stage_from_dir(&root.join("stage")).unwrap(), SYNTH_CONTENT_ID, keys, &out, &root.join("tmp"), &|_| {}).unwrap();
        let b = std::fs::read(&out).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        b
    }

    /// The package of the synthetic stage with the real key material (`F2_PS5_KEYS_DIR`, or the
    /// download) is the reference tool's, byte for byte.
    #[test]
    #[ignore]
    fn synthetic_matches_reference() {
        let keys = keys::load(None, &|m| eprintln!("{m}")).unwrap();
        let b = build_synthetic(&keys, "ref");
        let h: String = crate::ps4::crypto::sha256(&b).iter().map(|x| format!("{x:02x}")).collect();
        assert_eq!(h, SYNTH_SHA256);
    }

    /// With throwaway keys: deterministic, and laid out as FIH / PFS / CNT / SI.
    #[test]
    fn synthetic_layout() {
        let keys = keys::test_keyset();
        let a = build_synthetic(&keys, "a");
        assert_eq!(a, build_synthetic(&keys, "b"));
        assert_eq!(&a[..4], b"\x7fFIH");
        let pfs = u64::from_le_bytes(a[24..32].try_into().unwrap()) as usize;
        let cnt = 0x10000 + pfs;
        assert_eq!(&a[cnt..cnt + 4], b"\x7fCNT");
        let package_size = u64::from_be_bytes(a[cnt + 1072..cnt + 1080].try_into().unwrap()) as usize;
        assert_eq!(&a[package_size..package_size + 4], b"PK\x03\x04");
        assert_eq!(u64::from_be_bytes(a[cnt + 1040..cnt + 1048].try_into().unwrap()), 0x10000);
    }
}
