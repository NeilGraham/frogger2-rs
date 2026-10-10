//! Read-only access to game data needed by the simulation.
//!
//! The host provides a [`DataSource`] (usually the mod VFS). Gameplay needs
//! level files and a few facts about models (animation tables, radius), never
//! textures or audio data.

use std::collections::HashMap;
use std::sync::Arc;

use f2_formats::objbank::{Object, ObjectBank};
use serde::{Deserialize, Serialize};

/// File access with the original's relative paths (`MAPS\\ENTITY-1-0.DAT`, any case).
pub trait DataSource {
    fn read(&self, path: &str) -> Option<Vec<u8>>;

    // P7-1 (mods): parsed containers. The defaults parse [`DataSource::read`]; a modding
    // host overrides them to return the *effective* bank (retail + per-entry overrides).
    // `None` = file missing.

    /// `objects\<name>.dll` (`LoadObjBank`).
    fn object_bank(&self, path: &str) -> Option<Result<ObjectBank, String>> {
        self.read(path).map(|b| ObjectBank::parse(&b).map_err(|e| format!("{path}: {e}")))
    }

    /// `textures\<name>.fla` (`LoadTexBankFile`), decompressed and parsed.
    fn texture_bank(&self, path: &str) -> Option<Result<f2_formats::texture::TexBank, String>> {
        self.read(path).map(|b| {
            f2_formats::fla::maybe_decompress(b)
                .map_err(|e| format!("{path}: {e}"))
                .and_then(|b| f2_formats::texture::TexBank::parse(&b).map_err(|e| format!("{path}: {e}")))
        })
    }

    /// `sfx\<name>.wus` (`LoadSfxSet`).
    fn sound_bank(&self, path: &str) -> Option<Result<f2_formats::wus::SoundBank, String>> {
        self.read(path).map(|b| f2_formats::wus::SoundBank::parse(&b).map_err(|e| format!("{path}: {e}")))
    }

    /// `MAPS\LANGUAGE.FLA` (`gameTextInit`), decompressed and parsed.
    fn string_table(&self, path: &str) -> Option<Result<f2_formats::lang::StringTable, String>> {
        self.read(path).map(|b| {
            f2_formats::fla::maybe_decompress(b)
                .map_err(|e| e.to_string())
                .and_then(|b| f2_formats::lang::StringTable::parse(&b).map_err(|e| e.to_string()))
        })
    }
}

/// In-memory data source (tests).
#[derive(Default)]
pub struct MemData(pub HashMap<String, Vec<u8>>);

impl DataSource for MemData {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(&path.replace('\\', "/").to_lowercase()).cloned()
    }
}

/// Simple directory-backed source (case-insensitive by lowercasing; imported
/// data is stored lowercase).
pub struct DirData(pub std::path::PathBuf);

impl DataSource for DirData {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path.replace('\\', "/").to_lowercase())).ok()
    }
}

/// `MDX_ANIMATION` (`MDX Library/mdxObject.h:192`): one entry of a model's animation table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MdxAnimation {
    pub anim_start: i16,
    pub anim_end: i16,
    pub speed: f32,
    pub looped: i32,
}

/// Name tree of an `MDX_OBJECT` hierarchy (for `FindActorSubObject`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjNode {
    pub name: String,
    pub flags: i16,
    pub children: Vec<ObjNode>,
}

impl ObjNode {
    fn from_object(o: &Object) -> ObjNode {
        ObjNode { name: o.name.clone(), flags: o.flags, children: o.children.iter().map(ObjNode::from_object).collect() }
    }
}

/// What gameplay needs to know about a model (`MDX_OBJECT_CONTROLLER`).
#[derive(Clone, Debug, Default)]
pub struct ModelInfo {
    pub id: u32,
    pub name: String,
    pub radius: f32,
    pub animations: Vec<MdxAnimation>,
    /// `anims[numAnimations]`: the bank bytes after the table, read by `Animate`/`UpdateAnims`
    /// for `animNum == numAnimations` (BUG(original), see `MdxActorAnimation::anim`).
    pub anim_past_end: Option<MdxAnimation>,
    /// Names of every object in the hierarchy, pre-order: root subtree, then each
    /// root sibling subtree (sub-object lookups such as the frog's head).
    pub sub_objects: Vec<String>,
    /// Top-level object chain: the root object followed by its `next` siblings.
    pub hierarchy: Vec<ObjNode>,
    /// Root object keys + bounding box (`OBJECT_FLAGS_CLIPPED`, `view.rs`).
    pub root_object: Option<crate::view::RootObject>,
    /// `MDX_ACTOR::trueCentre` / `radius` from `FindActorCentre` (model constants).
    pub true_centre: [f32; 3],
    pub actor_radius: f32,
    /// `(textureID, obj->xlu)` of every object sprite, in `AddObjectsSpritesToSpriteList` order.
    pub obj_sprites: Vec<(u32, u8)>,
    /// Object tree with keys (root + `next` siblings) for sub-object transforms.
    pub xform_tree: Vec<crate::view::XNode>,
}

