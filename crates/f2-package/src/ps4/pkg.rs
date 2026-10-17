//! The PS4 package container (".pkg", "\x7FCNT"): header, entry table and body (keys, digests,
//! license, param.sfo, PlayGo files, sce_sys artwork), then the outer PFS image, written the way
//! fake-package tools do (fake keys, debug license, all-zero passcode).
//!
//! Layout and constants follow LibOrbisPkg's `PkgBuilder` / `PkgWriter` / `Entry` /
//! `PlayGo/ChunkDat` / `Rif/LicenseDat` (OpenOrbis, LGPL-2.1), reimplemented, so the result is
//! structurally identical to `PkgTool.Core pkg_build` output for the same project
//! (`tools/ps4/build-pkg.sh`); format notes on psdevwiki ("PKG files").

use super::crypto::{self, sha256};
use super::keys;
use super::pfs::{self, FileAt, Pfs};
use super::sfo::{Sfo, SfoValue};
use std::path::Path;

pub(crate) const ID_DIGESTS: u32 = 0x1;
pub(crate) const ID_ENTRY_KEYS: u32 = 0x10;
pub(crate) const ID_IMAGE_KEY: u32 = 0x20;
pub(crate) const ID_GENERAL_DIGESTS: u32 = 0x80;
pub(crate) const ID_METAS: u32 = 0x100;
pub(crate) const ID_ENTRY_NAMES: u32 = 0x200;
pub(crate) const ID_LICENSE_DAT: u32 = 0x400;
pub(crate) const ID_LICENSE_INFO: u32 = 0x401;
pub(crate) const ID_PSRESERVED_DAT: u32 = 0x409;
pub(crate) const ID_PARAM_SFO: u32 = 0x1000;
pub(crate) const ID_PLAYGO_CHUNK_DAT: u32 = 0x1001;
pub(crate) const ID_PLAYGO_CHUNK_SHA: u32 = 0x1002;
pub(crate) const ID_PLAYGO_MANIFEST_XML: u32 = 0x1003;

const BODY_OFFSET: u64 = 0x2000;
const CONTENT_TYPE_GD: u32 = 0x1A;
const DRM_TYPE_PS4: u32 = 0xF;

