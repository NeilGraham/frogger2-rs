//! Exports imported Frogger 2 game data to editable formats (PNG, WAV, glTF,
//! JSON, TSV). All parsing is done by `f2-formats`; files are read through the
//! `f2-mods` VFS so paths are case-insensitive like the original game.

#![forbid(unsafe_code)]

pub mod gltf;

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use f2_formats::entity::{Entities, Item, Place};
use f2_formats::fev::{Command, Script};
use f2_formats::fla::maybe_decompress;
use f2_formats::lang::StringTable;
use f2_formats::level::{parse_scenics, Collision, SVec};
use f2_formats::objbank::ObjectBank;
use f2_formats::texture::{parse_backdrop, TexBank, Texture};
use f2_formats::wus::SoundBank;
use f2_mods::{EntryKind, Vfs};
use serde_json::{json, Value};

/// What to export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Textures,
    Sounds,
    Models,
    Levels,
    Text,
    All,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s.to_ascii_lowercase().as_str() {
            "textures" => Kind::Textures,
            "sounds" | "sfx" => Kind::Sounds,
            "models" => Kind::Models,
            "levels" => Kind::Levels,
            "text" => Kind::Text,
            "all" => Kind::All,
            _ => return None,
        })
    }
    fn has(self, k: Kind) -> bool {
        self == Kind::All || self == k
    }
}

/// Counters and problems collected during an export.
#[derive(Debug, Default)]
pub struct Report {
    pub counts: BTreeMap<&'static str, usize>,
    /// Hard failures (file could not be parsed/written).
    pub errors: Vec<String>,
    /// Expected oddities (e.g. stale-version entity files the game also rejects).
    pub warnings: Vec<String>,
}

impl Report {
    fn add(&mut self, k: &'static str, n: usize) {
        *self.counts.entry(k).or_default() += n;
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (k, v) in &self.counts {
            writeln!(f, "  {k:<22} {v}")?;
        }
        for w in &self.warnings {
            writeln!(f, "  warning: {w}")?;
        }
        for e in &self.errors {
            writeln!(f, "  ERROR: {e}")?;
        }
        Ok(())
    }
}

/// Texture CRC → exported PNG location.
#[derive(Clone, Debug)]
pub struct TexEntry {
    pub bank: String,
    pub file: String,
    pub has_alpha: bool,
}

pub struct Exporter {
    pub vfs: Vfs,
    pub out: PathBuf,
    pub report: Report,
    /// Built from every texture bank (first bank in sorted order wins on CRC clashes).
    pub tex_index: BTreeMap<u32, TexEntry>,
}

/// Names of full-screen backdrop files in `textures/`.
pub fn is_backdrop(stem: &str) -> bool {
    stem == "title" || stem == "loading" || (stem.starts_with("artwork") && stem[7..].chars().all(|c| c.is_ascii_digit()))
}

/// File-system-safe version of an entry name.
pub fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    let t = t.trim_matches('.').to_string();
    if t.is_empty() {
        "_".into()
    } else {
        t
    }
}

/// PNG stem for a texture: its stored name without the (possibly truncated) extension.
pub fn texture_stem(t: &Texture) -> String {
    let n = t.name.trim();
    let stem = match n.rfind('.') {
        Some(i) if i > 0 => &n[..i],
        _ => n,
    };
    if stem.is_empty() {
        format!("{:08x}", t.crc)
    } else {
        sanitize(&stem.to_ascii_lowercase())
    }
}

pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut wr = e.write_header().expect("png header");
        wr.write_image_data(rgba).expect("png data");
    }
    out
}

fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(path, data)
}

fn write_json(path: &Path, v: &Value) -> io::Result<()> {
    let mut s = serde_json::to_string_pretty(v).expect("json");
    s.push('\n');
    write(path, s.as_bytes())
}

fn sv(v: SVec) -> Value {
    json!([v.x, v.y, v.z])
}

fn place(p: Place) -> Value {
    match p {
        Place::Tile(t) => json!({"tile": t}),
        Place::Pos(p) => json!({"pos": p}),
    }
}

fn command_json(c: &Command) -> Value {
    match c {
        Command::On { trigger, flags, block } => {
            json!({"on": format!("{trigger:?}"), "flags": flags, "block": block.iter().map(command_json).collect::<Vec<_>>()})
        }
        Command::If { trigger, block } => json!({"if": format!("{trigger:?}"), "block": block.iter().map(command_json).collect::<Vec<_>>()}),
        c => json!(format!("{c:?}")),
    }
}

