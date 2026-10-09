//! Case-insensitive layered virtual filesystem for game data and mods.
//!
//! The original game ran on Windows and addresses files case-insensitively with
//! backslashes (`objects\garden1.dll`, `MAPS\SCRIPT-1-0.FEV`). [`Vfs`] reproduces
//! that over any host filesystem and stacks *layers*: the imported retail data is
//! the bottom layer and each enabled mod adds layers on top (later layers win).
//!
//! Virtual layout (see `docs/modding.md`):
//!
//! ```text
//! <path>         -> <rendition>/game/<path>   (and mods/<m>/game/<path>)
//! music/<path>   -> <rendition>/music/<path>  (and mods/<m>/music/<path>)
//! video/<path>   -> <rendition>/video/<path>  (and mods/<m>/video/<path>)
//! ```
//!
//! Per-entry overrides (loose replacements for single entries of a container)
//! live under `<kind>/<bank>/<name>.<ext>`, e.g. `textures/garden/00wate04.png`;
//! see [`Vfs::entry_overrides`].

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Version of the engine/mod API that mods declare compatibility with.
pub const ENGINE_API_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Splits a game path into lowercase-comparable components.
///
/// Accepts `\` and `/` separators, ignores empty and `.` components. Returns
/// `None` for paths that try to escape (`..`) or contain a drive prefix.
pub fn normalize(path: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for c in path.split(['/', '\\']) {
        match c {
            "" | "." => {}
            ".." => return None,
            c if c.contains(':') => return None,
            c => out.push(c.to_string()),
        }
    }
    Some(out)
}

/// One directory tree mounted at a virtual prefix.
#[derive(Clone, Debug)]
pub struct Layer {
    /// Human-readable layer name (e.g. `base`, `mod:my-mod`).
    pub name: String,
    /// Lowercase virtual mount prefix components (empty = root).
    pub prefix: Vec<String>,
    /// Host directory.
    pub root: PathBuf,
}

/// Host entries of `dir` matching `name` ASCII case-insensitively (Windows
/// semantics for the game's ASCII names): exact spelling first, then the rest
/// in sorted order, so host dirs that differ only in case are all searched.
fn find_ci(dir: &Path, name: &str) -> Vec<PathBuf> {
    let exact = dir.join(name);
    let mut out = Vec::new();
    if exact.symlink_metadata().is_ok() {
        out.push(exact.clone());
    }
    if let Ok(rd) = fs::read_dir(dir) {
        let mut rest: Vec<PathBuf> =
            rd.flatten().filter(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name)).map(|e| e.path()).filter(|p| *p != exact).collect();
        rest.sort();
        out.extend(rest);
    }
    out
}

fn resolve_in(dir: PathBuf, rest: &[String], want_file: bool) -> Option<PathBuf> {
    let Some((first, tail)) = rest.split_first() else {
        return (!want_file || dir.is_file()).then_some(dir);
    };
    find_ci(&dir, first).into_iter().find_map(|p| resolve_in(p, tail, want_file))
}

impl Layer {
    /// Host path for the virtual components, if the layer is mounted on a
    /// prefix of them and the file/dir exists.
    fn resolve(&self, comps: &[String], want_file: bool) -> Option<PathBuf> {
        let rest = strip_prefix(&self.prefix, comps)?;
        if !self.root.is_dir() {
            return None;
        }
        resolve_in(self.root.clone(), rest, want_file)
    }
}

fn strip_prefix<'a>(prefix: &[String], comps: &'a [String]) -> Option<&'a [String]> {
    if comps.len() < prefix.len() {
        return None;
    }
    for (a, b) in prefix.iter().zip(comps) {
        if !a.eq_ignore_ascii_case(b) {
            return None;
        }
    }
    Some(&comps[prefix.len()..])
}

/// Kind of a directory entry returned by [`Vfs::list`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
}

/// A directory entry returned by [`Vfs::list`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// Name as spelled by the highest-priority layer that has it.
    pub name: String,
    pub kind: EntryKind,
}

