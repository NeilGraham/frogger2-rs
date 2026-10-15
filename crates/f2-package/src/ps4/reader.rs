//! A small fake-package reader for the tests: header, entries (decrypted), the outer PFS
//! (decrypted with the EKPFS derived from the content ID and passcode) and the inner PFS
//! (inodes, directory tree, file contents). Used to check packages structurally and to compare
//! them with `PkgTool.Core`'s (`compare_with_pkgtool`).

use super::crypto;
use std::collections::BTreeMap;

pub(crate) struct Pkg {
    pub(crate) bytes: Vec<u8>,
    pub(crate) content_id: String,
    pub(crate) entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub(crate) id: u32,
    pub(crate) name: String,
    pub(crate) flags1: u32,
    pub(crate) flags2: u32,
    pub(crate) offset: u32,
    pub(crate) size: u32,
    /// Decrypted contents.
    pub(crate) data: Vec<u8>,
}

fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}
fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(b[o..o + 8].try_into().unwrap())
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl Pkg {
    pub(crate) fn parse(bytes: Vec<u8>, passcode: &str) -> Pkg {
        assert_eq!(&bytes[..4], b"\x7FCNT");
        let content_id = String::from_utf8_lossy(&bytes[0x40..0x64]).into_owned();
        let n = be32(&bytes, 0x10) as usize;
        let table = be32(&bytes, 0x18) as usize;
        let mut entries: Vec<Entry> = (0..n)
            .map(|i| {
                let m = &bytes[table + 32 * i..table + 32 * i + 32];
                let (id, name_off, flags1, flags2, offset, size) = (be32(m, 0), be32(m, 4), be32(m, 8), be32(m, 12), be32(m, 16), be32(m, 20));
                let mut data = bytes[offset as usize..(offset + size) as usize].to_vec();
                if flags1 & 0x8000_0000 != 0 {
                    let mut seed = m.to_vec();
                    seed.extend(crypto::compute_key(&content_id, passcode, (flags2 & 0xF000) >> 12));
                    let k = crypto::sha256(&seed);
                    let whole = data.len() / 16 * 16;
                    crypto::aes_cbc_decrypt(&mut data[..whole], &k[16..], &k[..16]);
                }
                Entry { id, name: name_off.to_string(), flags1, flags2, offset, size, data }
            })
            .collect();
        let names = entries.iter().find(|e| e.id == 0x200).map(|e| e.data.clone()).unwrap_or_default();
        for e in &mut entries {
            let o: usize = e.name.parse().unwrap();
            e.name = if o == 0 { String::new() } else { String::from_utf8_lossy(&names[o..o + names[o..].iter().position(|&c| c == 0).unwrap()]).into_owned() };
        }
        Pkg { bytes, content_id, entries }
    }

    pub(crate) fn header(&self, o: usize, len: usize) -> &[u8] {
        &self.bytes[o..o + len]
    }

    pub(crate) fn entry(&self, id: u32) -> &Entry {
        self.entries.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("no entry {id:#x}"))
    }

    pub(crate) fn pfs_range(&self) -> (usize, usize) {
        (be64(&self.bytes, 0x410) as usize, be64(&self.bytes, 0x418) as usize)
    }

    /// The outer image, decrypted (every sector but the superblock's; the unencrypted empty
    /// block is restored from the original bytes).
    pub(crate) fn outer_image(&self, passcode: &str) -> Vec<u8> {
        let (o, s) = self.pfs_range();
        let mut img = self.bytes[o..o + s].to_vec();
        let ekpfs = crypto::compute_key(&self.content_id, passcode, 1);
        let seed = img[0x370..0x380].to_vec();
        let (tweak, data) = crypto::pfs_enc_keys(&ekpfs, &seed);
        let xts = crypto::Xts::new(&data, &tweak);
        for (i, sector) in img.as_chunks_mut::<0x1000>().0.iter_mut().enumerate().skip(16) {
            if sector.iter().all(|&b| b == 0) {
                continue; // the unencrypted empty block
            }
            xts.decrypt_sector(sector, i as u64);
        }
        img
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Inode {
    pub(crate) mode: u16,
    pub(crate) nlink: u16,
    pub(crate) flags: u32,
    pub(crate) size: u64,
    pub(crate) size_c: u64,
    pub(crate) time: u64,
    pub(crate) blocks: u32,
    pub(crate) db0: u32,
}

/// A parsed PFS image.
pub(crate) struct Pfs<'a> {
    pub(crate) img: &'a [u8],
    pub(crate) signed: bool,
    pub(crate) ndblock: u64,
    pub(crate) inodes: Vec<Inode>,
    /// path ("/data/game/x") -> inode number
    pub(crate) tree: BTreeMap<String, u32>,
}

