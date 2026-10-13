//! Texture registry keyed by name CRC (`MDX_TEXENTRY` list, `mdxTexture.cpp`).
//!
//! Each entry owns one GPU image (`TexHandle`). Animated textures follow the
//! bank's animation table: the *base* entry's displayed image changes over
//! time (`UpdateAnimatingTextures`, mdxTexture.cpp:768), so faces that
//! reference the base CRC animate automatically.

use crate::prelude::*;

use alloc::collections::BTreeMap;
use f2_formats::texture::{TexBank, TexType, Texture};

/// Index of an uploaded GPU image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TexHandle(pub u32);

/// A decoded RGBA8 image waiting to be uploaded (or already uploaded).
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct TexEntry {
    pub crc: u32,
    pub name: String,
    pub tex_type: TexType,
    /// The entry's own image.
    pub image: TexHandle,
    /// Animation frames (`frames[]`, `frameTimes[]`); empty when static.
    pub frames: Vec<(Option<TexHandle>, i32)>,
    pub last_frame: usize,
    pub last_game_frame: u32,
    /// Currently displayed image (`me->surf`).
    pub current: Option<TexHandle>,
}

/// CRC → entry map plus the list of images to upload.
#[derive(Default)]
pub struct TextureRegistry {
    pub entries: Vec<TexEntry>,
    by_crc: BTreeMap<u32, usize>,
    pub images: Vec<Image>,
    /// Images not yet uploaded to the GPU (indices into `images`).
    pub(crate) dirty: Vec<u32>,
    /// Indices of the entries with more than one animation frame, in order (`None`: to be
    /// rebuilt; entries change through `add_entry` / `add_rgba` / `apply_anims` only).
    animated: Option<Vec<u32>>,
    /// Console fast path: the last `by_crc` lookups (an entry keeps its index once added).
    #[cfg(feature = "lean")]
    memo: CrcMemo,
}

/// [`TextureRegistry::entry`]'s memo of `by_crc` (`lean`, single-threaded consoles): 64
/// direct-mapped `(crc, index + 1)` slots, 0 = empty. Exact: an entry is only ever replaced
/// in place, so a CRC's index never changes once it is in the map.
#[cfg(feature = "lean")]
struct CrcMemo([(core::sync::atomic::AtomicU32, core::sync::atomic::AtomicU32); 64]);

#[cfg(feature = "lean")]
impl Default for CrcMemo {
    fn default() -> Self {
        use core::sync::atomic::AtomicU32;
        CrcMemo([const { (AtomicU32::new(0), AtomicU32::new(0)) }; 64])
    }
}

