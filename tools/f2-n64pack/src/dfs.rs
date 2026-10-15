//! libdragon DragonFS image writer: a pure-Rust port of `mkdfs` (libdragon
//! `tools/mkdfs/mkdfs.c`, "DragonFS 2.1"), byte-for-byte identical to `mkdfs OUT DIR`.
//!
//! Layout (all big-endian `u32`, every allocation rounded up to an even size and zeroed):
//! * offset 0: the ID entry `{next = 0xDEADBEEF -> lookup size, flags = 0xFFFFFFFF,
//!   file_pointer -> lookup table, "DragonFS 2.1\0"}` (26 bytes);
//! * the root directory's entries from offset 26, depth first: each entry is
//!   `{next_entry, flags, file_pointer, name\0}` (`flags` = type << 28 | size), a file's data
//!   right after its entry, a directory's children right after its entry (entries within a
//!   directory sorted by `strcmp` of their paths, i.e. by name bytes);
//! * the lookup table `{num_files, path_ofs, [hash, len << 20 | ofs, data_ofs, data_len]...}`
//!   sorted by the path hash (`hash = hash * 31 + c`, over the `/`-separated path relative
//!   to the root, `c` a C `char`), then the paths (NUL-terminated, each padded to an even size).
//!
//! The N64 side opens files with `dfs_open("rom:/path")` (lookup by hash) and walks the
//! directories with `dfs_dir_findfirst`.

use std::path::Path;

const ROOT_FLAGS: u32 = 0xFFFF_FFFF;
const ROOT_NEXT_ENTRY: u32 = 0xDEAD_BEEF;
const ROOT_PATH: &str = "DragonFS 2.1";
const DFS_LOOKUP_PRIME: u32 = 31;
const FLAGS_FILE: u32 = 0;
const FLAGS_DIR: u32 = 1;
/// Size + entry fields of a directory entry before the name (`next_entry`, `flags`, `file_pointer`).
const DIRENT_HEADER: usize = 12;
/// Largest file mkdfs accepts (the size shares its `u32` with the type flags).
pub const MAX_FILE_SIZE: u64 = 0x0FFF_FFFF;

/// A file or directory to put into an image (what `mkdfs` reads from disk).
#[derive(Clone, Debug)]
pub enum Node {
    File { name: String, data: Vec<u8> },
    Dir { name: String, children: Vec<Node> },
}

impl Node {
    fn name(&self) -> &str {
        match self {
            Node::File { name, .. } | Node::Dir { name, .. } => name,
        }
    }
}

/// `prime_hash` of mkdfs (with x86's signed `char`).
pub fn path_hash(path: &str) -> u32 {
    path.bytes().fold(0u32, |h, c| h.wrapping_mul(DFS_LOOKUP_PRIME).wrapping_add(c as i8 as i32 as u32))
}

struct LookupFile {
    path: String,
    hash: u32,
    data_ofs: u32,
    data_len: u32,
}

struct Builder {
    img: Vec<u8>,
    files: Vec<LookupFile>,
}

impl Builder {
    /// `dfs_alloc`: appends `size` zero bytes (rounded up to even); returns the offset.
    fn alloc(&mut self, size: usize) -> usize {
        let at = self.img.len();
        self.img.resize(at + size + (size & 1), 0);
        at
    }

    fn put32(&mut self, at: usize, v: u32) {
        self.img[at..at + 4].copy_from_slice(&v.to_be_bytes());
    }

    fn new_entry(&mut self, name: &str) -> usize {
        let e = self.alloc(DIRENT_HEADER + name.len() + 1);
        self.img[e + DIRENT_HEADER..e + DIRENT_HEADER + name.len()].copy_from_slice(name.as_bytes());
        e
    }

