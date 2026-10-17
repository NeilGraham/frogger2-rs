//! The outer PFS: a flat, signed, plaintext ("PLAINTEXT_NOAUTH") PFS holding
//! `pfs_image.dat` (the inner image) and `naps_pkg_layout.dat` (LibProsperoPkg 1.2.0
//! `ProsperoOuterPfsBuilder.BuildForPackageToFile` without encryption).

use super::pfs::{dirent_bytes, hash_path, sha3, Dirent, DIRENT_DIR, DIRENT_DOT, DIRENT_DOTDOT, DIRENT_FILE};
use std::io::{Read, Seek, SeekFrom, Write};

const BLOCK: usize = 65536;
const DIRECT: usize = 12;
const PER_INDIRECT: usize = 1820;

pub(crate) const PLAINTEXT_SEED: &[u8; 16] = b"PPRPLAIN-NOAUTH!";

/// A file of the outer PFS.
pub(crate) struct OuterFile<'a> {
    pub name: &'a str,
    pub len: u64,
    pub size_compressed: u64,
    pub reader: Box<dyn Read + 'a>,
}

/// What the package and its metadata need from the outer PFS.
pub(crate) struct OuterImage {
    pub pfs_size: u64,
    pub image_digests: Vec<[u8; 32]>,
    pub superblock_index: usize,
}

struct Node {
    block: usize,
    depth: u32,
    first: usize,
    count: usize,
    children: Vec<Node>,
}

fn pow1820(e: u32) -> usize {
    PER_INDIRECT.saturating_pow(e)
}

fn alloc_node(depth: u32, first: usize, count: usize, next: &mut usize) -> Node {
    let mut n = Node { block: *next, depth, first, count, children: vec![] };
    *next += 1;
    if depth > 1 {
        let per = pow1820(depth - 1);
        let (mut left, mut at) = (count, first);
        while left > 0 {
            let c = left.min(per);
            n.children.push(alloc_node(depth - 1, at, c, next));
            at += c;
            left -= c;
        }
    }
    n
}

/// `PlanIndirectTrees`: (inode level, root) per file.
fn plan_trees(counts: &[usize], first_meta: usize, next: &mut usize) -> Result<Vec<Vec<(usize, Node)>>, String> {
    *next = first_meta;
    let mut out = Vec::new();
    for &c in counts {
        let mut left = c.saturating_sub(DIRECT);
        let mut at = DIRECT;
        let mut trees = Vec::new();
        for level in 0..5 {
            if left == 0 {
                break;
            }
            let n = left.min(pow1820(level as u32 + 1));
            trees.push((level, alloc_node(level as u32 + 1, at, n, next)));
            at += n;
            left -= n;
        }
        if left > 0 {
            return Err("an outer PFS file exceeds the signed inode address space".into());
        }
        out.push(trees);
    }
    Ok(out)
}

/// `WriteIndirectTree`: writes the node's blocks through `write`, returns its hash.
type BlockWriter<'a> = dyn FnMut(usize, &[u8]) -> Result<[u8; 32], String> + 'a;

fn write_tree(n: &Node, file_first: usize, digests: &[[u8; 32]], write: &mut BlockWriter) -> Result<[u8; 32], String> {
    let mut b = vec![0u8; BLOCK];
    if n.depth == 1 {
        for i in 0..n.count {
            let blk = file_first + n.first + i;
            b[i * 36..i * 36 + 32].copy_from_slice(&digests[blk]);
            b[i * 36 + 32..i * 36 + 36].copy_from_slice(&(blk as i32).to_le_bytes());
        }
    } else {
        for (j, c) in n.children.iter().enumerate() {
            let h = write_tree(c, file_first, digests, write)?;
            b[j * 36..j * 36 + 32].copy_from_slice(&h);
            b[j * 36 + 32..j * 36 + 36].copy_from_slice(&(c.block as i32).to_le_bytes());
        }
    }
    write(n.block, &b)
}

