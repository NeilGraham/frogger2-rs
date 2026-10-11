//! Texture banks and backdrops of the 1C pack (NTX1) into the `f2-engine` texture registry
//! and N64 textures, without ever making RGBA8 copies: every registry entry gets a
//! placeholder image (no pixels) whose N64 texture points straight into the loaded file
//! ([`RenderCpu::adopt_ntx`]); r3d aliases those textures for the meshes.
//!
//! `LoadTexBankFile` semantics: entries in bank order (a later CRC replaces an earlier one),
//! then the animation table against everything loaded so far. Each entry gets its own
//! registry image (the PC gives every entry its own surface: procedural textures and the
//! story book's render targets are rewritten per entry later), but entries showing the same
//! NTX image share the file bytes.
//!
//! I-format alpha masks / opaque I images are converted to IA (`ntx::ia_fixup`) so the
//! RDP's `TEX × SHADE` combiner shows the PC colours; the story book's source pictures
//! additionally keep decoded RGBA8 pixels (the book page is composed on the CPU).

use crate::data::AssetStore;
use crate::ffi::{self, NtxDesc};
use crate::ntx::{self, Ntx};
use crate::render_cpu::RenderCpu;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use f2_engine::{TexHandle, TextureRegistry};
use f2_formats::texture::{TexAnim, TexAnimFrame, TexType};

/// Pack files loaded for the current level (shared pools are used by several banks).
#[derive(Default)]
pub struct NtxCache {
    files: BTreeMap<String, Arc<Vec<u8>>>,
}

impl NtxCache {
    pub fn clear(&mut self) {
        self.files.clear();
    }

    fn get(&mut self, store: &dyn AssetStore, path: &str) -> Option<Arc<Vec<u8>>> {
        if let Some(f) = self.files.get(path) {
            return Some(f.clone());
        }
        let b = Arc::new(store.load(path)?);
        self.files.insert(String::from(path), b.clone());
        Some(b)
    }
}

/// Whether a bank texture is read back by the CPU (`f2_draw::book::draw_page_b`).
pub fn book_source(crc: u32) -> bool {
    crc == f2_formats::crc("page32.bmp") || f2_game::menus::CHAPTER_PIC.iter().any(|p| crc == f2_formats::crc(&format!("{p}.bmp").to_lowercase()))
}

fn tex_type(kind: u8) -> TexType {
    match kind {
        ntx::kind::ALPHA_INTENSITY => TexType::AlphaIntensity,
        ntx::kind::PROCEDURAL => TexType::Procedural,
        ntx::kind::NO_TEXTURE => TexType::NoTexture,
        ntx::kind::ALPHA_COLOUR => TexType::AlphaColour,
        _ => TexType::Normal,
    }
}

/// The N64 texture of image `i` of `f` (whose bytes are `owner`) for registry image `h`.
fn adopt(gfx: &mut RenderCpu, h: TexHandle, owner: &Arc<Vec<u8>>, f: &Ntx, i: usize) -> bool {
    let d = f.images[i];
    let flags = if d.flags & ntx::flag::SINGLE_TILE != 0 { ffi::NTX_SINGLE } else { 0 } | if d.flags & ntx::flag::TLUT_PER_TILE != 0 { ffi::NTX_PER_TILE } else { 0 };
    let tlut = f.tlut_bytes(i);
    if let Some((fmt, stride, texels)) = ntx::ia_fixup(&d, f.texels(i)) {
        let owned = Arc::new(texels);
        let desc = NtxDesc {
            fmt: fmt as u32,
            width: d.width as u32,
            height: d.height as u32,
            stride: stride as u32,
            texels: owned.as_ptr(),
            tlut: core::ptr::null(),
            tlut_count: 0,
            flags,
            tile_w: d.tile_w as u32,
            tile_h: d.tile_h as u32,
        };
        return gfx.adopt_ntx(h.0, owned, &desc);
    }
    let desc = NtxDesc {
        fmt: d.fmt as u32,
        width: d.width as u32,
        height: d.height as u32,
        stride: d.stride as u32,
        texels: f.texels(i).as_ptr(),
        tlut: if tlut.is_empty() { core::ptr::null() } else { tlut.as_ptr() },
        tlut_count: d.tlut_count as u32,
        flags,
        tile_w: d.tile_w as u32,
        tile_h: d.tile_h as u32,
    };
    gfx.adopt_ntx(h.0, owner.clone(), &desc)
}

/// Loads `textures/<bank>.ntx` (+ its pools) like `LoadTexBankFile`. Returns the number of
/// entries.
pub fn load_bank(store: &dyn AssetStore, path: &str, reg: &mut TextureRegistry, gfx: &mut RenderCpu, cache: &mut NtxCache) -> Result<usize, String> {
    let bank = cache.get(store, path).ok_or_else(|| format!("{path}: not found"))?;
    let bf = Ntx::parse(&bank).map_err(|e| format!("{path}: {e}"))?;
    // Files: 0 = the bank, k = deps[k - 1].
    let mut owners: Vec<Option<Arc<Vec<u8>>>> = alloc::vec![Some(bank.clone())];
    for d in &bf.deps {
        let p = format!("textures/{d}");
        let f = cache.get(store, &p);
        if f.is_none() {
            crate::log!("ntx: {p} (dependency of {path}) not found");
        }
        owners.push(f);
    }
    let parsed: Vec<Option<Ntx>> = owners.iter().map(|o| o.as_ref().and_then(|b| Ntx::parse(b).ok())).collect();
    let mut n = 0;
    for e in &bf.entries {
        let k = e.dep as usize;
        let (Some(Some(f)), Some(Some(owner))) = (parsed.get(k), owners.get(k)) else { continue };
        let i = e.image as usize;
        let Some(d) = f.images.get(i).copied() else { continue };
        let h = if book_source(e.crc) {
            // The CPU composes the story book from these: keep RGBA8 at the stored size.
            let h = reg.add_placeholder(d.width as u32, d.height as u32);
            reg.images[h.0 as usize].rgba = f.decode_rgba8(i);
            h
        } else {
            reg.add_placeholder(d.orig_width.max(1) as u32, d.orig_height.max(1) as u32)
        };
        if !adopt(gfx, h, owner, f, i) {
            crate::log!("ntx: {path}: texture {} not created", e.name());
        }
        reg.add_entry(e.crc, e.name(), tex_type(d.kind), h);
        n += 1;
        crate::rt::audio_poll();
    }
    let anims: Vec<TexAnim> =
        bf.anims.iter().map(|a| TexAnim { base_crc: a.base_crc, frames: a.frames.iter().map(|&(crc, time)| TexAnimFrame { crc, time }).collect() }).collect();
    reg.apply_anims(&anims);
    Ok(n)
}

/// A full-screen backdrop (`textures/backdrop/<x>.ntx`) as a registry image.
pub fn load_backdrop(store: &dyn AssetStore, path: &str, reg: &mut TextureRegistry, gfx: &mut RenderCpu) -> Result<TexHandle, String> {
    let b = Arc::new(store.load(path).ok_or_else(|| format!("{path}: not found"))?);
    let f = Ntx::parse(&b).map_err(|e| format!("{path}: {e}"))?;
    if f.images.is_empty() {
        return Err(format!("{path}: no image"));
    }
    let d = f.images[0];
    let h = reg.add_placeholder(d.width as u32, d.height as u32);
    if !adopt(gfx, h, &b, &f, 0) {
        return Err(format!("{path}: texture not created"));
    }
    Ok(h)
}
