//! Port of `Common/ptexture.c` (procedural textures: fire, candle, forcefield, water
//! drops/bubbler/trail/ripples/waves, steam, dissolve) plus their PC set-up
//! (`PTTextureLoad`, `CreateProceduralTexture`, `PTSurfaceBlit`, PC/pcmisc.cpp).
//!
//! Every texture whose bank name starts with `prc` becomes a 32x32 8-bit simulation with
//! a 256-entry ARGB4444 palette built from the texture's first 512 pixels. Each update
//! first "blits" the current buffer through the palette into the texture surface
//! (so the surface lags the simulation by one update), then advances the simulation.
//!
//! Renderer API: [`ProcTexState::list`] in update order; for each [`ProcTexture`], `crc`
//! names the bank texture to replace and [`ProcTexture::surface`] holds the last blitted
//! ARGB4444 pixels (`None` until the first update: show the bank texture);
//! [`ProcTexture::surface_rgba8`] converts them.

use crate::prelude::*;

use f2_formats::texture::TexBank;
use serde::{Deserialize, Serialize};

use crate::define::mode::{GAMEOVER_MODE, PAUSE_MODE};
use crate::Game;

/// `pt->Update` (selected from the texture name).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PtUpdate {
    Fire,
    Forcefield,
    WaterRipples,
    WaterDrops,
    WaterBubbler,
    WaterTrail,
    WaterWaves,
    Steam,
    Candle,
    Dissolve,
}

/// `PROCTEXTURE` (`ptexture.h`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcTexture {
    /// Texture name (as stored in the bank) and CRC (`tex`).
    pub name: String,
    pub crc: u32,
    pub timer: u32,
    pub buf1: Vec<u8>,
    pub buf2: Vec<u8>,
    pub active: u8,
    /// ARGB4444 palette.
    pub palette: Vec<u16>,
    pub update: Option<PtUpdate>,
    /// Last `PTSurfaceBlit` result (32x32 ARGB4444), None before the first update.
    pub surface: Option<Vec<u16>>,
}

impl ProcTexture {
    /// The surface as RGBA8 (4-bit channels expanded by `*17`).
    pub fn surface_rgba8(&self) -> Option<Vec<u8>> {
        let s = self.surface.as_ref()?;
        let mut out = Vec::with_capacity(s.len() * 4);
        for &c in s {
            out.push((((c >> 8) & 15) * 17) as u8);
            out.push((((c >> 4) & 15) * 17) as u8);
            out.push(((c & 15) * 17) as u8);
            out.push((((c >> 12) & 15) * 17) as u8);
        }
        Some(out)
    }

    /// Port of `PTSurfaceBlit` (PC/pcmisc.cpp:66), hardware path.
    fn blit(&mut self) {
        self.surface = Some(self.buf1.iter().map(|&b| self.palette[b as usize]).collect());
    }

    fn swap(&mut self) {
        core::mem::swap(&mut self.buf1, &mut self.buf2);
    }
}

/// Globals of `ptexture.c`: `prcTexList` (in list order), `dissolveTex`, and the
/// function-static positions of the bubbler / trail effects (never reset).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcTexState {
    pub list: Vec<ProcTexture>,
    /// `dissolveTex` (index into `list`).
    pub dissolve_tex: Option<usize>,
    bubbler_xy: (u32, u32),
    trail: (i16, i16, i16, i16),
}

impl Default for ProcTexState {
    fn default() -> Self {
        ProcTexState { list: Vec::new(), dissolve_tex: None, bubbler_xy: (16, 16), trail: (16, 16, 2, 2) }
    }
}

/// `buf[i]` with out-of-range reads as 0.
/// BUG(original): the fire/candle/steam smoothing reads `buf1[p+64]` up to index 1054 and
/// `buf1[p-64]` down to -31 (outside the 1024-byte buffer: neighbouring heap memory). The
/// port reads 0 there.
#[inline]
fn rd(b: &[u8], i: i32) -> u32 {
    if (0..1024).contains(&i) {
        b[i as usize] as u32
    } else {
        0
    }
}

