//! Locating the imported data and exposing it to the simulation.
//!
//! [`Data`] is the game's [`DataSource`]: the mod VFS over the selected rendition. With mods
//! enabled (`config.toml` `[mods] enabled`, `--mods`), banks are the *effective* containers
//! from `f2-modkit` (retail bank + per-entry overrides), ETC script sources are compiled
//! with `f2-etc` when read, and `levels/*/level.toml` manifests become the game's level
//! registry ([`crate::mods`]). Without mods every read is the plain retail file.

use f2_formats::lang::StringTable;
use f2_formats::objbank::ObjectBank;
use f2_formats::texture::TexBank;
use f2_formats::wus::SoundBank;
use f2_game::assets::DataSource;
use f2_mods::{ModManifest, Vfs};
use f2_platform::Config;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Effective containers built so far (deterministic in the enabled-mod list).
#[derive(Default)]
struct Cache {
    objects: HashMap<String, Option<Result<ObjectBank, String>>>,
    textures: HashMap<String, Option<Result<TexBank, String>>>,
    sounds: HashMap<String, Option<Result<SoundBank, String>>>,
    strings: Option<Option<Result<StringTable, String>>>,
    scripts: HashMap<String, Option<Vec<u8>>>,
}

/// The mod VFS over the selected rendition.
#[derive(Clone)]
pub struct Data {
    pub vfs: Arc<Vfs>,
    pub rendition: PathBuf,
    /// Enabled mods in load order (empty = authentic data).
    pub mods: Vec<ModManifest>,
    /// Levels/strings from the mods' level manifests (empty without mods).
    pub levels: f2_game::levels::LevelRegistry,
    cache: Arc<Mutex<Cache>>,
}

impl DataSource for Data {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        if !self.mods.is_empty() {
            if let Some(r) = self.script(path) {
                return r;
            }
        }
        self.vfs.read(path).ok()
    }

    fn object_bank(&self, path: &str) -> Option<Result<ObjectBank, String>> {
        if self.mods.is_empty() {
            return self.vfs.read(path).ok().map(|b| ObjectBank::parse(&b).map_err(|e| format!("{path}: {e}")));
        }
        let key = path.replace('\\', "/").to_ascii_lowercase();
        if let Some(v) = self.cache.lock().unwrap().objects.get(&key) {
            return v.clone();
        }
        let v = match bank_name(path, "objects", "dll") {
            Some(bank) if self.vfs.exists(path) || !f2_modkit::list_overrides(&self.vfs, "objects", &bank, &["glb", "gltf"]).is_empty() => {
                Some(f2_modkit::object_bank(&self.vfs, &bank).map(|l| self.loaded(path, l)).map_err(|e| e.to_string()))
            }
            _ => self.vfs.read(path).ok().map(|b| ObjectBank::parse(&b).map_err(|e| format!("{path}: {e}"))),
        };
        self.cache.lock().unwrap().objects.insert(key, v.clone());
        v
    }

    fn texture_bank(&self, path: &str) -> Option<Result<TexBank, String>> {
        let raw = || {
            self.vfs.read(path).ok().map(|b| {
                f2_formats::fla::maybe_decompress(b)
                    .map_err(|e| format!("{path}: {e}"))
                    .and_then(|b| TexBank::parse(&b).map_err(|e| format!("{path}: {e}")))
            })
        };
        if self.mods.is_empty() {
            return raw();
        }
        let key = path.replace('\\', "/").to_ascii_lowercase();
        if let Some(v) = self.cache.lock().unwrap().textures.get(&key) {
            return v.clone();
        }
        let v = match bank_name(path, "textures", "fla") {
            Some(bank) if !f2_modkit::is_backdrop(&bank) && (self.vfs.exists(path) || !f2_modkit::list_overrides(&self.vfs, "textures", &bank, &["png"]).is_empty()) => {
                Some(f2_modkit::tex_bank(&self.vfs, &bank).map(|l| self.loaded(path, l)).map_err(|e| e.to_string()))
            }
            _ => raw(),
        };
        self.cache.lock().unwrap().textures.insert(key, v.clone());
        v
    }

    fn sound_bank(&self, path: &str) -> Option<Result<SoundBank, String>> {
        if self.mods.is_empty() {
            return self.vfs.read(path).ok().map(|b| SoundBank::parse(&b).map_err(|e| format!("{path}: {e}")));
        }
        let key = path.replace('\\', "/").to_ascii_lowercase();
        if let Some(v) = self.cache.lock().unwrap().sounds.get(&key) {
            return v.clone();
        }
        let v = match bank_name(path, "sfx", "wus") {
            Some(bank) if self.vfs.exists(path) || !f2_modkit::list_overrides(&self.vfs, "sfx", &bank, &["wav", "flac", "ogg"]).is_empty() => {
                Some(f2_modkit::sound_bank(&self.vfs, &bank).map(|l| self.loaded(path, l)).map_err(|e| e.to_string()))
            }
            _ => self.vfs.read(path).ok().map(|b| SoundBank::parse(&b).map_err(|e| format!("{path}: {e}"))),
        };
        self.cache.lock().unwrap().sounds.insert(key, v.clone());
        v
    }

    fn string_table(&self, path: &str) -> Option<Result<StringTable, String>> {
        let is_language = f2_mods::normalize(path).is_some_and(|c| c.len() == 2 && c[0].eq_ignore_ascii_case("maps") && c[1].eq_ignore_ascii_case("language.fla"));
        if self.mods.is_empty() || !is_language {
            return self.vfs.read(path).ok().map(|b| {
                f2_formats::fla::maybe_decompress(b).map_err(|e| e.to_string()).and_then(|b| StringTable::parse(&b).map_err(|e| e.to_string()))
            });
        }
        if let Some(v) = &self.cache.lock().unwrap().strings {
            return v.clone();
        }
        let v = Some(f2_modkit::string_table(&self.vfs).map(|l| self.loaded(path, l)).map_err(|e| e.to_string()));
        self.cache.lock().unwrap().strings = Some(v.clone());
        v
    }
}

