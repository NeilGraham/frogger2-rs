//! The N64 [`DataSource`]: game files from the ROM in one of two layouts.
//!
//! **Pack** (default, phase 3I): the 1C asset pack of `tools/f2-n64pack` (DFS root =
//! `target/n64/pack`). Files are stored in one of three forms, told apart by their magic
//! ([`PackStore`]): `F2Z1` = F2Z stream (level data, decoded with the resident levels
//! dictionary `dict/levels.f2zd`), `DCA` = libdragon `mkasset` (models, textures, fonts:
//! `asset_load` decompresses them), anything else raw.
//!
//! | game asks for | read from the pack | |
//! |---|---|---|
//! | `MAPS\ENTITY-1-0.DAT`, collision, scenics, demos, `language.fla` | same path, lowercase | F2Z |
//! | `objects\<x>.dll` (`object_bank`) | `objects/<x>.dll.pcd` | postcard [`ObjectBank`] (vertex-only geometry) |
//! | (renderer) | `objects/<x>.dll.f2nm` | F2NM render meshes |
//! | `textures\<x>.fla` (`texture_bank`, the procedural sources) | `textures/<x>.ntx` (+ shared pools) | NTX1 |
//! | `sfx\<x>.wus` (`sound_bank`) | `sfx/index.bin` | F2SX: uid → `sfx/NNNN.wav64` |
//! | backdrop `textures\<x>.fla` | `textures/backdrop/<x>.ntx` | NTX1, 320x240 |
//!
//! **Prep** (phase 2G, `--features prep-data`): loose DFS files from `tools/f2-n64prep`
//! (`.pcd` banks, `.fla` texture banks parsed on the N64, `.wus.meta` + per-sample wav64).
//!
//! Sample data never passes through here: the audio module streams wav64 files.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use f2_formats::objbank::ObjectBank;
use f2_formats::texture::{TexBank, Texture};
use f2_formats::wus::{Sample, SoundBank};
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

/// `textures\Garden.fla` → `textures/garden.ntx` (a bank of the pack).
pub fn ntx_bank_path(fla: &str) -> String {
    let p = norm_path(fla);
    let stem = p.rsplit('/').next().unwrap_or(&p);
    let stem = stem.strip_suffix(".fla").unwrap_or(stem);
    format!("textures/{stem}.ntx")
}

/// A backdrop `textures\artwork00.fla` → `textures/backdrop/artwork00.ntx`.
pub fn ntx_backdrop_path(fla: &str) -> String {
    let p = norm_path(fla);
    let stem = p.rsplit('/').next().unwrap_or(&p);
    let stem = stem.strip_suffix(".fla").or_else(|| stem.strip_suffix(".bmp")).unwrap_or(stem);
    format!("textures/backdrop/{stem}.ntx")
}

// ------------------------------------------------------------------ F2SX sound index

/// `sfx/index.bin` (F2SX, `tools/f2-n64pack/src/audio/index.rs`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SfxIndex {
    /// (name, first entry, count).
    pub banks: Vec<(String, u32, u32)>,
    /// (uid, sample id).
    pub entries: Vec<(u32, u16)>,
    /// (source rate, looped, channels).
    pub samples: Vec<(u32, bool, u8)>,
}

/// One playable sound of a bank: uid, wav64 path (`rom:/...`), loop flag, playback rate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SfxRef {
    pub uid: u32,
    pub path: String,
    pub looped: bool,
    pub rate: u32,
}