    /// `add_directory`: the entries of one directory (already sorted); returns the offset of
    /// the first entry (0 for none).
    fn add_directory(&mut self, prefix: &str, nodes: &[Node]) -> Result<u32, String> {
        let mut first = 0usize;
        let mut cur = 0usize;
        for n in nodes {
            let e = self.new_entry(n.name());
            if cur != 0 {
                self.put32(cur, e as u32);
            }
            cur = e;
            if first == 0 {
                first = e;
            }
            let path = if prefix.is_empty() { n.name().to_string() } else { format!("{prefix}/{}", n.name()) };
            match n {
                Node::File { data, .. } => {
                    if data.len() as u64 > MAX_FILE_SIZE {
                        return Err(format!("{path}: too big for the filesystem ({} bytes)", data.len()));
                    }
                    let d = self.alloc(data.len());
                    self.img[d..d + data.len()].copy_from_slice(data);
                    self.put32(e + 4, (FLAGS_FILE << 28) | data.len() as u32);
                    self.put32(e + 8, d as u32);
                    self.files.push(LookupFile { hash: path_hash(&path), path, data_ofs: d as u32, data_len: data.len() as u32 });
                }
                Node::Dir { children, .. } => {
                    self.put32(e + 4, FLAGS_DIR << 28);
                    let sub = self.add_directory(&path, children)?;
                    self.put32(e + 8, sub);
                }
            }
        }
        Ok(first as u32)
    }

    /// `create_dfs_lookup`.
    fn lookup(&mut self) {
        // mkdfs qsorts by hash; equal hashes would be in an unspecified order there.
        self.files.sort_by_key(|f| f.hash);
        let n = self.files.len();
        let lookup_size = 8 + 16 * n;
        let lp = self.alloc(lookup_size);
        self.put32(0, lookup_size as u32);
        self.put32(8, lp as u32);
        let path_size: usize = self.files.iter().map(|f| (f.path.len() + 1).next_multiple_of(2)).sum();
        let po = self.alloc(path_size);
        self.put32(lp, n as u32);
        self.put32(lp + 4, po as u32);
        let mut cur = 0usize;
        let files = std::mem::take(&mut self.files);
        for (i, f) in files.iter().enumerate() {
            let len = f.path.len() + 1;
            let r = lp + 8 + 16 * i;
            self.put32(r, f.hash);
            self.put32(r + 4, ((len as u32) << 20) | cur as u32);
            self.put32(r + 8, f.data_ofs);
            self.put32(r + 12, f.data_len);
            self.img[po + cur..po + cur + f.path.len()].copy_from_slice(f.path.as_bytes());
            cur += len.next_multiple_of(2);
        }
        self.files = files;
    }
}

/// Sorts a directory listing the way mkdfs does (`strcmp` of the full paths, which share
/// the directory prefix: byte order of the names), recursively.
pub fn sort_tree(nodes: &mut [Node]) {
    nodes.sort_by(|a, b| a.name().as_bytes().cmp(b.name().as_bytes()));
    for n in nodes {
        if let Node::Dir { children, .. } = n {
            sort_tree(children);
        }
    }
}

/// The image `mkdfs` writes for a root directory holding `nodes` (sorted here).
pub fn build(mut nodes: Vec<Node>) -> Result<Vec<u8>, String> {
    sort_tree(&mut nodes);
    let mut b = Builder { img: Vec::new(), files: Vec::new() };
    let id = b.alloc(DIRENT_HEADER + ROOT_PATH.len() + 1);
    b.put32(id, ROOT_NEXT_ENTRY);
    b.put32(id + 4, ROOT_FLAGS);
    b.img[id + DIRENT_HEADER..id + DIRENT_HEADER + ROOT_PATH.len()].copy_from_slice(ROOT_PATH.as_bytes());
    b.add_directory("", &nodes)?;
    b.lookup();
    Ok(b.img)
}

/// Reads a directory tree from disk (regular files and directories, following symlinks
/// like mkdfs's `stat`; anything else is skipped).
pub fn read_tree(dir: &Path) -> Result<Vec<Node>, String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let md = std::fs::metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        if md.is_file() {
            out.push(Node::File { name, data: std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))? });
        } else if md.is_dir() {
            out.push(Node::Dir { name, children: read_tree(&p)? });
        }
    }
    Ok(out)
}

/// `mkdfs OUT dir`: the image of a directory on disk.
pub fn build_dir(dir: &Path) -> Result<Vec<u8>, String> {
    build(read_tree(dir)?)
}