impl Exporter {
    /// `rendition` is an imported data dir such as `game-data/usa`.
    pub fn new(rendition: &Path, out: &Path) -> Exporter {
        Exporter { vfs: Vfs::with_rendition(rendition), out: out.to_path_buf(), report: Report::default(), tex_index: BTreeMap::new() }
    }

    /// Files (not dirs) in a VFS dir with the given extension, as (stem, vfs path), sorted.
    fn files(&self, dir: &str, ext: &str) -> Vec<(String, String)> {
        let Ok(list) = self.vfs.list(dir) else { return Vec::new() };
        list.into_iter()
            .filter(|e| e.kind == EntryKind::File)
            .filter_map(|e| {
                let lower = e.name.to_ascii_lowercase();
                let stem = lower.strip_suffix(&format!(".{ext}"))?.to_string();
                Some((stem, format!("{dir}/{}", e.name)))
            })
            .collect()
    }

    fn load(&mut self, path: &str) -> Option<Vec<u8>> {
        match self.vfs.read(path).map_err(|e| e.to_string()).and_then(|d| maybe_decompress(d).map_err(|e| e.to_string())) {
            Ok(d) => Some(d),
            Err(e) => {
                self.report.errors.push(format!("{path}: {e}"));
                None
            }
        }
    }

    fn put(&mut self, rel: &str, data: &[u8]) {
        if let Err(e) = write(&self.out.join(rel), data) {
            self.report.errors.push(format!("write {rel}: {e}"));
        }
    }

    fn put_json(&mut self, rel: &str, v: &Value) {
        if let Err(e) = write_json(&self.out.join(rel), v) {
            self.report.errors.push(format!("write {rel}: {e}"));
        }
    }

    pub fn run(&mut self, kind: Kind) {
        if kind.has(Kind::Textures) || kind.has(Kind::Models) {
            self.textures(kind.has(Kind::Textures));
        }
        if kind.has(Kind::Sounds) {
            self.sounds();
        }
        if kind.has(Kind::Models) {
            self.models();
        }
        if kind.has(Kind::Levels) {
            self.levels();
        }
        if kind.has(Kind::Text) {
            self.text();
        }
    }

    /// Loads every texture bank (building the CRC index) and, if `write`, exports PNGs.
    pub fn textures(&mut self, write: bool) {
        for (stem, path) in self.files("textures", "fla") {
            let Some(data) = self.load(&path) else { continue };
            if is_backdrop(&stem) {
                if !write {
                    continue;
                }
                match parse_backdrop(&data) {
                    Ok(t) => {
                        let png = encode_png(t.width as u32, t.height as u32, &t.to_rgba8());
                        self.put(&format!("backdrops/{stem}.png"), &png);
                        self.report.add("backdrops", 1);
                    }
                    Err(e) => self.report.errors.push(format!("{path}: {e}")),
                }
                continue;
            }
            let bank = match TexBank::parse(&data) {
                Ok(b) => b,
                Err(e) => {
                    self.report.errors.push(format!("{path}: {e}"));
                    continue;
                }
            };
            self.report.add("texture banks", 1);
            let mut used: BTreeMap<String, u32> = BTreeMap::new();
            let mut index = Vec::new();
            for t in &bank.textures {
                let mut name = texture_stem(t);
                if used.contains_key(&name) {
                    name = format!("{name}_{:08x}", t.crc);
                }
                used.insert(name.clone(), t.crc);
                let file = format!("{name}.png");
                let rgba = t.to_rgba8();
                let has_alpha = rgba.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
                self.tex_index.entry(t.crc).or_insert(TexEntry { bank: stem.clone(), file: file.clone(), has_alpha });
                index.push(json!({"crc": format!("{:08x}", t.crc), "name": t.name, "file": file, "flags": t.flags, "width": t.width, "height": t.height}));
                if write {
                    let png = encode_png(t.width as u32, t.height as u32, &rgba);
                    self.put(&format!("textures/{stem}/{file}"), &png);
                    self.report.add("textures", 1);
                }
            }
            if write {
                let anims: Vec<Value> = bank
                    .anims
                    .iter()
                    .map(|a| {
                        json!({"baseCrc": format!("{:08x}", a.base_crc),
                               "frames": a.frames.iter().map(|f| json!({"crc": format!("{:08x}", f.crc), "time": f.time})).collect::<Vec<_>>()})
                    })
                    .collect();
                self.report.add("texture anims", anims.len());
                self.put_json(&format!("textures/{stem}/index.json"), &json!({"bank": stem, "textures": index, "anims": anims}));
            }
        }
    }

