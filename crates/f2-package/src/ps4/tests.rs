//! PS4 export tests: a package built from a synthetic runtime and rendition is read back and
//! checked (entries, digests, signatures, both PFS images, file contents); `compare_with_pkgtool`
//! compares an exported package with `PkgTool.Core`'s for the same inputs.

use super::reader::{pfsc_payload, Pfs, Pkg};
use super::{crypto, PASSCODE};
use crate::{Branding, Image, Job, Target};
use std::path::{Path, PathBuf};

pub(crate) fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("f2-package-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) fn test_branding() -> Branding {
    let mut rgba = Vec::new();
    for y in 0..64u32 {
        for x in 0..64u32 {
            rgba.extend([(x * 4) as u8, (y * 4) as u8, 90, 255]);
        }
    }
    let art = Image { width: 64, height: 64, rgba };
    Branding { title: "Frogger 2: Swampy's Revenge".into(), short_title: "Frogger 2".into(), art: art.clone(), icon: art, version: "0.0.0".into() }
}

/// A rendition with a few files, one of them larger than 12 PFS blocks.
pub(crate) fn test_rendition(root: &Path) -> PathBuf {
    let r = root.join("usa");
    for (p, len) in
        [("game/textures/a.bin", 1000usize), ("game/maps/b.bin", 13 * 65536 + 5), ("music/m.ogg", 7), ("video/v.bik", 70000), ("oracle/frogger2.exe", 3)]
    {
        let f = r.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, (0..len).map(|i| (i * 7 + p.len()) as u8).collect::<Vec<u8>>()).unwrap();
    }
    std::fs::write(r.join("manifest.json"), b"{}").unwrap();
    r
}

fn ps4_runtime(root: &Path) -> PathBuf {
    let rt = root.join("runtime-ps4");
    std::fs::create_dir_all(rt.join("sce_module")).unwrap();
    std::fs::create_dir_all(rt.join("sce_sys/about")).unwrap();
    std::fs::write(rt.join("eboot.bin"), vec![0x4F; 100_000]).unwrap();
    std::fs::write(rt.join("sce_module/libc.prx"), b"libc").unwrap();
    std::fs::write(rt.join("sce_module/libSceFios2.prx"), b"fios").unwrap();
    std::fs::write(rt.join("sce_sys/about/right.sprx"), b"right").unwrap();
    std::fs::write(
        rt.join("runtime.json"),
        r#"{"target":"ps4","version":"0.0.0","format":1,"title_id":"FROG20000","content_id":"IV0000-FROG20000_00-FROGGER2RUSTPORT",
            "app_ver":"01.03","attribute":0,"download_data_size":64,"data_dir":"data",
            "files":["eboot.bin","sce_sys/about/right.sprx","sce_module/libc.prx","sce_module/libSceFios2.prx"]}"#,
    )
    .unwrap();
    rt
}

