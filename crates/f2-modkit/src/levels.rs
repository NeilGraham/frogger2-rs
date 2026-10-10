//! Level manifests: `levels/<id>/level.toml` in a mod (see `docs/modding.md` §3).
//!
//! A manifest adds a level to (or overrides) one `worldVisualData[world].levelVisualData[level]`
//! slot. This module only parses and validates; the host turns a [`LevelManifest`] into the
//! game's level registry (`f2_game::levels`).
//!
//! File values are VFS paths (`COLLISION/GARDEN1.COL` reuses a retail file by name); a value
//! starting with `./` is relative to the manifest's directory (`levels/<id>/…`).

use std::collections::BTreeMap;

use f2_mods::{EntryKind, Vfs};
use serde::Deserialize;

use crate::Error;

/// `[files]` of a level manifest. Unset files come from `base` (or the slot's own row).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LevelFilesToml {
    pub collision: Option<String>,
    pub scenics: Option<String>,
    pub entities: Option<String>,
    /// Compiled `.fev`, or ETC source (`.etc`/`.fes`, compiled at load).
    pub script: Option<String>,
    pub sfx_mapping: Option<String>,
    pub texture_bank: Option<String>,
    pub world_object_bank: Option<String>,
    pub object_bank: Option<String>,
    pub sfx_bank: Option<String>,
}

/// `[world_info]`: world-level changes for the slot's world.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorldToml {
    pub world_id: Option<u32>,
    pub name: Option<String>,
    pub text_id: Option<String>,
    pub open: Option<bool>,
}

/// `level.toml`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LevelManifest {
    /// Defaults to the directory name.
    pub id: Option<String>,
    /// `worldVisualData` slot (0–9).
    pub world: u32,
    /// `levelVisualData` slot (0–19).
    pub level: u32,
    /// `[world, level]` of a retail slot whose row and files are copied first.
    pub base: Option<[u32; 2]>,
    /// Literal level name (Latin-1).
    pub name: Option<String>,
    /// Level name by string id (`STR_…`), instead of `name`.
    pub text_id: Option<String>,
    /// `levelID` (default file names `ENTITY-<worldID>-<levelID>.DAT` …).
    pub level_id: Option<u32>,
    /// `[next world, next level]` after completion (`nextWorldID`, `nextLevelID`).
    pub next: Option<[i32; 2]>,
    /// Fog / clear colour `[r, g, b]` (0–255).
    pub fog: Option<[u8; 3]>,
    pub fog_near: Option<u32>,
    pub fog_far: Option<u32>,
    pub far_clip: Option<i32>,
    /// Par time (seconds × 10 as in the original table).
    pub par_time: Option<u32>,
    /// CD track number (`music/trackNN.*`).
    pub music: Option<u32>,
    /// Open at the start of a new game (default true).
    pub open: Option<bool>,
    #[serde(default)]
    pub files: LevelFilesToml,
    /// `[world_info]`: changes to the slot's world.
    pub world_info: Option<WorldToml>,
    /// P7b behaviour scripts (`.f2s`, VFS paths; `./x` = this level's dir) whose hooks run
    /// only on this level.
    #[serde(default)]
    pub scripts: Vec<String>,
}

/// A parsed manifest and where it lives.
#[derive(Clone, Debug)]
pub struct FoundLevel {
    /// Directory name under `levels/` (lowercase as listed).
    pub dir: String,
    pub manifest: LevelManifest,
}

impl LevelManifest {
    pub fn parse(text: &str) -> Result<LevelManifest, Error> {
        let m: LevelManifest = toml::from_str(text).map_err(|e| Error::new(e.to_string()))?;
        m.check()?;
        Ok(m)
    }

    fn check(&self) -> Result<(), Error> {
        if self.world >= 10 {
            return Err(Error::new(format!("world {} out of range (0-9)", self.world)));
        }
        if self.level >= 20 {
            return Err(Error::new(format!("level {} out of range (0-19)", self.level)));
        }
        if let Some([w, l]) = self.base {
            if w >= 10 || l >= 20 {
                return Err(Error::new(format!("base [{w}, {l}] out of range")));
            }
        }
        if self.name.is_some() && self.text_id.is_some() {
            return Err(Error::new("give either name or text_id, not both"));
        }
        for n in self.name.iter().chain(self.world_info.iter().flat_map(|w| w.name.iter())) {
            if let Some(c) = n.chars().find(|c| *c as u32 > 0xff) {
                return Err(Error::new(format!("name {n:?}: {c:?} is outside Latin-1 (game font)")));
            }
        }
        Ok(())
    }