    pub fn sounds(&mut self) {
        for (stem, path) in self.files("sfx", "wus") {
            let Some(data) = self.load(&path) else { continue };
            let bank = match SoundBank::parse(&data) {
                Ok(b) => b,
                Err(e) => {
                    self.report.errors.push(format!("{path}: {e}"));
                    continue;
                }
            };
            self.report.add("sound banks", 1);
            let mut index = Vec::new();
            let mut seen = BTreeMap::new();
            for s in &bank.samples {
                let n = seen.entry(s.uid).or_insert(0u32);
                let file = if *n == 0 { format!("{:08x}.wav", s.uid) } else { format!("{:08x}_{n}.wav", s.uid) };
                *n += 1;
                self.put(&format!("sfx/{stem}/{file}"), &s.to_wav());
                self.report.add("samples", 1);
                index.push(
                    json!({"uid": format!("{:08x}", s.uid), "file": file, "looped": s.looped, "rate": s.sample_rate, "bits": s.bits, "channels": s.channels}),
                );
            }
            self.put_json(&format!("sfx/{stem}/index.json"), &json!({"bank": stem, "samples": index}));
        }
    }

    pub fn models(&mut self) {
        for (stem, path) in self.files("objects", "dll") {
            let Some(data) = self.load(&path) else { continue };
            let bank = match ObjectBank::parse(&data) {
                Ok(b) => b,
                Err(e) => {
                    self.report.errors.push(format!("{path}: {e}"));
                    continue;
                }
            };
            self.report.add("object banks", 1);
            let mut used = BTreeMap::new();
            for m in &bank.models {
                let mut name = sanitize(&m.name().to_ascii_lowercase());
                if used.contains_key(&name) {
                    name = format!("{name}_{:08x}", m.id);
                }
                used.insert(name.clone(), ());
                let idx = &self.tex_index;
                let tex = |crc: u32| idx.get(&crc).map(|t| gltf::TexRef { uri: format!("../../textures/{}/{}", t.bank, t.file), has_alpha: t.has_alpha });
                let g = gltf::model_to_gltf(m, &format!("{name}.bin"), &tex);
                let base = format!("models/{stem}/{name}");
                self.put_json(&format!("{base}.gltf"), &g.json);
                if !g.bin.is_empty() {
                    self.put(&format!("{base}.bin"), &g.bin);
                }
                self.report.add("models", 1);
                self.report.add("model nodes", g.stats.nodes);
                self.report.add("model triangles", g.stats.triangles);
                self.report.add("model animations", g.stats.animations);
                self.report.add("skinned models", g.stats.skinned as usize);
                self.report.add("unresolved tex refs", g.stats.missing_textures);
            }
        }
    }