#[test]
fn export_and_read_back() {
    let root = temp_dir("ps4");
    let rendition = test_rendition(&root);
    let runtime = ps4_runtime(&root);
    let out = root.join("out");
    let branding = test_branding();
    let job = Job { target: Target::Ps4, rendition: &rendition, runtime: &runtime, out_dir: &out, branding: &branding, log: &|_| {} };
    let path = crate::export(&job).unwrap();
    assert_eq!(path.file_name().unwrap(), "Frogger 2 - Swampy's Revenge.pkg");
    let pkg = Pkg::parse(std::fs::read(&path).unwrap(), PASSCODE);
    check_package(&pkg);

    // param.sfo
    let sfo = super::sfo::Sfo::parse(&pkg.entry(0x1000).data).unwrap();
    assert_eq!(sfo.get("TITLE").unwrap().display(), "Frogger 2: Swampy's Revenge");
    assert_eq!(sfo.get("TITLE_ID").unwrap().display(), "FROG20000");
    assert_eq!(sfo.get("CONTENT_ID").unwrap().display(), "IV0000-FROG20000_00-FROGGER2RUSTPORT");
    assert!(sfo.get("PUBTOOLINFO").unwrap().display().starts_with("c_date="));
    // artwork entries
    for (id, w, h) in [(0x1200u32, 512u32, 512u32), (0x1220, 1920, 1080), (0x1006, 1920, 1080)] {
        let d = &pkg.entry(id).data;
        assert_eq!(&d[1..4], b"PNG");
        assert_eq!((u32::from_be_bytes(d[16..20].try_into().unwrap()), u32::from_be_bytes(d[20..24].try_into().unwrap())), (w, h));
    }

    // file systems
    let outer_img = pkg.outer_image(PASSCODE);
    let outer = Pfs::parse(&outer_img);
    assert!(outer.signed);
    let dat = outer.file(outer.tree["/pfs_image.dat"]);
    let inner_img = pfsc_payload(dat);
    let inner = Pfs::parse(inner_img);
    assert!(!inner.signed);
    let paths: Vec<&str> = inner.tree.keys().map(String::as_str).collect();
    for p in [
        "/eboot.bin",
        "/sce_module/libc.prx",
        "/sce_sys/about/right.sprx",
        "/sce_sys/keystone",
        "/data/manifest.json",
        "/data/game/maps/b.bin",
        "/data/video/v.bik",
    ] {
        assert!(paths.contains(&p), "{p} missing: {paths:?}");
    }
    for p in ["/sce_sys/param.sfo", "/sce_sys/icon0.png", "/data/oracle"] {
        assert!(!paths.contains(&p), "{p} must not be in the image");
    }
    assert_eq!(inner.file(inner.tree["/data/game/maps/b.bin"]), std::fs::read(rendition.join("game/maps/b.bin")).unwrap());
    assert_eq!(inner.file(inner.tree["/sce_sys/keystone"]), crypto::keystone(PASSCODE));
    assert_eq!(inner.ndblock * 0x10000, inner_img.len() as u64);
    // outer block signatures: the pfs_image.dat inode's first data block
    let ekpfs = crypto::compute_key(&pkg.content_id, PASSCODE, 1);
    let key = crypto::pfs_sign_key(&ekpfs, &outer_img[0x370..0x380]);
    let ino = &outer.inodes[outer.tree["/pfs_image.dat"] as usize];
    let at = 0x10000 + 0x2C8 * outer.tree["/pfs_image.dat"] as usize + 0x64;
    let blk = &outer_img[ino.db0 as usize * 0x10000..][..0x10000];
    assert_eq!(outer_img[at..at + 32], crypto::hmac_sha256(&key, blk));
    // the superblock signature (over the superblock with the signature field zeroed)
    let mut sb = outer_img[..0x5A0].to_vec();
    sb[0x380..0x3A4].fill(0);
    assert_eq!(outer_img[0x380..0x3A0], crypto::hmac_sha256(&key, &sb));
    std::fs::remove_dir_all(&root).unwrap();
}

/// Checks a package's own consistency: entry digests, body / header digests, PFS digests.
fn check_package(pkg: &Pkg) {
    let b = &pkg.bytes;
    let digests = pkg.entry(0x1);
    let mut sorted = pkg.entries.clone();
    sorted.sort_by_key(|e| e.id);
    assert_eq!(sorted.iter().map(|e| e.id).collect::<Vec<_>>(), pkg.entries.iter().map(|e| e.id).collect::<Vec<_>>(), "entry table sorted by ID");
    for (i, e) in sorted.iter().enumerate().skip(1) {
        let raw = &b[e.offset as usize..(e.offset + e.size) as usize];
        assert_eq!(digests.data[32 * i..32 * i + 32], crypto::sha256(raw), "digest of entry {:#x}", e.id);
    }
    let body_off = u64::from_be_bytes(b[0x20..0x28].try_into().unwrap()) as usize;
    let body_size = u64::from_be_bytes(b[0x28..0x30].try_into().unwrap()) as usize;
    assert_eq!(b[0x160..0x180], crypto::sha256(&b[body_off..body_off + body_size]), "body digest");
    assert_eq!(b[0xFE0..0x1000], crypto::sha256(&b[..0xFE0]), "header digest");
    let (o, s) = pkg.pfs_range();
    assert_eq!(o + s, b.len());
    assert_eq!(b[0x440..0x460], crypto::sha256(&b[o..o + s]), "PFS image digest");
    assert_eq!(b[0x460..0x480], crypto::sha256(&b[o..o + 0x10000]), "PFS signed digest");
    // PlayGo chunk hashes
    let sha = &pkg.entry(0x1002).data;
    let c = o / 0x10000 + 1;
    assert_eq!(sha[c * 4..c * 4 + 4], crypto::sha256(&b[c * 0x10000..(c + 1) * 0x10000])[..4]);
    // the EKPFS in IMAGE_KEY is the one derived from the passcode (fake keyset, checked via
    // ENTRY_KEYS slot 1's digest)
    let ek = crypto::compute_key(&pkg.content_id, PASSCODE, 1);
    let mut d = crypto::sha256(&ek);
    for (a, k) in d.iter_mut().zip(ek) {
        *a ^= k;
    }
    assert_eq!(pkg.entry(0x10).data[32 + 32..32 + 64], d);
}

