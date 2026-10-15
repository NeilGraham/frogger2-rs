//! PFS images (the PS4 package file system), as fake packages carry them: an unsigned,
//! unencrypted **inner** image with the app's files, stored as `pfs_image.dat` (behind a PFSC
//! header with stored, uncompressed 64 KiB blocks) in a signed (HMAC-SHA256 per block) and
//! AES-XTS encrypted **outer** image.
//!
//! The layout (superblock, inode tables, super-root, `flat_path_table`, data block order,
//! signature placement, the unencrypted "empty" block) reproduces LibOrbisPkg's `PfsBuilder`
//! (OpenOrbis, LGPL-2.1, `PFS/PFSBuilder.cs`, `PfsStructs.cs`, `FlatPathTable.cs`,
//! `PFSCWriter.cs`), including its quirks, so the images match `PkgTool.Core`'s for the same
//! input; the format itself is documented on psdevwiki ("PFS").

use super::crypto;
use std::io;
use std::path::PathBuf;

pub(crate) const BLOCK: u64 = 0x10000;
const XTS_SECTOR: u64 = 0x1000;
/// Size of a signed 32-bit inode (`DinodeS32`) and of an unsigned one (`DinodeD32`).
const INODE_S32: u64 = 0x2C8;
const INODE_D32: u64 = 0xA8;
const DIRENT_MAX: u64 = 280;

const MODE_RX: u16 = 0o555;
const MODE_DIR: u16 = 0x4000 | MODE_RX;
const MODE_FILE: u16 = 0x8000 | MODE_RX;
const FLAG_COMPRESSED: u32 = 0x1;
const FLAG_UNK2: u32 = 0x4;
const FLAG_UNK3: u32 = 0x8;
const FLAG_READONLY: u32 = 0x10;
const FLAG_INTERNAL: u32 = 0x20000;

const DT_FILE: i32 = 2;
const DT_DIR: i32 = 3;
const DT_DOT: i32 = 4;
const DT_DOTDOT: i32 = 5;

/// Random-access output (a region of the package file, or memory in tests).
pub(crate) trait Io {
    fn write_at(&mut self, off: u64, data: &[u8]) -> io::Result<()>;
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> io::Result<()>;
}

impl Io for Vec<u8> {
    fn write_at(&mut self, off: u64, data: &[u8]) -> io::Result<()> {
        let end = off as usize + data.len();
        if self.len() < end {
            self.resize(end, 0);
        }
        self[off as usize..end].copy_from_slice(data);
        Ok(())
    }
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> io::Result<()> {
        let end = off as usize + buf.len();
        if self.len() < end {
            self.resize(end, 0);
        }
        buf.copy_from_slice(&self[off as usize..end]);
        Ok(())
    }
}

/// A file seen from `base` on.
pub(crate) struct FileAt<'a> {
    pub(crate) f: &'a mut std::fs::File,
    pub(crate) base: u64,
}

impl Io for FileAt<'_> {
    fn write_at(&mut self, off: u64, data: &[u8]) -> io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        self.f.seek(SeekFrom::Start(self.base + off))?;
        self.f.write_all(data)
    }
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        self.f.seek(SeekFrom::Start(self.base + off))?;
        // Past the end of what was written reads as zeroes (the file is sized up front).
        let mut n = 0;
        while n < buf.len() {
            match self.f.read(&mut buf[n..])? {
                0 => break,
                k => n += k,
            }
        }
        buf[n..].fill(0);
        Ok(())
    }
}

/// `inner` seen from `base` on.
struct Offset<'a> {
    inner: &'a mut dyn Io,
    base: u64,
}

impl Io for Offset<'_> {
    fn write_at(&mut self, off: u64, data: &[u8]) -> io::Result<()> {
        self.inner.write_at(self.base + off, data)
    }
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> io::Result<()> {
        self.inner.read_at(self.base + off, buf)
    }
}

/// What a file holds.
pub(crate) enum Content {
    Path(PathBuf),
    Bytes(Vec<u8>),
    /// A whole PFS image behind a PFSC header (`pfs_image.dat` of an outer image).
    Image(Box<Pfs>),
}

struct FileNode {
    name: String,
    parent: usize,
    content: Content,
    size: u64,
    /// What LibOrbisPkg calls the "compressed size": the logical size (inner image size for
    /// `pfs_image.dat`, else the file size).
    size_c: u64,
    compress: bool,
    ino: usize,
}

struct DirNode {
    name: String,
    parent: Option<usize>,
    dirs: Vec<usize>,
    files: Vec<usize>,
    dirents: Vec<Dirent>,
    ino: usize,
}