impl TextureRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Images added since the last call (indices into `images`), for a back end that
    /// uploads them itself.
    pub fn take_dirty(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.dirty)
    }

    /// Adds a raw RGBA8 image (no CRC entry) and returns its handle.
    pub fn add_image(&mut self, width: u32, height: u32, rgba: Vec<u8>) -> TexHandle {
        assert_eq!(rgba.len(), (width * height * 4) as usize, "image size mismatch");
        let h = TexHandle(self.images.len() as u32);
        self.images.push(Image { width, height, rgba });
        self.dirty.push(h.0);
        h
    }

    /// Replaces the pixels of an existing image (e.g. a video frame); the GPU
    /// texture is re-created/updated on the next upload.
    pub fn replace_image(&mut self, h: TexHandle, width: u32, height: u32, rgba: Vec<u8>) {
        assert_eq!(rgba.len(), (width * height * 4) as usize, "image size mismatch");
        self.images[h.0 as usize] = Image { width, height, rgba };
        if !self.dirty.contains(&h.0) {
            self.dirty.push(h.0);
        }
    }

    /// Releases an image's memory (it becomes a 1x1 transparent texture).
    pub fn free_image(&mut self, h: TexHandle) {
        self.replace_image(h, 1, 1, vec![0; 4]);
    }

    /// Adds an image without pixels (a back end that brings its own texels, e.g. the N64's
    /// pre-converted pack textures). It is not queued for upload.
    pub fn add_placeholder(&mut self, width: u32, height: u32) -> TexHandle {
        let h = TexHandle(self.images.len() as u32);
        self.images.push(Image { width, height, rgba: Vec::new() });
        h
    }

    /// Adds (or replaces, like `LoadTexBankFile`) the entry for `crc` showing `image`.
    pub fn add_entry(&mut self, crc: u32, name: &str, tex_type: TexType, image: TexHandle) {
        let e = TexEntry { crc, name: name.to_string(), tex_type, image, frames: Vec::new(), last_frame: 0, last_game_frame: 0, current: Some(image) };
        self.animated = None;
        if let Some(&i) = self.by_crc.get(&crc) {
            self.entries[i] = e;
        } else {
            self.by_crc.insert(crc, self.entries.len());
            self.entries.push(e);
        }
    }

    /// The animation table of a bank (`LoadTexBankFile`'s second half): applied against
    /// every entry loaded so far; stops at the first animation whose base is unknown.
    pub fn apply_anims(&mut self, anims: &[f2_formats::texture::TexAnim]) {
        self.animated = None;
        for a in anims {
            let Some(&ei) = self.by_crc.get(&a.base_crc) else { break };
            let frames: Vec<_> = a.frames.iter().map(|f| (self.by_crc.get(&f.crc).map(|&i| self.entries[i].image), f.time)).collect();
            let e = &mut self.entries[ei];
            e.frames = frames;
            e.last_frame = 0;
            e.last_game_frame = 0;
        }
    }

    /// Adds (or replaces) an entry for `crc` from an RGBA8 image.
    pub fn add_rgba(&mut self, crc: u32, name: &str, tex_type: TexType, width: u32, height: u32, rgba: Vec<u8>) -> TexHandle {
        let image = self.add_image(width, height, rgba);
        let e = TexEntry { crc, name: name.to_string(), tex_type, image, frames: Vec::new(), last_frame: 0, last_game_frame: 0, current: Some(image) };
        self.animated = None;
        if let Some(&i) = self.by_crc.get(&crc) {
            self.entries[i] = e;
        } else {
            self.by_crc.insert(crc, self.entries.len());
            self.entries.push(e);
        }
        image
    }

    /// Adds one bank texture, converted with the D3D upload rules (`Texture::to_rgba8`).
    pub fn add_texture(&mut self, t: &Texture) -> TexHandle {
        self.add_rgba(t.crc, &t.name, t.tex_type(), t.width.into(), t.height.into(), t.to_rgba8())
    }

    /// Port of `LoadTexBankFile` (mdxTexture.cpp:680): textures, then the animation table.
    pub fn load_bank(&mut self, bank: &TexBank) {
        self.load_bank_with(bank, |_, _, _| {});
    }

    /// [`load_bank`](Self::load_bank) calling `on_image` on each new image (with its bank
    /// texture) right after it is decoded, so a back end can convert it and drop the RGBA8
    /// pixels before the next one (the N64 keeps its RDRAM peak low this way).
    pub fn load_bank_with(&mut self, bank: &TexBank, mut on_image: impl FnMut(TexHandle, &mut Image, &Texture)) {
        for t in &bank.textures {
            let h = self.add_texture(t);
            on_image(h, &mut self.images[h.0 as usize], t);
        }
        self.apply_anims(&bank.anims);
    }

    pub fn entry(&self, crc: u32) -> Option<&TexEntry> {
        #[cfg(feature = "lean")]
        {
            use core::sync::atomic::Ordering::Relaxed;
            let (k, v) = &self.memo.0[((crc ^ crc >> 6 ^ crc >> 12 ^ crc >> 18) & 63) as usize];
            let i = v.load(Relaxed);
            if i != 0 && k.load(Relaxed) == crc {
                return self.entries.get(i as usize - 1);
            }
            let i = *self.by_crc.get(&crc)?;
            k.store(crc, Relaxed);
            v.store(i as u32 + 1, Relaxed);
            Some(&self.entries[i])
        }
        #[cfg(not(feature = "lean"))]
        self.by_crc.get(&crc).map(|&i| &self.entries[i])
    }

    /// `GetTexEntryFromCRC` + `tEntry->surf`: the image currently shown for `crc`.
    pub fn current(&self, crc: u32) -> Option<TexHandle> {
        self.entry(crc).and_then(|e| e.current)
    }

    /// Looks a texture up by file name (`"name.bmp"` → CRC).
    pub fn by_name(&self, name: &str) -> Option<TexHandle> {
        self.current(f2_formats::crc(name))
    }

    /// Port of `UpdateAnimatingTextures` (mdxTexture.cpp:768); `frame_count` is
    /// `timeInfo.frameCount` (game frames). Call once per rendered frame.
    pub fn update_animating_textures(&mut self, frame_count: u32) {
        // Only the animated entries (the same ones, in the same order, as a walk over all).
        let animated =
            self.animated.take().unwrap_or_else(|| self.entries.iter().enumerate().filter(|(_, e)| e.frames.len() > 1).map(|(i, _)| i as u32).collect());
        for &i in &animated {
            let me = &mut self.entries[i as usize];
            let n = me.frames.len();
            if n <= 1 {
                continue;
            }
            if me.frames[me.last_frame].1 == -1 {
                me.last_game_frame = me.last_game_frame.wrapping_add(1);
                me.last_frame += 1;
                if me.last_frame >= n {
                    me.last_frame = 0;
                }
            } else {
                // The original compares in float; times are small integers.
                let mut guard = 0;
                while (me.last_game_frame as f32 + me.frames[me.last_frame].1 as f32) < frame_count as f32 {
                    me.last_game_frame = (me.last_game_frame as i64 + me.frames[me.last_frame].1 as i64) as u32;
                    me.last_frame += 1;
                    if me.last_frame >= n {
                        me.last_frame = 0;
                    }
                    guard += 1;
                    if guard > 1_000_000 {
                        break; // zero/negative frame times would spin forever in the original
                    }
                }
            }
            me.current = me.frames[me.last_frame].0;
        }
        self.animated = Some(animated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use f2_formats::texture::{TexAnim, TexAnimFrame};

    fn tex(crc: u32, px: u16) -> Texture {
        Texture { flags: 0, crc, width: 1, height: 1, name: format!("{crc}"), pixels: vec![px] }
    }

    #[test]
    fn animation_advances_with_game_frames() {
        let bank = TexBank {
            textures: vec![tex(1, 0x7fff), tex(2, 0), tex(3, 0x1f)],
            anims: vec![TexAnim {
                base_crc: 1,
                frames: vec![TexAnimFrame { crc: 1, time: 5 }, TexAnimFrame { crc: 2, time: 5 }, TexAnimFrame { crc: 3, time: 5 }],
            }],
        };
        let mut r = TextureRegistry::new();
        r.load_bank(&bank);
        let h = |r: &TextureRegistry, c| r.entry(c).unwrap().image;
        r.update_animating_textures(0);
        assert_eq!(r.current(1), Some(h(&r, 1)));
        r.update_animating_textures(6);
        assert_eq!(r.current(1), Some(h(&r, 2)));
        r.update_animating_textures(11);
        assert_eq!(r.current(1), Some(h(&r, 3)));
        r.update_animating_textures(16);
        assert_eq!(r.current(1), Some(h(&r, 1)));
    }

    /// The lookup memo (console builds) never answers for a CRC it has not seen and follows
    /// replaced entries and animation frames.
    #[test]
    fn lookups_follow_the_map() {
        let mut r = TextureRegistry::new();
        let img: Vec<TexHandle> = (0..300).map(|i| r.add_image(1, 1, vec![i as u8; 4])).collect();
        // CRCs that share memo slots.
        let crcs: Vec<u32> = (0..200u32).map(|i| i.wrapping_mul(0x9e37_79b9) ^ (i << 6)).collect();
        for (k, &c) in crcs.iter().enumerate().take(150) {
            r.add_entry(c, "t", TexType::Normal, img[k]);
        }
        for _ in 0..3 {
            for (k, &c) in crcs.iter().enumerate() {
                assert_eq!(r.current(c), (k < 150).then(|| img[k]), "crc {c:08x}");
            }
        }
        // Replaced in place: the memo still finds the entry, with its new image.
        r.add_entry(crcs[3], "t", TexType::Normal, img[299]);
        assert_eq!(r.current(crcs[3]), Some(img[299]));
        // Added later: found once in the map.
        r.add_entry(crcs[170], "t", TexType::Normal, img[170]);
        assert_eq!(r.current(crcs[170]), Some(img[170]));
    }
}
