//! Mod validation (`f2-modkit pack`) and scaffolding (`f2-modkit new`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use f2_mods::{ModManifest, Vfs, ENGINE_API_VERSION};
use sha1::{Digest, Sha1};

use crate::Error;

/// Result of [`validate`].
#[derive(Debug, Default)]
pub struct PackReport {
    pub manifest: Option<ModManifest>,
    /// Files looked at, by category.
    pub counts: BTreeMap<&'static str, usize>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl PackReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
    fn count(&mut self, k: &'static str) {
        *self.counts.entry(k).or_default() += 1;
    }
}

/// SHA-1 (hex) of every retail file listed in `<rendition>/manifest.json`
/// (`files` map: relative path → sha1), keyed by hash.
pub fn retail_hashes(rendition: &Path) -> Result<BTreeMap<String, String>, Error> {
    let p = rendition.join("manifest.json");
    let text = fs::read_to_string(&p).map_err(|e| Error::new(format!("{}: {e}", p.display())))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| Error::new(format!("{}: {e}", p.display())))?;
    let mut out = BTreeMap::new();
    if let Some(files) = v["files"].as_object() {
        for (path, h) in files {
            if let Some(h) = h.as_str() {
                out.insert(h.to_ascii_lowercase(), path.clone());
            }
        }
    }
    Ok(out)
}