/// Size of the image `build_dir` would produce, without holding the file data.
pub fn image_size_dir(dir: &Path) -> Result<u64, String> {
    fn walk(dir: &Path, prefix: &str, files: &mut Vec<String>) -> Result<u64, String> {
        let mut size = 0u64;
        for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let e = e.map_err(|e| e.to_string())?;
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let md = std::fs::metadata(&p).map_err(|e| e.to_string())?;
            let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if md.is_file() {
                size += ((DIRENT_HEADER + name.len() + 1).next_multiple_of(2) as u64) + md.len().next_multiple_of(2);
                files.push(path);
            } else if md.is_dir() {
                size += (DIRENT_HEADER + name.len() + 1).next_multiple_of(2) as u64;
                size += walk(&p, &path, files)?;
            }
        }
        Ok(size)
    }
    let mut files = Vec::new();
    let body = walk(dir, "", &mut files)?;
    let id = (DIRENT_HEADER + ROOT_PATH.len() + 1).next_multiple_of(2) as u64;
    let lookup = 8 + 16 * files.len() as u64;
    let paths: u64 = files.iter().map(|p| (p.len() as u64 + 1).next_multiple_of(2)).sum();
    Ok(id + body + lookup + paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, data: &[u8]) -> Node {
        Node::File { name: name.into(), data: data.to_vec() }
    }

    /// A small tree, checked against `mkdfs` output (libdragon preview 39d0d60 + 3A patch,
    /// `mkdfs out.dfs root` for: `a.txt` = "hello", `sub/b.bin` = [1,2,3], `sub/empty/`, `z` = "").
    #[test]
    fn golden_small_tree() {
        let tree = vec![
            file("z", b""),
            Node::Dir { name: "sub".into(), children: vec![file("b.bin", &[1, 2, 3]), Node::Dir { name: "empty".into(), children: vec![] }] },
            file("a.txt", b"hello"),
        ];
        let img = build(tree).unwrap();
        assert_eq!(&img[12..24], b"DragonFS 2.1");
        assert_eq!(hex(&img), GOLDEN);
    }

    const GOLDEN: &str = "00000038ffffffff00000078447261676f6e465320322e31000000000032000000050000002c612e7478740068656c6c6f000000006a100000000000004273756200000000580000000300000054622e62696e0001020300000000001000000000000000656d707479000000000000000000000000787a0000000003000000b00000007a002000000000007800000000056d9363006000020000002c00000005aa7938ec00a0000800000054000000037a00612e747874007375622f622e62696e00";

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn hash_matches_c_char() {
        assert_eq!(path_hash(""), 0);
        assert_eq!(path_hash("a"), 97);
        assert_eq!(path_hash("ab"), 97 * 31 + 98);
        // bytes >= 0x80 are negative `char`s on x86
        assert_eq!(path_hash("\u{e9}"), (0xC3u8 as i8 as i32 as u32).wrapping_mul(31).wrapping_add(0xA9u8 as i8 as i32 as u32));
    }

    /// Against the real tool when it is installed (`$N64_INST/bin/mkdfs`).
    #[test]
    fn matches_mkdfs_when_available() {
        let Some(mkdfs) = crate::ctx::libdragon_tool("mkdfs") else {
            eprintln!("mkdfs not installed: skipped");
            return;
        };
        let tmp = std::env::temp_dir().join(format!("f2-dfs-test-{}", std::process::id()));
        let root = tmp.join("root");
        let _ = std::fs::remove_dir_all(&tmp);
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for d in ["", "objects", "objects/deep/er", "sfx", "Upper", "empty"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for i in 0..60 {
            let dir = ["", "objects", "objects/deep/er", "sfx", "Upper"][i % 5];
            let name = format!("{}{}.{}", ["a", "B", "file_", "x-y"][i % 4], i, ["bin", "wav64", "", "f2z"][i % 4]);
            let len = (rnd() % 3000) as usize;
            let data: Vec<u8> = (0..len).map(|_| rnd() as u8).collect();
            std::fs::write(root.join(dir).join(name.trim_end_matches('.')), data).unwrap();
        }
        let out = tmp.join("ref.dfs");
        let st = std::process::Command::new(mkdfs).arg(&out).arg(&root).output().unwrap();
        assert!(st.status.success());
        let ours = build_dir(&root).unwrap();
        assert_eq!(ours.len() as u64, image_size_dir(&root).unwrap());
        assert!(ours == std::fs::read(&out).unwrap(), "DFS image differs from mkdfs");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