#[derive(Clone)]
struct Dirent {
    ino: u32,
    kind: i32,
    name: String,
}

impl Dirent {
    fn ent_size(&self) -> u64 {
        (self.name.len() as u64 + 17).next_multiple_of(8)
    }
    fn bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(self.ent_size() as usize);
        b.extend(self.ino.to_le_bytes());
        b.extend(self.kind.to_le_bytes());
        b.extend((self.name.len() as i32).to_le_bytes());
        b.extend((self.ent_size() as i32).to_le_bytes());
        b.extend(self.name.as_bytes());
        b.resize(self.ent_size() as usize, 0);
        b
    }
}

#[derive(Clone, Default)]
struct Inode {
    number: u32,
    mode: u16,
    nlink: u16,
    flags: u32,
    size: i64,
    size_c: i64,
    blocks: u32,
    db: [i64; 12],
    ib: [i64; 5],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Node {
    Dir(usize),
    File(usize),
}

/// A block signature to compute: HMAC of `size` bytes at block `block`, stored (with the block
/// number) at byte `at` of the image.
#[derive(Clone, Copy)]
struct Sig {
    block: i64,
    at: u64,
    size: u64,
}

/// Image settings.
pub(crate) struct Props {
    /// Inode times (seconds since 1970).
    pub(crate) time: i64,
    pub(crate) min_blocks: i64,
    /// Signed + encrypted (outer image) with this EKPFS and seed.
    pub(crate) crypt: Option<([u8; 32], [u8; 16])>,
}

/// A PFS image being built: a directory tree, then [`Pfs::finish`] lays it out.
pub(crate) struct Pfs {
    props: Props,
    dirs: Vec<DirNode>,
    files: Vec<FileNode>,
    inodes: Vec<Inode>,
    superroot: usize,
    fpt_ino: usize,
    cr_ino: Option<usize>,
    superroot_dirents: Vec<Dirent>,
    all: Vec<Node>,
    fpt: Vec<(u32, u32)>,
    cr: Option<Vec<Vec<Dirent>>>,
    ndblock: i64,
    dinode_blocks: i64,
    hdr_db: Vec<i64>,
    empty_block: i64,
    data_sigs: Vec<Sig>,
    final_sigs: Vec<Sig>,
}

/// Size of the PFSC header in front of an image of `size` bytes (stored blocks).
pub(crate) fn pfsc_header_size(size: u64) -> u64 {
    let n = size.div_ceil(BLOCK) as i64;
    let table = 8 + n * 8;
    let extra = (table - 0xFC00 + 0xFFFF) / 0x10000;
    0x10000 + if extra > 0 { BLOCK * extra as u64 } else { 0 }
}

/// The PFSC header (block table of stored 64 KiB blocks) for an image of `size` bytes.
fn pfsc_header(size: u64) -> Vec<u8> {
    let hs = pfsc_header_size(size);
    let n = size.div_ceil(BLOCK);
    let mut h = vec![0u8; hs as usize];
    h[0..4].copy_from_slice(b"PFSC");
    h[8..12].copy_from_slice(&6i32.to_le_bytes());
    h[12..16].copy_from_slice(&(BLOCK as i32).to_le_bytes());
    h[16..24].copy_from_slice(&BLOCK.to_le_bytes());
    h[24..32].copy_from_slice(&0x400u64.to_le_bytes());
    h[32..40].copy_from_slice(&hs.to_le_bytes());
    h[40..48].copy_from_slice(&(n * BLOCK).to_le_bytes());
    for i in 0..=n {
        let o = 0x400 + 8 * i as usize;
        h[o..o + 8].copy_from_slice(&(hs + i * BLOCK).to_le_bytes());
    }
    h
}

/// `flat_path_table` hash of a full path ("/sce_sys/about"): h = upper(c) + 31 h.
fn path_hash(path: &str) -> u32 {
    path.encode_utf16().fold(0u32, |h, c| {
        let c = if (b'a' as u16..=b'z' as u16).contains(&c) { c - 32 } else { c };
        (c as u32).wrapping_add(h.wrapping_mul(31))
    })
}

impl Pfs {
    pub(crate) fn new(props: Props) -> Pfs {
        Pfs {
            props,
            dirs: vec![DirNode { name: String::new(), parent: None, dirs: vec![], files: vec![], dirents: vec![], ino: 0 }],
            files: vec![],
            inodes: vec![],
            superroot: 0,
            fpt_ino: 0,
            cr_ino: None,
            superroot_dirents: vec![],
            all: vec![],
            fpt: vec![],
            cr: None,
            ndblock: 0,
            dinode_blocks: 0,
            hdr_db: vec![],
            empty_block: 4,
            data_sigs: vec![],
            final_sigs: vec![],
        }
    }

