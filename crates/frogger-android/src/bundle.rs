//! The game bundled in an exported APK (`f2-package`'s Android export, docs/EXPORT.md
//! "Android"): stored assets `game-data/<rendition>/...` and the index
//! `game-data/bundle.json` (`{"format": 1, "id": "...", "rendition": "usa", "bytes": N,
//! "files": [["game/...", size], ...]}`).
//!
//! The game reads its data through the filesystem (the mod VFS, music and FMV by path, the
//! generated fonts written next to the data), so on the first start the bundle is unpacked
//! once into `files/data/<rendition>/` (with a progress screen) and marked with
//! `.bundle-id`; later starts use it directly. A different bundle id (a re-export with other
//! data installed as an update) unpacks again; an interrupted unpack resumes.

use std::io::{self, Read};
use std::path::{Component, Path};

/// Asset path of the index.
pub const INDEX: &str = "game-data/bundle.json";
/// In `files/data/<rendition>/`: the id of the unpacked bundle (written last).
pub const MARKER: &str = ".bundle-id";
/// In `files/data/<rendition>/`: the id of a bundle being unpacked (resume).
const PARTIAL: &str = ".bundle-partial";

#[derive(Clone, Debug, PartialEq)]
pub struct Index {
    pub id: String,
    pub rendition: String,
    pub bytes: u64,
    pub files: Vec<(String, u64)>,
}

/// A relative path made of normal components only (no `..`, no root).
fn safe(rel: &str) -> bool {
    !rel.is_empty() && Path::new(rel).components().all(|c| matches!(c, Component::Normal(_)))
}

