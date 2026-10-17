//! The FIH header that heads the package, and the SI segment (a stored ZIP) that ends it:
//! `naps_meta_18.dat` (the inner image's block map and integrity tables, AES-XTS),
//! `naps_meta_30x.dat`, `playgo-chunk.dat` and the CRC32C table of the package
//! (LibProsperoPkg 1.2.0 `ProsperoFihBuilder`, `ProsperoSiArchive`, `ProsperoNapsMeta`).

use super::inner::InnerImage;
use super::keys::Keyset;
use super::pfs::sha3;
use crate::ps4::crypto::{hmac_sha256, Xts};
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::{Aes128, Block};
use std::io::Read;

const UBLOCK: u64 = 262_144;

/// What the FIH header records about the package.
pub(crate) struct FihInput {
    pub pfs_size: u64,
    pub superblock_index: u64,
    pub game_digest: [u8; 32],
    pub naps_digest: [u8; 32],
    pub naps_len: u64,
    pub meta_base_blocks: u64,
    pub content_version_high: u32,
    pub naps_file_count: u32,
    pub app_file_count: u32,
    pub sparse_afid_count: u32,
    pub empty_file_count: u32,
    pub ndblock: u64,
}

/// `BuildFihHeaderBlock` (debug variant) for an nwonly outer PFS.
pub(crate) fn fih_header(f: &FihInput) -> Result<Vec<u8>, String> {
    let mut a = vec![0u8; 65536];
    let w32 = |a: &mut Vec<u8>, at: usize, v: u32| a[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let w64 = |a: &mut Vec<u8>, at: usize, v: u64| a[at..at + 8].copy_from_slice(&v.to_le_bytes());
    a[0..4].copy_from_slice(b"\x7fFIH");
    a[4] = 1;
    a[6] = 3;
    w32(&mut a, 8, 1);
    w64(&mut a, 16, 65536);
    w64(&mut a, 24, f.pfs_size);
    w64(&mut a, 40, 65536);
    w64(&mut a, 88, 65536 + f.pfs_size);
    w64(&mut a, 96, 65536);
    w64(&mut a, 104, 1 << 47);
    if f.meta_base_blocks > 0 {
        w64(&mut a, 80, f.meta_base_blocks);
    }
    let sb = f.superblock_index * 65536;
    w64(&mut a, 32, 65536 + sb);
    for at in [48, 112, 208] {
        a[at..at + 32].copy_from_slice(&f.game_digest);
    }
    let (n2, n3) = (f.superblock_index, f.pfs_size / 65536);
    if n2 >= 1 && n3 > n2 {
        let n4 = if f.naps_len > 0 { f.naps_len.div_ceil(65536) } else { 1 };
        if n4 == 0 || n4 > n2 {
            return Err("the NAPS layout extent crosses the outer-PFS superblock".into());
        }
        let n5 = (n2 - n4) as u32;
        w32(&mut a, 144, n5);
        w32(&mut a, 148, f.naps_file_count);
        w32(&mut a, 152, f.naps_file_count + f.empty_file_count);
        w64(&mut a, 160, f.ndblock * 65536);
        if f.content_version_high != 0 {
            w32(&mut a, 156, f.content_version_high);
        }
        if f.naps_len > 0 {
            w64(&mut a, 168, f.naps_len);
        }
        w32(&mut a, 240, if f.app_file_count > 0 { f.app_file_count } else { 1 });
        w32(&mut a, 244, f.sparse_afid_count);
        w32(&mut a, 252, f.empty_file_count);
        w32(&mut a, 248, 2);
    }
    a[176..208].copy_from_slice(&f.naps_digest);
    Ok(a)
}

pub(crate) fn crc32c(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0x82F6_3B78 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    !data.iter().fold(!0u32, |c, &b| t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8))
}