impl SfxIndex {
    pub fn parse(d: &[u8]) -> Option<SfxIndex> {
        let u16at = |o: usize| d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
        let u32at = |o: usize| d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        if d.get(..4) != Some(b"F2SX") || u16at(4)? != 1 {
            return None;
        }
        let (nb, ns, ne) = (u16at(6)? as usize, u16at(8)? as usize, u32at(12)? as usize);
        let (boff, eoff) = (16, 16 + nb * 24);
        let soff = eoff + ne * 8;
        if d.len() < soff + ns * 16 {
            return None;
        }
        let mut idx = SfxIndex::default();
        for i in 0..nb {
            let o = boff + i * 24;
            let raw = &d[o..o + 16];
            let end = raw.iter().position(|&c| c == 0).unwrap_or(16);
            idx.banks.push((String::from(core::str::from_utf8(&raw[..end]).ok()?), u32at(o + 16)?, u32at(o + 20)?));
        }
        for i in 0..ne {
            let o = eoff + i * 8;
            idx.entries.push((u32at(o)?, u16at(o + 4)?));
        }
        for i in 0..ns {
            let o = soff + i * 16;
            idx.samples.push((u32at(o)?, d[o + 13] & 1 != 0, d[o + 14]));
        }
        if idx.banks.iter().any(|b| b.1 as usize + b.2 as usize > ne) || idx.entries.iter().any(|e| e.1 as usize >= ns) {
            return None;
        }
        Some(idx)
    }

    fn bank(&self, name: &str) -> Option<&[(u32, u16)]> {
        let b = self.banks.iter().find(|b| b.0.eq_ignore_ascii_case(name))?;
        Some(&self.entries[b.1 as usize..(b.1 + b.2) as usize])
    }

    /// The `SoundBank` the simulation sees (metadata only, no PCM).
    pub fn sound_bank(&self, name: &str) -> Option<SoundBank> {
        let e = self.bank(name)?;
        let samples = e
            .iter()
            .map(|&(uid, s)| {
                let (rate, looped, ch) = self.samples[s as usize];
                Sample { uid, looped, format_tag: 1, channels: ch as u16, sample_rate: rate, bits: 16, pcm: Vec::new() }
            })
            .collect();
        Some(SoundBank { samples })
    }

    pub fn refs(&self, name: &str) -> Option<Vec<SfxRef>> {
        let e = self.bank(name)?;
        Some(
            e.iter()
                .map(|&(uid, s)| {
                    let (rate, looped, _) = self.samples[s as usize];
                    SfxRef { uid, path: format!("rom:/sfx/{s:04}.wav64"), looped, rate }
                })
                .collect(),
        )
    }
}

/// `sfx/<bank>.wus` → `<bank>`.
fn sfx_stem(path: &str) -> String {
    let p = norm_path(path);
    let s = p.rsplit('/').next().unwrap_or(&p);
    s.strip_suffix(".wus").unwrap_or(s).to_string()
}

// ------------------------------------------------------------------ NTX banks

/// The procedural-texture sources of an NTX bank as the `TexBank` the simulation's
/// `pt_texture_load` reads (only `prc*` entries, with their PC pixels; nothing else of
/// the bank is needed by the simulation).
pub fn ntx_proc_bank(store: &dyn AssetStore, bank_path: &str) -> Option<TexBank> {
    let bytes = store.load(bank_path)?;
    let f = crate::ntx::Ntx::parse(&bytes).ok()?;
    let mut deps: Vec<Option<Vec<u8>>> = Vec::new();
    let mut textures = Vec::new();
    for e in &f.entries {
        if !e.name().as_bytes().starts_with(b"prc") {
            continue;
        }
        let (file, img): (&[u8], usize) = if e.dep == 0 {
            (&bytes, e.image as usize)
        } else {
            let k = e.dep as usize - 1;
            if deps.len() < f.deps.len() {
                deps.resize(f.deps.len(), None);
            }
            if deps[k].is_none() {
                deps[k] = store.load(&format!("textures/{}", f.deps[k]));
            }
            match deps[k].as_deref() {
                Some(b) => (b, e.image as usize),
                None => continue,
            }
        };
        let Ok(df) = crate::ntx::Ntx::parse(file) else { continue };
        let Some(d) = df.images.get(img) else { continue };
        let Some(px) = df.source_555(img) else { continue };
        textures.push(Texture { flags: 2, crc: e.crc, width: d.width, height: d.height, name: e.name().to_string(), pixels: px });
    }
    Some(TexBank { textures, anims: Vec::new() })
}

