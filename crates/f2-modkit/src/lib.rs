//! Mod asset pipeline for the Frogger 2 port.
//!
//! Given an [`f2_mods::Vfs`] with mod layers on top of the imported retail
//! data, this crate produces the *effective* container contents the engine
//! should use: the retail bank parsed from game data, with single entries
//! replaced or added from loose, editable files (see `docs/modding.md`):
//!
//! | Override file | Becomes |
//! |---|---|
//! | `textures/<bank>/<name>.png` | [`Texture`] `crc("<name>.bmp")` in `textures/<bank>.fla` |
//! | `sfx/<bank>/<name>.wav\|.flac` | [`Sample`] uid `crc(lowercase name)` (or 8 hex digits) in `sfx/<bank>.wus` |
//! | `objects/<bank>/<model>.gltf\|.glb` | [`Model`] in `objects/<bank>.dll` |
//! | `text/language.tsv`, `text/<lang>.toml` | rows/columns merged into `maps/language.fla` |
//! | `music/trackNN.wav\|.flac` | CD track `NN` (plain VFS file, see [`music_track`]) |
//!
//! Bad override files never abort loading: they are skipped and reported in
//! [`Loaded::warnings`] (the `f2-modkit pack` CLI turns them into errors).
//!
//! [`Texture`]: f2_formats::texture::Texture
//! [`Sample`]: f2_formats::wus::Sample
//! [`Model`]: f2_formats::objbank::Model

#![forbid(unsafe_code)]

pub mod archive;
pub mod gltf;
pub mod levels;
pub mod material;
pub mod pack;
pub mod sound;
pub mod text;
pub mod texture;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use f2_formats::crc::crc;
use f2_formats::fla::maybe_decompress;
use f2_formats::lang::StringTable;
use f2_formats::objbank::{Model, ObjectBank};
use f2_formats::texture::{TexBank, TexType};
use f2_formats::wus::SoundBank;
use f2_mods::{EntryKind, Vfs};

/// Error with a human-readable message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl Error {
    pub fn new(m: impl Into<String>) -> Error {
        Error(m.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// An effective container plus what the overrides did to it.
#[derive(Clone, Debug)]
pub struct Loaded<T> {
    pub value: T,
    /// Base entries replaced by override files.
    pub replaced: Vec<String>,
    /// New entries added by override files.
    pub added: Vec<String>,
    /// Override files that were skipped (unreadable/invalid), with the reason.
    pub warnings: Vec<String>,
}

impl<T> Loaded<T> {
    fn new(value: T) -> Loaded<T> {
        Loaded { value, replaced: Vec::new(), added: Vec::new(), warnings: Vec::new() }
    }
}

/// One loose override file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverrideFile {
    /// Entry name: file stem, lowercase.
    pub name: String,
    /// Extension, lowercase.
    pub ext: String,
    /// Host path (from the topmost layer providing this entry).
    pub path: PathBuf,
}

/// Index of the topmost layer whose root contains `path` (higher = wins).
fn layer_rank(vfs: &Vfs, path: &Path) -> usize {
    vfs.layers().iter().rposition(|l| path.starts_with(&l.root)).unwrap_or(0)
}

/// Override files `<kind>/<bank>/<name>.<ext>` with `ext` in `exts`
/// (lowercase, in preference order for ties), one per name: the file from the
/// topmost layer wins. Sorted by name.
///
/// Unlike [`Vfs::entry_overrides`] this keys by (name, allowed extension), so
/// a model's `.bin` buffer or a texture next to it doesn't shadow the model.
pub fn list_overrides(vfs: &Vfs, kind: &str, bank: &str, exts: &[&str]) -> Vec<OverrideFile> {
    let dir = format!("{kind}/{bank}");
    let Ok(list) = vfs.list(&dir) else { return Vec::new() };
    let mut best: std::collections::BTreeMap<String, (usize, usize, OverrideFile)> = Default::default();
    for e in list.into_iter().filter(|e| e.kind == EntryKind::File) {
        let Some((stem, ext)) = e.name.rsplit_once('.') else { continue };
        let ext = ext.to_ascii_lowercase();
        let Some(pref) = exts.iter().position(|x| *x == ext) else { continue };
        if stem.is_empty() {
            continue;
        }
        let Some(path) = vfs.resolve_file(&format!("{dir}/{}", e.name)) else { continue };
        let rank = layer_rank(vfs, &path);
        let name = stem.to_ascii_lowercase();
        let f = OverrideFile { name: name.clone(), ext, path };
        match best.get(&name) {
            Some((r, p, _)) if (*r, usize::MAX - *p) >= (rank, usize::MAX - pref) => {}
            _ => {
                best.insert(name, (rank, pref, f));
            }
        }
    }
    best.into_values().map(|v| v.2).collect()
}

/// Bank names (lowercase) that have an override directory `<kind>/<bank>/`.
pub fn override_banks(vfs: &Vfs, kind: &str) -> Vec<String> {
    vfs.list(kind).map(|l| l.into_iter().filter(|e| e.kind == EntryKind::Dir).map(|e| e.name.to_ascii_lowercase()).collect()).unwrap_or_default()
}

fn read_container(vfs: &Vfs, path: &str) -> Result<Option<Vec<u8>>, Error> {
    if vfs.resolve_file(path).is_none() {
        return Ok(None);
    }
    let d = vfs.read(path).map_err(|e| Error::new(format!("{path}: {e}")))?;
    maybe_decompress(d).map(Some).map_err(|e| Error::new(format!("{path}: {e}")))
}

/// Same stem rule as `f2-export` (`texture_stem` + `sanitize`).
pub fn export_stem(name: &str) -> String {
    let n = name.trim();
    let stem = match n.rfind('.') {
        Some(i) if i > 0 => &n[..i],
        _ => n,
    };
    let t: String = stem.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    let t = t.trim_matches('.').to_string();
    if t.is_empty() {
        "_".into()
    } else {
        t
    }
}

fn hex8(s: &str) -> Option<u32> {
    (s.len() == 8 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| u32::from_str_radix(s, 16).unwrap())
}

/// Full-screen backdrop files in `textures/` (single image, not a bank).
pub fn is_backdrop(stem: &str) -> bool {
    let s = stem.to_ascii_lowercase();
    s == "title" || s == "loading" || s.strip_prefix("artwork").is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()))
}

