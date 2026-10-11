//! The N64 [`DataSource`]: game files from the ROM, with the PC containers the N64 must not
//! parse replaced by the host pre-pass outputs (`tools/f2-n64prep`):
//!
//! | game asks for | read from the store | |
//! |---|---|---|
//! | any file (`MAPS\ENTITY-1-0.DAT`, ...) | `maps/entity-1-0.dat` | lowercase, `/` |
//! | `objects\<x>.dll` (`object_bank`) | `objects/<x>.dll.pcd` | postcard [`ObjectBank`] |
//! | `sfx\<x>.wus` (`sound_bank`) | `sfx/<x>.wus.meta` | postcard [`SoundBank`] without PCM |
//! | `textures\<x>.fla` (`texture_bank`) | `textures/<x>.fla` | parsed on the N64 (LZSS) |
//!
//! Sample data never passes through here: the audio module streams `sfx/<bank>/<uid>.wav64`.
//!
//! **Pack-format hook (phase 1C)**: [`AssetStore`] is the only storage interface. The current
//! store ([`DfsStore`], n64 only) reads loose DFS files (optionally `mkasset`-compressed);
//! the compressed pack (`tools/f2-n64pack`, `crates/f2-n64z`) implements `AssetStore` over its
//! archive (same logical paths) and plugs into [`N64Data`] unchanged. A pack that stores
//! other derived forms (e.g. pre-extracted model facts) overrides the matching
//! [`DataSource`] method in a wrapper.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use f2_formats::objbank::ObjectBank;
use f2_formats::wus::SoundBank;
use f2_game::assets::DataSource;

/// Suffix of pre-parsed object banks (`f2_n64prep::OBJBANK_SUFFIX`).
pub const OBJBANK_SUFFIX: &str = ".pcd";
/// Suffix of an object bank's F2NM render meshes (`f2_n64prep::F2NM_SUFFIX`).
pub const F2NM_SUFFIX: &str = ".f2nm";
/// Suffix of sound-bank metadata (`f2_n64prep::SFX_META_SUFFIX`).
pub const SFX_META_SUFFIX: &str = ".meta";

/// Read-only storage of logical asset paths (lowercase, `/`-separated, no leading `/`).
pub trait AssetStore: Send + Sync {
    /// The whole (decompressed) file, `None` if absent.
    fn load(&self, path: &str) -> Option<Vec<u8>>;
}

/// The game's relative path (`MAPS\ENTITY-1-0.DAT`) as a logical store path.
pub fn norm_path(path: &str) -> String {
    let mut s: String = path.chars().map(|c| if c == '\\' { '/' } else { c.to_ascii_lowercase() }).collect();
    while s.starts_with('/') {
        s.remove(0);
    }
    s
}

/// [`DataSource`] over an [`AssetStore`] holding the pre-pass layout (module docs).
pub struct N64Data<S> {
    pub store: S,
    /// Called before every access (the front end frees the previous level's renderer side
    /// at the first access of a level load, see `app::unload_level_render`).
    pub before_access: Option<fn()>,
}

impl<S: AssetStore> N64Data<S> {
    pub fn new(store: S) -> Self {
        N64Data { store, before_access: None }
    }

    fn load(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(f) = self.before_access {
            f();
        }
        self.store.load(path)
    }

    /// Sound-bank metadata of `sfx/<bank>.wus` (uid, loop flag, format; no PCM).
    pub fn sound_meta(&self, path: &str) -> Option<Result<SoundBank, String>> {
        let p = norm_path(path) + SFX_META_SUFFIX;
        self.load(&p).map(|b| postcard::from_bytes(&b).map_err(|e| alloc::format!("{p}: {e}")))
    }
}

impl<S: AssetStore> DataSource for N64Data<S> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.load(&norm_path(path))
    }

    fn object_bank(&self, path: &str) -> Option<Result<ObjectBank, String>> {
        let p = norm_path(path) + OBJBANK_SUFFIX;
        let bytes = self.load(&p)?;
        Some(postcard::from_bytes(&bytes).map_err(|e| alloc::format!("{p}: {e}")))
    }

    fn sound_bank(&self, path: &str) -> Option<Result<SoundBank, String>> {
        self.sound_meta(path)
    }
}

/// Loose files on the cartridge's DragonFS (`rom:/<path>`), read with `asset_load` (which
/// transparently decompresses `mkasset` files).
#[cfg(target_vendor = "nintendo64")]
pub struct DfsStore;

#[cfg(target_vendor = "nintendo64")]
impl AssetStore for DfsStore {
    fn load(&self, path: &str) -> Option<Vec<u8>> {
        crate::rt::audio_poll();
        let mut p = String::with_capacity(path.len() + 5);
        p.push_str("rom:/");
        p.push_str(path);
        let r = crate::rt::file_read(&p);
        crate::rt::audio_poll();
        r
    }
}

/// In-memory store (host tests).
#[derive(Default)]
pub struct MemStore(pub alloc::collections::BTreeMap<String, Vec<u8>>);

impl AssetStore for MemStore {
    fn load(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(path).cloned()
    }
}

impl MemStore {
    pub fn insert(&mut self, path: &str, data: Vec<u8>) {
        self.0.insert(path.to_string(), data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(norm_path("MAPS\\ENTITY-1-0.DAT"), "maps/entity-1-0.dat");
        assert_eq!(norm_path("/textures/font/BigFont.bmp"), "textures/font/bigfont.bmp");
    }

    #[test]
    fn decodes_prepass_layout() {
        use f2_formats::wus::Sample;
        let mut st = MemStore::default();
        let bank =
            SoundBank { samples: alloc::vec![Sample { uid: 7, looped: true, format_tag: 1, channels: 1, sample_rate: 22050, bits: 16, pcm: Vec::new() }] };
        st.insert("sfx/garden.wus.meta", postcard::to_allocvec(&bank).unwrap());
        st.insert("maps/x.dat", alloc::vec![1, 2, 3]);
        let d = N64Data::new(st);
        let b = d.sound_bank("SFX\\GARDEN.WUS").unwrap().unwrap();
        assert_eq!((b.samples[0].uid, b.samples[0].looped), (7, true));
        assert_eq!(d.read("MAPS\\X.DAT"), Some(alloc::vec![1, 2, 3]));
        assert!(d.object_bank("objects\\none.dll").is_none());
    }

    /// The real pre-pass output decodes (needs `F2_GAME_DIR`; skipped otherwise).
    #[test]
    fn real_object_bank() {
        let Some(dir) = std::env::var_os("F2_GAME_DIR") else { return };
        let dll = std::path::Path::new(&dir).join("objects/garden1.dll");
        let Ok(bytes) = std::fs::read(&dll) else { return };
        let bank = ObjectBank::parse(&bytes).unwrap();
        let mut st = MemStore::default();
        st.insert("objects/garden1.dll.pcd", postcard::to_allocvec(&bank).unwrap());
        let d = N64Data::new(st);
        let b = d.object_bank("OBJECTS\\GARDEN1.DLL").unwrap().unwrap();
        assert_eq!(b.models, bank.models);
    }
}