    pub fn levels(&mut self) {
        for (stem, path) in self.files("collision", "col") {
            let Some(data) = self.load(&path) else { continue };
            match Collision::parse(&data) {
                Ok(c) => {
                    let tiles: Vec<Value> = c
                        .tiles
                        .iter()
                        .map(|t| json!({"adj": t.adj, "state": t.state, "centre": sv(t.centre), "normal": sv(t.normal), "dirVector": t.dir_vector.map(sv)}))
                        .collect();
                    let v = json!({
                        "playerStart": c.player_start, "babyStart": c.baby_start, "tiles": tiles,
                        "camPlanes": c.planes.iter().map(|p| json!({"normal": p.normal, "k": p.k, "status": p.status})).collect::<Vec<_>>(),
                        "camBoxes": c.boxes.iter().map(|b| json!({"numPlanes": b.num_planes, "firstPlane": b.first_plane})).collect::<Vec<_>>(),
                    });
                    self.put_json(&format!("levels/collision/{stem}.json"), &v);
                    self.report.add("collision files", 1);
                }
                Err(e) => self.report.errors.push(format!("{path}: {e}")),
            }
        }
        for (stem, path) in self.files("scenics", "sce") {
            let Some(data) = self.load(&path) else { continue };
            match parse_scenics(&data) {
                Ok(s) => {
                    let v: Vec<Value> = s.iter().map(|s| json!({"name": s.name, "pos": sv(s.pos), "rot": [s.rot.x, s.rot.y, s.rot.z, s.rot.w]})).collect();
                    self.put_json(&format!("levels/scenics/{stem}.json"), &json!(v));
                    self.report.add("scenic files", 1);
                }
                Err(e) => self.report.errors.push(format!("{path}: {e}")),
            }
        }
        for (stem, path) in self.files("maps", "dat") {
            if !stem.starts_with("entity-") {
                continue;
            }
            let Some(data) = self.load(&path) else { continue };
            match Entities::parse(&data) {
                Ok(e) => {
                    let paths: Vec<Value> = e
                        .paths
                        .iter()
                        .map(|p| {
                            p.nodes
                                .iter()
                                .map(|n| json!({"place": place(n.place), "offset": n.offset, "offset2": n.offset2, "speed": n.speed, "waitTime": n.wait_time, "sampleUid": n.sample_uid}))
                                .collect::<Value>()
                        })
                        .collect();
                    let items: Vec<Value> = e
                        .items
                        .iter()
                        .map(|i| match i {
                            Item::Actor(a) => json!({"type": format!("{:?}", a.kind), "model": a.model, "flags": a.flags, "uid": a.uid, "scale": a.scale,
                                "radius": a.radius, "animSpeed": a.anim_speed, "value1": a.value1, "facing": a.facing, "objFlags": a.obj_flags,
                                "depthShift": a.depth_shift, "dffClipping": a.dff_clipping, "effects": a.effects, "startNode": a.start_node, "pathIndex": a.path_index}),
                            Item::Garib(g) => json!({"type": "Garib", "garibType": g.garib_type, "place": place(g.place), "heightOffset": g.height_offset}),
                            Item::CameraCase(c) => json!({"type": "CameraCase", "flags": c.flags, "pos": c.pos, "fov": c.fov, "speed": c.speed,
                                "target": c.target, "nodes": c.nodes.iter().map(|p| place(*p)).collect::<Vec<_>>()}),
                        })
                        .collect();
                    let v = json!({"fast": e.fast, "totalPathNodes": e.total_path_nodes, "totalEnemies": e.total_enemies, "totalPlatforms": e.total_platforms,
                        "totalGaribs": e.total_garibs, "totalCameraCases": e.total_camera_cases, "totalPlaceholders": e.total_placeholders,
                        "paths": paths, "items": items});
                    self.put_json(&format!("levels/entities/{stem}.json"), &v);
                    self.report.add("entity files", 1);
                }
                Err(err) => {
                    let ver = Entities::version(&data);
                    if ver != Some(f2_formats::entity::ENTITY_VERSION) {
                        self.report.warnings.push(format!("{path}: stale entity version {ver:?} (rejected by the game too): {err}"));
                    } else {
                        self.report.errors.push(format!("{path}: {err}"));
                    }
                }
            }
        }
        for (stem, path) in self.files("maps", "fev") {
            let Some(data) = self.load(&path) else { continue };
            match Script::parse(&data) {
                Ok(s) => {
                    self.put_json(&format!("levels/scripts/{stem}.json"), &json!(s.commands.iter().map(command_json).collect::<Vec<_>>()));
                    self.report.add("script files", 1);
                }
                Err(e) => self.report.errors.push(format!("{path}: {e}")),
            }
        }
    }

    pub fn text(&mut self) {
        let Some(data) = self.load("maps/language.fla") else { return };
        let t = match StringTable::parse(&data) {
            Ok(t) => t,
            Err(e) => {
                self.report.errors.push(format!("maps/language.fla: {e}"));
                return;
            }
        };
        let clean = |s: &str| s.replace(['\t', '\r', '\n'], " ");
        let mut tsv = format!("id\tcomment\t{}\n", t.languages.join("\t"));
        let mut rows = Vec::new();
        for (id, comment, texts) in &t.rows {
            tsv.push_str(&format!("{}\t{}\t{}\n", clean(id), clean(comment), texts.iter().map(|s| clean(s)).collect::<Vec<_>>().join("\t")));
            let mut o = serde_json::Map::new();
            o.insert("id".into(), json!(id));
            o.insert("comment".into(), json!(comment));
            for (l, s) in t.languages.iter().zip(texts) {
                o.insert(l.clone(), json!(s));
            }
            rows.push(Value::Object(o));
        }
        self.put("text/language.tsv", tsv.as_bytes());
        self.put_json("text/language.json", &json!({"languages": t.languages, "rows": rows}));
        self.report.add("text rows", t.rows.len());
    }
}

#[cfg(test)]
mod tests;
