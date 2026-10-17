//! The inner image (`pfs_image.dat`): the PS5 PFS (superblock version 2) holding the app's
//! files, data compressed in 256 KiB Kraken blocks, the inodes, directories and flat path
//! tables in a compressed metadata region at its end (LibProsperoPkg 1.2.0
//! `ProsperoPs5InnerImageAssembler`, the "nwonly" publisher layout the package builder uses).

use super::naps;
use super::pfs::{dirent_bytes, flat_path_table, hash_path, pack_apr_entry, pack_inode_entry, sha3, Dirent, DIRENT_DIR, DIRENT_DOT, DIRENT_DOTDOT, DIRENT_FILE};
use crate::ps5::kraken::{self, EncodedBlock};
use std::collections::{BTreeMap, HashMap};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const BLOCK: i64 = 65536;
const UBLOCK: i64 = 262_144;

/// Where an inner file's bytes come from.
#[derive(Clone)]
pub(crate) enum Source {
    Path(PathBuf),
    Bytes(Vec<u8>),
}

impl Source {
    pub fn read(&self) -> Result<Vec<u8>, String> {
        match self {
            Source::Path(p) => std::fs::read(p).map_err(|e| format!("{}: {e}", p.display())),
            Source::Bytes(b) => Ok(b.clone()),
        }
    }
}

/// One file of the inner image: its path in the image (`/eboot.bin`) and source.
pub(crate) struct InputFile {
    pub path: String,
    pub len: i64,
    pub source: Source,
}

/// One data block as written (`ProsperoInnerDataBlockChunk`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockChunk {
    pub compressed_size: i32,
    pub uncompressed_size: i32,
    pub stored: bool,
    pub multi_chunk: bool,
    pub first_chunk_compressed_size: i32,
    pub boundary_flags: i32,
    pub on_disk_offset: i64,
}

impl BlockChunk {
    pub fn is_stored(&self) -> bool {
        self.stored
    }
}

/// One block of the compressed metadata (`ProsperoInnerMetaBlockChunk`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct MetaChunk {
    pub compressed_size: i32,
    pub uncompressed_size: i32,
    pub multi_chunk: bool,
    pub first_chunk_compressed_size: i32,
    pub boundary_flags: i32,
}

/// A data file of the image in AFID order (`ProsperoPs5InnerPlacement`).
pub(crate) struct Placement {
    pub afid: u32,
    pub path: String,
    pub on_disk_offset: i64,
    pub logical_offset: i64,
    pub on_disk_size: i64,
    pub uncompressed_size: i64,
    pub store_raw: bool,
    pub blocks: Vec<BlockChunk>,
    pub source: Source,
}

/// A node of the image for the later stages (`ProsperoPs5MetaNode`, the parts they use).
pub(crate) struct Node {
    pub full_path: String,
    pub is_directory: bool,
    pub parent_inode: i32,
    pub mode: u16,
    pub afid: u32,
    pub size: i64,
}

/// The built image (written to `spool`) and what the NAPS table and the package need.
pub(crate) struct InnerImage {
    pub image_length: i64,
    pub ndblock: i64,
    pub data_end_logical: i64,
    pub meta_base_logical: i64,
    pub block_info_on_disk: i64,
    pub metadata_on_disk: i64,
    pub metadata_blocks: Vec<MetaChunk>,
    pub afid_logical_offsets: Vec<i64>,
    /// AFID slots left empty (indexes into `afid_logical_offsets`).
    pub sparse_afid_holes: Vec<usize>,
    pub empty_file_count: usize,
    pub placements: Vec<Placement>,
    pub nodes: Vec<Node>,
    pub deduplicated_blocks: usize,
}

struct Dir {
    name: String,
    full_path: String,
    parent: Option<usize>,
    sub_dirs: Vec<usize>,
    files: Vec<usize>,
    inode: u32,
    dirent_offset: i32,
    dirents: Vec<Dirent>,
}

struct FileNode {
    name: String,
    full_path: String,
    input: usize,
    length: i64,
    is_executable: bool,
    parent: usize,
    inode: u32,
    afid: u32,
    whole_block_raw: bool,
    logical_offset: i64,
    on_disk_offset: i64,
    on_disk_size: i64,
    store_raw: bool,
    blocks: Vec<BlockChunk>,
    dirent_offset: i32,
}

fn is_executable_magic(data: &[u8]) -> bool {
    data.len() >= 4 && matches!(u32::from_le_bytes(data[..4].try_into().unwrap()), 0x1D3D_154F | 0x464C_457F | 0xEEF5_1454)
}

/// Data the PFS may not compress (`IsInputDataDisallowedForPfs`): SELF / ELF / PFSC heads.
fn compression_disallowed(data: &[u8]) -> bool {
    data.len() >= 4 && matches!(u32::from_le_bytes(data[..4].try_into().unwrap()), 0xEEF5_1454 | 0x1D3D_154F | 0x4353_4650 | 0x464C_457F)
}

fn has_executable_extension(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    l.ends_with(".elf") || l.ends_with(".self") || l.ends_with(".prx") || l.ends_with(".sprx")
}