    /// `(field, value)` of every file the manifest names.
    pub fn file_entries(&self) -> Vec<(&'static str, &str)> {
        let f = &self.files;
        [
            ("collision", &f.collision),
            ("scenics", &f.scenics),
            ("entities", &f.entities),
            ("script", &f.script),
            ("sfx_mapping", &f.sfx_mapping),
            ("texture_bank", &f.texture_bank),
            ("world_object_bank", &f.world_object_bank),
            ("object_bank", &f.object_bank),
            ("sfx_bank", &f.sfx_bank),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.as_deref().map(|v| (k, v)))
        .collect()
    }
}

/// Resolves a manifest file value to a VFS path (`./x` → `levels/<dir>/x`).
pub fn level_path(dir: &str, value: &str) -> String {
    match value.strip_prefix("./") {
        Some(rest) => format!("levels/{dir}/{rest}"),
        None => value.to_string(),
    }
}

/// Every `levels/<dir>/level.toml` visible in `vfs` (topmost layer per dir), sorted by dir.
/// Broken manifests are skipped and reported in the second value.
pub fn find_levels(vfs: &Vfs) -> (Vec<FoundLevel>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    let Ok(list) = vfs.list("levels") else { return (out, warnings) };
    for e in list.into_iter().filter(|e| e.kind == EntryKind::Dir) {
        let path = format!("levels/{}/level.toml", e.name);
        let Ok(bytes) = vfs.read(&path) else { continue };
        match LevelManifest::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(m) => out.push(FoundLevel { dir: e.name.clone(), manifest: m }),
            Err(err) => warnings.push(format!("{path}: {err}")),
        }
    }
    (out, warnings)
}

/// Checks that every file a manifest names exists in `vfs` (with `.etc`/`.fes` scripts as
/// source). Returns error messages.
pub fn check_files(vfs: &Vfs, l: &FoundLevel) -> Vec<String> {
    let mut errs = Vec::new();
    for (k, v) in l.manifest.file_entries() {
        let p = level_path(&l.dir, v);
        if vfs.resolve_file(&p).is_none() {
            errs.push(format!("levels/{}/level.toml: {k} = {v:?}: {p} not found", l.dir));
        }
    }
    for v in &l.manifest.scripts {
        let p = level_path(&l.dir, v);
        if vfs.resolve_file(&p).is_none() {
            errs.push(format!("levels/{}/level.toml: scripts: {v:?}: {p} not found", l.dir));
        }
    }
    errs
}

/// Slots claimed by more than one manifest (later dir wins at load), for warnings.
pub fn duplicate_slots(levels: &[FoundLevel]) -> Vec<String> {
    let mut seen: BTreeMap<(u32, u32), &str> = BTreeMap::new();
    let mut out = Vec::new();
    for l in levels {
        if let Some(prev) = seen.insert((l.manifest.world, l.manifest.level), &l.dir) {
            out.push(format!("levels {prev} and {} both use slot {}-{}", l.dir, l.manifest.world, l.manifest.level));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_manifest() {
        let m = LevelManifest::parse(
            r#"world = 0
level = 1
base = [0, 0]
name = "Hello"
fog = [200, 100, 255]
music = 4
[files]
script = "./script.etc"
"#,
        )
        .unwrap();
        assert_eq!((m.world, m.level, m.base), (0, 1, Some([0, 0])));
        assert_eq!(m.file_entries(), vec![("script", "./script.etc")]);
        assert_eq!(level_path("hello", "./script.etc"), "levels/hello/script.etc");
        assert_eq!(level_path("hello", "MAPS/ENTITY-0-0.DAT"), "MAPS/ENTITY-0-0.DAT");
        assert!(LevelManifest::parse("world = 12\nlevel = 0\n").is_err());
        assert!(LevelManifest::parse("world = 0\nlevel = 0\nbogus = 1\n").is_err());
        assert!(LevelManifest::parse("world = 0\nlevel = 0\nname = \"a\"\ntext_id = \"STR_X\"\n").is_err());
    }
}