/// `BuildMeta300`.
pub(crate) fn meta300(inner_len: u64) -> Vec<u8> {
    let r = inner_len - 131_072;
    let mut a = vec![0u8; 48];
    a[16..24].copy_from_slice(&r.to_le_bytes());
    a[24..32].copy_from_slice(&1001u64.to_le_bytes());
    a[32..40].copy_from_slice(&r.to_le_bytes());
    a[40..48].copy_from_slice(&131_072u64.to_le_bytes());
    a
}

fn reductions(input: &[u8]) -> (u64, u64) {
    let n = input.len().min(65536);
    let (mut sum, mut weighted) = (0u64, 0u64);
    for &b in &input[..n] {
        sum += b as u64;
        weighted += sum;
    }
    let rolling_w = weighted + (65536 - n) as u64 * sum;
    let (sum2, w2) = if input.len() == n {
        (sum as u32, (input.len() as u32).wrapping_add(weighted as u32))
    } else {
        let (mut s, mut w) = (sum as u32, weighted as u32);
        for &b in &input[n..] {
            s = s.wrapping_add(b as u32);
            w = w.wrapping_add(s);
        }
        (s, (input.len() as u32).wrapping_add(w))
    };
    (sum2 as u64 | (w2 as u64) << 32, sum ^ (rolling_w << 25))
}

/// A block of the meta-18 map.
struct Block18 {
    co: u64,
    cs: u32,
    ps: u32,
    c0: u32,
    c1: u32,
    flag: u32,
    owner: u32,
    tail: u64,
    logical: i64,
    /// (placement index, offset in the file) of the plaintext, or None for zeros / metadata.
    plain: Plain,
}

enum Plain {
    File(usize, u64),
    Zero,
    Meta(usize),
}

fn blocks18(inner: &InnerImage, exec: &[bool]) -> Vec<Block18> {
    let mut list = Vec::new();
    for (i, p) in inner.placements.iter().enumerate() {
        let owner = exec[i] as u32;
        if !p.blocks.is_empty() {
            let mut phys = p.on_disk_offset;
            let mut off = 0u64;
            for b in &p.blocks {
                let at = if b.on_disk_offset >= 0 { b.on_disk_offset } else { phys };
                let cs = b.compressed_size as u32;
                let c0 = if b.multi_chunk { b.first_chunk_compressed_size as u32 } else { cs };
                let c1 = if b.multi_chunk { cs - c0 } else { 0 };
                let flag = if p.store_raw {
                    1_074_331_648
                } else if c1 != 0 {
                    1_078_263_808
                } else {
                    1_074_069_504
                };
                list.push(Block18 { co: at as u64, cs, ps: b.uncompressed_size as u32, c0, c1, flag, owner, tail: 0, logical: p.logical_offset + off as i64, plain: Plain::File(i, off) });
                phys = at + cs as i64;
                off += b.uncompressed_size as u64;
            }
        } else {
            let mut off = 0u64;
            loop {
                let n = UBLOCK.min((p.uncompressed_size as u64).saturating_sub(off)) as u32;
                let c0 = n.min(131_072);
                list.push(Block18 { co: p.on_disk_offset as u64 + off, cs: n, ps: n, c0, c1: n - c0, flag: 1_074_331_648, owner, tail: 0, logical: p.logical_offset + off as i64, plain: Plain::File(i, off) });
                off += n as u64;
                if n == 0 || off >= p.uncompressed_size as u64 {
                    break;
                }
            }
        }
    }
    for &h in &inner.sparse_afid_holes {
        list.push(Block18 { co: inner.block_info_on_disk as u64, cs: 16, ps: UBLOCK as u32, c0: 8, c1: 8, flag: 1_074_855_936, owner: 0, tail: 0, logical: inner.afid_logical_offsets[h], plain: Plain::Zero });
    }
    let gap = inner.meta_base_logical - inner.data_end_logical;
    if gap > 0 {
        let n = (gap as u64).div_ceil(UBLOCK);
        let mut seen: Vec<(u32, u64)> = Vec::new();
        let mut next = inner.block_info_on_disk as u64;
        for j in 0..n {
            let size = UBLOCK.min(gap as u64 - j * UBLOCK) as u32;
            let co = match seen.iter().find(|s| s.0 == size) {
                Some(s) => s.1,
                None => {
                    seen.push((size, next));
                    next += 16;
                    next - 16
                }
            };
            list.push(Block18 { co, cs: 16, ps: size, c0: 8, c1: 8, flag: 1_074_855_936, owner: 0, tail: 1001, logical: inner.data_end_logical + (j * UBLOCK) as i64, plain: Plain::Zero });
        }
    }
    let mut co = inner.metadata_on_disk as u64;
    let mut off = 0usize;
    for m in &inner.metadata_blocks {
        let cs = m.compressed_size as u32;
        let stored = m.compressed_size == m.uncompressed_size;
        let c0 = if stored || !m.multi_chunk { cs } else { m.first_chunk_compressed_size as u32 };
        let c1 = if !stored && m.multi_chunk { cs - c0 } else { 0 };
        let flag = if stored {
            1_074_331_648
        } else if c1 != 0 {
            1_078_263_808
        } else {
            1_074_069_504
        };
        list.push(Block18 { co, cs, ps: m.uncompressed_size as u32, c0, c1, flag, owner: 0, tail: 1001, logical: inner.meta_base_logical + off as i64, plain: Plain::Meta(off) });
        co += cs as u64;
        off += m.uncompressed_size as usize;
    }
    list.sort_by_key(|b| b.logical);
    list
}