fn potential_executable_destination(p: &str) -> bool {
    p == "/eboot.bin" || p.starts_with("/fakelib/") || p.starts_with("/sce_module/") || p.starts_with("/sce_sys/about/")
}

fn round_up(v: i64, g: i64) -> i64 {
    (v + g - 1) / g * g
}

fn payload_blocks(len: usize) -> i64 {
    (len.div_ceil(65536)).max(1) as i64
}

/// `KeepSdkCompressedPayload`: the compressed form is kept when it saves enough.
pub(crate) fn keep_compressed(compressed: usize, uncompressed: usize) -> bool {
    if uncompressed <= 256 {
        return false;
    }
    let halves = if uncompressed <= 131_072 { 1 } else { 2 };
    compressed as i64 + 5 + 3 * halves <= (uncompressed as i64 * 15) >> 4
}

/// One 256 KiB block compressed for the data region: (chunk without offset, bytes to write).
fn encode_data_block(raw: &[u8], allow: bool) -> (BlockChunk, Vec<u8>) {
    let count = raw.len();
    let encoded: Option<EncodedBlock> = if allow { kraken::encode_block(raw) } else { None };
    if let Some(e) = encoded {
        if count > 256 && keep_compressed(e.payload.len(), count) {
            let chunk = BlockChunk {
                compressed_size: e.payload.len() as i32,
                uncompressed_size: count as i32,
                stored: false,
                multi_chunk: e.multi_chunk,
                first_chunk_compressed_size: e.first_chunk_comp_size as i32,
                boundary_flags: e.boundary_flags,
                on_disk_offset: -1,
            };
            return (chunk, e.payload);
        }
    }
    let chunk = BlockChunk {
        compressed_size: count as i32,
        uncompressed_size: count as i32,
        stored: true,
        multi_chunk: false,
        first_chunk_compressed_size: 0,
        boundary_flags: if count > 131_072 { 204 } else { 12 },
        on_disk_offset: -1,
    };
    (chunk, raw.to_vec())
}

/// Compresses `data` in 256 KiB blocks on all cores; results in order.
fn encode_blocks(data: &[u8], allow: bool) -> Vec<(BlockChunk, Vec<u8>, [u8; 32])> {
    let blocks: Vec<&[u8]> = data.chunks(UBLOCK as usize).collect();
    let n = blocks.len();
    let workers = std::thread::available_parallelism().map_or(1, |p| p.get()).min(n.max(1));
    if workers <= 1 {
        return blocks
            .iter()
            .map(|b| {
                let (c, d) = encode_data_block(b, allow);
                (c, d, sha3(b))
            })
            .collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<(BlockChunk, Vec<u8>, [u8; 32])>>> = (0..n).map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let (c, d) = encode_data_block(blocks[i], allow);
                *results[i].lock().unwrap() = Some((c, d, sha3(blocks[i])));
            });
        }
    });
    results.into_iter().map(|m| m.into_inner().unwrap().unwrap()).collect()
}

/// The PFS v3 container blocks of the metadata (`CompressPayload` of `ProsperoPs5InnerImageBuilder`):
/// the concatenated block payloads and their records; the plaintext itself when no block
/// compresses.
fn compress_metadata(plain: &[u8]) -> (Vec<u8>, Vec<MetaChunk>) {
    let mut out = Vec::new();
    let mut chunks = Vec::new();
    let mut all_stored = true;
    for block in plain.chunks(UBLOCK as usize) {
        let len = block.len();
        let enc = if len == 0 { None } else { kraken::encode_block(block) };
        match enc.filter(|e| keep_compressed(e.payload.len(), len)) {
            Some(e) => {
                all_stored = false;
                let first = if e.multi_chunk { e.first_chunk_comp_size } else { e.payload.len() };
                let hint = (first.max(1) - 1).min(131_071) as i32;
                chunks.push(MetaChunk {
                    compressed_size: e.payload.len() as i32,
                    uncompressed_size: len as i32,
                    multi_chunk: e.payload.len() != len && len > 131_072,
                    first_chunk_compressed_size: hint + 1,
                    boundary_flags: e.boundary_flags,
                });
                out.extend_from_slice(&e.payload);
            }
            None => {
                let first = len.min(131_072);
                chunks.push(MetaChunk {
                    compressed_size: len as i32,
                    uncompressed_size: len as i32,
                    multi_chunk: false,
                    first_chunk_compressed_size: (first.max(1) - 1).min(131_071) as i32 + 1,
                    boundary_flags: 0xC | if len > 131_072 { 192 } else { 0 },
                });
                out.extend_from_slice(block);
            }
        }
    }
    if all_stored {
        let chunks = plain
            .chunks(UBLOCK as usize)
            .map(|b| MetaChunk {
                compressed_size: b.len() as i32,
                uncompressed_size: b.len() as i32,
                multi_chunk: b.len() > 131_072,
                first_chunk_compressed_size: b.len().min(131_072) as i32,
                boundary_flags: 0,
            })
            .collect();
        return (plain.to_vec(), chunks);
    }
    (out, chunks)
}

