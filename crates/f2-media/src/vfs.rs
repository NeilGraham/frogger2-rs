//! Uniform read-only file-system view over discs and installed directories.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

/// One regular file. `path` is relative, `/`-separated, original case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
}

/// Lowercase hex of a digest.
pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

/// Read-only file system: a disc image tree or a directory on disk.
/// Path lookup is case-insensitive (the game runs on Windows).
pub trait Vfs {
    fn entries(&self) -> &[FileEntry];

    /// Streams the whole file `idx` into `sink` in chunks.
    fn stream_file(&mut self, idx: usize, sink: &mut dyn FnMut(&[u8])) -> io::Result<()>;

    fn read_file(&mut self, idx: usize) -> io::Result<Vec<u8>> {
        let mut v = Vec::with_capacity(self.entries().get(idx).map_or(0, |e| e.size as usize));
        self.stream_file(idx, &mut |b| v.extend_from_slice(b))?;
        Ok(v)
    }

    /// SHA-1 of file `idx` as lowercase hex.
    fn sha1_file(&mut self, idx: usize) -> io::Result<String> {
        let mut h = Sha1::new();
        self.stream_file(idx, &mut |b| h.update(b))?;
        Ok(hex(&h.finalize()))
    }

    /// Case-insensitive lookup of a `/`-separated path (a leading `/` or `\` separators are accepted).
    fn find(&self, path: &str) -> Option<usize> {
        let want = normalize(path);
        self.entries().iter().position(|e| e.path.to_lowercase() == want)
    }
}

/// Lowercases and converts `\` to `/`, trimming leading separators.
pub fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_lowercase()
}

impl<V: Vfs + ?Sized> Vfs for Box<V> {
    fn entries(&self) -> &[FileEntry] {
        (**self).entries()
    }
    fn stream_file(&mut self, idx: usize, sink: &mut dyn FnMut(&[u8])) -> io::Result<()> {
        (**self).stream_file(idx, sink)
    }
}

/// A directory tree on disk (extracted file set or installed game).
/// Symlinks are not followed.
pub struct DirFs {
    root: PathBuf,
    entries: Vec<FileEntry>,
}

impl DirFs {
    pub fn open(root: &Path) -> io::Result<Self> {
        let mut entries = Vec::new();
        walk(root, "", &mut entries, 0)?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(DirFs { root: root.to_path_buf(), entries })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<FileEntry>, depth: u32) -> io::Result<()> {
    if depth > 64 {
        return Ok(());
    }
    for ent in fs::read_dir(dir)? {
        let ent = ent?;
        let ty = ent.file_type()?;
        let name = ent.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        if ty.is_dir() {
            walk(&ent.path(), &path, out, depth + 1)?;
        } else if ty.is_file() {
            out.push(FileEntry { path, size: ent.metadata()?.len() });
        }
    }
    Ok(())
}

impl Vfs for DirFs {
    fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    fn stream_file(&mut self, idx: usize, sink: &mut dyn FnMut(&[u8])) -> io::Result<()> {
        let e = self.entries.get(idx).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "bad file index"))?;
        let mut f = File::open(self.root.join(&e.path))?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                return Ok(());
            }
            sink(&buf[..n]);
        }
    }
}
