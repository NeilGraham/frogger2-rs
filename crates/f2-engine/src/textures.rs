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

    /// Adds (or replaces) an entry for `crc` from an RGBA8 image.
    pub fn add_rgba(&mut self, crc: u32, name: &str, tex_type: TexType, width: u32, height: u32, rgba: Vec<u8>) -> TexHandle {
        let image = self.add_image(width, height, rgba);
        let e = TexEntry { crc, name: name.to_string(), tex_type, image, frames: Vec::new(), last_frame: 0, last_game_frame: 0, current: Some(image) };
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
        self.load_bank_with(bank, |_, _| {});
    }

    /// [`load_bank`](Self::load_bank) calling `on_image` on each new image right after it is
    /// decoded, so a back end can convert it and drop the RGBA8 pixels before the next one
    /// (the N64 keeps its RDRAM peak low this way).
    pub fn load_bank_with(&mut self, bank: &TexBank, mut on_image: impl FnMut(TexHandle, &mut Image)) {
        for t in &bank.textures {
            let h = self.add_texture(t);
            on_image(h, &mut self.images[h.0 as usize]);
        }
        for a in &bank.anims {
            // The original stops at the first animation whose base is unknown.
            let Some(&ei) = self.by_crc.get(&a.base_crc) else { break };
            let frames: Vec<_> = a.frames.iter().map(|f| (self.by_crc.get(&f.crc).map(|&i| self.entries[i].image), f.time)).collect();
            let e = &mut self.entries[ei];
            e.frames = frames;
            e.last_frame = 0;
            e.last_game_frame = 0;
        }
    }

    pub fn entry(&self, crc: u32) -> Option<&TexEntry> {
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
        for me in &mut self.entries {
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
}