    fn signed(&self) -> bool {
        self.props.crypt.is_some()
    }

    /// The directory `path` ("a/b"), created (in call order) if missing.
    pub(crate) fn dir(&mut self, path: &str) -> usize {
        let mut d = 0;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            d = match self.dirs[d].dirs.iter().copied().find(|&c| self.dirs[c].name == part) {
                Some(c) => c,
                None => {
                    let c = self.dirs.len();
                    self.dirs.push(DirNode { name: part.to_string(), parent: Some(d), dirs: vec![], files: vec![], dirents: vec![], ino: 0 });
                    self.dirs[d].dirs.push(c);
                    c
                }
            };
        }
        d
    }

    /// Adds a file at `path` ("sce_sys/keystone"); its directory must exist already for the
    /// directory order to be the caller's.
    pub(crate) fn add_file(&mut self, path: &str, content: Content, size: u64) {
        let (dir, name) = match path.rsplit_once('/') {
            Some((d, n)) => (self.dir(d), n),
            None => (0, path),
        };
        let (size, size_c, compress) = match &content {
            Content::Image(img) => {
                let s = img.size();
                (s + pfsc_header_size(s), s, true)
            }
            _ => (size, size, false),
        };
        let f = self.files.len();
        self.files.push(FileNode { name: name.to_string(), parent: dir, content, size, size_c, compress, ino: 0 });
        self.dirs[dir].files.push(f);
    }

    fn full_path(&self, n: Node) -> String {
        let (mut name, mut parent) = match n {
            Node::Dir(d) => (self.dirs[d].name.clone(), self.dirs[d].parent),
            Node::File(f) => (self.files[f].name.clone(), Some(self.files[f].parent)),
        };
        if parent.is_none() {
            return String::new();
        }
        let mut s = format!("/{name}");
        while let Some(p) = parent {
            if self.dirs[p].parent.is_none() {
                break;
            }
            name = self.dirs[p].name.clone();
            s = format!("/{name}{s}");
            parent = self.dirs[p].parent;
        }
        s
    }

    /// `GetAllChildrenDirs`: direct children, then each child's descendants.
    fn all_dirs(&self, d: usize) -> Vec<usize> {
        let mut v = self.dirs[d].dirs.clone();
        for &c in &self.dirs[d].dirs {
            v.extend(self.all_dirs(c));
        }
        v
    }

    /// `GetAllChildrenFiles`: this directory's files, then those of every descendant.
    fn all_files(&self, d: usize) -> Vec<usize> {
        let mut v = self.dirs[d].files.clone();
        for c in self.all_dirs(d) {
            v.extend(self.dirs[c].files.iter().copied());
        }
        v
    }

    fn node_size(&self, n: Node) -> u64 {
        match n {
            Node::Dir(d) => self.dirs[d].dirents.iter().map(Dirent::ent_size).sum(),
            Node::File(f) => self.files[f].size,
        }
    }

