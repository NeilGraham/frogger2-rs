//! Game data access through the mod VFS, plus the bank-selection heuristics
//! (`Common/layout.c` world/level tables, approximated by file-name prefixes).

use f2_formats::objbank::ObjectBank;
use f2_formats::texture::{TexBank, Texture};
use f2_mods::Vfs;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// World prefix → (master object bank, world texture bank). Order matters
/// (longest/most specific prefixes first).
const WORLDS: &[(&str, &str, &str)] = &[
    ("ancients", "ancientsmaster", "ancient.fla"),
    ("garden", "gardenmaster", "garden.fla"),
    ("city", "citymaster", "city.fla"),
    ("halloween", "halloweenmaster", "halloween.fla"),
    ("lab", "labmaster", "lab.fla"),
    ("space", "spacemaster", "space.fla"),
    ("sub", "submaster", "subter.fla"),
    ("sretro", "sretromaster", "super.fla"),
    ("multisel", "", "titles.fla"),
    ("start", "", "titles.fla"),
    ("level", "", "titles.fla"),
    ("language", "", "titles.fla"),
];

/// Full-screen backdrop files (one texture record) rather than texture banks.
pub fn is_backdrop_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("title.") || n.starts_with("loading.") || n.starts_with("artwork")
}

fn world_of(name: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    let n = name.to_ascii_lowercase();
    WORLDS.iter().find(|(p, _, _)| n.starts_with(p))
}

pub fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// A loaded texture source: a bank or a single backdrop image.
pub enum TexSource {
    Bank(TexBank),
    Backdrop(Texture),
}

impl TexSource {
    pub fn textures(&self) -> &[Texture] {
        match self {
            TexSource::Bank(b) => &b.textures,
            TexSource::Backdrop(t) => std::slice::from_ref(t),
        }
    }
}

pub struct Data {
    pub vfs: Vfs,
    pub rendition: PathBuf,
    tex_cache: HashMap<String, Arc<TexSource>>,
    crc_sets: HashMap<String, HashSet<u32>>,
}

impl Data {
    /// `arg` may be a rendition dir (`game-data/usa`) or a data root holding
    /// renditions; default: `F2_DATA_ROOT` / the platform data root, else `game-data`.
    pub fn open(arg: Option<PathBuf>) -> Result<Data, String> {
        let candidates: Vec<PathBuf> = match arg {
            Some(p) => vec![p],
            None => vec![f2_platform::paths::default_data_root(), PathBuf::from("game-data")],
        };
        for c in &candidates {
            if let Some(r) = find_rendition(c) {
                return Ok(Data { vfs: Vfs::with_rendition(&r), rendition: r, tex_cache: HashMap::new(), crc_sets: HashMap::new() });
            }
        }
        Err(format!(
            "no game data found (tried {}); pass a rendition dir like game-data/usa",
            candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        ))
    }

    /// File names in a virtual dir with extension `ext` (case-insensitive), sorted.
    pub fn list(&self, dir: &str, ext: &str) -> Vec<String> {
        let mut v: Vec<String> =
            self.vfs.list(dir).map(|l| l.into_iter().filter(|e| e.kind == f2_mods::EntryKind::File).map(|e| e.name).collect()).unwrap_or_default();
        v.retain(|n| n.to_ascii_lowercase().ends_with(&format!(".{ext}")));
        v.sort_by_key(|n| n.to_ascii_lowercase());
        v
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        self.vfs.read(path).map_err(|e| format!("{path}: {e}"))
    }

    pub fn exists(&self, path: &str) -> bool {
        self.vfs.resolve_file(path).is_some()
    }

    pub fn object_bank(&self, name: &str) -> Result<ObjectBank, String> {
        let file = if name.contains('.') { name.to_string() } else { format!("{name}.dll") };
        let data = self.read(&format!("objects/{file}"))?;
        ObjectBank::parse(&data).map_err(|e| format!("{file}: {e}"))
    }

    /// A texture bank or backdrop from `textures/` (cached).
    pub fn tex_source(&mut self, file: &str) -> Result<Arc<TexSource>, String> {
        let key = file.to_ascii_lowercase();
        if let Some(t) = self.tex_cache.get(&key) {
            return Ok(t.clone());
        }
        let data = self.read(&format!("textures/{file}"))?;
        let data = f2_formats::fla::maybe_decompress(data).map_err(|e| format!("{file}: {e}"))?;
        let src = if is_backdrop_file(file) {
            match f2_formats::texture::parse_backdrop(&data) {
                Ok(t) => TexSource::Backdrop(t),
                Err(_) => TexSource::Bank(TexBank::parse(&data).map_err(|e| format!("{file}: {e}"))?),
            }
        } else {
            match TexBank::parse(&data) {
                Ok(b) => TexSource::Bank(b),
                Err(e) => TexSource::Backdrop(f2_formats::texture::parse_backdrop(&data).map_err(|_| format!("{file}: {e}"))?),
            }
        };
        let src = Arc::new(src);
        self.tex_cache.insert(key, src.clone());
        Ok(src)
    }

