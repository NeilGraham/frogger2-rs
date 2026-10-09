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
    /// Names of every object in the hierarchy, pre-order: root subtree, then each
    /// root sibling subtree (sub-object lookups such as the frog's head).
    pub sub_objects: Vec<String>,
    /// Top-level object chain: the root object followed by its `next` siblings.
    pub hierarchy: Vec<ObjNode>,
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
            self.models
                .entry(m.id)
                .or_insert_with(|| Arc::new(ModelInfo { id: m.id, name: m.root.name.clone(), radius: m.radius, animations, sub_objects: subs, hierarchy }));
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