/// A loose per-entry override found by [`Vfs::entry_overrides`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryOverride {
    /// Bank (container) name, lowercase, e.g. `garden`.
    pub bank: String,
    /// Entry name without extension, lowercase, e.g. `00wate04`.
    pub name: String,
    /// File extension, lowercase, e.g. `png`.
    pub ext: String,
    /// Host path of the winning file.
    pub path: PathBuf,
    /// Name of the layer providing it.
    pub layer: String,
}

/// Case-insensitive layered virtual filesystem.
#[derive(Clone, Debug, Default)]
pub struct Vfs {
    layers: Vec<Layer>,
}

impl Vfs {
    pub fn new() -> Vfs {
        Vfs::default()
    }

    /// Base layers for an imported rendition dir (`<data_root>/<rendition>`):
    /// `game/` at the root, `music/` and `video/` as virtual top-level dirs.
    pub fn with_rendition(rendition_dir: impl AsRef<Path>) -> Vfs {
        let mut v = Vfs::new();
        v.add_rendition_layers("base", rendition_dir.as_ref());
        v
    }

    fn add_rendition_layers(&mut self, name: &str, dir: &Path) {
        self.add_layer(name, dir.join("game"));
        self.mount(format!("{name}/music"), "music", dir.join("music"));
        self.mount(format!("{name}/video"), "video", dir.join("video"));
    }

    /// Adds a layer mounted at the virtual root. Later layers override earlier ones.
    pub fn add_layer(&mut self, name: impl Into<String>, root_dir: impl Into<PathBuf>) {
        self.mount(name, "", root_dir);
    }

    /// Adds a layer mounted at virtual directory `prefix` (e.g. `music`).
    pub fn mount(&mut self, name: impl Into<String>, prefix: &str, root_dir: impl Into<PathBuf>) {
        let prefix = normalize(prefix).unwrap_or_default().into_iter().map(|c| c.to_ascii_lowercase()).collect();
        self.layers.push(Layer { name: name.into(), prefix, root: root_dir.into() });
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Host path of the topmost layer's file or directory for `path`.
    pub fn resolve(&self, path: &str) -> Option<PathBuf> {
        let comps = normalize(path)?;
        self.layers.iter().rev().find_map(|l| l.resolve(&comps, false))
    }

    /// Like [`Vfs::resolve`] but only matches regular files.
    pub fn resolve_file(&self, path: &str) -> Option<PathBuf> {
        let comps = normalize(path)?;
        self.layers.iter().rev().find_map(|l| l.resolve(&comps, true))
    }

    pub fn exists(&self, path: &str) -> bool {
        self.resolve(path).is_some() || self.is_virtual_dir(path)
    }

    fn is_virtual_dir(&self, path: &str) -> bool {
        let Some(comps) = normalize(path) else { return false };
        self.layers.iter().any(|l| l.prefix.len() > comps.len() && strip_prefix(&comps, &l.prefix).is_some())
    }

    /// Opens the topmost version of a file for reading.
    pub fn open(&self, path: &str) -> io::Result<fs::File> {
        fs::File::open(self.resolve_file(path).ok_or_else(|| not_found(path))?)
    }

    /// Reads the topmost version of a file.
    pub fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        fs::read(self.resolve_file(path).ok_or_else(|| not_found(path))?)
    }