fn record(dst: &mut Vec<u8>, tag: &str, payload: &[u8]) {
    let t = tag.as_bytes();
    let version = if tag == "file" { 2 } else { 1 };
    dst.extend([t[3], t[2], t[1], t[0], version, 0, 0, 0]);
    dst.extend((payload.len() as u64).to_le_bytes());
    dst.extend(payload);
}

/// `BuildMeta18` for the package's inner image. `content` is (path, size) of every non-empty
/// file in AFID order; `physical_digests` the SHA3 of the inner image's 64 KiB blocks (the
/// outer PFS's first digests).
pub(crate) fn meta18(keys: &Keyset, inner: &InnerImage, inner_spool: &std::path::Path, content: &[(String, u64)], outer_pfs_size: u64, physical_digests: &[[u8; 32]], pfs_image_key: &[u8; 32], seed: &[u8; 16]) -> Result<Vec<u8>, String> {
    let inner_len = inner.image_length as u64;
    let exec: Vec<bool> = inner.placements.iter().map(|p| inner.nodes.iter().any(|n| !n.is_directory && n.afid == p.afid && n.executable)).collect();
    let blocks = blocks18(inner, &exec);
    let meta_idx = blocks.iter().position(|b| b.tail == 1001);
    // Plaintext digests and reductions.
    let mut cache: Option<(usize, Vec<u8>)> = None;
    let mut prefixes = Vec::with_capacity(blocks.len() * 8);
    let mut rolling = Vec::with_capacity(blocks.len() * 8);
    let mut digests = Vec::with_capacity(blocks.len());
    for b in &blocks {
        let zero;
        let data: &[u8] = match b.plain {
            Plain::File(i, off) => {
                if cache.as_ref().is_none_or(|c| c.0 != i) {
                    cache = Some((i, inner.placements[i].source.read()?));
                }
                let d = &cache.as_ref().unwrap().1;
                &d[off as usize..off as usize + b.ps as usize]
            }
            Plain::Zero => {
                zero = vec![0u8; b.ps as usize];
                &zero
            }
            Plain::Meta(off) => &inner.meta_plain[off..off + b.ps as usize],
        };
        digests.push(sha3(data));
        let (p, r) = reductions(data);
        prefixes.extend(p.to_le_bytes());
        rolling.extend(r.to_le_bytes());
    }
    // Check codes of the encrypted physical blocks.
    let mut ks = 1u32.to_le_bytes().to_vec();
    ks.extend(seed);
    let k = hmac_sha256(pfs_image_key, &ks);
    let xts = Xts::new(k[16..].try_into().unwrap(), k[..16].try_into().unwrap());
    let nblk = (inner_len / 65536) as usize;
    let mut obcc = Vec::with_capacity(nblk * 4);
    let mut f = std::fs::File::open(inner_spool).map_err(|e| format!("{}: {e}", inner_spool.display()))?;
    let mut buf = vec![0u8; 65536];
    for j in 0..nblk {
        f.read_exact(&mut buf).map_err(|e| e.to_string())?;
        xts.encrypt_sector(&mut buf, j as u64);
        obcc.extend(crc32c(&buf).to_le_bytes());
    }

    let mut out = Vec::new();
    let mut phdr = Vec::new();
    for v in [1u32, 80, (inner_len / 65536) as u32, (inner_len - 65536) as u32, 1, 65536] {
        phdr.extend(v.to_le_bytes());
    }
    record(&mut out, "phdr", &phdr);
    let block_count = |i: usize| -> u32 {
        let p = &inner.placements[i];
        if !p.blocks.is_empty() {
            p.blocks.len() as u32
        } else {
            (p.uncompressed_size as u64).div_ceil(UBLOCK).max(1) as u32
        }
    };
    let mut file = Vec::new();
    let mut ftyp = Vec::new();
    let mut first = 0u32;
    for (i, (path, size)) in content.iter().enumerate() {
        let is_meta = path == "*PFSmetadata";
        let count = if is_meta { 3 } else if i < inner.placements.len() { block_count(i) } else { 1 };
        let holes = if !is_meta && i < inner.placements.len() { inner.sparse_afid_holes.iter().filter(|&&h| (h as u32) < inner.placements[i].afid).count() as u32 } else { 0 };
        let start = if is_meta { meta_idx.map_or(first, |m| m as u32) } else { first + holes };
        let size = if is_meta { inner.meta_plain.len() as u64 } else { *size };
        let _ = outer_pfs_size;
        file.extend(size.to_le_bytes());
        for v in [start, count, if is_meta { 1001 } else { 0 }, (!is_meta && i < exec.len() && exec[i]) as u32] {
            file.extend(v.to_le_bytes());
        }
        if !is_meta {
            first += count;
        }
        let mut t = [0u8; 56];
        t[0..8].copy_from_slice(&1u64.to_le_bytes());
        t[8..16].copy_from_slice(&size.to_le_bytes());
        t[16..24].copy_from_slice(&size.to_le_bytes());
        t[24..32].copy_from_slice(&(count as u64).to_le_bytes());
        ftyp.extend(t);
    }
    record(&mut out, "file", &file);
    record(&mut out, "ftyp", &ftyp);
    record(&mut out, "ibcl", &blocks.iter().map(|b| if b.owner == 1 { 1 } else { 15 }).collect::<Vec<u8>>());
    record(&mut out, "ibcl", &vec![0u8; blocks.len()]);
    let mut i2ob = Vec::new();
    let mut i2op = Vec::new();
    let mut ihsh = Vec::new();
    for (n, b) in blocks.iter().enumerate() {
        let mut e = [0u8; 40];
        e[0..8].copy_from_slice(&b.co.to_le_bytes());
        for (at, v) in [(8, b.cs), (12, b.ps), (16, b.c0), (20, b.c1), (24, (b.co >> 16) as u32), (32, 1), (36, b.flag)] {
            e[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        i2ob.extend(e);
        i2op.extend(b.co.to_le_bytes());
        i2op.extend((b.co >> 16).to_le_bytes());
        ihsh.extend(&prefixes[n * 8..n * 8 + 8]);
        ihsh.extend(digests[n]);
        ihsh.extend(b.tail.to_le_bytes());
    }
    record(&mut out, "i2ob", &i2ob);
    record(&mut out, "i2op", &i2op);
    record(&mut out, "ihsh", &ihsh);
    record(&mut out, "rhsh", &rolling);
    let mut fstr = Vec::new();
    for (p, _) in content {
        fstr.extend(p.replace('\\', "/").as_bytes());
        fstr.push(0);
    }
    record(&mut out, "fstr", &fstr);
    let mut twek = [0u8; 20];
    twek[4..8].copy_from_slice(&((inner_len / 65536) as u32).to_le_bytes());
    record(&mut out, "twek", &twek);
    record(&mut out, "obdg", &physical_digests[..nblk].concat());
    record(&mut out, "obcc", &obcc);
    let m300 = meta300(inner_len);
    for tag in ["pgpl", "pgil", "pgpi", "pgpu"] {
        record(&mut out, tag, &m300);
    }
    record(&mut out, "gitt", b"v1.5.0\0");
    record(&mut out, "gith", b"7553c74caeba25754fbd4bee717652da631c08e4\0");
    let pad = (16 - out.len() % 16) % 16;
    record(&mut out, "zero", &vec![0u8; pad]);
    // AES-XTS with one tweak over the whole record stream.
    let data = Aes128::new_from_slice(&keys.meta18_data_key).unwrap();
    let tk = Aes128::new_from_slice(&keys.meta18_tweak_key).unwrap();
    let mut t = Block::from(keys.meta18_tweak);
    tk.encrypt_block(&mut t);
    let mut tw: [u8; 16] = t.into();
    for chunk in out.chunks_mut(16) {
        for (b, k) in chunk.iter_mut().zip(tw) {
            *b ^= k;
        }
        data.encrypt_block(Block::from_mut_slice(chunk));
        for (b, k) in chunk.iter_mut().zip(tw) {
            *b ^= k;
        }
        let mut carry = 0u8;
        for b in tw.iter_mut() {
            let c = *b >> 7;
            *b = (*b << 1) | carry;
            carry = c;
        }
        if carry != 0 {
            tw[0] ^= 0x87;
        }
    }
    Ok(out)
}

/// `WriteZip`: stored members, version 2.0, DOS date 0x21 (1980-01-01).
pub(crate) fn zip(members: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = Vec::new();
    for (name, data) in members {
        let crc = crc32fast::hash(data);
        at.push((crc, data.len() as u32, out.len() as u32));
        out.extend(0x0403_4B50u32.to_le_bytes());
        for v in [20u16, 0, 0, 0, 33] {
            out.extend(v.to_le_bytes());
        }
        out.extend(crc.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(name.as_bytes());
        out.extend(data);
    }
    let cd = out.len();
    for ((name, _), (crc, len, off)) in members.iter().zip(&at) {
        out.extend(0x0201_4B50u32.to_le_bytes());
        for v in [0u16, 20, 0, 0, 0, 33] {
            out.extend(v.to_le_bytes());
        }
        out.extend(crc.to_le_bytes());
        out.extend(len.to_le_bytes());
        out.extend(len.to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        for _ in 0..4 {
            out.extend(0u16.to_le_bytes());
        }
        out.extend(0u32.to_le_bytes());
        out.extend(off.to_le_bytes());
        out.extend(name.as_bytes());
    }
    let cd_len = out.len() - cd;
    out.extend(0x0605_4B50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((members.len() as u16).to_le_bytes());
    out.extend((members.len() as u16).to_le_bytes());
    out.extend((cd_len as u32).to_le_bytes());
    out.extend((cd as u32).to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32c_check_value() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }

    #[test]
    fn reductions_match_definition() {
        let d: Vec<u8> = (0..70000u32).map(|i| (i * 7) as u8).collect();
        let (p, r) = reductions(&d);
        let (mut s, mut w) = (0u32, d.len() as u32);
        for &b in &d {
            s = s.wrapping_add(b as u32);
            w = w.wrapping_add(s);
        }
        assert_eq!(p, s as u64 | (w as u64) << 32);
        let (mut s, mut w) = (0u64, 0u64);
        for i in 0..65536 {
            s += d[i] as u64;
            w += s;
        }
        assert_eq!(r, s ^ (w << 25));
    }
}