// ------------------------------------------------------------------ the data source

/// [`DataSource`] over an [`AssetStore`] (module docs).
pub struct N64Data<S> {
    pub store: S,
    /// The pack layout (else the phase-2G prep layout).
    pub pack: bool,
    /// Called before every access (the front end frees the previous level's renderer side
    /// at the first access of a level load, see `app::unload_level_render`).
    pub before_access: Option<fn()>,
    sfx_index: spin::Mutex<Option<Box<SfxIndex>>>,
}

impl<S: AssetStore> N64Data<S> {
    pub fn new(store: S) -> Self {
        N64Data { store, pack: false, before_access: None, sfx_index: spin::Mutex::new(None) }
    }

    pub fn new_pack(store: S) -> Self {
        N64Data { pack: true, ..Self::new(store) }
    }

    fn load(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(f) = self.before_access {
            f();
        }
        self.store.load(path)
    }

    fn with_index<T>(&self, f: impl FnOnce(&SfxIndex) -> Option<T>) -> Option<T> {
        let mut g = self.sfx_index.lock();
        if g.is_none() {
            *g = Some(Box::new(SfxIndex::parse(&self.store.load("sfx/index.bin")?)?));
        }
        f(g.as_ref().unwrap())
    }

    /// Sound-bank metadata of `sfx/<bank>.wus` (uid, loop flag, format; no PCM).
    pub fn sound_meta(&self, path: &str) -> Option<Result<SoundBank, String>> {
        if self.pack {
            return self.with_index(|i| i.sound_bank(&sfx_stem(path))).map(Ok);
        }
        let p = norm_path(path) + SFX_META_SUFFIX;
        self.load(&p).map(|b| postcard::from_bytes(&b).map_err(|e| format!("{p}: {e}")))
    }

    /// The wav64 files of a sound bank (`sfx\<bank>.wus`).
    pub fn sfx_refs(&self, path: &str) -> Option<Vec<SfxRef>> {
        if self.pack {
            return self.with_index(|i| i.refs(&sfx_stem(path)));
        }
        let stem = norm_path(path);
        let stem = stem.trim_end_matches(".wus").to_string();
        let bank = self.sound_meta(path)?.ok()?;
        Some(bank.samples.iter().map(|s| SfxRef { uid: s.uid, path: format!("rom:/{stem}/{:08x}.wav64", s.uid), looped: s.looped, rate: s.sample_rate }).collect())
    }
}

impl<S: AssetStore> DataSource for N64Data<S> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.load(&norm_path(path))
    }

    fn object_bank(&self, path: &str) -> Option<Result<ObjectBank, String>> {
        let p = norm_path(path) + OBJBANK_SUFFIX;
        let bytes = self.load(&p)?;
        Some(postcard::from_bytes(&bytes).map_err(|e| format!("{p}: {e}")))
    }

    fn texture_bank(&self, path: &str) -> Option<Result<TexBank, String>> {
        if self.pack {
            if let Some(f) = self.before_access {
                f();
            }
            return ntx_proc_bank(&self.store, &ntx_bank_path(path)).map(Ok);
        }
        self.read(path).map(|b| {
            f2_formats::fla::maybe_decompress(b)
                .map_err(|e| e.to_string())
                .and_then(|b| f2_formats::texture::TexBank::parse(&b).map_err(|e| format!("{path}: {e}")))
        })
    }

    fn sound_bank(&self, path: &str) -> Option<Result<SoundBank, String>> {
        self.sound_meta(path)
    }
}

// ------------------------------------------------------------------ stores

/// Reads raw (possibly `mkasset`-compressed, decompressed transparently) files.
pub trait RawRead: Send + Sync {
    fn read_raw(&self, path: &str) -> Option<Vec<u8>>;
}

/// The 1C pack: [`RawRead`] + F2Z decoding of `F2Z1` files (dictionaries by id, loaded on
/// first use from `dict/levels.f2zd` and kept resident).
pub struct PackStore<R> {
    pub raw: R,
    f2z: spin::Mutex<F2z>,
}