/// Smooth, move up and fade (fire, candle, steam).
fn smooth_up(pt: &mut ProcTexture) {
    for i in (1..=30).rev() {
        for j in (1..=30).rev() {
            let p = (i << 5) + j;
            let t = rd(&pt.buf1, p + 31) + rd(&pt.buf1, p + 33) + rd(&pt.buf1, p + 64);
            pt.buf2[p as usize] = (((t + rd(&pt.buf1, p - 64)) + (t + rd(&pt.buf1, p - 32))) >> 3) as u8;
        }
    }
}

/// The water ripple filter: `res = sum8 >> 2; res -= buf2[p] + (buf2[p] >> damp); buf2[p] = max(res,0)`.
fn water(pt: &mut ProcTexture, damp: u32) {
    for i in (1..=30).rev() {
        for j in (1..=30).rev() {
            let p = ((i << 5) + j) as usize;
            let b = &pt.buf1;
            let sum = b[p + 32] as i32
                + b[p - 32] as i32
                + b[p + 1] as i32
                + b[p - 1] as i32
                + b[p + 33] as i32
                + b[p + 31] as i32
                + b[p - 33] as i32
                + b[p - 31] as i32;
            let mut res = (sum >> 2) as i16;
            res -= pt.buf2[p] as i16 + (pt.buf2[p] >> damp) as i16;
            pt.buf2[p] = res.max(0) as u8;
        }
    }
}

/// Port of `CreateProceduralTexture` (PC/pcmisc.cpp:156) for one bank texture.
pub fn create_procedural_texture(tex: &f2_formats::texture::Texture) -> ProcTexture {
    let px = |i: usize| tex.pixels.get(i).copied().unwrap_or(0);
    let mut palette = vec![0u16; 256];
    for (i, pal) in palette.iter_mut().enumerate() {
        let d = px(i);
        let nr = ((d >> 10) & 0x1f) * 0x0f / 0x1f;
        let ng = ((d >> 5) & 0x1f) * 0x0f / 0x1f;
        let nb = (d & 0x1f) * 0x0f / 0x1f;
        let na = (px(i + 256) & 0x1f) * 0x0f / 0x1f;
        *pal = (na << 12) | (nr << 8) | (ng << 4) | nb;
    }
    let n = tex.name.as_bytes();
    let c = |i: usize| n.get(i).copied().unwrap_or(0);
    let is = |s: &[u8; 4]| c(4) == s[0] && c(5) == s[1] && c(6) == s[2] && c(7) == s[3];
    let mut active = 1;
    let update = if is(b"fire") {
        Some(PtUpdate::Fire)
    } else if is(b"ffld") {
        Some(PtUpdate::Forcefield)
    } else if is(b"watr") {
        match c(8) {
            b'r' => Some(PtUpdate::WaterRipples),
            b'd' => Some(PtUpdate::WaterDrops),
            b'b' => Some(PtUpdate::WaterBubbler),
            b't' => Some(PtUpdate::WaterTrail),
            b'w' => Some(PtUpdate::WaterWaves),
            _ => None,
        }
    } else if is(b"stea") {
        active = 0;
        Some(PtUpdate::Steam)
    } else if is(b"cndl") {
        Some(PtUpdate::Candle)
    } else if is(b"dslv") {
        active = 0;
        Some(PtUpdate::Dissolve)
    } else {
        None
    };
    ProcTexture { name: tex.name.clone(), crc: tex.crc, timer: 0, buf1: vec![0; 1024], buf2: vec![0; 1024], active, palette, update, surface: None }
}