/// A `DinodeS32` (712 bytes); `db`/`ib` are (digest, block) pairs. Times are 0.
#[allow(clippy::too_many_arguments)]
fn dinode32(
    out: &mut Vec<u8>,
    mode: u16,
    nlink: u16,
    flags: u32,
    size: i64,
    size_compressed: i64,
    blocks: u32,
    db: &[([u8; 32], i32)],
    ib: &[(usize, [u8; 32], i32)],
) {
    let start = out.len();
    out.extend_from_slice(&mode.to_le_bytes());
    out.extend_from_slice(&nlink.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size_compressed.to_le_bytes());
    out.resize(start + 96, 0); // times (48), uid, gid, unk1, unk2
    out.extend_from_slice(&blocks.to_le_bytes());
    for i in 0..DIRECT {
        let (sig, blk) = db.get(i).copied().unwrap_or(([0; 32], 0));
        out.extend_from_slice(&sig);
        out.extend_from_slice(&blk.to_le_bytes());
    }
    let ib_at = out.len();
    out.resize(ib_at + 5 * 36, 0);
    for &(level, sig, blk) in ib {
        out[ib_at + level * 36..ib_at + level * 36 + 32].copy_from_slice(&sig);
        out[ib_at + level * 36 + 32..ib_at + level * 36 + 36].copy_from_slice(&blk.to_le_bytes());
    }
    debug_assert_eq!(out.len() - start, 712);
}

/// The superblock (`PfsHeader` + `DinodeS64` of the inode table) and its ICV.
fn superblock(dinode_count: usize, ndblock: usize, inode_table: usize, inode_table_hash: &[u8; 32]) -> Vec<u8> {
    let mut b = vec![0u8; BLOCK];
    b[0..8].copy_from_slice(&2i64.to_le_bytes());
    b[8..16].copy_from_slice(&20_130_315i64.to_le_bytes());
    b[26] = 1; // read-only
    b[28..30].copy_from_slice(&(1u16 | 4 | 8).to_le_bytes());
    b[32..36].copy_from_slice(&65536u32.to_le_bytes());
    b[40..48].copy_from_slice(&1i64.to_le_bytes());
    b[48..56].copy_from_slice(&(dinode_count as i64).to_le_bytes());
    b[56..64].copy_from_slice(&(ndblock as i64).to_le_bytes());
    b[64..72].copy_from_slice(&1i64.to_le_bytes());
    // DinodeS64 at 80: mode 0, nlink 1, flags 0, size 65536 twice, times 0, blocks 1.
    let d = 80;
    b[d + 2..d + 4].copy_from_slice(&1u16.to_le_bytes());
    b[d + 8..d + 16].copy_from_slice(&65536i64.to_le_bytes());
    b[d + 16..d + 24].copy_from_slice(&65536i64.to_le_bytes());
    b[d + 96..d + 100].copy_from_slice(&1u32.to_le_bytes());
    b[184..216].copy_from_slice(inode_table_hash);
    b[216..224].copy_from_slice(&(inode_table as i64).to_le_bytes());
    b[876..880].copy_from_slice(&1i32.to_le_bytes());
    b[880..896].copy_from_slice(PLAINTEXT_SEED);
    let mut cover = b[..1440].to_vec();
    cover[896..928].fill(0);
    let icv = sha3(&cover);
    b[896..928].copy_from_slice(&icv);
    b
}

