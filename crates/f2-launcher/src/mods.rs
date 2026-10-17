//! Mods found in the mods folder (`docs/modding.md`): one sub-folder per mod with a `mod.toml`.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModInfo {
    /// The id `[mods] enabled` lists (`mod.toml` `id`, else the folder name).
    pub id: String,
    pub name: String,
    pub version: String,
    pub dir: PathBuf,
}

/// The mods in `dir`, sorted by id; folders without a readable `mod.toml` are skipped.
pub fn list(dir: &Path) -> Vec<ModInfo> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<ModInfo> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let path = e.path();
            let text = std::fs::read_to_string(path.join("mod.toml")).ok()?;
            let v: toml::Table = text.parse().ok()?;
            let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
            let folder = e.file_name().to_string_lossy().into_owned();
            let id = get("id").unwrap_or_else(|| folder.clone());
            Some(ModInfo { name: get("name").unwrap_or_else(|| id.clone()), version: get("version").unwrap_or_default(), id, dir: path })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_mods_with_a_manifest() {
        let t = std::env::temp_dir().join(format!("f2-launcher-mods-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(t.join("b-mod")).unwrap();
        std::fs::create_dir_all(t.join("a-mod")).unwrap();
        std::fs::create_dir_all(t.join("not-a-mod")).unwrap();
        std::fs::write(t.join("b-mod/mod.toml"), "id = \"b\"\nname = \"Bee\"\nversion = \"1.0.0\"\n").unwrap();
        std::fs::write(t.join("a-mod/mod.toml"), "name = \"Ay\"\n").unwrap();
        std::fs::write(t.join("stray.txt"), "x").unwrap();
        let l = list(&t);
        assert_eq!(l.iter().map(|m| (m.id.as_str(), m.name.as_str(), m.version.as_str())).collect::<Vec<_>>(), [("a-mod", "Ay", ""), ("b", "Bee", "1.0.0")]);
        assert!(list(&t.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&t);
    }
}