impl<'a> Pfs<'a> {
    pub(crate) fn parse(img: &'a [u8]) -> Pfs<'a> {
        assert_eq!(le64(img, 0), 1);
        assert_eq!(le64(img, 8), 20130315);
        let mode = u16::from_le_bytes(img[0x1C..0x1E].try_into().unwrap());
        let signed = mode & 1 != 0;
        let count = le64(img, 0x30) as usize;
        let ndblock = le64(img, 0x38);
        let isz = if signed { 0x2C8 } else { 0xA8 };
        let mut inodes = Vec::new();
        let mut pos = 0x10000usize;
        for _ in 0..count {
            let b = &img[pos..pos + isz];
            inodes.push(Inode {
                mode: u16::from_le_bytes(b[0..2].try_into().unwrap()),
                nlink: u16::from_le_bytes(b[2..4].try_into().unwrap()),
                flags: le32(b, 4),
                size: le64(b, 8),
                size_c: le64(b, 16),
                time: le64(b, 24),
                blocks: le32(b, 0x60),
                db0: if signed { le32(b, 0x64 + 32) } else { le32(b, 0x64) },
            });
            pos += isz;
            if pos % 0x10000 > 0x10000 - isz {
                pos += 0x10000 - pos % 0x10000;
            }
        }
        let mut pfs = Pfs { img, signed, ndblock, inodes, tree: BTreeMap::new() };
        // super-root -> uroot
        let sr = pfs.dirents(0);
        let uroot = sr.iter().find(|(n, ..)| n == "uroot").expect("uroot").1;
        let mut stack = vec![(String::new(), uroot)];
        while let Some((path, ino)) = stack.pop() {
            for (name, child, kind) in pfs.dirents(ino) {
                if kind == 4 || kind == 5 {
                    continue;
                }
                let p = format!("{path}/{name}");
                if kind == 3 {
                    stack.push((p.clone(), child));
                }
                pfs.tree.insert(p, child);
            }
        }
        pfs
    }

    /// (name, inode, type) of a directory's entries.
    pub(crate) fn dirents(&self, ino: u32) -> Vec<(String, u32, i32)> {
        let i = &self.inodes[ino as usize];
        let mut out = vec![];
        for blk in 0..i.blocks.max(1) as usize {
            let base = (i.db0 as usize + blk) * 0x10000;
            let mut p = base;
            while p + 16 <= base + 0x10000 {
                let entsize = le32(self.img, p + 12) as usize;
                if entsize == 0 {
                    break;
                }
                let nl = le32(self.img, p + 8) as usize;
                out.push((String::from_utf8_lossy(&self.img[p + 16..p + 16 + nl]).into_owned(), le32(self.img, p), le32(self.img, p + 4) as i32));
                p += entsize;
            }
        }
        out
    }

    /// The contents of a file (stored contiguously from its first block).
    pub(crate) fn file(&self, ino: u32) -> &'a [u8] {
        let i = &self.inodes[ino as usize];
        let o = i.db0 as usize * 0x10000;
        &self.img[o..o + i.size as usize]
    }
}

/// The inner image inside a `pfs_image.dat` (behind its PFSC header).
pub(crate) fn pfsc_payload(dat: &[u8]) -> &[u8] {
    assert_eq!(&dat[..4], b"PFSC");
    let start = le64(dat, 0x20) as usize;
    let len = le64(dat, 0x28) as usize;
    &dat[start..start + len]
}