fn block_info_table(data_end: i64) -> [u8; 256] {
    let num = ((data_end & 0xFFFF) * 4) as u32;
    let num2 = 2_621_436u32.wrapping_sub(num) & 0xFF_FFFF;
    let entry = ((num2 & 0xFF) << 16) | (num2 & 0xFF00) | ((num2 >> 16) & 0xFF);
    let mut t = [0u8; 256];
    for i in 0..32 {
        let v = if i < 31 { 16_580_391u32 } else { entry };
        t[i * 8..i * 8 + 4].copy_from_slice(&v.to_le_bytes());
        t[i * 8 + 4..i * 8 + 8].copy_from_slice(&4_194_307u32.to_le_bytes());
    }
    t
}

/// The inode and superblock writer (`ProsperoPs5InnerMetadata`), build time 0.
struct MetaNode {
    inode: u32,
    is_dir: bool,
    mode: u16,
    nlink: u16,
    flags: u32,
    afid: u32,
    size: i64,
    logical_offset: u64,
    parent_inode: i32,
    dirent_offset: i32,
}

fn write_inode(dst: &mut [u8], n: &MetaNode) {
    dst[0..2].copy_from_slice(&n.mode.to_le_bytes());
    dst[2..4].copy_from_slice(&n.nlink.to_le_bytes());
    dst[4..8].copy_from_slice(&n.flags.to_le_bytes());
    dst[8..16].copy_from_slice(&n.size.to_le_bytes());
    dst[16..24].copy_from_slice(&n.size.to_le_bytes());
    // Times (24..72): 0.
    dst[96..104].copy_from_slice(&n.logical_offset.to_le_bytes());
    let has_parent = n.parent_inode >= 0;
    let afid: i32 = if has_parent && !n.is_dir { n.afid as i32 } else { -1 };
    let parent = if has_parent { n.parent_inode } else { -1 };
    let dirent = if has_parent { n.dirent_offset } else { -1 };
    dst[104..108].copy_from_slice(&afid.to_le_bytes());
    dst[108..112].copy_from_slice(&parent.to_le_bytes());
    dst[112..116].copy_from_slice(&dirent.to_le_bytes());
}

fn metadata_plaintext(nodes: &[MetaNode], ndblock: i64, payloads: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let inode_blocks = nodes.len().div_ceil(390) as i64;
    let payload_total: i64 = payloads.iter().map(|p| payload_blocks(p.len())).sum();
    let total = 1 + inode_blocks + payload_total;
    let mut out = vec![0u8; (total * BLOCK) as usize];
    let meta_start = ndblock - total;
    if meta_start < 0 {
        return Err("inner metadata exceeds the mount".into());
    }
    // Superblock.
    let sb = &mut out[..873];
    sb[0..8].copy_from_slice(&2i64.to_le_bytes());
    sb[8..16].copy_from_slice(&20_130_315i64.to_le_bytes());
    sb[26] = 1;
    sb[28..30].copy_from_slice(&24u16.to_le_bytes());
    sb[32..36].copy_from_slice(&65536u32.to_le_bytes());
    sb[40..48].copy_from_slice(&1i64.to_le_bytes());
    sb[48..56].copy_from_slice(&(nodes.len() as i64).to_le_bytes());
    sb[56..64].copy_from_slice(&ndblock.to_le_bytes());
    sb[64..72].copy_from_slice(&inode_blocks.to_le_bytes());
    sb[82..84].copy_from_slice(&1u16.to_le_bytes());
    sb[84..88].copy_from_slice(&16u32.to_le_bytes());
    sb[88..96].copy_from_slice(&(inode_blocks * BLOCK).to_le_bytes());
    sb[96..104].copy_from_slice(&(inode_blocks * BLOCK).to_le_bytes());
    sb[176..184].copy_from_slice(&inode_blocks.to_le_bytes());
    sb[216..224].copy_from_slice(&(meta_start + 1).to_le_bytes());
    sb[872] = 1;
    for (i, n) in nodes.iter().enumerate() {
        let at = (1 + i / 390) * 65536 + (i % 390) * 168;
        write_inode(&mut out[at..at + 168], n);
    }
    let mut at = ((1 + inode_blocks) * BLOCK) as usize;
    for p in payloads {
        out[at..at + p.len()].copy_from_slice(p);
        at += (payload_blocks(p.len()) * BLOCK) as usize;
    }
    Ok(out)
}