/// Compares an exported package with PkgTool.Core's (`tools/ps4/build-pkg.sh --data`) built
/// from the same runtime files and data:
/// `F2_PS4_REF_PKG=ref.pkg F2_PS4_NEW_PKG=new.pkg cargo test -p f2-package --release compare_with_pkgtool -- --ignored --nocapture`.
/// Everything must match except time stamps, the artwork entries, and what hashes those.
#[test]
#[ignore]
fn compare_with_pkgtool() {
    let (Ok(rp), Ok(np)) = (std::env::var("F2_PS4_REF_PKG"), std::env::var("F2_PS4_NEW_PKG")) else {
        eprintln!("set F2_PS4_REF_PKG and F2_PS4_NEW_PKG");
        return;
    };
    let r = Pkg::parse(std::fs::read(rp).unwrap(), PASSCODE);
    let n = Pkg::parse(std::fs::read(np).unwrap(), PASSCODE);
    check_package(&r);
    check_package(&n);
    // header: same except the digests (0x100..0x180, 0x440..0x480) and the header digest / signature
    for (o, len, what) in
        [(0, 0x100, "header 0x000-0x100"), (0x180, 0x280, "header 0x180-0x400"), (0x400, 0x40, "header 0x400-0x440"), (0x480, 0xB60, "header 0x480-0xFE0")]
    {
        if r.header(o, len) != n.header(o, len) {
            let at = r.header(o, len).iter().zip(n.header(o, len)).position(|(a, b)| a != b).unwrap() + o;
            if std::env::var_os("F2_CMP_SOFT").is_some() {
                println!("MISMATCH {what} at {at:#x}");
                continue;
            }
            panic!("{what} differs at {at:#x}");
        }
    }
    println!("header: identical apart from digests and signature");
    assert_eq!(r.entries.len(), n.entries.len());
    let differ_ok = [0x1u32, 0x80, 0x1000, 0x1002, 0x1006, 0x1200, 0x1220];
    for (a, b) in r.entries.iter().zip(&n.entries) {
        assert_eq!((a.id, &a.name, a.flags1, a.flags2), (b.id, &b.name, b.flags1, b.flags2));
        let art = [0x1006, 0x1200, 0x1220].contains(&a.id);
        if !art && std::env::var_os("F2_CMP_SOFT").is_none() {
            assert_eq!((a.offset, a.size), (b.offset, b.size), "entry {:#x} placement", a.id);
        }
        let same = a.data == b.data;
        println!("entry {:#06x} {:<20} offset {:#08x} size {:>8}  {}", a.id, a.name, b.offset, b.size, if same { "identical" } else { "differs" });
        if !same && std::env::var_os("F2_CMP_SOFT").is_none() {
            assert!(differ_ok.contains(&a.id), "entry {:#x} differs", a.id);
        }
    }
    // param.sfo: identical apart from PUBTOOLINFO's date
    let (sr, sn) = (super::sfo::Sfo::parse(&r.entry(0x1000).data).unwrap(), super::sfo::Sfo::parse(&n.entry(0x1000).data).unwrap());
    for ((ka, va), (kb, vb)) in sr.values.iter().zip(&sn.values) {
        assert_eq!(ka, kb);
        if ka != "PUBTOOLINFO" {
            assert_eq!(va, vb, "{ka}");
        }
    }
    println!("param.sfo: same keys and values ({})", sr.values.iter().map(|(k, v)| format!("{k}={}", v.display())).collect::<Vec<_>>().join(", "));
    // file systems: identical apart from inode times (and the signatures that cover them)
    let (ri, ni) = (r.outer_image(PASSCODE), n.outer_image(PASSCODE));
    let (ro, no) = (Pfs::parse(&ri), Pfs::parse(&ni));
    println!("outer blocks: {} vs {}", ro.ndblock, no.ndblock);
    {
        let (rd, nd) = (pfsc_payload(ro.file(ro.tree["/pfs_image.dat"])), pfsc_payload(no.file(no.tree["/pfs_image.dat"])));
        let (a, b) = (Pfs::parse(rd), Pfs::parse(nd));
        println!("inner: blocks {} vs {}, inodes {} vs {}", a.ndblock, b.ndblock, a.inodes.len(), b.inodes.len());
        for (p, &i) in &a.tree {
            match b.tree.get(p) {
                None => println!("only in ref: {p}"),
                Some(&j) if i != j || Inode { time: 0, ..a.inodes[i as usize].clone() } != Inode { time: 0, ..b.inodes[j as usize].clone() } => {
                    println!("{p}: ref ino {i} {:?} / new ino {j} {:?}", a.inodes[i as usize], b.inodes[j as usize]);
                }
                _ => {}
            }
        }
        for p in b.tree.keys().filter(|p| !a.tree.contains_key(*p)) {
            println!("only in new: {p}");
        }
    }
    assert_eq!(ro.ndblock, no.ndblock, "outer block count");
    assert_eq!(ro.tree, no.tree, "outer tree");
    for (a, b) in ro.inodes.iter().zip(&no.inodes) {
        assert_eq!(Inode { time: 0, ..a.clone() }, Inode { time: 0, ..b.clone() });
    }
    let (rd, nd) = (pfsc_payload(ro.file(ro.tree["/pfs_image.dat"])), pfsc_payload(no.file(no.tree["/pfs_image.dat"])));
    assert_eq!(ro.file(ro.tree["/pfs_image.dat"])[..0x10000], no.file(no.tree["/pfs_image.dat"])[..0x10000], "PFSC header");
    let (rin, nin) = (Pfs::parse(rd), Pfs::parse(nd));
    assert_eq!(rin.tree, nin.tree, "inner tree (paths and inode numbers)");
    assert_eq!(rin.inodes.len(), nin.inodes.len());
    for (i, (a, b)) in rin.inodes.iter().zip(&nin.inodes).enumerate() {
        assert_eq!(Inode { time: 0, ..a.clone() }, Inode { time: 0, ..b.clone() }, "inner inode {i}");
    }
    // byte for byte, with the time stamps blanked
    let blank = |img: &[u8], p: &Pfs| {
        let mut v = img.to_vec();
        v[0x50 + 24..0x50 + 56].fill(0);
        let mut pos = 0x10000;
        for _ in 0..p.inodes.len() {
            v[pos + 24..pos + 56].fill(0);
            pos += 0xA8;
            if pos % 0x10000 > 0x10000 - 0xA8 {
                pos += 0x10000 - pos % 0x10000;
            }
        }
        v
    };
    let (rb, nb) = (blank(rd, &rin), blank(nd, &nin));
    assert_eq!(rb.len(), nb.len(), "inner image size");
    let diff = rb.iter().zip(&nb).filter(|(a, b)| a != b).count();
    assert_eq!(diff, 0, "inner images differ in {diff} bytes (first at {:#x})", rb.iter().zip(&nb).position(|(a, b)| a != b).unwrap_or(0));
    println!("inner PFS: {} inodes, {} files/dirs, {} MB: byte-identical apart from time stamps", rin.inodes.len(), rin.tree.len(), rb.len() >> 20);
    println!("outer PFS: {} blocks, same layout and inodes", ro.ndblock);
}