    /// Merged listing of a virtual directory across all layers (names
    /// deduplicated case-insensitively, sorted by lowercase name). Mount points
    /// below `dir` appear as directories.
    pub fn list(&self, dir: &str) -> io::Result<Vec<DirEntry>> {
        let comps = normalize(dir).ok_or_else(|| not_found(dir))?;
        let mut found = false;
        let mut map: BTreeMap<String, DirEntry> = BTreeMap::new();
        for l in self.layers.iter().rev() {
            // Mount points nested under `dir`.
            if l.prefix.len() > comps.len() && strip_prefix(&comps, &l.prefix).is_some() {
                found = true;
                let n = &l.prefix[comps.len()];
                map.entry(n.to_ascii_lowercase()).or_insert(DirEntry { name: n.clone(), kind: EntryKind::Dir });
                continue;
            }
            let Some(p) = l.resolve(&comps, false) else { continue };
            let Ok(rd) = fs::read_dir(&p) else { continue };
            found = true;
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let kind = if e.path().is_dir() { EntryKind::Dir } else { EntryKind::File };
                map.entry(name.to_ascii_lowercase()).or_insert(DirEntry { name, kind });
            }
        }
        if !found {
            return Err(not_found(dir));
        }
        Ok(map.into_values().collect())
    }

    /// Loose per-entry overrides of container kind `kind` (`textures`, `sfx`,
    /// `objects`, …): every file `<kind>/<bank>/<name>.<ext>` in any layer,
    /// keyed by (bank, name) case-insensitively; the topmost layer wins.
    /// Sorted by (bank, name).
    pub fn entry_overrides(&self, kind: &str) -> Vec<EntryOverride> {
        let mut map: BTreeMap<(String, String), EntryOverride> = BTreeMap::new();
        let Some(kcomps) = normalize(kind) else { return Vec::new() };
        for l in self.layers.iter().rev() {
            let Some(kdir) = l.resolve(&kcomps, false) else { continue };
            let Ok(banks) = fs::read_dir(&kdir) else { continue };
            for b in banks.flatten() {
                if !b.path().is_dir() {
                    continue;
                }
                let bank = b.file_name().to_string_lossy().to_ascii_lowercase();
                let Ok(files) = fs::read_dir(b.path()) else { continue };
                for f in files.flatten() {
                    let path = f.path();
                    if !path.is_file() {
                        continue;
                    }
                    let fname = f.file_name().to_string_lossy().into_owned();
                    let (name, ext) = match fname.rsplit_once('.') {
                        Some((n, e)) if !n.is_empty() => (n.to_ascii_lowercase(), e.to_ascii_lowercase()),
                        _ => (fname.to_ascii_lowercase(), String::new()),
                    };
                    map.entry((bank.clone(), name.clone())).or_insert(EntryOverride { bank: bank.clone(), name, ext, path, layer: l.name.clone() });
                }
            }
        }
        map.into_values().collect()
    }

    /// Adds the layers of every enabled mod (in `enabled_ids` order, later wins)
    /// found in `mods_dir`. Each mod directory holds `mod.toml` plus optional
    /// `game/`, `music/`, `video/` trees.
    pub fn load_enabled_mods(&mut self, mods_dir: impl AsRef<Path>, enabled_ids: &[&str]) -> Result<Vec<ModManifest>, ModError> {
        let available = scan_mods(mods_dir.as_ref())?;
        let mut loaded = Vec::new();
        for (i, id) in enabled_ids.iter().enumerate() {
            if enabled_ids[..i].contains(id) {
                return Err(ModError::Duplicate(id.to_string()));
            }
            let (dir, m) = available.iter().find(|(_, m)| m.id == *id).ok_or_else(|| ModError::NotFound(id.to_string()))?;
            m.check_compatible()?;
            self.add_rendition_layers(&format!("mod:{}", m.id), dir);
            loaded.push(m.clone());
        }
        Ok(loaded)
    }
}

/// Builds the VFS for `rendition_dir` (e.g. `game-data/usa`) with the enabled
/// mods from `mods_dir` stacked on top.
pub fn load_enabled_mods(rendition_dir: impl AsRef<Path>, mods_dir: impl AsRef<Path>, enabled_ids: &[&str]) -> Result<(Vfs, Vec<ModManifest>), ModError> {
    let mut v = Vfs::with_rendition(rendition_dir);
    let mods = v.load_enabled_mods(mods_dir, enabled_ids)?;
    Ok((v, mods))
}