struct F2z {
    dec: Option<Box<f2_n64z::Decoder>>,
    /// (id, dictionary over leaked bytes).
    dicts: Vec<(u8, f2_n64z::Dict<'static>)>,
    loaded: u8,
}

/// The decoder (~31 KB), allocated zeroed straight on the heap (never on the stack).
fn new_decoder() -> Box<f2_n64z::Decoder> {
    let layout = core::alloc::Layout::new::<f2_n64z::Decoder>();
    // SAFETY: `Decoder` consists only of integer and bool arrays/fields, for which the
    // all-zero bit pattern is valid (it equals `Decoder::new()`); the pointer comes from the
    // global allocator with `Decoder`'s layout, as `Box::from_raw` requires.
    unsafe {
        let p = alloc::alloc::alloc_zeroed(layout) as *mut f2_n64z::Decoder;
        if p.is_null() {
            alloc::alloc::handle_alloc_error(layout);
        }
        Box::from_raw(p)
    }
}

/// Dictionary files of the pack (`dict/<class>.f2zd`).
pub const DICTS: [&str; 3] = ["dict/levels.f2zd", "dict/models.f2zd", "dict/textures.f2zd"];

impl<R: RawRead> PackStore<R> {
    pub fn new(raw: R) -> Self {
        PackStore { raw, f2z: spin::Mutex::new(F2z { dec: None, dicts: Vec::new(), loaded: 0 }) }
    }