    fn node_ino(&self, n: Node) -> usize {
        match n {
            Node::Dir(d) => self.dirs[d].ino,
            Node::File(f) => self.files[f].ino,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_inode(&self, mode: u16, blocks: u32, size: i64, size_c: i64, nlink: u16, number: u32, flags: u32) -> Inode {
        let flags = if self.signed() { flags | FLAG_UNK2 | FLAG_UNK3 } else { flags };
        Inode { number, mode, nlink, flags, size, size_c, blocks, ..Default::default() }
    }

    /// Lays the image out. `skip` says which files are left out of the image (files under
    /// `sce_sys/` that the package carries as entries instead).
    pub(crate) fn finish(&mut self, skip: &dyn Fn(&str) -> bool) {
        self.final_sigs.push(Sig { block: 0, at: 0x380, size: 0x5A0 });
        let all_dirs = self.all_dirs(0);
        let all_files: Vec<usize> = self
            .all_files(0)
            .into_iter()
            .filter(|&f| {
                // name relative to a top-level sce_sys directory, if the file is under one
                let mut name = self.files[f].name.clone();
                let mut p = self.files[f].parent;
                while let Some(pp) = self.dirs[p].parent {
                    if pp == 0 && self.dirs[p].name == "sce_sys" {
                        return !skip(&name);
                    }
                    name = format!("{}/{name}", self.dirs[p].name);
                    p = pp;
                }
                true
            })
            .collect();
        let mut sorted_dirs = all_dirs.clone();
        sorted_dirs.sort_by_key(|&d| self.full_path(Node::Dir(d)));
        let mut all: Vec<Node> = sorted_dirs.iter().map(|&d| Node::Dir(d)).collect();
        all.extend(all_files.iter().map(|&f| Node::File(f)));

        // super-root, flat_path_table, collision_resolver, uroot
        let mut seen = std::collections::HashSet::new();
        let collision = !all.iter().all(|&n| seen.insert(path_hash(&self.full_path(n))));
        let mut num = 0u32;
        let (rx_flags, sz) = (FLAG_INTERNAL | FLAG_READONLY, BLOCK as i64);
        self.superroot = self.inodes.len();
        let sr = self.make_inode(MODE_DIR, 1, sz, sz, 1, num, rx_flags);
        self.inodes.push(sr);
        num += 1;
        self.fpt_ino = self.inodes.len();
        let fpt = self.make_inode(MODE_FILE, 1, 0, 0, 1, num, rx_flags);
        self.inodes.push(fpt);
        num += 1;
        if collision {
            self.cr_ino = Some(self.inodes.len());
            let cr = self.make_inode(MODE_FILE, 1, 0, 0, 1, num, rx_flags);
            self.inodes.push(cr);
            num += 1;
        }
        let mut uroot = self.make_inode(MODE_DIR, 1, sz, sz, 3, num, FLAG_READONLY);
        self.superroot_dirents = vec![Dirent { ino: 1, kind: DT_FILE, name: "flat_path_table".into() }];
        if collision {
            self.superroot_dirents.push(Dirent { ino: 2, kind: DT_FILE, name: "collision_resolver".into() });
        }
        self.superroot_dirents.push(Dirent { ino: uroot.number, kind: DT_DIR, name: "uroot".into() });
        self.dirs[0].name = "uroot".into();
        self.dirs[0].dirents =
            vec![Dirent { ino: uroot.number, kind: DT_DOT, name: ".".into() }, Dirent { ino: uroot.number, kind: DT_DOTDOT, name: "..".into() }];
        if self.signed() {
            for i in [self.superroot, self.fpt_ino] {
                self.inodes[i].flags &= !FLAG_READONLY;
            }
            uroot.flags &= !FLAG_READONLY;
        }

        // directory inodes (sorted by path)
        self.dirs[0].ino = self.inodes.len();
        self.inodes.push(uroot);
        for &d in &sorted_dirs {
            let number = self.inodes.len() as u32;
            let ino = self.make_inode(MODE_DIR, 1, sz, 0, 2, number, FLAG_READONLY);
            let parent = self.dirs[d].parent.unwrap();
            let pnum = self.inodes[self.dirs[parent].ino].number;
            self.dirs[d].ino = self.inodes.len();
            self.dirs[d].dirents.push(Dirent { ino: number, kind: DT_DOT, name: ".".into() });
            self.dirs[d].dirents.push(Dirent { ino: pnum, kind: DT_DOTDOT, name: "..".into() });
            let name = self.dirs[d].name.clone();
            self.dirs[parent].dirents.push(Dirent { ino: number, kind: DT_DIR, name });
            let pi = self.dirs[parent].ino;
            self.inodes[pi].nlink += 1;
            self.inodes.push(ino);
        }
        // file inodes (sorted by path)
        let mut sorted_files = all_files.clone();
        sorted_files.sort_by_key(|&f| self.full_path(Node::File(f)));
        for &f in &sorted_files {
            let number = self.inodes.len() as u32;
            let file = &self.files[f];
            let flags = FLAG_READONLY | if file.compress { FLAG_COMPRESSED } else { 0 };
            let mut ino = self.make_inode(MODE_FILE, file.size.div_ceil(BLOCK) as u32, file.size as i64, file.size_c as i64, 1, number, flags);
            if self.signed() {
                ino.flags &= !FLAG_READONLY;
            }
            let (parent, name) = (file.parent, file.name.clone());
            self.files[f].ino = self.inodes.len();
            self.dirs[parent].dirents.push(Dirent { ino: number, kind: DT_FILE, name });
            self.inodes.push(ino);
        }

        // flat_path_table (+ collision_resolver)
        let mut map: std::collections::BTreeMap<u32, u32> = Default::default();
        let mut nodes_of: std::collections::HashMap<u32, Vec<Node>> = Default::default();
        for &n in &all {
            let h = path_hash(&self.full_path(n));
            let ino = &self.inodes[self.node_ino(n)];
            let v = ino.number | if matches!(n, Node::Dir(_)) { 0x2000_0000 } else { 0 };
            map.entry(h).and_modify(|x| *x = 0x8000_0000).or_insert(v);
            nodes_of.entry(h).or_default().push(n);
        }
        if collision {
            let mut off = 0u32;
            let mut ents = vec![];
            for (h, v) in map.iter_mut() {
                if *v != 0x8000_0000 {
                    continue;
                }
                *v = 0x8000_0000 | off;
                let list: Vec<Dirent> = nodes_of[h]
                    .iter()
                    .map(|&n| Dirent {
                        ino: self.inodes[self.node_ino(n)].number,
                        kind: if matches!(n, Node::Dir(_)) { DT_DIR } else { DT_FILE },
                        name: self.full_path(n),
                    })
                    .collect();
                off += list.iter().map(|d| d.ent_size() as u32).sum::<u32>() + 0x18;
                ents.push(list);
            }
            self.cr = Some(ents);
        }
        self.fpt = map.into_iter().collect();

        all.insert(0, Node::Dir(0));
        self.all = all;
        self.layout();
    }

    fn fpt_size(&self) -> u64 {
        self.fpt.len() as u64 * 8
    }

    fn cr_size(&self) -> u64 {
        self.cr.as_ref().map_or(0, |c| c.iter().map(|l| l.iter().map(Dirent::ent_size).sum::<u64>() + 0x18).sum())
    }

    /// `inoNumberToOffset`: byte offset of the `db`-th block pointer of signed inode `number`.
    fn ino_off(number: u32, db: u64) -> u64 {
        BLOCK + INODE_S32 * number as u64 + 0x64 + 36 * db
    }

    fn indirect_blocks(size: u64) -> i64 {
        let per = (BLOCK / 36) as i64;
        let mut blocks = size.div_ceil(BLOCK) as i64;
        let mut ib = 0;
        if blocks > 12 {
            blocks -= 12;
            ib += 1;
        }
        if blocks > per {
            blocks -= per;
            ib += 1 + (blocks + per - 1) / per;
        }
        ib
    }

    /// `CalculateDataBlockLayout`.
    fn layout(&mut self) {
        let n_inodes = self.inodes.len() as i64;
        let (sr, fpt) = (self.superroot, self.fpt_ino);
        if self.signed() {
            self.ndblock = 1;
            let per = (BLOCK / INODE_S32) as i64;
            self.dinode_blocks = (n_inodes + per - 1) / per;
            for i in 0..self.dinode_blocks {
                self.hdr_db.push(1 + i);
                self.final_sigs.push(Sig { block: 1 + i, at: 0xB8 + 36 * i as u64, size: BLOCK });
            }
            self.ndblock += self.dinode_blocks;
            self.inodes[sr].db[0] = self.dinode_blocks + 1;
            self.final_sigs.push(Sig { block: self.inodes[sr].db[0], at: Self::ino_off(self.inodes[sr].number, 0), size: BLOCK });
            self.ndblock += self.inodes[sr].blocks as i64;

            let fs = self.fpt_size();
            self.inodes[fpt].db[0] = self.inodes[sr].db[0] + 1;
            self.inodes[fpt].size = fs as i64;
            self.inodes[fpt].size_c = fs as i64;
            self.inodes[fpt].blocks = fs.div_ceil(BLOCK) as u32;
            let fnum = self.inodes[fpt].number;
            self.final_sigs.push(Sig { block: self.inodes[fpt].db[0], at: Self::ino_off(fnum, 0), size: BLOCK });
            for i in 1..(self.inodes[fpt].blocks as usize).min(12) {
                self.inodes[fpt].db[i] = self.ndblock;
                self.ndblock += 1;
                // (LibOrbisPkg signs the first block here again)
                self.final_sigs.push(Sig { block: self.inodes[fpt].db[0], at: Self::ino_off(fnum, i as u64), size: BLOCK });
            }
            self.ndblock += 1;
            self.empty_block = self.ndblock;
            self.ndblock += 1;

            let mut ib_start = self.ndblock;
            let all = self.all.clone();
            self.ndblock += all.iter().map(|&n| Self::indirect_blocks(self.node_size(n))).sum::<i64>();
            let per = (BLOCK / 36) as i64;
            for n in all {
                let size = self.node_size(n);
                let blocks = size.div_ceil(BLOCK) as i64;
                let ii = self.node_ino(n);
                let num = self.inodes[ii].number;
                self.inodes[ii].db[0] = self.ndblock;
                self.inodes[ii].blocks = blocks as u32;
                self.inodes[ii].size = if matches!(n, Node::Dir(_)) { (size.div_ceil(BLOCK) * BLOCK) as i64 } else { size as i64 };
                if self.inodes[ii].size_c == 0 {
                    self.inodes[ii].size_c = self.inodes[ii].size;
                }
                for i in 0..blocks.min(12) {
                    self.data_sigs.push(Sig { block: self.ndblock, at: Self::ino_off(num, i as u64), size: BLOCK });
                    self.ndblock += 1;
                }
                if blocks > 12 {
                    self.final_sigs.push(Sig { block: ib_start, at: Self::ino_off(num, 12), size: BLOCK });
                    let mut i = 12;
                    let mut p = 0u64;
                    while blocks - i > 0 && i < 12 + per {
                        self.data_sigs.push(Sig { block: self.ndblock, at: ib_start as u64 * BLOCK + p, size: BLOCK });
                        self.ndblock += 1;
                        i += 1;
                        p += 36;
                    }
                    ib_start += 1;
                }
                if blocks > 12 + per {
                    let mut done = 12 + per;
                    self.final_sigs.push(Sig { block: ib_start, at: Self::ino_off(num, 13), size: BLOCK });
                    let ib1 = ib_start;
                    let mut i = 0;
                    while i < per && done < blocks {
                        ib_start += 1;
                        self.final_sigs.push(Sig { block: ib_start, at: ib1 as u64 * BLOCK + i as u64 * 36, size: BLOCK });
                        let mut j = 0;
                        while j < per && done < blocks {
                            self.data_sigs.push(Sig { block: self.ndblock, at: ib_start as u64 * BLOCK + j as u64 * 36, size: BLOCK });
                            self.ndblock += 1;
                            j += 1;
                            done += 1;
                        }
                        i += 1;
                    }
                }
            }
        } else {
            self.ndblock = 1;
            let per = (BLOCK / INODE_D32) as i64;
            self.dinode_blocks = (n_inodes + per - 1) / per;
            self.hdr_db.push(self.ndblock);
            self.ndblock += 1;
            for i in 1..self.dinode_blocks {
                if i < 12 {
                    self.hdr_db.push(-1);
                }
                self.ndblock += 1;
            }
            self.inodes[sr].db[0] = self.ndblock;
            self.ndblock += self.inodes[sr].blocks as i64;

            let fs = self.fpt_size();
            self.inodes[fpt].db[0] = self.ndblock;
            self.ndblock += 1;
            self.inodes[fpt].size = fs as i64;
            self.inodes[fpt].size_c = fs as i64;
            self.inodes[fpt].blocks = fs.div_ceil(BLOCK) as u32;
            for i in 1..(self.inodes[fpt].blocks as usize).min(12) {
                self.inodes[fpt].db[i] = self.ndblock;
                self.ndblock += 1;
            }
            match self.cr_ino {
                None => self.ndblock += 1,
                Some(cr) => {
                    let cs = self.cr_size();
                    self.inodes[cr].db[0] = self.ndblock;
                    self.ndblock += 1;
                    self.inodes[cr].size = cs as i64;
                    self.inodes[cr].size_c = cs as i64;
                    self.inodes[cr].blocks = cs.div_ceil(BLOCK) as u32;
                    for i in 1..(self.inodes[cr].blocks as usize).min(12) {
                        self.inodes[cr].db[i] = self.ndblock;
                        self.ndblock += 1;
                    }
                }
            }
            for n in self.all.clone() {
                let size = self.node_size(n);
                let blocks = size.div_ceil(BLOCK) as i64;
                let ii = self.node_ino(n);
                self.inodes[ii].db[0] = self.ndblock;
                self.inodes[ii].blocks = blocks as u32;
                self.inodes[ii].size = if matches!(n, Node::Dir(_)) { (size.div_ceil(BLOCK) * BLOCK) as i64 } else { size as i64 };
                if self.inodes[ii].size_c == 0 {
                    self.inodes[ii].size_c = self.inodes[ii].size;
                }
                for i in 1..(blocks as usize).min(12) {
                    self.inodes[ii].db[i] = -1;
                }
                self.ndblock += blocks;
            }
        }
        self.ndblock = self.ndblock.max(self.props.min_blocks);
    }

    /// Size of the image in bytes (after [`Pfs::finish`]).
    pub(crate) fn size(&self) -> u64 {
        self.ndblock as u64 * BLOCK
    }

    fn inode_bytes(&self, ino: &Inode) -> Vec<u8> {
        let t = self.props.time;
        let mut b = Vec::with_capacity(INODE_S32 as usize);
        b.extend(ino.mode.to_le_bytes());
        b.extend(ino.nlink.to_le_bytes());
        b.extend(ino.flags.to_le_bytes());
        b.extend(ino.size.to_le_bytes());
        b.extend(ino.size_c.to_le_bytes());
        for _ in 0..4 {
            b.extend(t.to_le_bytes());
        }
        b.extend([0u8; 16 + 8 + 16]); // nsec x4, uid, gid, unk1, unk2
        b.extend(ino.blocks.to_le_bytes());
        if self.signed() {
            for x in ino.db.iter().chain(&ino.ib) {
                b.extend([0u8; 32]);
                b.extend((*x as i32).to_le_bytes());
            }
        } else {
            for x in ino.db.iter().chain(&ino.ib) {
                b.extend((*x as i32).to_le_bytes());
            }
        }
        b
    }

    fn header_bytes(&self) -> Vec<u8> {
        let mut h = vec![0u8; 0x5A0];
        h[0..8].copy_from_slice(&1i64.to_le_bytes());
        h[8..16].copy_from_slice(&20130315i64.to_le_bytes());
        h[0x1A] = 1; // read-only
        let mode: u16 = if self.signed() { 0x1 | 0x4 | 0x8 } else { 0x8 };
        h[0x1C..0x1E].copy_from_slice(&mode.to_le_bytes());
        h[0x20..0x24].copy_from_slice(&(BLOCK as u32).to_le_bytes());
        h[0x28..0x30].copy_from_slice(&1i64.to_le_bytes());
        h[0x30..0x38].copy_from_slice(&(self.inodes.len() as i64).to_le_bytes());
        h[0x38..0x40].copy_from_slice(&self.ndblock.to_le_bytes());
        h[0x40..0x48].copy_from_slice(&self.dinode_blocks.to_le_bytes());
        // the inode of the inode blocks (a signed 64-bit inode)
        let mut s = Vec::new();
        s.extend(0u16.to_le_bytes());
        s.extend(1u16.to_le_bytes());
        s.extend((if self.signed() { 0 } else { FLAG_READONLY }).to_le_bytes());
        let sz = self.dinode_blocks * BLOCK as i64;
        s.extend(sz.to_le_bytes());
        s.extend(sz.to_le_bytes());
        for _ in 0..4 {
            s.extend(self.props.time.to_le_bytes());
        }
        s.extend([0u8; 40]);
        s.extend((self.dinode_blocks as u32).to_le_bytes());
        s.extend([0u8; 4]);
        for i in 0..17 {
            s.extend([0u8; 32]);
            s.extend(self.hdr_db.get(i).copied().filter(|_| i < 12).unwrap_or(0).to_le_bytes());
        }
        h[0x50..0x50 + s.len()].copy_from_slice(&s);
        match self.props.crypt {
            Some((_, seed)) => {
                h[0x36C..0x370].copy_from_slice(&1i32.to_le_bytes());
                h[0x370..0x380].copy_from_slice(&seed);
            }
            None => h[0x368..0x36C].copy_from_slice(&1i32.to_le_bytes()),
        }
        h
    }

    /// Writes the image (signed and encrypted for an outer image) at offset 0 of `out`.
    /// `progress` is called with bytes of file data written.
    pub(crate) fn write(&self, out: &mut dyn Io, progress: &mut dyn FnMut(u64)) -> io::Result<()> {
        self.write_data(out, progress)?;
        if let Some((ekpfs, seed)) = self.props.crypt {
            let key = crypto::pfs_sign_key(&ekpfs, &seed);
            let mut buf = vec![0u8; BLOCK as usize];
            for s in self.data_sigs.iter().chain(self.final_sigs.iter().rev()) {
                let b = &mut buf[..s.size as usize];
                out.read_at(s.block as u64 * BLOCK, b)?;
                let mut sig = crypto::hmac_sha256(&key, b).to_vec();
                sig.extend((s.block as i32).to_le_bytes());
                out.write_at(s.at, &sig)?;
            }
        }
        Ok(())
    }

    /// Encrypts the image in `out` (outer images), block by block, calling `block` with each
    /// final 64 KiB block (index, bytes) so the caller can hash it on the way.
    pub(crate) fn encrypt(&self, out: &mut dyn Io, block: &mut dyn FnMut(u64, &[u8])) -> io::Result<()> {
        let Some((ekpfs, seed)) = self.props.crypt else { return Ok(()) };
        let (tweak, data) = crypto::pfs_enc_keys(&ekpfs, &seed);
        let xts = crypto::Xts::new(&data, &tweak);
        let mut buf = vec![0u8; BLOCK as usize];
        for b in 0..self.ndblock as u64 {
            out.read_at(b * BLOCK, &mut buf)?;
            if b != 0 && b != self.empty_block as u64 {
                for (i, sector) in buf.as_chunks_mut::<{ XTS_SECTOR as usize }>().0.iter_mut().enumerate() {
                    xts.encrypt_sector(sector, b * 16 + i as u64);
                }
                out.write_at(b * BLOCK, &buf)?;
            }
            block(b, &buf);
        }
        Ok(())
    }

    fn write_data(&self, out: &mut dyn Io, progress: &mut dyn FnMut(u64)) -> io::Result<()> {
        out.write_at(0, &self.header_bytes())?;
        // inodes
        let isz = if self.signed() { INODE_S32 } else { INODE_D32 };
        let mut pos = BLOCK;
        for ino in &self.inodes {
            out.write_at(pos, &self.inode_bytes(ino))?;
            pos += isz;
            if pos % BLOCK > BLOCK - isz {
                pos += BLOCK - pos % BLOCK;
            }
        }
        // super-root dirents
        let mut sr = Vec::new();
        for d in &self.superroot_dirents {
            sr.extend(d.bytes());
        }
        out.write_at(BLOCK * (self.dinode_blocks as u64 + 1), &sr)?;
        // flat_path_table, collision_resolver
        let mut fpt = Vec::with_capacity(self.fpt.len() * 8);
        for (h, v) in &self.fpt {
            fpt.extend(h.to_le_bytes());
            fpt.extend(v.to_le_bytes());
        }
        out.write_at(self.inodes[self.fpt_ino].db[0] as u64 * BLOCK, &fpt)?;
        if let (Some(cr_ino), Some(cr)) = (self.cr_ino, &self.cr) {
            let mut b = Vec::new();
            for l in cr {
                for d in l {
                    b.extend(d.bytes());
                }
                b.extend([0u8; 0x18]);
            }
            out.write_at(self.inodes[cr_ino].db[0] as u64 * BLOCK, &b)?;
        }
        // directories and files
        for &n in &self.all {
            let start = self.inodes[self.node_ino(n)].db[0] as u64;
            match n {
                Node::Dir(d) => {
                    let mut block = start;
                    let mut p = block * BLOCK;
                    for e in &self.dirs[d].dirents {
                        out.write_at(p, &e.bytes())?;
                        p += e.ent_size();
                        if p % BLOCK > BLOCK - DIRENT_MAX {
                            block += 1;
                            p = block * BLOCK;
                        }
                    }
                }
                Node::File(f) => {
                    let base = start * BLOCK;
                    match &self.files[f].content {
                        Content::Bytes(b) => {
                            out.write_at(base, b)?;
                            progress(b.len() as u64);
                        }
                        Content::Path(p) => {
                            use std::io::Read;
                            let mut src = std::fs::File::open(p).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.display())))?;
                            let mut buf = vec![0u8; 1 << 20];
                            let mut off = 0u64;
                            loop {
                                let n = src.read(&mut buf)?;
                                if n == 0 {
                                    break;
                                }
                                out.write_at(base + off, &buf[..n])?;
                                off += n as u64;
                                progress(n as u64);
                            }
                            if off != self.files[f].size {
                                return Err(io::Error::other(format!("{}: size changed while packing", p.display())));
                            }
                        }
                        Content::Image(img) => {
                            let hdr = pfsc_header(img.size());
                            out.write_at(base, &hdr)?;
                            let mut sub = Offset { inner: out, base: base + hdr.len() as u64 };
                            img.write(&mut sub, progress)?;
                        }
                    }
                }
            }
        }
        // make sure the image has its full length
        let end = self.size();
        let mut last = [0u8; 1];
        out.read_at(end - 1, &mut last)?;
        out.write_at(end - 1, &last)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pfsc_sizes() {
        assert_eq!(pfsc_header_size(500 << 20), 0x10000);
        // 0xFC00 bytes of table = 8063 block pointers
        assert_eq!(pfsc_header_size(8000 * BLOCK), 0x10000);
        assert_eq!(pfsc_header_size(9000 * BLOCK), 0x20000);
    }

    #[test]
    fn hashes() {
        assert_eq!(path_hash("/a"), (b'/' as u32) * 31 + b'A' as u32);
    }

    #[test]
    fn dirent_sizes() {
        let d = Dirent { ino: 1, kind: DT_FILE, name: "eboot.bin".into() };
        assert_eq!(d.ent_size(), 32);
        assert_eq!(d.bytes().len(), 32);
    }
}