/// CRC addressed by texture override `name` in `bank` (see [`tex_bank`]).
pub fn override_tex_crc(bank: &TexBank, name: &str) -> u32 {
    let by_stem = bank.textures.iter().find(|t| export_stem(&t.name) == name).map(|t| t.crc);
    by_stem.or_else(|| hex8(name)).unwrap_or_else(|| texture::texture_crc(name))
}

/// The material maps a texture can have beside it (`<name>_normal.png`, ...): the map kind
/// and `<name>`.
pub(crate) fn material_map(name: &str) -> Option<(&'static str, &str)> {
    ["_normal", "_height", "_orm"].into_iter().find_map(|k| name.strip_suffix(k).filter(|b| !b.is_empty()).map(|b| (k, b)))
}

/// The materials of bank `textures/<bank>`'s textures (Enhanced / Path-Traced graphics): for a
/// texture `<name>` (its exported stem, as for overrides), any of `<name>_normal.png`
/// (tangent space, OpenGL +Y), `<name>_height.png` (R, white = high: parallax occlusion),
/// `<name>_orm.png` (R occlusion, G roughness, B metal) and `<name>.pbr.toml` (`roughness`,
/// `metallic`, `normal_scale`, `occlusion_strength`, `parallax_scale`) in `textures/<bank>/`.
/// The material applies wherever the texture is drawn ([`MODEL_ANY`](f2_formats::material::MODEL_ANY));
/// the texture itself is unchanged (a `<name>.png` beside them still replaces it).
pub fn tex_materials(vfs: &Vfs, bank: &str) -> Result<Loaded<f2_formats::material::ModelMaterials>, Error> {
    use f2_formats::material::{ModelMaterials, PbrMaterial, MODEL_ANY};
    let base = match read_container(vfs, &format!("textures/{bank}.fla"))? {
        Some(d) => TexBank::parse(&d).unwrap_or(TexBank { textures: vec![], anims: vec![] }),
        None => TexBank { textures: vec![], anims: vec![] },
    };
    let mut out = Loaded::new(ModelMaterials { model_id: MODEL_ANY, ..Default::default() });
    let files = list_overrides(vfs, "textures", bank, &["png", "toml"]);
    let mut names: Vec<String> = files.iter().filter_map(|f| material_map(&f.name).map(|(_, b)| b.to_string())).collect();
    names.extend(files.iter().filter(|f| f.ext == "toml").filter_map(|f| f.name.strip_suffix(".pbr").map(str::to_string)));
    names.sort();
    names.dedup();
    for name in names {
        let m = &mut out.value;
        let mut image = |kind: &str, warnings: &mut Vec<String>| -> Option<u32> {
            let f = files.iter().find(|f| f.ext == "png" && f.name == format!("{name}{kind}"))?;
            match fs::read(&f.path).map_err(|e| Error::new(e.to_string())).and_then(|d| material::png_image(format!("{name}{kind}"), &d)) {
                Ok(img) => {
                    m.images.push(img);
                    Some(m.images.len() as u32 - 1)
                }
                Err(e) => {
                    warnings.push(format!("{}: {e}", f.path.display()));
                    None
                }
            }
        };
        let mut warnings = Vec::new();
        let (normal, height, orm) = (image("_normal", &mut warnings), image("_height", &mut warnings), image("_orm", &mut warnings));
        out.warnings.extend(warnings);
        let mut p = PbrMaterial {
            crc: override_tex_crc(&base, &name),
            base_color: [1.0; 4],
            metallic: if orm.is_some() { 1.0 } else { 0.0 },
            roughness: if orm.is_some() { 1.0 } else { 0.7 },
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            parallax_scale: 0.03,
            normal,
            metal_rough: orm,
            occlusion: orm,
            emissive_map: None,
            height,
            base_map: None,
            tiling: Default::default(),
        };
        if let Some(f) = files.iter().find(|f| f.ext == "toml" && f.name == format!("{name}.pbr")) {
            match fs::read_to_string(&f.path).map_err(|e| e.to_string()).and_then(|t| t.parse::<toml::Table>().map_err(|e| e.to_string())) {
                Ok(t) => {
                    let num = |k: &str| t.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).map(|v| v as f32);
                    if let Some(v) = num("roughness") {
                        p.roughness = v;
                    }
                    if let Some(v) = num("metallic") {
                        p.metallic = v;
                    }
                    if let Some(v) = num("normal_scale") {
                        p.normal_scale = v;
                    }
                    if let Some(v) = num("occlusion_strength") {
                        p.occlusion_strength = v;
                    }
                    if let Some(v) = num("parallax_scale") {
                        p.parallax_scale = v;
                    }
                    if let Some(tl) = t.get("tiling").and_then(|v| v.as_table()) {
                        p.tiling = tiling(tl);
                    }
                }
                Err(e) => out.warnings.push(format!("{}: {e}", f.path.display())),
            }
        }
        out.value.materials.push(p);
    }
    Ok(out)
}