/// Writes the outer PFS of `files` at `offset` in `out`.
pub(crate) fn build<W: Write + Seek>(out: &mut W, offset: u64, mut files: Vec<OuterFile>) -> Result<OuterImage, String> {
    let counts: Vec<usize> = files.iter().map(|f| (f.len as usize).div_ceil(BLOCK).max(1)).collect();
    let mut firsts = Vec::with_capacity(files.len());
    let mut total = 0;
    for &c in &counts {
        firsts.push(total);
        total += c;
    }
    let (sb, inode_table, super_root, flt) = (total, total + 1, total + 2, total + 3);
    let mut next = 0;
    let trees = plan_trees(&counts, flt + 1, &mut next)?;
    let uroot = next;
    let nblocks = uroot + 1;
    // (Which blocks count as "signed" only matters for the XTS sectors of an encrypted image.)
    let mut digests = vec![[0u8; 32]; nblocks];
    let io = |e: std::io::Error| e.to_string();
    let write = |out: &mut W, idx: usize, bytes: &[u8], digests: &mut Vec<[u8; 32]>| -> Result<[u8; 32], String> {
        let h = sha3(bytes);
        digests[idx] = h;
        out.seek(SeekFrom::Start(offset + (idx * BLOCK) as u64)).map_err(io)?;
        out.write_all(bytes).map_err(io)?;
        Ok(h)
    };
    // The files' data blocks.
    let mut buf = vec![0u8; BLOCK];
    for (fi, f) in files.iter_mut().enumerate() {
        let mut left = f.len;
        for k in 0..counts[fi] {
            let n = left.min(BLOCK as u64) as usize;
            buf.fill(0);
            f.reader.read_exact(&mut buf[..n]).map_err(|e| format!("{}: {e}", f.name))?;
            write(out, firsts[fi] + k, &buf, &mut digests)?;
            left -= n as u64;
        }
    }
    // Super-root, flat path table, uroot.
    let block = |bytes: Vec<u8>| -> Vec<u8> {
        let mut b = bytes;
        b.resize(BLOCK, 0);
        b
    };
    let sr = block(dirent_bytes(&[Dirent::new(1, DIRENT_FILE, "inode_flat_path_table"), Dirent::new(2, DIRENT_DIR, "uroot")]));
    write(out, super_root, &sr, &mut digests)?;
    let mut fl = vec![0u8; 64];
    fl[0..4].copy_from_slice(&1u32.to_le_bytes());
    fl[4..8].copy_from_slice(&16u32.to_le_bytes());
    fl[8..12].copy_from_slice(&64u32.to_le_bytes());
    fl[32..36].copy_from_slice(&[127, b'F', b'L', b'T']);
    fl[44..48].copy_from_slice(&(files.len() as u32).to_le_bytes());
    fl[48..56].copy_from_slice(&10_577_419_142_525_243_217u64.to_le_bytes());
    fl[56..64].copy_from_slice(&701_355_796_979_237_965u64.to_le_bytes());
    for (i, f) in files.iter().enumerate() {
        fl.extend_from_slice(&hash_path(f.name).to_le_bytes());
        fl.extend_from_slice(&((3 + i as u64) | (i as u64) << 40).to_le_bytes());
    }
    write(out, flt, &block(fl), &mut digests)?;
    let mut ur = vec![Dirent::new(2, DIRENT_DOT, "."), Dirent::new(2, DIRENT_DOTDOT, "..")];
    for (i, f) in files.iter().enumerate() {
        ur.push(Dirent::new(3 + i as u32, DIRENT_FILE, f.name));
    }
    write(out, uroot, &block(dirent_bytes(&ur)), &mut digests)?;
    // Inodes: the super-root, the flat path table, uroot, then the files.
    let mut inodes = Vec::with_capacity(BLOCK);
    let flt_size = 64 + files.len() as i64 * 16;
    dinode32(&mut inodes, 16749, 1, 131_084, 65536, 65536, 1, &[(digests[super_root], super_root as i32)], &[]);
    dinode32(&mut inodes, 33133, 1, 131_084, flt_size, flt_size, 1, &[(digests[flt], flt as i32)], &[]);
    dinode32(&mut inodes, 16749, 3, 12, 65536, 65536, 1, &[(digests[uroot], uroot as i32)], &[]);
    for (i, f) in files.iter().enumerate() {
        let db: Vec<([u8; 32], i32)> = (0..counts[i].min(DIRECT)).map(|k| (digests[firsts[i] + k], (firsts[i] + k) as i32)).collect();
        let mut ib = Vec::new();
        for (level, root) in &trees[i] {
            let snapshot = digests.clone();
            let mut w = |idx: usize, bytes: &[u8]| -> Result<[u8; 32], String> { write(out, idx, bytes, &mut digests) };
            let h = write_tree(root, firsts[i], &snapshot, &mut w)?;
            ib.push((*level, h, root.block as i32));
        }
        dinode32(&mut inodes, 33133, 1, 13, f.len as i64, f.size_compressed as i64, counts[i] as u32, &db, &ib);
    }
    if inodes.len() > BLOCK {
        return Err("the outer PFS inode table exceeds one block".into());
    }
    let it = block(inodes);
    let it_hash = write(out, inode_table, &it, &mut digests)?;
    let sblock = superblock(3 + files.len(), nblocks, inode_table, &it_hash);
    write(out, sb, &sblock, &mut digests)?;
    Ok(OuterImage { pfs_size: (nblocks * BLOCK) as u64, image_digests: digests, superblock_index: sb })
}