/// Port of `FindObj` (`MDX Library/mdxActor.cpp:978`) on a sibling chain starting at
/// `chain[i]`: checks the object, then (recursively) its `next` siblings, then its
/// children; case-insensitive (`stricmp`).
fn find_obj<'a>(chain: &'a [ObjNode], i: usize, name: &str) -> Option<&'a ObjNode> {
    let me = &chain[i];
    if me.name.eq_ignore_ascii_case(name) {
        return Some(me);
    }
    if i + 1 < chain.len() {
        if let Some(t) = find_obj(chain, i + 1, name) {
            return Some(t);
        }
    }
    if !me.children.is_empty() {
        return find_obj(&me.children, 0, name);
    }
    None
}

impl ModelInfo {
    /// Port of `FindActorSubObject` (`MDX Library/mdxActor.cpp:1003`).
    pub fn find_sub_object(&self, sub_name: &str) -> Option<&ObjNode> {
        if self.hierarchy.is_empty() {
            return None;
        }
        find_obj(&self.hierarchy, 0, sub_name)
    }

    /// `objectController->object->flags` of the root object.
    pub fn root_flags(&self) -> i16 {
        self.hierarchy.first().map_or(0, |o| o.flags)
    }
}

/// Models from all loaded object banks, keyed by `crc("name.obe")`.
#[derive(Clone, Debug, Default)]
pub struct ModelTable {
    pub models: HashMap<u32, Arc<ModelInfo>>,
}

impl ModelTable {
    /// Adds every model of a bank. Like `FindObject` (`mdxObject.cpp:587`), which scans
    /// banks in load order and returns the first match, an id already present (from an
    /// earlier bank) is kept.
    pub fn add_bank(&mut self, bank: &ObjectBank) {
        for m in &bank.models {
            fn walk(o: &Object, out: &mut Vec<String>) {
                out.push(o.name.clone());
                for c in &o.children {
                    walk(c, out);
                }
            }
            let mut subs = Vec::new();
            walk(&m.root, &mut subs);
            for s in &m.root_siblings {
                walk(s, &mut subs);
            }
            let mut hierarchy = vec![ObjNode::from_object(&m.root)];
            hierarchy.extend(m.root_siblings.iter().map(ObjNode::from_object));
            let animations = m.animations.iter().map(|a| MdxAnimation { anim_start: a.start, anim_end: a.end, speed: a.speed, looped: a.looped }).collect();
            self.models.entry(m.id).or_insert_with(|| {
                let (true_centre, actor_radius) = crate::view::find_actor_centre(m, !m.animations.is_empty());
                fn sprites(o: &Object, out: &mut Vec<(u32, u8)>) {
                    out.extend(o.sprites.iter().map(|s| (s.texture, o.xlu)));
                    for c in &o.children {
                        sprites(c, out);
                    }
                }
                let mut obj_sprites = Vec::new();
                sprites(&m.root, &mut obj_sprites);
                for s in &m.root_siblings {
                    sprites(s, &mut obj_sprites);
                }
                Arc::new(ModelInfo {
                    id: m.id,
                    name: m.root.name.clone(),
                    radius: m.radius,
                    animations,
                    sub_objects: subs,
                    hierarchy,
                    root_object: Some(crate::view::RootObject::from_object(&m.root)),
                    true_centre,
                    actor_radius,
                    obj_sprites,
                    xform_tree: std::iter::once(&m.root).chain(m.root_siblings.iter()).map(crate::view::XNode::from_object).collect(),
                    anim_past_end: bank
                        .anim_past_end
                        .iter()
                        .find(|e| e.0 == m.id)
                        .and_then(|e| e.1.clone())
                        .map(|a| MdxAnimation { anim_start: a.start, anim_end: a.end, speed: a.speed, looped: a.looped }),
                })
            });
        }
    }

    pub fn get(&self, id: u32) -> Option<&Arc<ModelInfo>> {
        self.models.get(&id)
    }

    /// Port of `FindObject(&ctl, UpdateCRC(name), name)` (`mdxObject.cpp:587`).
    pub fn find_object(&self, name: &str) -> Option<&Arc<ModelInfo>> {
        self.get(f2_formats::crc(name))
    }

    pub fn clear(&mut self) {
        self.models.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(name: &str, children: Vec<ObjNode>) -> ObjNode {
        ObjNode { name: name.into(), flags: 0, children }
    }

    #[test]
    fn find_obj_order() {
        // root(a,b(c)), sibling s
        let m =
            ModelInfo { hierarchy: vec![n("root", vec![n("a", vec![]), n("b", vec![n("c", vec![])])]), n("s", vec![n("t", vec![])])], ..Default::default() };
        assert_eq!(m.find_sub_object("C").unwrap().name, "c");
        assert_eq!(m.find_sub_object("t").unwrap().name, "t");
        assert!(m.find_sub_object("zz").is_none());
    }
}