    fn decode(&self, z: &[u8]) -> Option<Vec<u8>> {
        let h = f2_n64z::header(z).ok()?;
        let mut st = self.f2z.lock();
        if st.dec.is_none() {
            st.dec = Some(new_decoder());
        }
        if h.dict_id != 0 && !st.dicts.iter().any(|d| d.0 == h.dict_id) {
            // Load every dictionary of the pack once (each is F2Z without a dictionary).
            for p in DICTS {
                let Some(dz) = self.raw.read_raw(p) else { continue };
                let Ok(dh) = f2_n64z::header(&dz) else { continue };
                let mut content = alloc::vec![0u8; dh.raw_size as usize];
                let mut tmp = alloc::vec![0u8; if dh.needs_scratch() { dh.raw_size as usize } else { 0 }];
                if st.dec.as_mut().unwrap().decode(&dz, None, &mut content, &mut tmp).is_err() {
                    continue;
                }
                let bytes: &'static [u8] = Box::leak(content.into_boxed_slice());
                if let Ok(d) = f2_n64z::Dict::parse(bytes) {
                    if !st.dicts.iter().any(|x| x.0 == d.id) {
                        st.dicts.push((d.id, d));
                    }
                }
            }
            st.loaded = 0;
        }
        let F2z { dec, dicts, loaded } = &mut *st;
        let dec = dec.as_mut().unwrap();
        let dict = if h.dict_id != 0 {
            let d = &dicts.iter().find(|d| d.0 == h.dict_id)?.1;
            if *loaded != h.dict_id {
                dec.load_dict(d);
                *loaded = h.dict_id;
            }
            Some(d)
        } else {
            None
        };
        let mut out = alloc::vec![0u8; h.raw_size as usize];
        let mut tmp = alloc::vec![0u8; if h.needs_scratch() { h.raw_size as usize } else { 0 }];
        dec.decode(z, dict, &mut out, &mut tmp).ok()?;
        Some(out)
    }
}

impl<R: RawRead> AssetStore for PackStore<R> {
    fn load(&self, path: &str) -> Option<Vec<u8>> {
        let raw = self.raw.read_raw(path)?;
        if raw.starts_with(b"F2Z1") {
            self.decode(&raw)
        } else {
            Some(raw)
        }
    }
}

/// Files on the cartridge's DragonFS (`rom:/<path>`), read with `asset_load` (which
/// transparently decompresses `mkasset` files).
#[cfg(target_vendor = "nintendo64")]
pub struct DfsStore;

#[cfg(target_vendor = "nintendo64")]
impl RawRead for DfsStore {
    fn read_raw(&self, path: &str) -> Option<Vec<u8>> {
        crate::rt::audio_poll();
        let mut p = String::with_capacity(path.len() + 5);
        p.push_str("rom:/");
        p.push_str(path);
        let r = crate::rt::file_read(&p);
        crate::rt::audio_poll();
        r
    }
}

#[cfg(target_vendor = "nintendo64")]
impl AssetStore for DfsStore {
    fn load(&self, path: &str) -> Option<Vec<u8>> {
        self.read_raw(path)
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

impl RawRead for MemStore {
    fn read_raw(&self, path: &str) -> Option<Vec<u8>> {
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
        assert_eq!(ntx_bank_path("textures\\Garden.fla"), "textures/garden.ntx");
        assert_eq!(ntx_backdrop_path("textures\\artwork00.fla"), "textures/backdrop/artwork00.ntx");
        assert_eq!(sfx_stem("SFX\\GARDEN.WUS"), "garden");
    }

    #[test]
    fn decodes_prepass_layout() {
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
        assert_eq!(d.sfx_refs("sfx\\garden.wus").unwrap()[0].path, "rom:/sfx/garden/00000007.wav64");
    }

    fn f2sx() -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(b"F2SX");
        o.extend_from_slice(&1u16.to_be_bytes());
        o.extend_from_slice(&1u16.to_be_bytes()); // banks
        o.extend_from_slice(&2u16.to_be_bytes()); // samples
        o.extend_from_slice(&0u16.to_be_bytes());
        o.extend_from_slice(&2u32.to_be_bytes()); // entries
        let mut n = [0u8; 16];
        n[..6].copy_from_slice(b"garden");
        o.extend_from_slice(&n);
        o.extend_from_slice(&0u32.to_be_bytes());
        o.extend_from_slice(&2u32.to_be_bytes());
        for (uid, s) in [(0xAAu32, 1u16), (0xBB, 0)] {
            o.extend_from_slice(&uid.to_be_bytes());
            o.extend_from_slice(&s.to_be_bytes());
            o.extend_from_slice(&0u16.to_be_bytes());
        }
        for (rate, looped) in [(22050u32, 0u8), (11025, 1)] {
            o.extend_from_slice(&rate.to_be_bytes());
            o.extend_from_slice(&100u32.to_be_bytes());
            o.extend_from_slice(&50u32.to_be_bytes());
            o.extend_from_slice(&[1, looped, 1, 0]);
        }
        o
    }

    #[test]
    fn pack_sound_index() {
        let mut st = MemStore::default();
        st.insert("sfx/index.bin", f2sx());
        let d = N64Data::new_pack(st);
        let b = d.sound_bank("sfx\\Garden.wus").unwrap().unwrap();
        assert_eq!(b.samples.len(), 2);
        assert_eq!((b.samples[0].uid, b.samples[0].looped, b.samples[0].sample_rate), (0xAA, true, 11025));
        let r = d.sfx_refs("sfx\\garden.wus").unwrap();
        assert_eq!(r[0].path, "rom:/sfx/0001.wav64");
        assert_eq!(r[1].path, "rom:/sfx/0000.wav64");
        assert!(d.sound_bank("sfx\\none.wus").is_none());
    }

    #[test]
    fn pack_store_decodes_f2z() {
        // An F2Z stream without a dictionary is decoded; raw files pass through.
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8 ^ (i / 97) as u8).collect();
        let z = f2_n64z::enc::compress(&data, &[], None, &f2_n64z::enc::Options::default());
        let mut st = MemStore::default();
        st.insert("maps/a.dat", z);
        st.insert("maps/b.dat", alloc::vec![9, 9]);
        let p = PackStore::new(st);
        assert_eq!(p.load("maps/a.dat").unwrap(), data);
        assert_eq!(p.load("maps/b.dat").unwrap(), alloc::vec![9, 9]);
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