impl Index {
    pub fn parse(text: &str) -> Result<Index, String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("{INDEX}: {e}"))?;
        if v["format"].as_u64() != Some(1) {
            return Err(format!("{INDEX}: unsupported format {}", v["format"]));
        }
        let id = v["id"].as_str().filter(|s| !s.is_empty()).ok_or("bundle without id")?.to_string();
        let rendition = v["rendition"].as_str().filter(|r| safe(r) && !r.contains('/')).ok_or("bundle without a valid rendition")?.to_string();
        let mut files = Vec::new();
        for f in v["files"].as_array().ok_or("bundle without files")? {
            let (Some(rel), Some(len)) = (f[0].as_str(), f[1].as_u64()) else { return Err("bad bundle file entry".into()) };
            if !safe(rel) {
                return Err(format!("bad bundle path {rel}"));
            }
            files.push((rel.to_string(), len));
        }
        let bytes = files.iter().map(|f| f.1).sum();
        Ok(Index { id, rendition, bytes, files })
    }

    /// Asset path of a bundled file.
    pub fn asset(&self, rel: &str) -> String {
        format!("game-data/{}/{rel}", self.rendition)
    }

    /// True when `data_root/<rendition>` holds this bundle, complete.
    pub fn installed(&self, data_root: &Path) -> bool {
        let dir = data_root.join(&self.rendition);
        std::fs::read_to_string(dir.join(MARKER)).is_ok_and(|id| id.trim() == self.id)
            && self.files.iter().all(|(rel, len)| std::fs::metadata(dir.join(rel)).is_ok_and(|m| m.len() == *len))
    }

    /// Copies the bundle into `data_root/<rendition>`; `open` reads a bundled file by asset
    /// path, `progress(done, total, file)` is called as bytes are written.
    pub fn unpack(&self, data_root: &Path, open: &dyn Fn(&str) -> io::Result<Box<dyn Read>>, progress: &mut dyn FnMut(u64, u64, &str)) -> Result<(), String> {
        let dir = data_root.join(&self.rendition);
        let err = |p: &Path, e: io::Error| format!("{}: {e}", p.display());
        std::fs::create_dir_all(&dir).map_err(|e| err(&dir, e))?;
        let _ = std::fs::remove_file(dir.join(MARKER));
        // Files of an interrupted unpack of the same bundle with the right size are kept.
        let resume = std::fs::read_to_string(dir.join(PARTIAL)).is_ok_and(|id| id.trim() == self.id);
        std::fs::write(dir.join(PARTIAL), &self.id).map_err(|e| err(&dir, e))?;
        let mut done = 0u64;
        let mut buf = vec![0u8; 1 << 20];
        for (rel, len) in &self.files {
            let dst = dir.join(rel);
            if resume && std::fs::metadata(&dst).is_ok_and(|m| m.len() == *len) {
                done += len;
                progress(done, self.bytes, rel);
                continue;
            }
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| err(parent, e))?;
            }
            let tmp = dst.with_file_name(format!("{}.part", dst.file_name().unwrap().to_string_lossy()));
            let mut src = open(&self.asset(rel)).map_err(|e| format!("{} (in the APK): {e}", self.asset(rel)))?;
            let mut out = std::fs::File::create(&tmp).map_err(|e| err(&tmp, e))?;
            let mut copied = 0u64;
            loop {
                let n = src.read(&mut buf).map_err(|e| format!("{} (in the APK): {e}", self.asset(rel)))?;
                if n == 0 {
                    break;
                }
                io::Write::write_all(&mut out, &buf[..n]).map_err(|e| err(&tmp, e))?;
                copied += n as u64;
                progress(done + copied, self.bytes, rel);
            }
            drop(out);
            if copied != *len {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!("{}: {copied} bytes in the APK, expected {len}", self.asset(rel)));
            }
            std::fs::rename(&tmp, &dst).map_err(|e| err(&dst, e))?;
            done += len;
        }
        std::fs::write(dir.join(MARKER), &self.id).map_err(|e| err(&dir, e))?;
        let _ = std::fs::remove_file(dir.join(PARTIAL));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX_JSON: &str = r#"{"format": 1, "id": "abc", "rendition": "usa", "bytes": 7,
        "files": [["game/a.fla", 3], ["music/track02.wav", 4]]}"#;

    #[test]
    fn parses() {
        let i = Index::parse(INDEX_JSON).unwrap();
        assert_eq!(i.rendition, "usa");
        assert_eq!(i.bytes, 7);
        assert_eq!(i.asset("game/a.fla"), "game-data/usa/game/a.fla");
        assert!(Index::parse(&INDEX_JSON.replace("game/a.fla", "../evil")).is_err());
        assert!(Index::parse(&INDEX_JSON.replace("\"usa\"", "\"/abs\"")).is_err());
        assert!(Index::parse(&INDEX_JSON.replace("\"format\": 1", "\"format\": 2")).is_err());
    }

    #[test]
    fn unpacks_and_resumes() {
        let root = std::env::temp_dir().join(format!("f2-android-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let i = Index::parse(INDEX_JSON).unwrap();
        assert!(!i.installed(&root));
        let opened = std::cell::RefCell::new(Vec::new());
        let open = |p: &str| -> io::Result<Box<dyn Read>> {
            opened.borrow_mut().push(p.to_string());
            let data: &'static [u8] = if p.ends_with("a.fla") { b"abc" } else { b"wxyz" };
            Ok(Box::new(data))
        };
        let mut last = 0;
        i.unpack(&root, &open, &mut |d, t, _| {
            assert_eq!(t, 7);
            last = d;
        })
        .unwrap();
        assert_eq!(last, 7);
        assert!(i.installed(&root));
        assert_eq!(std::fs::read(root.join("usa/music/track02.wav")).unwrap(), b"wxyz");
        // A deleted file means "not installed"; an interrupted unpack of the same bundle
        // only copies what is missing.
        std::fs::remove_file(root.join("usa/game/a.fla")).unwrap();
        assert!(!i.installed(&root));
        std::fs::write(root.join("usa").join(PARTIAL), "abc").unwrap();
        opened.borrow_mut().clear();
        i.unpack(&root, &open, &mut |_, _, _| {}).unwrap();
        assert_eq!(*opened.borrow(), vec!["game-data/usa/game/a.fla".to_string()]);
        assert!(i.installed(&root));
        // Another bundle id: everything again.
        let j = Index { id: "def".into(), ..i.clone() };
        assert!(!j.installed(&root));
        opened.borrow_mut().clear();
        j.unpack(&root, &open, &mut |_, _, _| {}).unwrap();
        assert_eq!(opened.borrow().len(), 2);
        // A short file in the APK is an error.
        let short = |_: &str| -> io::Result<Box<dyn Read>> { Ok(Box::new(&b"ab"[..])) };
        assert!(j.unpack(&root, &short, &mut |_, _, _| {}).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