/// All mods (directories with a `mod.toml`) in `mods_dir`, sorted by dir name.
pub fn scan_mods(mods_dir: &Path) -> Result<Vec<(PathBuf, ModManifest)>, ModError> {
    let mut out = Vec::new();
    let rd = match fs::read_dir(mods_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(ModError::Io(mods_dir.to_path_buf(), e)),
    };
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for d in dirs {
        let Some(mt) = find_ci(&d, "mod.toml").into_iter().find(|p| p.is_file()) else { continue };
        let m = ModManifest::load(&mt)?;
        out.push((d, m));
    }
    Ok(out)
}

fn not_found(path: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("{path}: not found in VFS"))
}

/// `mod.toml`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub authors: Vec<String>,
    /// Semver requirement on [`ENGINE_API_VERSION`], e.g. `">=0.1, <0.2"`.
    pub engine_version: String,
    /// Mods that change gameplay are excluded from vanilla-only modes.
    #[serde(default)]
    pub gameplay_affecting: bool,
    #[serde(default)]
    pub description: String,
}

impl ModManifest {
    pub fn parse(text: &str) -> Result<ModManifest, ModError> {
        let m: ModManifest = toml::from_str(text).map_err(|e| ModError::Manifest(e.to_string()))?;
        if m.id.is_empty() || !m.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(ModError::Manifest(format!("invalid mod id {:?} (use [A-Za-z0-9_-])", m.id)));
        }
        semver::VersionReq::parse(&m.engine_version).map_err(|e| ModError::Manifest(format!("{}: bad engine_version {:?}: {e}", m.id, m.engine_version)))?;
        Ok(m)
    }

    pub fn load(path: &Path) -> Result<ModManifest, ModError> {
        let text = fs::read_to_string(path).map_err(|e| ModError::Io(path.to_path_buf(), e))?;
        ModManifest::parse(&text).map_err(|e| match e {
            ModError::Manifest(m) => ModError::Manifest(format!("{}: {m}", path.display())),
            e => e,
        })
    }

    /// Whether this mod accepts engine API version `engine`.
    pub fn accepts(&self, engine: &str) -> bool {
        match (semver::VersionReq::parse(&self.engine_version), semver::Version::parse(engine)) {
            (Ok(r), Ok(v)) => r.matches(&v),
            _ => false,
        }
    }

    pub fn check_compatible(&self) -> Result<(), ModError> {
        if self.accepts(ENGINE_API_VERSION) {
            Ok(())
        } else {
            Err(ModError::Incompatible { id: self.id.clone(), requires: self.engine_version.clone(), engine: ENGINE_API_VERSION.to_string() })
        }
    }
}

#[derive(Debug)]
pub enum ModError {
    Io(PathBuf, io::Error),
    Manifest(String),
    NotFound(String),
    Duplicate(String),
    Incompatible { id: String, requires: String, engine: String },
}

impl fmt::Display for ModError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            ModError::Manifest(m) => write!(f, "bad mod.toml: {m}"),
            ModError::NotFound(id) => write!(f, "enabled mod {id:?} not found"),
            ModError::Duplicate(id) => write!(f, "mod {id:?} enabled twice"),
            ModError::Incompatible { id, requires, engine } => {
                write!(f, "mod {id:?} requires engine {requires}, this engine is {engine}")
            }
        }
    }
}