/// Names in `sce_sys/` that become package entries instead of inner files.
fn is_cnt_entry_name(name: &str) -> bool {
    const FIXED: [&str; 44] = [
        "license.dat",
        "license.info",
        "nptitle.dat",
        "npbind.dat",
        "selfinfo.dat",
        "imageinfo.dat",
        "target-deltainfo.dat",
        "origin-deltainfo.dat",
        "psreserved.dat",
        "param.sfo",
        "playgo-chunk.dat",
        "playgo-chunk.sha",
        "playgo-manifest.xml",
        "pronunciation.sig",
        "pronunciation.xml",
        "pic1.png",
        "pubtoolinfo.dat",
        "app/playgo-chunk.dat",
        "app/playgo-chunk.sha",
        "app/playgo-manifest.xml",
        "shareparam.json",
        "shareoverlayimage.png",
        "save_data.png",
        "shareprivacyguardimage.png",
        "icon0.png",
        "pic0.png",
        "snd0.at9",
        "changeinfo/changeinfo.xml",
        "icon0.dds",
        "pic0.dds",
        "pic1.dds",
        "pic2.png",
        "pic2.dds",
        "playgo-hash-table.dat",
        "playgo-ficm.dat",
        "playgo-scenario.json",
        "trophy2/trophy00.ucp",
        "uds/uds00.ucp",
        "uds/npbind.dat",
        "trophy2/npbind.dat",
        "param.json",
        "pfs-region-hints.json",
        ".digests",
        ".entry_keys",
    ];
    if FIXED.contains(&name) {
        return true;
    }
    // The numbered families (scenario art, icon/pic variants, changeinfo, keymaps, trophies).
    let numbered = |prefix: &str, suffix: &str| {
        name.strip_prefix(prefix).and_then(|r| r.strip_suffix(suffix)).is_some_and(|d| d.len() == 2 && d.bytes().all(|b| b.is_ascii_digit()))
    };
    numbered("playgo-scenario", ".png")
        || numbered("playgo-scenario", ".dds")
        || numbered("icon0_", ".png")
        || numbered("icon0_", ".dds")
        || numbered("pic1_", ".png")
        || numbered("pic1_", ".dds")
        || numbered("changeinfo/changeinfo_", ".xml")
        || numbered("trophy/trophy", ".trp")
        || name.starts_with("keymap_rp/")
}

/// Whether `path` (`/sce_sys/icon0.png`) stays out of the inner image.
pub(crate) fn excluded_from_inner(path: &str) -> bool {
    path.strip_prefix("/sce_sys/").is_some_and(is_cnt_entry_name)
}