/// A `.pbr.toml`'s `[tiling]` table ([`Tiling`](f2_formats::material::Tiling): `variants`
/// "none" / "mirror" / "turn", `turn_color`, `wear`, `tint`, `uneven`, `edges`, `grime`).
fn tiling(t: &toml::Table) -> f2_formats::material::Tiling {
    let num = |k: &str| t.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).map_or(0.0, |v| (v as f32).clamp(0.0, 1.0));
    f2_formats::material::Tiling {
        variants: match t.get("variants").and_then(|v| v.as_str()) {
            Some("mirror") => 1,
            Some("turn") => 2,
            _ => 0,
        },
        turn_color: t.get("turn_color").and_then(|v| v.as_bool()).unwrap_or(false),
        wear: num("wear"),
        tint: num("tint"),
        uneven: num("uneven"),
        edges: num("edges"),
        grime: num("grime"),
    }
}

/// Effective texture bank `textures/<bank>.fla` with PNG overrides from
/// `textures/<bank>/`. A missing base bank starts empty (new bank).
///
/// Override name → CRC: a retail texture whose (exported) stem equals the
/// name keeps its CRC (retail names are truncated to 11 chars); 8 hex digits
/// are a raw CRC; otherwise `crc("<name>.bmp")`.
pub fn tex_bank(vfs: &Vfs, bank: &str) -> Result<Loaded<TexBank>, Error> {
    if is_backdrop(bank) {
        return Err(Error::new(format!("textures/{bank}.fla is a full-screen backdrop, not a texture bank (replace the whole file)")));
    }
    let path = format!("textures/{bank}.fla");
    let base = match read_container(vfs, &path)? {
        Some(d) => TexBank::parse(&d).map_err(|e| Error::new(format!("{path}: {e}")))?,
        None => TexBank { textures: vec![], anims: vec![] },
    };
    let mut out = Loaded::new(base);
    for f in list_overrides(vfs, "textures", bank, &["png"]) {
        // A texture's material maps (`tex_materials`) are not textures of their own.
        if material_map(&f.name).is_some() {
            continue;
        }
        let b = &mut out.value;
        let crc = override_tex_crc(b, &f.name);
        // Replacing a retail entry keeps its special type (`ai_`/`ac_`/`prc`);
        // otherwise the name prefix / size decides.
        let keep = b.textures.iter().find(|x| x.crc == crc).map(|x| x.tex_type()).filter(|t| !matches!(t, TexType::Normal | TexType::NoTexture));
        let t = match fs::read(&f.path)
            .map_err(|e| Error::new(e.to_string()))
            .and_then(|d| texture::decode_png(&d))
            .and_then(|img| texture::texture_from_rgba_typed(&f.name, &img, Some(crc), keep))
        {
            Ok(t) => t,
            Err(e) => {
                out.warnings.push(format!("{}: {e}", f.path.display()));
                continue;
            }
        };
        match b.textures.iter_mut().find(|x| x.crc == crc) {
            Some(x) => {
                let name = std::mem::take(&mut x.name);
                *x = t;
                x.name = name;
                out.replaced.push(f.name);
            }
            None => {
                b.textures.push(t);
                out.added.push(f.name);
            }
        }
    }
    Ok(out)
}