fn sha1_hex(data: &[u8]) -> String {
    Sha1::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    v.sort();
    for p in v {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn lower_rel(root: &Path, p: &Path) -> Vec<String> {
    p.strip_prefix(root).unwrap_or(p).components().map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase()).collect()
}

/// Validates the mod in `mod_dir`: manifest, every override file parses, and
/// (with `rendition`, an imported data dir such as `game-data/usa`) no file is
/// byte-identical to a retail file and no override entry is an unchanged copy
/// of the retail entry it replaces (warnings).
pub fn validate(mod_dir: &Path, rendition: Option<&Path>) -> PackReport {
    let mut r = PackReport::default();
    let mt = mod_dir.join("mod.toml");
    match ModManifest::load(&mt) {
        Ok(m) => {
            if let Err(e) = m.check_compatible() {
                r.errors.push(e.to_string());
            }
            let dir_name = mod_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if dir_name != m.id {
                r.warnings.push(format!("mod directory {dir_name:?} differs from id {:?} (enable it by id)", m.id));
            }
            r.manifest = Some(m);
        }
        Err(e) => r.errors.push(e.to_string()),
    }
    let hashes = match rendition.map(retail_hashes).transpose() {
        Ok(h) => h,
        Err(e) => {
            r.warnings.push(format!("retail hash check skipped: {e}"));
            None
        }
    };
    let mut files = Vec::new();
    walk(mod_dir, &mut files);
    // (kind, bank) pairs to resolve.
    let mut banks: BTreeSet<(&'static str, String)> = BTreeSet::new();
    let mut text = false;
    for p in &files {
        let rel = lower_rel(mod_dir, p);
        let shown = rel.join("/");
        let data = match fs::read(p) {
            Ok(d) => d,
            Err(e) => {
                r.errors.push(format!("{shown}: {e}"));
                continue;
            }
        };
        if let Some(h) = &hashes {
            if let Some(orig) = h.get(&sha1_hex(&data)) {
                r.warnings.push(format!("{shown}: byte-identical to retail asset {orig}; mods must not redistribute game files"));
            }
        }
        let parts: Vec<&str> = rel.iter().map(|s| s.as_str()).collect();
        let ext = parts.last().and_then(|f| f.rsplit_once('.')).map(|(_, e)| e).unwrap_or("");
        match parts.as_slice() {
            ["mod.toml"] => {}
            ["game", "textures", bank, _] if ext == "png" => {
                r.count("textures");
                banks.insert(("textures", bank.to_string()));
            }
            ["game", "sfx", bank, _] if matches!(ext, "wav" | "flac" | "ogg") => {
                r.count("sounds");
                banks.insert(("sfx", bank.to_string()));
            }
            ["game", "objects", bank, _] if matches!(ext, "gltf" | "glb") => {
                r.count("models");
                banks.insert(("objects", bank.to_string()));
            }
            ["game", "objects", _, _] if matches!(ext, "bin" | "png") => r.count("model buffers/images"),
            ["game", "text", _] if matches!(ext, "tsv" | "toml") => {
                r.count("text");
                text = true;
            }
            ["music", f] => {
                r.count("music");
                let ok_name = f
                    .strip_prefix("track")
                    .and_then(|s| s.split_once('.'))
                    .is_some_and(|(n, e)| n.len() == 2 && n.bytes().all(|b| b.is_ascii_digit()) && matches!(e, "wav" | "flac"));
                if !ok_name {
                    r.errors.push(format!("{shown}: music must be named trackNN.wav or trackNN.flac"));
                } else if let Err(e) = f2_audio::wav::WavStream::open_any(p) {
                    r.errors.push(format!("{shown}: {e}"));
                }
            }
            ["game", ..] => {
                r.count("container replacements");
                r.warnings.push(format!("{shown}: replaces a whole retail file (prefer per-entry overrides)"));
            }
            ["video", ..] => r.count("videos"),
            _ => r.warnings.push(format!("{shown}: not used by the engine")),
        }
    }

    // Parse everything through the same resolver the engine uses.
    let mut vfs = match rendition {
        Some(d) => Vfs::with_rendition(d),
        None => Vfs::new(),
    };
    vfs.add_layer("mod", mod_dir.join("game"));
    for (kind, bank) in &banks {
        let res = match *kind {
            "textures" => crate::tex_bank(&vfs, bank).map(|l| (l.warnings, l.replaced.len() + l.added.len())),
            "sfx" => crate::sound_bank(&vfs, bank).map(|l| (l.warnings, l.replaced.len() + l.added.len())),
            _ => crate::object_bank(&vfs, bank).map(|l| (l.warnings, l.replaced.len() + l.added.len())),
        };
        match res {
            Ok((w, _)) => r.errors.extend(w),
            Err(e) => r.errors.push(format!("base {kind}/{bank}: {e}")),
        }
        if rendition.is_some() {
            unchanged_entries(&vfs, kind, bank, &mut r);
        }
    }
    if text {
        if rendition.is_some() {
            match crate::string_table(&vfs) {
                Ok(l) => {
                    r.errors.extend(l.warnings);
                    for (id, c) in crate::text::non_latin1(&l.value) {
                        r.warnings.push(format!("text {id}: {c:?} is outside Latin-1 and can't be drawn by the game font"));
                    }
                }
                Err(e) => r.errors.push(e.to_string()),
            }
        } else {
            let empty = f2_formats::lang::StringTable { languages: vec!["English".into()], rows: vec![] };
            for p in crate::layer_files(&vfs, "text/language.tsv") {
                if let Err(e) = crate::text::merge_tsv(&mut empty.clone(), &String::from_utf8_lossy(&fs::read(&p).unwrap_or_default())) {
                    r.errors.push(e.to_string());
                }
            }
        }
    }
    r
}

/// Warns about overrides that decode to exactly the retail entry they replace.
fn unchanged_entries(full: &Vfs, kind: &str, bank: &str, r: &mut PackReport) {
    let base = {
        let mut v = Vfs::new();
        for l in full.layers().iter().filter(|l| l.name != "mod") {
            v.mount(l.name.clone(), &l.prefix.join("/"), l.root.clone());
        }
        v
    };
    match kind {
        "textures" => {
            let (Ok(a), Ok(b)) = (crate::tex_bank(&base, bank), crate::tex_bank(full, bank)) else { return };
            for n in &b.replaced {
                let crc = crate::override_tex_crc(&a.value, n);
                let (Some(o), Some(t)) = (a.value.find(crc), b.value.find(crc)) else { continue };
                if o.pixels == t.pixels && o.width == t.width {
                    r.warnings.push(format!("textures/{bank}/{n}: identical to the retail texture (drop it)"));
                }
            }
        }
        "sfx" => {
            let (Ok(a), Ok(b)) = (crate::sound_bank(&base, bank), crate::sound_bank(full, bank)) else { return };
            for n in &b.replaced {
                let uid = crate::sound::sample_uid(n);
                let (Some(o), Some(s)) = (a.value.find(uid), b.value.find(uid)) else { continue };
                if o.pcm == s.pcm {
                    r.warnings.push(format!("sfx/{bank}/{n}: identical to the retail sample (drop it)"));
                }
            }
        }
        _ => {}
    }
}

/// `mod.toml` template for [`scaffold`].
pub fn manifest_template(id: &str) -> String {
    let v = semver_major_minor(ENGINE_API_VERSION);
    format!(
        "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nauthors = []\n# Semver requirement on the engine/mod API version.\nengine_version = \"{v}\"\n# Set when the mod changes gameplay (excluded from vanilla-only modes).\ngameplay_affecting = false\ndescription = \"\"\n"
    )
}

fn semver_major_minor(v: &str) -> String {
    let mut it = v.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    let (maj, min) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    if maj == 0 {
        format!(">={maj}.{min}, <{maj}.{}", min + 1)
    } else {
        format!(">={maj}.{min}, <{}", maj + 1)
    }
}

/// Creates `<mods_dir>/<id>/` with `mod.toml` and the override directories.
pub fn scaffold(mods_dir: &Path, id: &str) -> Result<PathBuf, Error> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(Error::new(format!("invalid mod id {id:?} (use [A-Za-z0-9_-])")));
    }
    let dir = mods_dir.join(id);
    if dir.exists() {
        return Err(Error::new(format!("{} already exists", dir.display())));
    }
    let io = |e: std::io::Error| Error::new(format!("{}: {e}", dir.display()));
    for sub in ["game/textures", "game/sfx", "game/objects", "game/text", "music"] {
        fs::create_dir_all(dir.join(sub)).map_err(io)?;
    }
    fs::write(dir.join("mod.toml"), manifest_template(id)).map_err(io)?;
    ModManifest::parse(&manifest_template(id)).map_err(|e| Error::new(e.to_string()))?;
    Ok(dir)
}