/// Builds the inner image of `files` (and the empty directories `dirs`, paths without a
/// leading slash) into `spool`.
pub(crate) fn build(files: Vec<InputFile>, dirs: &[String], spool: &Path, log: &dyn Fn(&str)) -> Result<InnerImage, String> {
    if files.is_empty() {
        return Err("the inner image needs at least one file".into());
    }
    // The tree.
    let mut dir_list: Vec<Dir> = vec![Dir { name: "uroot".into(), full_path: String::new(), parent: None, sub_dirs: vec![], files: vec![], inode: 0, dirent_offset: -1, dirents: vec![] }];
    let mut lookup: HashMap<String, usize> = HashMap::from([(String::new(), 0)]);
    fn get_dir(path: &str, dir_list: &mut Vec<Dir>, lookup: &mut HashMap<String, usize>) -> usize {
        if let Some(&d) = lookup.get(path) {
            return d;
        }
        let (parent_path, name) = match path.rfind('/') {
            Some(i) => (&path[..i], &path[i + 1..]),
            None => ("", path),
        };
        let parent = get_dir(parent_path, dir_list, lookup);
        let id = dir_list.len();
        dir_list.push(Dir { name: name.into(), full_path: path.into(), parent: Some(parent), sub_dirs: vec![], files: vec![], inode: 0, dirent_offset: -1, dirents: vec![] });
        dir_list[parent].sub_dirs.push(id);
        lookup.insert(path.into(), id);
        id
    }
    for d in dirs {
        let d = d.trim_matches('/');
        if !d.is_empty() {
            get_dir(d, &mut dir_list, &mut lookup);
        }
    }
    let mut nodes: Vec<FileNode> = Vec::with_capacity(files.len());
    let mut has_eboot = false;
    for (i, f) in files.iter().enumerate() {
        let rel = f.path.trim_start_matches('/');
        if rel == "eboot.bin" {
            has_eboot = true;
        }
        let (parent_path, name) = match rel.rfind('/') {
            Some(i) => (&rel[..i], &rel[i + 1..]),
            None => ("", rel),
        };
        let parent = get_dir(parent_path, &mut dir_list, &mut lookup);
        let full_path = format!("/{rel}");
        let exec = has_executable_extension(&full_path)
            || (potential_executable_destination(&full_path) && {
                let head = match &f.source {
                    Source::Bytes(b) => b.iter().take(4).copied().collect::<Vec<u8>>(),
                    Source::Path(p) => {
                        let mut h = [0u8; 4];
                        let n = std::fs::File::open(p).and_then(|mut fh| std::io::Read::read(&mut fh, &mut h)).unwrap_or(0);
                        h[..n].to_vec()
                    }
                };
                is_executable_magic(&head)
            });
        let id = nodes.len();
        nodes.push(FileNode {
            name: name.into(),
            full_path,
            input: i,
            length: f.len,
            is_executable: exec,
            parent,
            inode: 0,
            afid: 0,
            whole_block_raw: false,
            logical_offset: 0,
            on_disk_offset: 0,
            on_disk_size: 0,
            store_raw: false,
            blocks: vec![],
            dirent_offset: -1,
        });
        dir_list[parent].files.push(id);
    }
    if !has_eboot {
        get_dir("data", &mut dir_list, &mut lookup);
    }
    let sorted_dirs = |dir_list: &Vec<Dir>, d: usize| -> Vec<usize> {
        let mut v = dir_list[d].sub_dirs.clone();
        v.sort_by(|a, b| dir_list[*a].name.as_bytes().cmp(dir_list[*b].name.as_bytes()));
        v
    };
    let sorted_files = |dir_list: &Vec<Dir>, nodes: &Vec<FileNode>, d: usize| -> Vec<usize> {
        let mut v = dir_list[d].files.clone();
        v.sort_by(|a, b| nodes[*a].name.as_bytes().cmp(nodes[*b].name.as_bytes()));
        v
    };
    // Inodes: 0..3 the metadata files, then directories (pre-order), then files (per
    // directory in post-order, by name).
    fn pre_order(d: usize, dl: &Vec<Dir>, sd: &dyn Fn(&Vec<Dir>, usize) -> Vec<usize>, out: &mut Vec<usize>) {
        out.push(d);
        for c in sd(dl, d) {
            pre_order(c, dl, sd, out);
        }
    }
    fn post_order(d: usize, dl: &Vec<Dir>, sd: &dyn Fn(&Vec<Dir>, usize) -> Vec<usize>, out: &mut Vec<usize>) {
        for c in sd(dl, d) {
            post_order(c, dl, sd, out);
        }
        out.push(d);
    }
    let mut dirs_pre = Vec::new();
    pre_order(0, &dir_list, &sorted_dirs, &mut dirs_pre);
    let mut next_inode = 4u32;
    for &d in &dirs_pre {
        dir_list[d].inode = next_inode;
        next_inode += 1;
    }
    let mut dirs_post = Vec::new();
    post_order(0, &dir_list, &sorted_dirs, &mut dirs_post);
    let mut file_order: Vec<usize> = Vec::new();
    for &d in &dirs_post {
        for f in sorted_files(&dir_list, &nodes, d) {
            nodes[f].inode = next_inode;
            next_inode += 1;
            file_order.push(f);
        }
    }
    // AFID candidates: sce_sys first (keystone, executables, the rest, pfs-version.dat),
    // then everything else in entry (name) order.
    let mut candidates: Vec<usize> = Vec::new();
    let sce_sys = dir_list[0].sub_dirs.iter().copied().find(|&d| dir_list[d].name == "sce_sys");
    if let Some(s) = sce_sys {
        fn files_pre(d: usize, dl: &Vec<Dir>, nodes: &Vec<FileNode>, out: &mut Vec<usize>) {
            let mut f = dl[d].files.clone();
            f.sort_by(|a, b| nodes[*a].name.as_bytes().cmp(nodes[*b].name.as_bytes()));
            out.extend(f);
            let mut s = dl[d].sub_dirs.clone();
            s.sort_by(|a, b| dl[*a].name.as_bytes().cmp(dl[*b].name.as_bytes()));
            for c in s {
                files_pre(c, dl, nodes, out);
            }
        }
        let mut v = Vec::new();
        files_pre(s, &dir_list, &nodes, &mut v);
        let rank = |n: &FileNode| -> i32 {
            if n.full_path == "/sce_sys/keystone" {
                0
            } else if n.is_executable {
                1
            } else if n.full_path == "/sce_sys/pfs-version.dat" {
                3
            } else {
                2
            }
        };
        v.sort_by(|a, b| rank(&nodes[*a]).cmp(&rank(&nodes[*b])).then(nodes[*a].full_path.as_bytes().cmp(nodes[*b].full_path.as_bytes())));
        candidates.extend(v);
    }
    fn entry_order(d: usize, excluded: Option<usize>, dl: &Vec<Dir>, nodes: &Vec<FileNode>, out: &mut Vec<usize>) {
        let mut entries: Vec<(&str, Option<usize>, Option<usize>)> = Vec::new();
        for &c in &dl[d].sub_dirs {
            if Some(c) != excluded {
                entries.push((&dl[c].name, Some(c), None));
            }
        }
        for &f in &dl[d].files {
            entries.push((&nodes[f].name, None, Some(f)));
        }
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        for (_, dir, file) in entries {
            if let Some(f) = file {
                out.push(f);
            } else if let Some(c) = dir {
                entry_order(c, excluded, dl, nodes, out);
            }
        }
    }
    entry_order(0, sce_sys, &dir_list, &nodes, &mut candidates);
    let empty_files: Vec<usize> = candidates.iter().copied().filter(|&f| nodes[f].length == 0).collect();
    candidates.retain(|&f| nodes[f].length != 0);
    // AFID slots: at most 28 entries in the first 256 KiB window and 30 in the others; a
    // raw-stored file next to another costs two.
    let mut slots: Vec<Option<usize>> = Vec::new();
    {
        let (mut cursor, mut window, mut used) = (0i64, -1i64, 0i32);
        let (mut prev_raw, mut first) = (false, true);
        for &f in &candidates {
            let raw = nodes[f].full_path == "/sce_sys/keystone" || nodes[f].is_executable;
            nodes[f].whole_block_raw = raw;
            if cursor / UBLOCK != window {
                window = cursor / UBLOCK;
                used = 0;
            }
            let cap = if window == 0 { 28 } else { 30 };
            let cost = if first || !(raw || prev_raw) { 1 } else { 2 };
            if used + cost > cap {
                slots.push(None);
                cursor += UBLOCK;
                window = cursor / UBLOCK;
                used = 0;
            }
            nodes[f].afid = slots.len() as u32;
            slots.push(Some(f));
            cursor += nodes[f].length;
            used += cost;
            prev_raw = raw;
            first = false;
        }
    }
    let base = slots.len() as u32 + 2;
    for (i, &f) in empty_files.iter().enumerate() {
        nodes[f].afid = base + i as u32;
    }
    let data_files = slots.iter().filter(|s| s.is_some()).count();
    log(&format!(
        "Inner image: {} data files, {} empty, {} sparse slots, {} directories",
        data_files,
        empty_files.len(),
        slots.len() - data_files,
        dir_list.len()
    ));
    // Data: logical layout in AFID order; on disk, raw files 64 KiB aligned, the rest in
    // compressed 256 KiB blocks with identical blocks stored once.
    let mut out = std::fs::File::create(spool).map_err(|e| format!("{}: {e}", spool.display()))?;
    let mut logical: i64 = 0;
    let mut phys: i64 = 0;
    let mut afid_offsets = vec![0i64; slots.len()];
    let mut holes = Vec::new();
    let mut dedup: HashMap<(i32, i32, bool, bool, i32, i32, [u8; 32]), i64> = HashMap::new();
    let mut dedup_count = 0usize;
    let total: i64 = candidates.iter().map(|&f| nodes[f].length).sum();
    let mut done: i64 = 0;
    let mut last_pct = -1;
    for (slot, &entry) in slots.iter().enumerate() {
        afid_offsets[slot] = logical;
        let Some(f) = entry else {
            holes.push(slot);
            logical += UBLOCK;
            continue;
        };
        nodes[f].logical_offset = logical;
        logical += nodes[f].length;
        let data = files[nodes[f].input].source.read()?;
        if data.len() as i64 != nodes[f].length {
            return Err(format!("{} changed size while packing", nodes[f].full_path));
        }
        if nodes[f].whole_block_raw {
            phys = round_up(phys, BLOCK);
            nodes[f].on_disk_offset = phys;
            out.seek(SeekFrom::Start(phys as u64)).map_err(|e| e.to_string())?;
            out.write_all(&data).map_err(|e| e.to_string())?;
            nodes[f].store_raw = true;
            nodes[f].on_disk_size = nodes[f].length;
            phys = round_up(phys + nodes[f].length, BLOCK);
        } else {
            let allow = !compression_disallowed(&data);
            let encoded = encode_blocks(&data, allow);
            let mut chunks = Vec::with_capacity(encoded.len());
            for (mut chunk, bytes, digest) in encoded {
                let key = (chunk.compressed_size, chunk.uncompressed_size, chunk.stored, chunk.multi_chunk, chunk.first_chunk_compressed_size, chunk.boundary_flags, digest);
                let at = match dedup.get(&key) {
                    Some(&at) => {
                        dedup_count += 1;
                        at
                    }
                    None => {
                        out.seek(SeekFrom::Start(phys as u64)).map_err(|e| e.to_string())?;
                        out.write_all(&bytes).map_err(|e| e.to_string())?;
                        let at = phys;
                        phys += bytes.len() as i64;
                        dedup.insert(key, at);
                        at
                    }
                };
                chunk.on_disk_offset = at;
                chunks.push(chunk);
            }
            nodes[f].store_raw = chunks.iter().all(|c| c.stored);
            nodes[f].on_disk_offset = chunks[0].on_disk_offset;
            nodes[f].on_disk_size = chunks.iter().map(|c| c.compressed_size as i64).sum();
            nodes[f].blocks = chunks;
        }
        done += nodes[f].length;
        let pct = if total == 0 { 100 } else { (done * 100 / total) as i32 };
        if pct / 10 != last_pct / 10 {
            last_pct = pct;
            log(&format!("Compressing the game data: {pct}%"));
        }
    }
    for &f in &empty_files {
        nodes[f].logical_offset = logical;
    }
    let data_end = logical;
    let original_end = phys;
    // The physical layout pass of the original only moves blocks when that saves at least
    // 1 MiB; the alignment gaps of an app's few executables never do.
    {
        let movable: BTreeMap<i64, i32> =
            slots.iter().flatten().filter(|&&f| !nodes[f].whole_block_raw).flat_map(|&f| nodes[f].blocks.iter().map(|b| (b.on_disk_offset, b.compressed_size))).collect();
        let fixed: i64 = slots.iter().flatten().filter(|&&f| nodes[f].whole_block_raw).map(|&f| nodes[f].length).sum();
        let used = movable.values().map(|&v| v as i64).sum::<i64>() + fixed;
        let min_saving = 1_048_576i64.max(((original_end as f64) * 0.1 / 100.0).ceil() as i64);
        let saving = round_up(original_end, BLOCK) - round_up(used, BLOCK);
        if saving > 0 && saving >= min_saving {
            return Err(format!("the inner image would need the physical layout pass (it could save {saving} bytes), which is not supported"));
        }
    }
    // The NAPS u2c table must address every u-block in one byte.
    {
        let order: Vec<usize> = slots.iter().flatten().copied().collect();
        let placements: Vec<naps::Placement> = order
            .iter()
            .map(|&f| naps::Placement {
                on_disk_offset: nodes[f].on_disk_offset,
                logical_offset: nodes[f].logical_offset,
                on_disk_size: nodes[f].on_disk_size,
                uncompressed_size: nodes[f].length,
                store_raw: nodes[f].store_raw,
                blocks: &nodes[f].blocks,
            })
            .collect();
        let mut blocks = naps::data_region_blocks(&placements);
        blocks.sort_by_key(|b| b.logical_offset);
        let u = naps::append_capacity_tail(&mut blocks, data_end, original_end);
        if naps::measure_u2c_capacity(&blocks, u).iter().any(|&d| d > 255) {
            return Err("the inner image's NAPS u2c table overflows (block deduplication repair is not supported)".into());
        }
    }
    // Directory entries.
    let order_dirs = dirs_pre.clone();
    for &d in &order_dirs {
        let mut ents = vec![Dirent::new(dir_list[d].inode, DIRENT_DOT, ".")];
        let parent_inode = dir_list[d].parent.map_or(dir_list[d].inode, |p| dir_list[p].inode);
        ents.push(Dirent::new(parent_inode, DIRENT_DOTDOT, ".."));
        for c in sorted_dirs(&dir_list, d) {
            ents.push(Dirent::new(dir_list[c].inode, DIRENT_DIR, &dir_list[c].name));
        }
        for f in sorted_files(&dir_list, &nodes, d) {
            ents.push(Dirent::new(nodes[f].inode, DIRENT_FILE, &nodes[f].name));
        }
        let mut offset = 0i32;
        let mut offsets: HashMap<String, i32> = HashMap::new();
        for i in 0..ents.len() {
            let in_block = offset % 65536;
            let room = 65536 - in_block;
            if in_block != 0 && ents[i].ent_size > room {
                if i == 0 {
                    return Err("the first directory entry exceeds one block".into());
                }
                ents[i - 1].ent_size += room;
                offset += room;
            }
            offsets.insert(ents[i].name.clone(), offset);
            offset += ents[i].ent_size;
        }
        for &c in &dir_list[d].sub_dirs.clone() {
            dir_list[c].dirent_offset = offsets[&dir_list[c].name];
        }
        for &f in &dir_list[d].files.clone() {
            nodes[f].dirent_offset = offsets[&nodes[f].name];
        }
        dir_list[d].dirents = ents;
    }
    // Metadata payloads: the super-root, the inode and APR flat path tables, the AFID table,
    // then every directory.
    let mut inode_flt = Vec::new();
    let mut apr_flt = Vec::new();
    for &d in &dirs_pre {
        if d == 0 {
            continue;
        }
        let fp = &dir_list[d].full_path;
        let apr = !(fp == "sce_sys" || fp.starts_with("sce_sys/"));
        inode_flt.push((hash_path(fp), pack_inode_entry(dir_list[d].inode, true, !apr, 0, false)));
    }
    for &f in &file_order {
        let n = &nodes[f];
        let apr = !n.full_path.starts_with("/sce_sys/");
        inode_flt.push((hash_path(&n.full_path), pack_inode_entry(n.inode, false, !apr, n.afid, n.length == 0)));
        if apr {
            apr_flt.push((hash_path(&n.full_path), pack_apr_entry(n.length, n.afid)));
        }
    }
    let mut afid_table: Vec<i32> = vec![(slots.len() + 2 + empty_files.len()) as i32];
    afid_table.extend(slots.iter().map(|s| s.map_or(-1, |f| nodes[f].inode as i32)));
    afid_table.extend([-1, -1]);
    afid_table.extend(empty_files.iter().map(|&f| nodes[f].inode as i32));
    let mut payloads: Vec<Vec<u8>> = vec![
        dirent_bytes(&[
            Dirent::new(1, DIRENT_FILE, "inode_flat_path_table"),
            Dirent::new(2, DIRENT_FILE, "apr_flat_path_table"),
            Dirent::new(3, DIRENT_FILE, "afid_to_ino_table"),
            Dirent::new(dir_list[0].inode, DIRENT_DIR, "uroot"),
        ]),
        flat_path_table(inode_flt),
        flat_path_table(apr_flt),
        afid_table.iter().flat_map(|v| v.to_le_bytes()).collect(),
    ];
    for &d in &dirs_pre {
        payloads.push(dirent_bytes(&dir_list[d].dirents));
    }
    // Nodes and the metadata region's place in the mount.
    let logical_data_blocks = round_up(data_end.max(1), UBLOCK) / BLOCK;
    let payload_total: i64 = payloads.iter().map(|p| payload_blocks(p.len())).sum();
    let inode_blocks = ((4 + dirs_pre.len() + file_order.len()) as i64 + 389) / 390;
    let used_blocks = 1 + inode_blocks + payload_total;
    let even_blocks = (used_blocks + 1) & !1;
    let trailing = even_blocks - used_blocks;
    let meta_base_block = logical_data_blocks + 60;
    let first_payload = (meta_base_block + 1 + inode_blocks) * BLOCK;
    let ndblock = meta_base_block + even_blocks;
    let mut payload_at = Vec::with_capacity(payloads.len());
    let mut at = first_payload;
    for p in &payloads {
        payload_at.push(at);
        at += payload_blocks(p.len()) * BLOCK;
    }
    let mut meta_nodes: Vec<MetaNode> = Vec::new();
    meta_nodes.push(MetaNode {
        inode: 0,
        is_dir: true,
        mode: 16749,
        nlink: 1,
        flags: 131_088,
        afid: 0,
        size: payload_blocks(payloads[0].len()) * BLOCK,
        logical_offset: payload_at[0] as u64,
        parent_inode: -1,
        dirent_offset: -1,
    });
    for k in 1..4 {
        meta_nodes.push(MetaNode {
            inode: k as u32,
            is_dir: false,
            mode: 33133,
            nlink: 1,
            flags: 131_088,
            afid: 0,
            size: payloads[k].len() as i64,
            logical_offset: payload_at[k] as u64,
            parent_inode: -1,
            dirent_offset: -1,
        });
    }
    let mut out_nodes: Vec<Node> = Vec::new();
    for (j, &d) in dirs_pre.iter().enumerate() {
        let dir = &dir_list[d];
        let is_root = d == 0;
        let sys = dir.full_path == "sce_sys" || dir.full_path.starts_with("sce_sys/");
        let parent = if is_root { -1 } else { dir_list[dir.parent.unwrap()].inode as i32 };
        meta_nodes.push(MetaNode {
            inode: dir.inode,
            is_dir: true,
            mode: if is_root || !sys { 16749 } else { 16744 },
            nlink: (2 + dir.sub_dirs.len() + is_root as usize) as u16,
            flags: if is_root || !sys { 16 } else { 131_088 },
            afid: 0,
            size: payload_blocks(payloads[4 + j].len()) * BLOCK,
            logical_offset: payload_at[4 + j] as u64,
            parent_inode: parent,
            dirent_offset: if is_root { -1 } else { dir.dirent_offset },
        });
        out_nodes.push(Node { full_path: dir.full_path.clone(), is_directory: true, parent_inode: parent, mode: 0, afid: 0, size: 0 });
    }
    for &f in &file_order {
        let n = &nodes[f];
        let sys = n.full_path.starts_with("/sce_sys/");
        let mode = if sys { 33128 } else { 33133 };
        meta_nodes.push(MetaNode {
            inode: n.inode,
            is_dir: false,
            mode,
            nlink: 1,
            flags: 0x10 | if n.is_executable { 64 } else { 32 } | if sys { 131_072 } else { 0 },
            afid: n.afid,
            size: n.length,
            logical_offset: n.logical_offset as u64,
            parent_inode: dir_list[n.parent].inode as i32,
            dirent_offset: n.dirent_offset,
        });
        out_nodes.push(Node { full_path: n.full_path.clone(), is_directory: false, parent_inode: dir_list[n.parent].inode as i32, mode, afid: n.afid, size: n.length });
    }
    meta_nodes.sort_by_key(|n| n.inode);
    let mut all_payloads = payloads;
    if trailing != 0 {
        all_payloads.push(Vec::new());
    }
    let meta_plain = metadata_plaintext(&meta_nodes, ndblock, &all_payloads)?;
    let meta_base_logical = ndblock * BLOCK - meta_plain.len() as i64;
    // Block-info table and the compressed metadata after the data.
    let block_info = round_up(original_end, BLOCK);
    out.seek(SeekFrom::Start(block_info as u64)).map_err(|e| e.to_string())?;
    out.write_all(&block_info_table(data_end)).map_err(|e| e.to_string())?;
    let (compressed_meta, meta_chunks) = compress_metadata(&meta_plain);
    let meta_at = round_up(block_info + 256, BLOCK);
    out.seek(SeekFrom::Start(meta_at as u64)).map_err(|e| e.to_string())?;
    out.write_all(&compressed_meta).map_err(|e| e.to_string())?;
    let image_length = round_up(meta_at + compressed_meta.len() as i64, BLOCK);
    out.set_len(image_length as u64).map_err(|e| e.to_string())?;
    if dedup_count > 0 {
        log(&format!("Inner image: {dedup_count} repeated blocks stored once"));
    }
    let placements = slots
        .iter()
        .flatten()
        .map(|&f| {
            let n = &nodes[f];
            Placement {
                afid: n.afid,
                path: n.full_path.clone(),
                on_disk_offset: n.on_disk_offset,
                logical_offset: n.logical_offset,
                on_disk_size: n.on_disk_size,
                uncompressed_size: n.length,
                store_raw: n.store_raw,
                blocks: n.blocks.clone(),
                source: files[n.input].source.clone(),
            }
        })
        .collect();
    Ok(InnerImage {
        image_length,
        ndblock,
        data_end_logical: data_end,
        meta_base_logical,
        block_info_on_disk: block_info,
        metadata_on_disk: meta_at,
        metadata_blocks: meta_chunks,
        afid_logical_offsets: afid_offsets,
        sparse_afid_holes: holes,
        empty_file_count: empty_files.len(),
        placements,
        nodes: out_nodes,
        deduplicated_blocks: dedup_count,
    })
}