impl Game {
    /// Port of `PTTextureLoad` (PC/pcmisc.cpp:127): every loaded texture named `prc*`
    /// becomes a procedural texture. `banks` in load order; `texList` is newest-first
    /// and each new procedural texture is pushed at the front of `prcTexList`, so the
    /// update order is the load order.
    pub fn pt_texture_load(&mut self, banks: &[TexBank]) {
        let mut list = Vec::new();
        for b in banks {
            for t in &b.textures {
                let n = t.name.as_bytes();
                if n.len() >= 3 && &n[..3] == b"prc" {
                    list.push(create_procedural_texture(t));
                }
            }
        }
        // dissolveTex = the last created = first in list order with a dissolve update
        self.ptexture.dissolve_tex = list.iter().position(|p| p.update == Some(PtUpdate::Dissolve));
        self.ptexture.list = list;
    }

    /// Port of `FreeProcTextures` (Common/ptexture.c:469).
    pub fn free_proc_textures(&mut self) {
        self.ptexture.dissolve_tex = None;
        self.ptexture.list.clear();
    }

    /// Port of `ProcessProcTextures` (Common/ptexture.c:449), called at the end of
    /// `DrawLoop` unless paused / game over.
    pub fn process_proc_textures(&mut self) {
        let afc = self.act_frame_count;
        for i in 0..self.ptexture.list.len() {
            let pt = &self.ptexture.list[i];
            if let Some(u) = pt.update {
                if afc > pt.timer && pt.active != 0 {
                    self.ptexture.list[i].timer = afc.wrapping_add(1); // Max 60fps
                    self.process_pt(i, u);
                }
            }
        }
    }

    /// The DrawLoop condition around `ProcessProcTextures`.
    pub fn process_proc_textures_drawloop(&mut self) {
        if self.game_state.mode != PAUSE_MODE && self.game_state.mode != GAMEOVER_MODE {
            self.process_proc_textures();
        }
    }