/// Effective sound bank `sfx/<bank>.wus` with WAV/FLAC overrides from `sfx/<bank>/`.
pub fn sound_bank(vfs: &Vfs, bank: &str) -> Result<Loaded<SoundBank>, Error> {
    let path = format!("sfx/{bank}.wus");
    let base = match read_container(vfs, &path)? {
        Some(d) => SoundBank::parse(&d).map_err(|e| Error::new(format!("{path}: {e}")))?,
        None => SoundBank { samples: vec![] },
    };
    let mut out = Loaded::new(base);
    for f in list_overrides(vfs, "sfx", bank, &["wav", "flac", "ogg"]) {
        let uid = sound::sample_uid(&f.name);
        let pcm = match fs::read(&f.path).map_err(|e| Error::new(e.to_string())).and_then(|d| sound::decode_audio(&f.ext, &d)) {
            Ok(p) => p,
            Err(e) => {
                out.warnings.push(format!("{}: {e}", f.path.display()));
                continue;
            }
        };
        let b = &mut out.value;
        match b.samples.iter_mut().find(|s| s.uid == uid) {
            Some(s) => {
                *s = sound::sample_from_pcm(uid, &pcm, Some(s));
                out.replaced.push(f.name);
            }
            None => {
                b.samples.push(sound::sample_from_pcm(uid, &pcm, None));
                out.added.push(f.name);
            }
        }
    }
    Ok(out)
}

/// Export names of a bank's models (`f2-export` naming: sanitized lowercase
/// root name, `_<id>` suffix on duplicates).
pub fn model_names(bank: &ObjectBank) -> Vec<String> {
    let mut used = std::collections::BTreeSet::new();
    bank.models
        .iter()
        .map(|m| {
            let mut name = export_stem(&format!("{}.x", m.name()));
            if used.contains(&name) {
                name = format!("{name}_{:08x}", m.id);
            }
            used.insert(name.clone());
            name
        })
        .collect()
}

/// Imports one glTF/GLB file as a model with id `id`.
pub fn load_model_file(path: &Path, id: u32) -> Result<Model, Error> {
    let doc = gltf::GltfDoc::load(path)?;
    gltf::import_model(&doc, Some(id))
}

/// Imports one glTF/GLB file as model `id` replacing `base` (mesh-only when the document
/// allows it), with its images and materials.
pub fn load_model_file_over(path: &Path, id: u32, base: Option<&Model>) -> Result<gltf::Imported, Error> {
    let doc = gltf::GltfDoc::load(path)?;
    gltf::import(&doc, Some(id), base)
}

/// Effective object bank `objects/<bank>.dll` with glTF overrides from `objects/<bank>/`.
///
/// Override name → model: the retail model with that export name (see
/// [`model_names`]) is replaced and keeps its id; otherwise a new model with
/// id `crc("<name>.obe")` (the game's `FindObject` key) is added.
pub fn object_bank(vfs: &Vfs, bank: &str) -> Result<Loaded<ObjectBank>, Error> {
    let path = format!("objects/{bank}.dll");
    let base = match read_container(vfs, &path)? {
        Some(d) => ObjectBank::parse(&d).map_err(|e| Error::new(format!("{path}: {e}")))?,
        None => ObjectBank { models: vec![], anim_past_end: vec![], materials: vec![] },
    };
    let names = model_names(&base);
    let mut out = Loaded::new(base);
    for f in list_overrides(vfs, "objects", bank, &["glb", "gltf"]) {
        let slot = names.iter().position(|n| *n == f.name);
        let id = slot.map(|i| out.value.models[i].id).unwrap_or_else(|| crc(&format!("{}.obe", f.name)));
        let imp = match load_model_file_over(&f.path, id, slot.map(|i| &out.value.models[i])) {
            Ok(m) => m,
            Err(e) => {
                out.warnings.push(format!("{}: {e}", f.path.display()));
                continue;
            }
        };
        out.warnings.extend(imp.warnings.iter().map(|w| format!("{}: {w}", f.path.display())));
        let m = imp.model;
        out.value.materials.retain(|x| x.model_id != id);
        if !imp.materials.is_empty() {
            out.value.materials.push(std::sync::Arc::new(imp.materials));
        }
        match slot.or_else(|| out.value.models.iter().position(|x| x.id == id)) {
            Some(i) => {
                out.value.models[i] = m;
                out.replaced.push(f.name);
            }
            None => {
                out.value.models.push(m);
                out.added.push(f.name);
            }
        }
    }
    Ok(out)
}

