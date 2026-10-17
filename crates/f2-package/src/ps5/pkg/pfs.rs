//! Pieces shared by the inner image and the outer PFS: directory entries, the flat path
//! table and its path hash, SHA3-256.

use sha3::{Digest, Sha3_256};

pub(crate) fn sha3(data: &[u8]) -> [u8; 32] {
    Sha3_256::digest(data).into()
}

pub(crate) const DIRENT_FILE: i32 = 2;
pub(crate) const DIRENT_DIR: i32 = 3;
pub(crate) const DIRENT_DOT: i32 = 4;
pub(crate) const DIRENT_DOTDOT: i32 = 5;

/// A directory entry (`PfsDirent`): 16-byte header and the name, padded to 8 bytes.
#[derive(Clone)]
pub(crate) struct Dirent {
    pub inode: u32,
    pub kind: i32,
    pub name: String,
    pub ent_size: i32,
}

impl Dirent {
    pub fn new(inode: u32, kind: i32, name: &str) -> Dirent {
        let mut size = name.len() as i32 + 17;
        if size % 8 != 0 {
            size += 8 - size % 8;
        }
        Dirent { inode, kind, name: name.to_string(), ent_size: size }
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(&self.inode.to_le_bytes());
        out.extend_from_slice(&self.kind.to_le_bytes());
        out.extend_from_slice(&(self.name.len() as i32).to_le_bytes());
        out.extend_from_slice(&self.ent_size.to_le_bytes());
        out.extend_from_slice(self.name.as_bytes());
        out.resize(start + self.ent_size as usize, 0);
    }
}

pub(crate) fn dirent_bytes(dirents: &[Dirent]) -> Vec<u8> {
    let mut out = Vec::new();
    for d in dirents {
        d.write(&mut out);
    }
    out
}

const SEED0: u64 = 10_577_419_142_525_243_217;
const SEED1: u64 = 701_355_796_979_237_965;
const ROUND: u64 = 0x8000_0000_8000_8081;

/// The flat path table's hash of a path (`ProsperoPs5FlatPathTable.HashPath`): leading
/// slashes dropped, ASCII letters upper-cased, backslashes as slashes.
pub(crate) fn hash_path(path: &str) -> u64 {
    let trimmed = path.trim_start_matches(['/', '\\']);
    let bytes: Vec<u8> = trimmed
        .bytes()
        .map(|b| match b {
            b'a'..=b'z' => b - 32,
            b'\\' => b'/',
            _ => b,
        })
        .collect();
    hash_bytes(&bytes)
}

fn hash_bytes(s: &[u8]) -> u64 {
    let (mut a, mut b, mut c) = (SEED0, SEED0.rotate_left(11), SEED0.rotate_left(23));
    let mut tail: u64 = 0;
    if !s.is_empty() {
        let words = (s.len() - 1) >> 3;
        let mut p = 0;
        for _ in 0..words {
            a ^= u64::from_le_bytes(s[p..p + 8].try_into().unwrap());
            p += 8;
            let n11 = ((c ^ b).rotate_left(5) ^ a).rotate_right(11);
            let n12 = ((c ^ a).rotate_left(17) ^ b).rotate_left(11);
            c = ((b ^ a).rotate_left(1) ^ c).rotate_right(5);
            a = (!n12 & c) ^ n11 ^ ROUND;
            b = (!c & n11) ^ n12;
            c ^= !n11 & n12;
        }
        let rest = ((s.len() - 1) & 7) + 1;
        for j in 0..rest {
            tail |= (s[p + j] as u64) << (8 * j);
        }
    }
    let (n14, n15) = (b, c);
    let n16 = tail ^ a ^ SEED1;
    let x = (n15 ^ n14).rotate_left(5) ^ n16;
    let x2 = (n15 ^ n16).rotate_left(17) ^ n14;
    let n15b = (n14 ^ n16).rotate_left(1) ^ n15;
    (!x2.rotate_left(11) & n15b.rotate_right(5)) ^ x.rotate_right(11) ^ ROUND
}

/// The flat path table (`ProsperoPs5FlatPathTable.Write`): a 64-byte header and the
/// (hash, packed) entries sorted by hash.
pub(crate) fn flat_path_table(mut entries: Vec<(u64, u64)>) -> Vec<u8> {
    entries.sort_by_key(|e| e.0);
    let mut out = vec![0u8; 64];
    out[0..4].copy_from_slice(&1u32.to_le_bytes());
    out[4] = 16;
    out[8..12].copy_from_slice(&64u32.to_le_bytes());
    out[32] = 127;
    out[33..36].copy_from_slice(b"FLT");
    out[44..48].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    out[48..64].copy_from_slice(&[81, 79, 162, 38, 171, 138, 202, 146, 77, 196, 27, 164, 97, 183, 187, 9]);
    for (h, p) in entries {
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&p.to_le_bytes());
    }
    out
}

pub(crate) fn pack_inode_entry(inode: u32, is_dir: bool, is_subtree: bool, afid: u32, is_empty: bool) -> u64 {
    let mut v = inode as u64 & 0xFFFFFF;
    if is_empty {
        v |= 0x2000_0000;
    }
    if is_dir {
        v |= 0x4000_0000;
    }
    if is_subtree {
        v |= 0x8000_0000;
    }
    v | ((if is_dir { 0xFFFFFF } else { afid as u64 }) << 40)
}

pub(crate) fn pack_apr_entry(size: i64, afid: u32) -> u64 {
    (size as u64 & 0xFF_FFFF_FFFF) | ((afid as u64) << 40)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirent_sizes_round_to_eight() {
        assert_eq!(Dirent::new(1, DIRENT_FILE, ".").ent_size, 24);
        assert_eq!(Dirent::new(1, DIRENT_FILE, "inode_flat_path_table").ent_size, 40);
        let mut v = Vec::new();
        Dirent::new(7, DIRENT_DIR, "uroot").write(&mut v);
        assert_eq!(v.len(), 24);
        assert_eq!(&v[16..21], b"uroot");
    }

    #[test]
    fn hash_ignores_case_and_leading_slash() {
        assert_eq!(hash_path("/data/a.txt"), hash_path("DATA/A.TXT"));
        assert_ne!(hash_path("a"), hash_path("b"));
    }
}