impl std::error::Error for ModError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, data: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    fn rendition() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "game/objects/Garden1.dll", b"base-garden");
        put(t.path(), "game/maps/script-1-0.fev", b"base-script");
        put(t.path(), "game/textures/Garden.fla", b"bank");
        put(t.path(), "music/track02.wav", b"music");
        put(t.path(), "video/01s.bik", b"video");
        t
    }

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize("objects\\garden1.dll").unwrap(), ["objects", "garden1.dll"]);
        assert_eq!(normalize("/MAPS//./x").unwrap(), ["MAPS", "x"]);
        assert!(normalize("a/../b").is_none());
        assert!(normalize("c:\\x").is_none());
    }

    #[test]
    fn case_insensitive_and_backslashes() {
        let t = rendition();
        let v = Vfs::with_rendition(t.path());
        assert_eq!(v.read("objects\\garden1.dll").unwrap(), b"base-garden");
        assert_eq!(v.read("OBJECTS/GARDEN1.DLL").unwrap(), b"base-garden");
        assert_eq!(v.read("MAPS/SCRIPT-1-0.FEV").unwrap(), b"base-script");
        assert!(v.exists("Maps"));
        assert!(!v.exists("maps/nope.fev"));
        assert_eq!(v.read("maps/nope.fev").unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(v.read("maps").is_err(), "directories are not readable files");
        assert!(v.resolve("../game/maps").is_none());
        assert_eq!(v.read("Music\\TRACK02.WAV").unwrap(), b"music");
        assert_eq!(v.read("video/01S.BIK").unwrap(), b"video");
        let mut s = String::new();
        io::Read::read_to_string(&mut v.open("objects/garden1.dll").unwrap(), &mut s).unwrap();
        assert_eq!(s, "base-garden");
    }

    #[test]
    fn later_layers_override() {
        let t = rendition();
        let m1 = tempfile::tempdir().unwrap();
        let m2 = tempfile::tempdir().unwrap();
        put(m1.path(), "OBJECTS/garden1.DLL", b"mod1");
        put(m1.path(), "objects/new.dll", b"new1");
        put(m2.path(), "objects/GARDEN1.dll", b"mod2");
        let mut v = Vfs::with_rendition(t.path());
        v.add_layer("m1", m1.path());
        assert_eq!(v.read("objects\\garden1.dll").unwrap(), b"mod1");
        v.add_layer("m2", m2.path());
        assert_eq!(v.read("objects\\garden1.dll").unwrap(), b"mod2");
        assert_eq!(v.read("objects/NEW.dll").unwrap(), b"new1");
        assert!(v.resolve("objects/garden1.dll").unwrap().starts_with(m2.path()));
    }

    #[test]
    fn listing_merges_layers_and_mounts() {
        let t = rendition();
        let m = tempfile::tempdir().unwrap();
        put(m.path(), "objects/GARDEN1.DLL", b"x");
        put(m.path(), "objects/extra.dll", b"x");
        let mut v = Vfs::with_rendition(t.path());
        v.add_layer("m", m.path());
        let names: Vec<_> = v.list("Objects").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["extra.dll", "GARDEN1.DLL"]);
        let root = v.list("").unwrap();
        let dirs: Vec<_> = root.iter().filter(|e| e.kind == EntryKind::Dir).map(|e| e.name.to_ascii_lowercase()).collect();
        for d in ["maps", "music", "objects", "textures", "video"] {
            assert!(dirs.contains(&d.to_string()), "{d} in {dirs:?}");
        }
        assert_eq!(v.list("music").unwrap()[0].name, "track02.wav");
        assert!(v.list("nope").is_err());
    }

    #[test]
    fn entry_overrides_listing() {
        let t = rendition();
        let m1 = tempfile::tempdir().unwrap();
        let m2 = tempfile::tempdir().unwrap();
        put(m1.path(), "textures/Garden/00WATE04.png", b"a");
        put(m1.path(), "textures/garden/other.png", b"b");
        put(m2.path(), "Textures/GARDEN/00wate04.png", b"c");
        put(m2.path(), "sfx/generic/hop.wav", b"d");
        let mut v = Vfs::with_rendition(t.path());
        v.add_layer("m1", m1.path());
        v.add_layer("m2", m2.path());
        let o = v.entry_overrides("textures");
        assert_eq!(o.len(), 2, "{o:?}");
        assert_eq!((o[0].bank.as_str(), o[0].name.as_str(), o[0].ext.as_str(), o[0].layer.as_str()), ("garden", "00wate04", "png", "m2"));
        assert_eq!(fs::read(&o[0].path).unwrap(), b"c");
        assert_eq!(o[1].name, "other");
        let s = v.entry_overrides("SFX");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].bank, "generic");
        // Base data's textures/Garden.fla is a file, not a bank dir: not an override.
        assert!(Vfs::with_rendition(t.path()).entry_overrides("textures").is_empty());
    }

    const MANIFEST: &str = r#"