/// `<dir>\<name>.<ext>` → `name` (a bank `f2-modkit` can resolve with per-entry overrides).
fn bank_name(path: &str, dir: &str, ext: &str) -> Option<String> {
    let c = f2_mods::normalize(path)?;
    if c.len() != 2 || !c[0].eq_ignore_ascii_case(dir) {
        return None;
    }
    let (stem, e) = c[1].rsplit_once('.')?;
    (e.eq_ignore_ascii_case(ext) && !stem.is_empty()).then(|| stem.to_string())
}

impl Data {
    pub fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        self.vfs.read(path).map_err(|e| format!("{path}: {e}"))
    }

    fn new(vfs: Vfs, rendition: PathBuf) -> Data {
        Data { vfs: Arc::new(vfs), rendition, mods: Vec::new(), levels: Default::default(), cache: Default::default() }
    }

    /// Logs what the overrides did and returns the effective container.
    fn loaded<T>(&self, path: &str, l: f2_modkit::Loaded<T>) -> T {
        if !l.replaced.is_empty() || !l.added.is_empty() {
            eprintln!("frogger: mods: {path}: {} entries replaced, {} added", l.replaced.len(), l.added.len());
        }
        for w in &l.warnings {
            eprintln!("frogger: mods: warning: {w}");
        }
        l.value
    }

    /// Stacks the enabled mods (`ids`, load order) from `mods_dir` on the VFS and builds the
    /// level registry from their `levels/*/level.toml`.
    pub fn enable_mods(&mut self, mods_dir: &Path, ids: &[String]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut vfs = (*self.vfs).clone();
        let ids: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        let mods = vfs.load_enabled_mods(mods_dir, &ids).map_err(|e| format!("mods ({}): {e}", mods_dir.display()))?;
        for m in &mods {
            eprintln!("frogger: mod {} {} ({}){}", m.id, m.version, m.name, if m.gameplay_affecting { " [gameplay]" } else { "" });
        }
        self.vfs = Arc::new(vfs);
        self.mods = mods;
        *self.cache.lock().unwrap() = Cache::default();
        let (reg, warnings) = crate::mods::build_registry(self);
        for w in warnings {
            eprintln!("frogger: mods: warning: {w}");
        }
        for l in &reg.levels {
            eprintln!("frogger: mods: level {}-{} registered", l.world, l.level);
        }
        self.levels = reg;
        Ok(())
    }

    /// Host path of CD track `n` (topmost layer's `music/trackNN.wav|.flac`).
    pub fn music_track(&self, n: u32) -> Option<PathBuf> {
        f2_modkit::music_track(&self.vfs, n)
    }

    /// ETC script compilation: `path` is ETC source (`.etc`/`.fes`), or a `.fev` with a
    /// source of the same name in a layer at least as high as the `.fev`'s. `None` = not a
    /// script read (plain VFS file); `Some(None)` = compile failed (logged).
    fn script(&self, path: &str) -> Option<Option<Vec<u8>>> {
        let (stem, ext) = path.rsplit_once('.')?;
        let ext = ext.to_ascii_lowercase();
        let src = match ext.as_str() {
            "etc" | "fes" => self.vfs.resolve_file(path)?,
            "fev" => {
                let rank = |p: &Path| self.vfs.layers().iter().rposition(|l| p.starts_with(&l.root)).unwrap_or(0);
                let fev = self.vfs.resolve_file(path).map(|p| rank(&p));
                let src = ["etc", "fes"].iter().filter_map(|e| self.vfs.resolve_file(&format!("{stem}.{e}"))).max_by_key(|p| rank(p))?;
                if fev.is_some_and(|r| r > rank(&src)) {
                    return None;
                }
                src
            }
            _ => return None,
        };
        let key = path.replace('\\', "/").to_ascii_lowercase();
        if let Some(v) = self.cache.lock().unwrap().scripts.get(&key) {
            return Some(v.clone());
        }
        let out = match std::fs::read(&src) {
            Ok(text) => match f2_etc::compile(&src.to_string_lossy(), &text) {
                Ok(c) => {
                    for w in &c.warnings {
                        eprintln!("frogger: mods: {w}");
                    }
                    eprintln!("frogger: mods: compiled {} for {path} ({} bytes)", src.display(), c.data.len());
                    Some(c.data)
                }
                Err(diags) => {
                    for d in diags {
                        eprintln!("frogger: mods: error: {d}");
                    }
                    None
                }
            },
            Err(e) => {
                eprintln!("frogger: mods: {}: {e}", src.display());
                None
            }
        };
        self.cache.lock().unwrap().scripts.insert(key, out.clone());
        Some(out)
    }

    /// Finds the rendition: `--data <dir>` (a rendition or a data root), else the config's
    /// data root/rendition, else `$F2_GAME_DIR` / `game-data` (developer checkouts).
    pub fn locate(cfg: &Config, arg: Option<PathBuf>) -> Result<Data, String> {
        let mut tried = Vec::new();
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(a) = arg {
            candidates.push(a);
        } else {
            let root = cfg.data_root();
            if let Some(r) = &cfg.rendition {
                candidates.push(root.join(r));
            }
            candidates.push(root);
            if let Some(g) = std::env::var_os("F2_GAME_DIR") {
                candidates.push(PathBuf::from(g));
            }
            candidates.push(PathBuf::from("game-data"));
        }
        for c in candidates {
            if let Some(r) = find_rendition(&c) {
                return Ok(Data::new(Vfs::with_rendition(&r), r));
            }
            tried.push(c.display().to_string());
        }
        Err(format!(
            "no imported game data found (looked in: {}).\n\
             Import your Frogger 2 CD first:  f2-import <disc image (.cue/.iso/.bin/.zip) or disc/install dir>\n\
             or point the game at existing data with --data <dir> or F2_DATA_ROOT.",
            tried.join(", ")
        ))
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