use super::reader::Inode;

/// Extracts a package into an app folder (what a console installs), for running it in an
/// emulator without a package installer (shadPS4: `shadPS4.exe -g <dir>/eboot.bin`):
/// `F2_PS4_PKG=x.pkg F2_PS4_EXTRACT_TO=dir cargo test -p f2-package --release extract_package -- --ignored`.
#[test]
#[ignore]
fn extract_package() {
    let (Ok(p), Ok(to)) = (std::env::var("F2_PS4_PKG"), std::env::var("F2_PS4_EXTRACT_TO")) else {
        eprintln!("set F2_PS4_PKG and F2_PS4_EXTRACT_TO");
        return;
    };
    let to = PathBuf::from(to);
    let pkg = Pkg::parse(std::fs::read(p).unwrap(), PASSCODE);
    check_package(&pkg);
    let outer_img = pkg.outer_image(PASSCODE);
    let outer = Pfs::parse(&outer_img);
    let inner = Pfs::parse(pfsc_payload(outer.file(outer.tree["/pfs_image.dat"])));
    let mut n = 0;
    for (path, &ino) in &inner.tree {
        let dst = to.join(path.trim_start_matches('/'));
        if inner.inodes[ino as usize].mode & 0x4000 != 0 {
            std::fs::create_dir_all(&dst).unwrap();
        } else {
            std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
            std::fs::write(&dst, inner.file(ino)).unwrap();
            n += 1;
        }
    }
    for e in pkg.entries.iter().filter(|e| e.id >= 0x1000 && !e.name.is_empty() && !e.name.starts_with("playgo")) {
        std::fs::write(to.join("sce_sys").join(&e.name), &e.data).unwrap();
        n += 1;
    }
    println!("extracted {n} files to {}", to.display());
}