id = "hd-water"
name = "HD Water"
version = "1.2.0"
authors = ["someone"]
engine_version = ">=0.1, <1.0"
gameplay_affecting = false
"#;

    #[test]
    fn manifest_parse() {
        let m = ModManifest::parse(MANIFEST).unwrap();
        assert_eq!(m.id, "hd-water");
        assert_eq!(m.authors, ["someone"]);
        assert!(!m.gameplay_affecting);
        assert!(m.accepts("0.1.0"));
        assert!(!m.accepts("1.0.0"));
        assert!(m.check_compatible().is_ok());
        assert!(ModManifest::parse("id='x'\nname='x'\nversion='1'\nengine_version='nonsense'").is_err());
        assert!(ModManifest::parse("id='bad id'\nname='x'\nversion='1'\nengine_version='*'").is_err());
        assert!(ModManifest::parse("name='x'").is_err());
        let g = ModManifest::parse("id='g'\nname='G'\nversion='1'\nengine_version='*'\ngameplay_affecting=true").unwrap();
        assert!(g.gameplay_affecting);
        assert!(g.authors.is_empty());
    }

    #[test]
    fn load_mods_stack() {
        let t = rendition();
        let mods = tempfile::tempdir().unwrap();
        put(mods.path(), "a/mod.toml", b"id='a'\nname='A'\nversion='1'\nengine_version='*'");
        put(mods.path(), "a/game/objects/garden1.dll", b"A");
        put(mods.path(), "a/music/TRACK02.wav", b"A-music");
        put(mods.path(), "b-dir/MOD.TOML", b"id='b'\nname='B'\nversion='1'\nengine_version='*'\ngameplay_affecting=true");
        put(mods.path(), "b-dir/game/OBJECTS/Garden1.dll", b"B");
        put(mods.path(), "old/mod.toml", b"id='old'\nname='O'\nversion='1'\nengine_version='<0.0.1'");
        put(mods.path(), "notamod/readme.txt", b"");

        let (v, loaded) = load_enabled_mods(t.path(), mods.path(), &["a", "b"]).unwrap();
        assert_eq!(loaded.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(v.read("objects\\garden1.dll").unwrap(), b"B");
        assert_eq!(v.read("music/track02.wav").unwrap(), b"A-music");
        assert_eq!(v.read("maps/script-1-0.fev").unwrap(), b"base-script");

        let (v, _) = load_enabled_mods(t.path(), mods.path(), &["b", "a"]).unwrap();
        assert_eq!(v.read("objects\\garden1.dll").unwrap(), b"A");

        let (v, none) = load_enabled_mods(t.path(), mods.path(), &[]).unwrap();
        assert!(none.is_empty());
        assert_eq!(v.read("objects\\garden1.dll").unwrap(), b"base-garden");

        assert!(matches!(load_enabled_mods(t.path(), mods.path(), &["zzz"]), Err(ModError::NotFound(_))));
        assert!(matches!(load_enabled_mods(t.path(), mods.path(), &["a", "a"]), Err(ModError::Duplicate(_))));
        let e = load_enabled_mods(t.path(), mods.path(), &["old"]).unwrap_err();
        assert!(matches!(e, ModError::Incompatible { .. }), "{e}");
        // Missing mods dir is simply empty.
        assert!(scan_mods(&mods.path().join("missing")).unwrap().is_empty());
    }

    /// Smoke test over real imported data when present (`F2_GAME_DIR`).
    #[test]
    fn real_data_smoke() {
        let dir = std::env::var("F2_GAME_DIR").unwrap_or_else(|_| "../../game-data/usa".into());
        let dir = PathBuf::from(dir);
        if !dir.join("game").is_dir() {
            eprintln!("skipping: no game data at {}", dir.display());
            return;
        }
        let v = Vfs::with_rendition(&dir);
        assert!(v.exists("OBJECTS\\GARDEN1.DLL"));
        assert!(v.exists("maps/language.fla"));
        assert!(v.exists("music"));
    }
}