/// The entry ID of a file under `sce_sys/` that packages carry as an entry (and leave out of
/// the PFS), e.g. "icon0.png"; `None` for files that stay in the PFS.
pub(crate) fn entry_id(name: &str) -> Option<u32> {
    const FIXED: &[(&str, u32)] = &[
        ("license.dat", 0x400),
        ("license.info", 0x401),
        ("nptitle.dat", 0x402),
        ("npbind.dat", 0x403),
        ("selfinfo.dat", 0x404),
        ("imageinfo.dat", 0x406),
        ("target-deltainfo.dat", 0x407),
        ("origin-deltainfo.dat", 0x408),
        ("psreserved.dat", 0x409),
        ("param.sfo", 0x1000),
        ("playgo-chunk.dat", 0x1001),
        ("playgo-chunk.sha", 0x1002),
        ("playgo-manifest.xml", 0x1003),
        ("pronunciation.xml", 0x1004),
        ("pronunciation.sig", 0x1005),
        ("pic1.png", 0x1006),
        ("pubtoolinfo.dat", 0x1007),
        ("app/playgo-chunk.dat", 0x1008),
        ("app/playgo-chunk.sha", 0x1009),
        ("app/playgo-manifest.xml", 0x100A),
        ("shareparam.json", 0x100B),
        ("shareoverlayimage.png", 0x100C),
        ("save_data.png", 0x100D),
        ("shareprivacyguardimage.png", 0x100E),
        ("icon0.png", 0x1200),
        ("pic0.png", 0x1220),
        ("snd0.at9", 0x1240),
        ("changeinfo/changeinfo.xml", 0x1260),
        ("icon0.dds", 0x1280),
        ("pic0.dds", 0x12A0),
        ("pic1.dds", 0x12C0),
    ];
    if let Some(&(_, id)) = FIXED.iter().find(|(n, _)| *n == name) {
        return Some(id);
    }
    let num = |s: &str| -> Option<u32> { (s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten().filter(|&n| n < 31) };
    let pats: [(&str, &str, u32); 5] = [
        ("icon0_", ".png", 0x1201),
        ("icon0_", ".dds", 0x1281),
        ("pic1_", ".png", 0x1241),
        ("pic1_", ".dds", 0x12C1),
        ("changeinfo/changeinfo_", ".xml", 0x1261),
    ];
    for (pre, suf, base) in pats {
        if let Some(n) = name.strip_prefix(pre).and_then(|r| r.strip_suffix(suf)).and_then(num) {
            return Some(base + n);
        }
    }
    if let Some(n) = name.strip_prefix("trophy/trophy").and_then(|r| r.strip_suffix(".trp")) {
        if n.len() == 2 && n.bytes().all(|b| b.is_ascii_digit()) {
            return Some(0x1400 + n.parse::<u32>().unwrap());
        }
    }
    None
}

/// What goes into a package.
pub(crate) struct PkgInput {
    pub(crate) content_id: String,
    pub(crate) passcode: String,
    /// `sce_sys/param.sfo` as staged (the package adds PUBTOOLINFO / PUBTOOLVER to its entry).
    pub(crate) sfo: Sfo,
    /// `sce_sys` files that are package entries (besides param.sfo), in staging order.
    pub(crate) sce_sys_entries: Vec<(String, Vec<u8>)>,
    /// Creation date for PUBTOOLINFO, "YYYYMMDD".
    pub(crate) c_date: String,
    /// The finished inner image.
    pub(crate) inner: Pfs,
    /// Inode time stamps of the outer image.
    pub(crate) time: i64,
}

struct Entry {
    id: u32,
    name: Option<String>,
    data: Vec<u8>,
    // meta
    name_off: u32,
    flags1: u32,
    flags2: u32,
    offset: u32,
    size: u32,
}

impl Entry {
    fn meta_bytes(&self) -> [u8; 32] {
        let mut b = [0u8; 32];
        for (i, v) in [self.id, self.name_off, self.flags1, self.flags2, self.offset, self.size].into_iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        b
    }
    fn key_index(&self) -> u32 {
        (self.flags2 & 0xF000) >> 12
    }
    fn encrypted(&self) -> bool {
        self.flags1 & 0x8000_0000 != 0
    }
}

fn align(v: u64, a: u64) -> u64 {
    v.next_multiple_of(a)
}

/// The package header fields (big-endian on disk).
#[derive(Default)]
struct Header {
    entry_count: u32,
    sc_entry_count: u16,
    main_ent_data_size: u32,
    body_size: u64,
    content_id: String,
    promote_size: u32,
    sc_entries1_hash: [u8; 32],
    sc_entries2_hash: [u8; 32],
    digest_table_hash: [u8; 32],
    body_digest: [u8; 32],
    pfs_image_offset: u64,
    pfs_image_size: u64,
    package_size: u64,
    pfs_image_digest: [u8; 32],
    pfs_signed_digest: [u8; 32],
}

impl Header {
    fn bytes(&self) -> Vec<u8> {
        let mut h = vec![0u8; 0x1000];
        let mut put = |o: usize, b: &[u8]| h[o..o + b.len()].copy_from_slice(b);
        put(0x00, b"\x7FCNT");
        put(0x04, &1u32.to_be_bytes()); // flags
        put(0x0C, &0xFu32.to_be_bytes());
        put(0x10, &self.entry_count.to_be_bytes());
        put(0x14, &self.sc_entry_count.to_be_bytes());
        put(0x16, &(self.entry_count as u16).to_be_bytes());
        put(0x18, &0x2A80u32.to_be_bytes()); // entry table offset
        put(0x1C, &self.main_ent_data_size.to_be_bytes());
        put(0x20, &BODY_OFFSET.to_be_bytes());
        put(0x28, &self.body_size.to_be_bytes());
        put(0x40, self.content_id.as_bytes());
        put(0x70, &DRM_TYPE_PS4.to_be_bytes());
        put(0x74, &CONTENT_TYPE_GD.to_be_bytes());
        put(0x78, &0x0A00_0000u32.to_be_bytes()); // content flags: 0x8000000 | GD_AC
        put(0x7C, &self.promote_size.to_be_bytes());
        put(0x80, &0x2016_1020u32.to_be_bytes()); // version date
        put(0x84, &0x0173_8551u32.to_be_bytes()); // version hash
        put(0x9C, &1u32.to_be_bytes()); // ekc version
        put(0x100, &self.sc_entries1_hash);
        put(0x120, &self.sc_entries2_hash);
        put(0x140, &self.digest_table_hash);
        put(0x160, &self.body_digest);
        put(0x400, &1u32.to_be_bytes());
        put(0x404, &1u32.to_be_bytes()); // pfs image count
        put(0x408, &0x8000_0000_0000_03CCu64.to_be_bytes()); // pfs flags
        put(0x410, &self.pfs_image_offset.to_be_bytes());
        put(0x418, &self.pfs_image_size.to_be_bytes());
        put(0x428, &self.package_size.to_be_bytes()); // mount image size
        put(0x430, &self.package_size.to_be_bytes());
        put(0x438, &0x10000u32.to_be_bytes()); // pfs signed size
        put(0x43C, &0xD0000u32.to_be_bytes()); // pfs cache size
        put(0x440, &self.pfs_image_digest);
        put(0x460, &self.pfs_signed_digest);
        h
    }
}

/// ENTRY_KEYS: seed digest, 7 key digests, 7 RSA-encrypted keys (slot 0: the passcode).
fn entry_keys(content_id: &str, passcode: &str) -> Vec<u8> {
    let mut out = sha256(&crypto::padded(content_id.as_bytes(), 48)).to_vec();
    let mut enc = Vec::new();
    for i in 0..7u32 {
        let k = crypto::compute_key(content_id, passcode, i);
        let mut d = sha256(&k);
        for (a, b) in d.iter_mut().zip(k) {
            *a ^= b;
        }
        out.extend(d);
        let v: [u8; 32] = if i == 0 { passcode.as_bytes().try_into().unwrap() } else { k };
        enc.extend(crypto::rsa_encrypt_key(&keys::get().pkg_public_keys[i as usize], &v));
    }
    out.extend(enc);
    out
}

/// license.dat: a debug RIF for the content, its secret encrypted with the debug key, signed.
fn license_dat(content_id: &str) -> Vec<u8> {
    let mut b = Vec::with_capacity(0x400);
    b.extend(0x5249_4600u32.to_be_bytes()); // "RIF\0"
    b.extend(1i16.to_be_bytes());
    b.extend((-1i16).to_be_bytes());
    b.extend(0u64.to_be_bytes());
    b.extend(1_364_222_275i64.to_be_bytes());
    b.extend(i64::MAX.to_be_bytes());
    b.extend(crypto::padded(content_id.as_bytes(), 48));
    b.extend(0x200i16.to_be_bytes()); // license type: debug
    b.extend((DRM_TYPE_PS4 as i16).to_be_bytes());
    b.extend((CONTENT_TYPE_GD as i16).to_be_bytes());
    b.extend(3i16.to_be_bytes()); // SKU flag (GD)
    for v in [0i32, 0, 0, 1, 0] {
        b.extend(v.to_be_bytes());
    }
    b.resize(0x240, 0);
    b.extend([0u8; 32]); // disc key
    let h = sha256(&crypto::padded(content_id.as_bytes(), 48));
    let iv = &h[..16];
    let mut secret = vec![0u8; 144];
    secret[..16].copy_from_slice(&h[16..]);
    crypto::aes_cbc_encrypt(&mut secret, &keys::get().rif_debug_key, iv);
    b.extend(iv);
    b.extend(secret);
    assert_eq!(b.len(), 0x300);
    let sig = crypto::rsa_sign_sha256_debug_rif(&sha256(&b));
    b.extend(sig);
    b
}

fn license_info(content_id: &str) -> Vec<u8> {
    let mut b = crypto::padded(content_id.as_bytes(), 48);
    b.extend([0u8; 16]); // entitlement key
    for v in [0i32, CONTENT_TYPE_GD as i32, 0, 1] {
        b.extend(v.to_be_bytes());
    }
    b.resize(0x200, 0);
    b
}

/// playgo-chunk.dat: one chunk, one scenario, covering the whole package.
fn chunk_dat(content_id: &str, package_size: u64, inner_size: u64) -> Vec<u8> {
    let mut b = vec![0u8; 416];
    let mut put = |o: usize, d: &[u8]| b[o..o + d.len()].copy_from_slice(d);
    put(0, b"plgo");
    for (i, v) in [0u16, 0, 1, 1, 1, 1].into_iter().enumerate() {
        put(4 + 2 * i, &v.to_le_bytes());
    }
    put(0x10, &416u32.to_le_bytes());
    put(0x16, &1u16.to_le_bytes()); // attrib
    put(0x20, &[0xFF; 32]);
    put(0x40, content_id.as_bytes());
    for (i, v) in [256u32, 32, 288, 2, 304, 9, 320, 16, 352, 32, 384, 2, 400, 12, 336, 16].into_iter().enumerate() {
        put(0xC0 + 4 * i, &v.to_le_bytes());
    }
    // chunk attribute
    put(0x100, &[0x80, 0, 3]);
    put(0x10E, &1u16.to_le_bytes());
    put(0x110, &u64::MAX.to_le_bytes());
    put(0x130, b"Chunk #0");
    put(0x148, &package_size.to_le_bytes());
    put(0x158, &inner_size.to_le_bytes());
    // scenario attribute
    put(0x160, &[1]);
    put(0x174, &1u16.to_le_bytes());
    put(0x176, &1u16.to_le_bytes());
    put(0x190, b"Scenario #0");
    b
}

const PLAYGO_MANIFEST: &str = "\u{FEFF}<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>\r\n<psproject fmt=\"playgo-manifest\" version=\"0990\">\r\n  <volume>\r\n    <chunk_info chunk_count=\"1\" scenario_count=\"1\">\r\n      <scenarios default_id=\"0\">\r\n        <scenario id=\"0\" type=\"sp\" initial_chunk_count=\"1\" label=\"Scenario #0\">0</scenario>\r\n      </scenarios>\r\n    </chunk_info>\r\n  </volume>\r\n</psproject>\r\n";

/// The "major param" string of the content digest.
fn major_param_string(sfo: &Sfo) -> String {
    let v = |k: &str| sfo.get(k).map(SfoValue::display).unwrap_or_default();
    let mut s = format!("ATTRIBUTE{}", v("ATTRIBUTE"));
    if sfo.get("ATTRIBUTE2").is_some() {
        s += &format!("ATTRIBUTE2{}", v("ATTRIBUTE2"));
    }
    s + &format!("CATEGORY{}FORMAT{}PUBTOOLVER{}", v("CATEGORY"), v("FORMAT"), v("PUBTOOLVER"))
}

/// Summary of a written package.
pub(crate) struct Written {
    pub(crate) package_size: u64,
}

/// Writes the package to `path`.
pub(crate) fn write(path: &Path, input: PkgInput, log: &dyn Fn(&str)) -> Result<Written, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let PkgInput { content_id, passcode, sfo, sce_sys_entries, c_date, inner, time } = input;
    if content_id.len() != 36 {
        return Err(format!("content ID {content_id:?} must be 36 characters"));
    }
    let ekpfs = crypto::compute_key(&content_id, &passcode, 1);

    // outer image: pfs_image.dat = PFSC header + inner image
    let inner_size = inner.size();
    let mut outer = Pfs::new(pfs::Props { time, min_blocks: 0, crypt: Some((ekpfs, [0u8; 16])) });
    outer.add_file("pfs_image.dat", pfs::Content::Image(Box::new(inner)), 0);
    outer.finish(&|_| false);
    let pfs_size = outer.size();

    // param.sfo entry: + PUBTOOLINFO / PUBTOOLVER
    let mut sfo = sfo;
    let prelim_size = 0x80000 + pfs_size;
    let info = format!("c_date={c_date},img0_l0_size={},img0_l1_size=0,img0_sc_ksize=512,img0_pc_ksize=832", prelim_size.div_ceil(1 << 20));
    sfo.set("PUBTOOLINFO", SfoValue::Utf8(info, 0x200));
    sfo.set_int("PUBTOOLVER", 0x0289_0000);
    let sfo_bytes = sfo.to_bytes();

    // entries, in LibOrbisPkg's order
    let e =
        |id: u32, name: Option<&str>, data: Vec<u8>| Entry { id, name: name.map(String::from), data, name_off: 0, flags1: 0, flags2: 0, offset: 0, size: 0 };
    let mut entries = vec![
        e(ID_ENTRY_KEYS, None, entry_keys(&content_id, &passcode)),
        e(ID_IMAGE_KEY, None, crypto::rsa_encrypt_key(&keys::get().fake_modulus, &ekpfs).to_vec()),
        e(ID_GENERAL_DIGESTS, None, vec![0; 0x180]),
        e(ID_METAS, None, vec![]),
        e(ID_DIGESTS, None, vec![]),
        e(ID_ENTRY_NAMES, None, vec![]),
        e(ID_PLAYGO_CHUNK_DAT, Some("playgo-chunk.dat"), vec![0; 416]),
        e(ID_PLAYGO_CHUNK_SHA, Some("playgo-chunk.sha"), vec![]),
        e(ID_PLAYGO_MANIFEST_XML, Some("playgo-manifest.xml"), PLAYGO_MANIFEST.as_bytes().to_vec()),
        e(ID_LICENSE_DAT, None, license_dat(&content_id)),
        e(ID_LICENSE_INFO, None, license_info(&content_id)),
        e(ID_PARAM_SFO, Some("param.sfo"), sfo_bytes.clone()),
        e(ID_PSRESERVED_DAT, None, vec![0; 0x2000]),
    ];
    for (name, data) in sce_sys_entries {
        let id = entry_id(&name).ok_or_else(|| format!("sce_sys/{name} is not a package entry"))?;
        entries.push(e(id, Some(&name), data));
    }
    let n = entries.len();
    let idx = |id: u32, entries: &[Entry]| entries.iter().position(|x| x.id == id).unwrap();
    let (i_metas, i_digests, i_names, i_sha) =
        (idx(ID_METAS, &entries), idx(ID_DIGESTS, &entries), idx(ID_ENTRY_NAMES, &entries), idx(ID_PLAYGO_CHUNK_SHA, &entries));
    entries[i_digests].data = vec![0; n * 32];

    // name table (names in sorted order)
    let mut names: Vec<String> = entries.iter().filter_map(|x| x.name.clone()).collect();
    names.sort();
    let mut table = vec![0u8];
    let mut offsets = std::collections::HashMap::new();
    for nm in names {
        offsets.entry(nm.clone()).or_insert_with(|| {
            let o = table.len() as u32;
            table.extend(nm.as_bytes());
            table.push(0);
            o
        });
    }
    entries[i_names].data = table;

    // offsets and sizes
    let mut off = BODY_OFFSET;
    for i in 0..n {
        let len = |x: &Entry| if x.id == ID_METAS { 0 } else { x.data.len() as u64 };
        let (flags1, flags2) = match entries[i].id {
            ID_DIGESTS | ID_ENTRY_NAMES => (0x4000_0000, 0),
            ID_ENTRY_KEYS | ID_GENERAL_DIGESTS | ID_METAS => (0x6000_0000, 0),
            ID_IMAGE_KEY => (0xE000_0000, 3 << 12),
            ID_LICENSE_DAT => (0x8000_0000, 3 << 12),
            ID_LICENSE_INFO => (0x8000_0000, 2 << 12),
            _ => (0, 0),
        };
        let mut size = len(&entries[i]);
        if i == i_metas {
            size = n as u64 * 32;
        }
        if i == i_sha {
            // LibOrbisPkg's estimate (the metas table counted as far as built so far)
            let sum: u64 = entries.iter().enumerate().map(|(j, x)| if j == i_metas { align((i as u64 + 1) * 32, 16) } else { align(len(x), 16) }).sum();
            let mut pkg = align(BODY_OFFSET + sum, 0x80000) + pfs_size;
            pkg += ((pkg + 16) / 0x10000) * 4;
            size = (pkg / 0x10000) * 4;
        }
        let x = &mut entries[i];
        x.name_off = x.name.as_ref().map_or(0, |nm| offsets[nm]);
        x.flags1 = flags1;
        x.flags2 = flags2;
        x.offset = off as u32;
        x.size = size as u32;
        off = align(off + size, 16);
    }
    let body_size = align(off, 0x80000) - BODY_OFFSET;
    let pfs_offset = BODY_OFFSET + body_size;
    let package_size = pfs_offset + pfs_size;
    let sha_len = 4 * (package_size / 0x10000);
    if sha_len > entries[i_sha].size as u64 {
        return Err("playgo-chunk.sha estimate too small".into());
    }
    entries[i_sha].size = sha_len as u32;
    entries[i_sha].data = vec![0; sha_len as usize];
    let ci = idx(ID_PLAYGO_CHUNK_DAT, &entries);
    entries[ci].data = chunk_dat(&content_id, package_size, inner_size);
    let mut hdr = Header {
        entry_count: n as u32,
        sc_entry_count: 6,
        main_ent_data_size: (0x800 + 0x100 + 0x180 + 2 * n * 32) as u32,
        body_size,
        content_id: content_id.clone(),
        promote_size: pfs_offset as u32,
        pfs_image_offset: pfs_offset,
        pfs_image_size: pfs_size,
        package_size,
        ..Default::default()
    };

    // outer image, signed and encrypted, hashed on the way (PlayGo chunk hashes too)
    log(&format!("Writing the package ({} MB)...", package_size >> 20));
    let mut f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path).map_err(|e| err(&e))?;
    f.set_len(package_size).map_err(|e| err(&e))?;
    {
        let mut io = FileAt { f: &mut f, base: pfs_offset };
        let total = inner_size.max(1);
        let mut done = 0u64;
        let mut next = total / 5;
        outer
            .write(&mut io, &mut |k| {
                done += k;
                if done >= next {
                    log(&format!("  {}%", done * 100 / total));
                    next += total / 5;
                }
            })
            .map_err(|e| err(&e))?;
        log("Signing and encrypting the file system...");
        let mut full = sha2::Sha256::default();
        let chunk0 = pfs_offset / 0x10000;
        let sha = &mut entries[i_sha].data;
        let mut first = [0u8; 32];
        outer
            .encrypt(&mut io, &mut |b, data| {
                use sha2::Digest;
                full.update(data);
                let h = sha256(data);
                if b == 0 {
                    first = h;
                }
                let o = ((chunk0 + b) * 4) as usize;
                sha[o..o + 4].copy_from_slice(&h[..4]);
            })
            .map_err(|e| err(&e))?;
        hdr.pfs_signed_digest = first;
        hdr.pfs_image_digest = sha2::Digest::finalize(full).into();
    }

    // general digests
    let hb = hdr.bytes();
    let header_digest = sha256(&[&hb[..0x40], &hb[0x400..0x480]].concat());
    let major = sha256(major_param_string(&sfo).as_bytes());
    let mut cd = crypto::padded(content_id.as_bytes(), 48);
    cd.extend(DRM_TYPE_PS4.to_be_bytes());
    cd.extend(CONTENT_TYPE_GD.to_be_bytes());
    cd.extend(hdr.pfs_image_digest);
    cd.extend(major);
    let content_digest = sha256(&cd);
    let mut gd = Vec::with_capacity(0x180);
    gd.extend(0xD256u16.to_be_bytes());
    gd.extend(0x100u16.to_be_bytes());
    gd.extend([0u8; 24]);
    gd.extend(0x6Eu32.to_be_bytes()); // content | game | header | major param | param
    for d in [content_digest, hdr.pfs_image_digest, header_digest, [0; 32], major, sha256(&sfo_bytes)] {
        gd.extend(d);
    }
    gd.resize(0x180, 0);
    let gi = idx(ID_GENERAL_DIGESTS, &entries);
    entries[gi].data = gd;

    // metas table (sorted by ID)
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| entries[i].id);
    let mut metas = Vec::with_capacity(n * 32);
    for &i in &order {
        metas.extend(entries[i].meta_bytes());
    }
    entries[i_metas].data = metas;

    // body
    let mut body = vec![0u8; body_size as usize];
    for x in &entries {
        let mut data = x.data.clone();
        data.resize(x.size as usize, 0);
        if x.encrypted() {
            let mut seed = x.meta_bytes().to_vec();
            seed.extend(crypto::compute_key(&content_id, &passcode, x.key_index()));
            let iv_key = sha256(&seed);
            crypto::aes_cbc_encrypt(&mut data, &iv_key[16..], &iv_key[..16]);
        }
        let o = (x.offset as u64 - BODY_OFFSET) as usize;
        body[o..o + data.len()].copy_from_slice(&data);
    }
    let at = |x: &Entry| (x.offset as u64 - BODY_OFFSET) as usize;
    // per-entry digests (index 0 is the digests entry itself)
    let doff = at(&entries[i_digests]);
    for (k, &i) in order.iter().enumerate().skip(1) {
        let (o, s) = (at(&entries[i]), entries[i].size as usize);
        let h = sha256(&body[o..o + s]);
        body[doff + 32 * k..doff + 32 * k + 32].copy_from_slice(&h);
    }
    hdr.body_digest = sha256(&body);
    hdr.digest_table_hash = sha256(&body[doff..doff + n * 32]);
    let sc = |ids: &[u32], metas_len: Option<usize>| {
        let mut v = Vec::new();
        for &id in ids {
            let x = &entries[idx(id, &entries)];
            let s = if id == ID_METAS { metas_len.unwrap_or(x.size as usize) } else { x.size as usize };
            v.extend(&body[at(x)..at(x) + s]);
        }
        sha256(&v)
    };
    hdr.sc_entries1_hash = sc(&[ID_ENTRY_KEYS, ID_IMAGE_KEY, ID_GENERAL_DIGESTS, ID_METAS, ID_DIGESTS], None);
    hdr.sc_entries2_hash = sc(&[ID_ENTRY_KEYS, ID_IMAGE_KEY, ID_GENERAL_DIGESTS, ID_METAS], Some(6 * 0x20));

    // header, its digest and signature
    let mut head = hdr.bytes();
    head.resize(BODY_OFFSET as usize, 0);
    let hd = sha256(&head[..0xFE0]);
    head[0xFE0..0x1000].copy_from_slice(&hd);
    let hs = sha256(&head[..0x1000]);
    head[0x1000..0x1100].copy_from_slice(&crypto::rsa_encrypt_key(&keys::get().pkg_public_keys[3], &hs));
    {
        use std::io::{Seek, SeekFrom, Write};
        f.seek(SeekFrom::Start(0)).map_err(|e| err(&e))?;
        f.write_all(&head).map_err(|e| err(&e))?;
        f.write_all(&body).map_err(|e| err(&e))?;
        f.sync_all().map_err(|e| err(&e))?;
    }
    Ok(Written { package_size })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_names() {
        assert_eq!(entry_id("icon0.png"), Some(0x1200));
        assert_eq!(entry_id("pic1.png"), Some(0x1006));
        assert_eq!(entry_id("icon0_05.png"), Some(0x1206));
        assert_eq!(entry_id("trophy/trophy02.trp"), Some(0x1402));
        assert_eq!(entry_id("about/right.sprx"), None);
        assert_eq!(entry_id("keystone"), None);
    }

    #[test]
    fn license_layout() {
        let l = license_dat("IV0000-FROG20000_00-FROGGER2RUSTPORT");
        assert_eq!(l.len(), 0x400);
        assert_eq!(&l[..4], b"RIF\0");
        assert_eq!(license_info("IV0000-FROG20000_00-FROGGER2RUSTPORT").len(), 0x200);
        assert_eq!(PLAYGO_MANIFEST.as_bytes()[..3], [0xEF, 0xBB, 0xBF]);
    }
}