    fn process_pt(&mut self, i: usize, u: PtUpdate) {
        let gs = self.game_speed;
        let mut pt = core::mem::replace(
            &mut self.ptexture.list[i],
            ProcTexture {
                name: String::new(),
                crc: 0,
                timer: 0,
                buf1: Vec::new(),
                buf2: Vec::new(),
                active: 0,
                palette: Vec::new(),
                update: None,
                surface: None,
            },
        );
        pt.blit();
        let rng = &mut self.rng;
        match u {
            PtUpdate::Fire => {
                // Port of `ProcessPTFire` (Common/ptexture.c:40).
                for b in &mut pt.buf1[992..1024] {
                    *b = 0;
                }
                for _ in 0..9 {
                    let p = 1008 + (rng.random(30) - 15);
                    for o in [0, 1, -1, -32, -31, -33] {
                        pt.buf1[(p + o) as usize] = 0xff;
                    }
                }
                smooth_up(&mut pt);
            }
            PtUpdate::Candle => {
                // Port of `ProcessPTCandle` (Common/ptexture.c:96).
                for o in [0i32, 1, -1, 32, -32, 31, 33, -31, -33] {
                    pt.buf1[(528 + o) as usize] = (200 + rng.random(56)) as u8;
                }
                smooth_up(&mut pt);
            }
            PtUpdate::Forcefield => {
                // Port of `ProcessPTForcefield` (Common/ptexture.c:137).
                // NB: two Random() calls in one expression; MSVC evaluates the left one first.
                let a = rng.random(30) + 1;
                let b = rng.random(30) + 1;
                let p = (a * 32 + b) as usize;
                pt.buf1[p] = pt.buf1[p].wrapping_sub(30);
                for q in [p + 1, p - 1, p - 32, p + 32] {
                    pt.buf1[q] = pt.buf1[q].wrapping_sub(15);
                }
                for i in 0..1024u32 {
                    let b = &pt.buf1;
                    let sum = b[(i & 1023) as usize] as i32
                        + b[(i.wrapping_sub(1) & 1023) as usize] as i32
                        + b[(i.wrapping_sub(31) & 1023) as usize] as i32
                        + b[((i + 32) & 1023) as usize] as i32
                        - 10;
                    let res = ((sum as u32) >> 2) as u16;
                    pt.buf2[i as usize] = res as u8;
                }
            }
            PtUpdate::WaterDrops => {
                // Port of `ProcessPTWaterDrops` (Common/ptexture.c:176).
                if rng.random(2) != 0 {
                    let a = rng.random(30) + 1;
                    let b = rng.random(30) + 1;
                    let p = (a * 32 + b) as usize;
                    pt.buf1[p] = pt.buf1[p].wrapping_add(200);
                }
                water(&mut pt, 2);
            }
            PtUpdate::WaterBubbler => {
                // Port of `ProcessPTWaterBubbler` (Common/ptexture.c:212).
                let (mut x, mut y) = self.ptexture.bubbler_xy;
                x = x.wrapping_add((rng.random(3) - 1) as u32);
                y = y.wrapping_add((rng.random(3) - 1) as u32);
                x = x.clamp(10, 20);
                y = y.clamp(10, 20);
                self.ptexture.bubbler_xy = (x, y);
                let p = (y * 32 + x) as usize;
                pt.buf1[p] = pt.buf1[p].wrapping_add(200);
                water(&mut pt, 2);
            }
            PtUpdate::WaterTrail => {
                // Port of `ProcessPTWaterTrail` (Common/ptexture.c:255).
                let (mut x, mut y, mut xv, mut yv) = self.ptexture.trail;
                if rng.random(10) > 8 {
                    xv = xv.wrapping_add((rng.random(3) - 1) as i16);
                    yv = yv.wrapping_add((rng.random(3) - 1) as i16);
                    xv = xv.clamp(-2, 2);
                    yv = yv.clamp(-2, 2);
                }
                x = x.wrapping_add(xv);
                y = y.wrapping_add(yv);
                if !(2..=30).contains(&x) {
                    xv = xv.wrapping_neg();
                    x = x.wrapping_add(xv);
                }
                if !(2..=30).contains(&y) {
                    yv = yv.wrapping_neg();
                    y = y.wrapping_add(yv);
                }
                self.ptexture.trail = (x, y, xv, yv);
                let p = (y as i32 * 32 + x as i32) as usize;
                if p < 1024 {
                    pt.buf1[p] = pt.buf1[p].wrapping_add(200);
                }
                water(&mut pt, 2);
            }
            PtUpdate::WaterRipples => {
                // Port of `ProcessPTWaterRipples` (Common/ptexture.c:314).
                let p = (rng.random(30) + 1 + 960) as usize;
                pt.buf1[p] = 255;
                water(&mut pt, 3);
            }
            PtUpdate::WaterWaves => {
                // Port of `ProcessPTWaterWaves` (Common/ptexture.c:345).
                if rng.random(3) > 1 {
                    for i in 928..992 {
                        if rng.random(3) > 1 {
                            pt.buf1[i] = (200 + rng.random(56)) as u8;
                        }
                    }
                }
                water(&mut pt, 3);
            }
            PtUpdate::Steam => {
                // Port of `ProcessPTSteam` (Common/ptexture.c:378).
                for i in 935..953 {
                    if rng.random(3) > 1 {
                        pt.buf1[i] = (200 + rng.random(56)) as u8;
                    }
                }
                for i in 968..984 {
                    if rng.random(3) > 1 {
                        pt.buf1[i] = (200 + rng.random(56)) as u8;
                    }
                }
                smooth_up(&mut pt);
            }
            PtUpdate::Dissolve => {
                // Port of `ProcessPTDissolve` (Common/ptexture.c:416).
                let lim = ((10 * gs) >> 12) as u32;
                for i in (0..1024).rev() {
                    pt.buf2[i] = if rng.random(lim) != 0 { pt.buf1[i] } else { (pt.active == 1) as u8 };
                }
            }
        }
        pt.swap();
        self.ptexture.list[i] = pt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use f2_formats::texture::Texture;

    fn tex(name: &str) -> Texture {
        // palette: grey ramp in the first 256 pixels, opaque alpha in the next 256
        let mut pixels = vec![0u16; 1024];
        for i in 0..256 {
            let v = (i as u16 >> 3) & 0x1f;
            pixels[i] = (v << 10) | (v << 5) | v;
            pixels[i + 256] = 0x1f;
        }
        Texture { flags: 2, crc: f2_formats::crc::crc(name), width: 32, height: 32, name: name.to_string(), pixels }
    }

    fn run(names: &[&str], frames: u32) -> (Game, Vec<Vec<u8>>) {
        let mut g = Game::new();
        let bank = TexBank { textures: names.iter().map(|n| tex(n)).collect(), anims: vec![] };
        g.pt_texture_load(&[bank]);
        for _ in 0..frames {
            g.act_frame_count += 1;
            g.process_proc_textures();
        }
        let bufs = g.ptexture.list.iter().map(|p| p.buf1.clone()).collect();
        (g, bufs)
    }

    #[test]
    fn names_select_updates() {
        let (g, _) = run(&["prc_fire1", "prc_watrr1", "prc_watrd", "prc_stea1", "prc_dslv", "prc_cndl1", "prc_ffld1", "noprc"], 0);
        let u: Vec<_> = g.ptexture.list.iter().map(|p| (p.update, p.active)).collect();
        assert_eq!(
            u,
            vec![
                (Some(PtUpdate::Fire), 1),
                (Some(PtUpdate::WaterRipples), 1),
                (Some(PtUpdate::WaterDrops), 1),
                (Some(PtUpdate::Steam), 0),
                (Some(PtUpdate::Dissolve), 0),
                (Some(PtUpdate::Candle), 1),
                (Some(PtUpdate::Forcefield), 1)
            ]
        );
        assert_eq!(g.ptexture.dissolve_tex, Some(4));
        // palette: 5-bit grey 31 -> 15, alpha 15
        assert_eq!(g.ptexture.list[0].palette[255], 0xffff);
        assert_eq!(g.ptexture.list[0].palette[0], 0xf000);
    }

    #[test]
    fn deterministic_and_rng_consuming() {
        let names = ["prc_fire1", "prc_watrr1", "prc_watrd1", "prc_watrb1", "prc_watrt1", "prc_watrw1", "prc_cndl1", "prc_ffld1"];
        let (g1, b1) = run(&names, 300);
        let (g2, b2) = run(&names, 300);
        assert_eq!(b1, b2);
        assert_eq!(g1.rng, g2.rng);
        assert_ne!(g1.rng, Game::new().rng);
        // fire has heat near the bottom rows
        assert!(b1[0][900..1000].iter().any(|&v| v > 0));
        let s = g1.ptexture.list[0].surface_rgba8().unwrap();
        assert_eq!(s.len(), 4096);
        // one update per frame at most (timer)
        let mut g = g1;
        let seed = g.rng;
        g.process_proc_textures();
        assert_eq!(g.rng, seed);
    }

    #[test]
    fn fire_first_frame_exact() {
        let mut g = Game::new();
        g.pt_texture_load(&[TexBank { textures: vec![tex("prc_fire1")], anims: vec![] }]);
        g.act_frame_count = 1;
        let mut r = g.rng;
        g.process_proc_textures();
        // expected spark columns from the same RNG sequence
        let mut buf = vec![0u8; 1024];
        for _ in 0..9 {
            let p = 1008 + (r.random(30) - 15);
            for o in [0, 1, -1, -32, -31, -33] {
                buf[(p + o) as usize] = 0xff;
            }
        }
        // after the update buf1 is the smoothed buffer; the blitted surface was all zero
        let pt = &g.ptexture.list[0];
        assert!(pt.surface.as_ref().unwrap().iter().all(|&c| c == pt.palette[0]));
        assert_eq!(pt.buf2, buf);
        assert_eq!(g.rng, r);
    }
}