    /// All texture-bank (non-backdrop) file names.
    pub fn tex_bank_files(&self) -> Vec<String> {
        self.list("textures", "fla").into_iter().filter(|n| !is_backdrop_file(n)).collect()
    }

    fn crc_set(&mut self, file: &str) -> HashSet<u32> {
        let key = file.to_ascii_lowercase();
        if let Some(s) = self.crc_sets.get(&key) {
            return s.clone();
        }
        let s: HashSet<u32> = self.tex_source(file).map(|t| t.textures().iter().map(|t| t.crc).collect()).unwrap_or_default();
        self.crc_sets.insert(key, s.clone());
        s
    }

    /// Texture banks for a set of object banks: `system.fla` + the world bank
    /// (like `LoadTextureBank(INGAMEGENERIC)` + the world's `masterTextureBank`),
    /// then any other bank that provides still-missing face texture CRCs.
    pub fn tex_banks_for(&mut self, bank_names: &[&str], banks: &[&ObjectBank]) -> Vec<String> {
        let mut out = vec!["system.fla".to_string()];
        for n in bank_names {
            if let Some((_, _, t)) = world_of(n) {
                if !out.iter().any(|o| o == t) {
                    out.push(t.to_string());
                }
            }
        }
        out.retain(|f| self.exists(&format!("textures/{f}")));
        let mut needed: HashSet<u32> = HashSet::new();
        for b in banks {
            for m in &b.models {
                collect_crcs(&m.root, &mut needed);
                for s in &m.root_siblings {
                    collect_crcs(s, &mut needed);
                }
            }
        }
        for f in out.clone() {
            for c in self.crc_set(&f) {
                needed.remove(&c);
            }
        }
        if !needed.is_empty() {
            for f in self.tex_bank_files() {
                if out.iter().any(|o| o.eq_ignore_ascii_case(&f)) {
                    continue;
                }
                let s = self.crc_set(&f);
                if needed.iter().any(|c| s.contains(c)) {
                    needed.retain(|c| !s.contains(c));
                    out.push(f);
                    if needed.is_empty() {
                        break;
                    }
                }
            }
        }
        out
    }

    /// Object banks for a level (collision file stem): level bank, world master
    /// bank and `gamegeneric` (cf. `InitLevel` → `LoadObjectBank` calls).
    pub fn level_object_banks(&self, level: &str) -> Vec<String> {
        let level = level.to_ascii_lowercase();
        let dlls: Vec<String> = self.list("objects", "dll").iter().map(|n| stem(n).to_ascii_lowercase()).collect();
        let mut out = Vec::new();
        if dlls.contains(&level) {
            out.push(level.clone());
        } else {
            // e.g. garden2 → garden1, labbossb → labbossa: longest common prefix.
            let lcp = |a: &str, b: &str| a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
            if let Some(best) = dlls.iter().filter(|d| !d.ends_with("master")).max_by_key(|d| (lcp(d, &level), std::cmp::Reverse(d.len()))) {
                if lcp(best, &level) >= 3 {
                    out.push(best.clone());
                }
            }
        }
        if let Some((_, master, _)) = world_of(&level) {
            if !master.is_empty() && dlls.iter().any(|d| d == master) {
                out.push(master.to_string());
            }
        }
        if dlls.iter().any(|d| d == "gamegeneric") {
            out.push("gamegeneric".into());
        }
        out
    }
}

fn collect_crcs(o: &f2_formats::objbank::Object, out: &mut HashSet<u32>) {
    if let Some(m) = &o.mesh {
        out.extend(m.textures.iter().copied().filter(|&c| c != 0));
    }
    for c in &o.children {
        collect_crcs(c, out);
    }
}

fn find_rendition(p: &Path) -> Option<PathBuf> {
    if p.join("game").is_dir() {
        return Some(p.to_path_buf());
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(p).ok()?.flatten().map(|e| e.path()).filter(|d| d.join("game").is_dir()).collect();
    subs.sort();
    subs.into_iter().next()
}