/// Host paths of virtual file `path` in every layer that has it, bottom to
/// top (for files that merge across mods instead of the topmost one winning).
pub fn layer_files(vfs: &Vfs, path: &str) -> Vec<PathBuf> {
    let Some(comps) = f2_mods::normalize(path) else { return Vec::new() };
    let mut out = Vec::new();
    for l in vfs.layers() {
        if comps.len() < l.prefix.len() || !l.prefix.iter().zip(&comps).all(|(a, b)| a.eq_ignore_ascii_case(b)) {
            continue;
        }
        let mut cur = l.root.clone();
        let mut ok = true;
        for c in &comps[l.prefix.len()..] {
            let exact = cur.join(c);
            if exact.exists() {
                cur = exact;
                continue;
            }
            match fs::read_dir(&cur).ok().and_then(|rd| rd.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(c))) {
                Some(e) => cur = e.path(),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && cur.is_file() {
            out.push(cur);
        }
    }
    out
}

/// Effective string table: `maps/language.fla` (topmost container) with every
/// layer's `text/language.tsv` then `text/*.toml` merged on top, lowest layer first.
pub fn string_table(vfs: &Vfs) -> Result<Loaded<StringTable>, Error> {
    let path = "maps/language.fla";
    let d = read_container(vfs, path)?.ok_or_else(|| Error::new(format!("{path}: not found")))?;
    let base = StringTable::parse(&d).map_err(|e| Error::new(format!("{path}: {e}")))?;
    let mut out = Loaded::new(base);
    let mut tomls: Vec<(usize, String, PathBuf)> = Vec::new();
    if let Ok(list) = vfs.list("text") {
        for e in list.into_iter().filter(|e| e.kind == EntryKind::File) {
            if let Some(stem) = e.name.strip_suffix(".toml").or_else(|| e.name.strip_suffix(".TOML")) {
                for p in layer_files(vfs, &format!("text/{}", e.name)) {
                    tomls.push((layer_rank(vfs, &p), stem.to_string(), p));
                }
            }
        }
    }
    tomls.sort();
    let tsvs: Vec<PathBuf> = layer_files(vfs, "text/language.tsv");
    // Merge per layer: TSV first, then that layer's TOML files.
    let mut ranks: Vec<usize> = tsvs.iter().map(|p| layer_rank(vfs, p)).chain(tomls.iter().map(|t| t.0)).collect();
    ranks.sort();
    ranks.dedup();
    for r in ranks {
        for p in tsvs.iter().filter(|p| layer_rank(vfs, p) == r) {
            let res = fs::read(p).map_err(|e| Error::new(e.to_string())).and_then(|d| text::merge_tsv(&mut out.value, &String::from_utf8_lossy(&d)));
            match res {
                Ok(()) => out.replaced.push(p.display().to_string()),
                Err(e) => out.warnings.push(format!("{}: {e}", p.display())),
            }
        }
        for (_, stem, p) in tomls.iter().filter(|t| t.0 == r) {
            let res = fs::read_to_string(p).map_err(|e| Error::new(e.to_string())).and_then(|s| text::merge_toml(&mut out.value, stem, &s));
            match res {
                Ok(()) => out.replaced.push(p.display().to_string()),
                Err(e) => out.warnings.push(format!("{}: {e}", p.display())),
            }
        }
    }
    Ok(out)
}

/// Host path of CD track `n` (`music/trackNN.wav`, else `.flac`) from the
/// topmost layer — what the host should hand to `f2_audio::wav::WavStream::open_any`.
pub fn music_track(vfs: &Vfs, n: u32) -> Option<PathBuf> {
    ["wav", "flac"].iter().filter_map(|e| vfs.resolve_file(&format!("music/track{n:02}.{e}"))).max_by_key(|p| layer_rank(vfs, p))
}

#[cfg(test)]
mod tests;
