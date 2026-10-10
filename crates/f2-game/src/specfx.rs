//! Port of `Common/specfx.c`: special effects (`SPECFX`), their creation presets
//! (`CreateSpecialEffect*`), per-type update functions and the composite effects
//! (`CreateLightningEffect`, `CreateRingEffect`, `CreateExplodeEffect`, ...).
//!
//! State model (mirrors the C exactly, including allocation order, which decides list
//! order and the `Random()` call sequence):
//! * `sfxList`: fixed pool of `maxSpecfx = 128` effects with a free stack and a circular
//!   list; new effects are inserted before `lastAdded[updateType]` (or at the front), so
//!   effects with the same update function stay grouped. Slot `MAX_SPECFX` is the static
//!   list head (its `updateType` is 0, which `DeallocateFX` can store in `lastAdded`).
//! * `p2List`: pool of 64 `PLANE2` rebound planes.
//! * Sprites/particles come from [`crate::sprite`] / [`crate::particle`].
//!
//! `IsPointVisible`/`OutcodeCheck` need the renderer's view transform (previous frame's
//! `vMatrix`); the host may provide it in [`SpecFxState::view`]. Without one every point
//! counts as visible (DEVIATION: retail culls off-screen effects, which changes the
//! `Random()` sequence).
//!
//! Renderer API: iterate [`SpecFxState::iter_list`] (list order, as `DrawSpecialFX`);
//! effects with `draw == Some(FxDraw::Decal|Ring|Lightning)` are drawn from the effect
//! fields (`origin`, `normal`, `scale`, `angle`, `tilt`, `r,g,b,a`, `tex`, particle polys
//! via [`Game::fx_particles`], ring shape [`SpecFxState::ring_vtx`]); all other effects
//! are drawn through their sprites in the sprite list.

use f2_math::{
    add_to_vector_ff, add_to_vector_fs, add_to_vector_sf, add_to_vector_ss, add_vector_sfs, add_vector_ssf, add_vector_sss, arccos, cross_product_fff,
    distance_between_points_sf, distance_between_points_ss, dot_product_ff, fdiv, fixed_get_quaternion_from_rotation, fmul, iquat_slerp, magnitude_f,
    make_unit, new_set_vector_fs, rcos, rsin, scale_vector, scale_vector_ff, scale_vector_s, set_quaternion, set_vector_ff, set_vector_fs, set_vector_sf,
    set_vector_ss, sub_from_vector_ff, sub_vector_fsf, sub_vector_fss, sub_vector_ssf, FVector, Fixed, IQuaternion, SVector,
};
use serde::{Deserialize, Serialize};

use crate::actor2::ACTOR_DRAWDISTANCEOUTER;
use crate::ids::{ActorId, EnemyId, PlatformId, SpecFxId, SpriteId, TileId};
use crate::map::SCALE;
use crate::particle::{ParticleId, PTYPE_NUMTYPES};
use crate::sprite::{SPRITE_ADDITIVE, SPRITE_FLAGS_ROTATE, SPRITE_TRANSLUCENT};
use crate::Game;

/// `ENTITY_PLATFORM` / `ENTITY_ENEMY` (`specfx.h:34`) as a typed entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxEntity {
    Platform(PlatformId),
    Enemy(EnemyId),
}

// `EF_*` attached-effect flags (`specfx.h:39`).
pub const EF_RIPPLE_RINGS: u32 = 1 << 0;
pub const EF_SMOKE_STATIC: u32 = 1 << 1;
pub const EF_SPARKBURST: u32 = 1 << 2;
pub const EF_GREENGLOOP: u32 = 1 << 3;
pub const EF_BATSWARM: u32 = 1 << 4;
pub const EF_BUBBLES: u32 = 1 << 5;
pub const EF_SMOKE_GROWS: u32 = 1 << 6;
pub const EF_SMOKEBURST: u32 = 1 << 7;
pub const EF_FIERYSMOKE: u32 = 1 << 8;
pub const EF_BUTTERFLYSWARM: u32 = 1 << 9;
pub const EF_LASER: u32 = 1 << 10;
pub const EF_TRAIL: u32 = 1 << 11;
pub const EF_BILLBOARDTRAIL: u32 = 1 << 12;
pub const EF_LIGHTNING: u32 = 1 << 13;
pub const EF_SPACETHING1: u32 = 1 << 14;
pub const EF_SPARKLYTRAIL: u32 = 1 << 15;
pub const EF_GLOW: u32 = 1 << 16;
pub const EF_TWINKLE: u32 = 1 << 17;
pub const EF_RANDOMCREATE: u32 = 1 << 25;
pub const EF_FAST: u32 = 1 << 26;
pub const EF_MEDIUM: u32 = 1 << 27;
pub const EF_SLOW: u32 = 1 << 28;
pub const EF_TINTRED: u32 = 1 << 29;
pub const EF_TINTGREEN: u32 = 1 << 30;
pub const EF_TINTBLUE: u32 = 1 << 31;

// `FXTYPE_*` (`specfx.h:70`).
pub const FXTYPE_CUSTOM: i16 = 0;
pub const FXTYPE_WATERRIPPLE: i16 = 1;
pub const FXTYPE_GARIBCOLLECT: i16 = 2;
pub const FXTYPE_SPARKLYTRAIL: i16 = 3;
pub const FXTYPE_FROGSTUN: i16 = 4;
pub const FXTYPE_SMOKE_STATIC: i16 = 5;
pub const FXTYPE_SMOKE_GROWS: i16 = 6;
pub const FXTYPE_DECAL: i16 = 7;
pub const FXTYPE_SPLASH: i16 = 8;
pub const FXTYPE_SMOKEBURST: i16 = 9;
pub const FXTYPE_GREENGLOOP: i16 = 10;
pub const FXTYPE_BATSWARM: i16 = 11;
pub const FXTYPE_BUBBLES: i16 = 12;
pub const FXTYPE_SPARKBURST: i16 = 13;
pub const FXTYPE_FIERYSMOKE: i16 = 14;
pub const FXTYPE_POLYRING: i16 = 15;
pub const FXTYPE_BUTTERFLYSWARM: i16 = 16;
pub const FXTYPE_LASER: i16 = 17;
pub const FXTYPE_TRAIL: i16 = 18;
pub const FXTYPE_BILLBOARDTRAIL: i16 = 19;
pub const FXTYPE_LIGHTNING: i16 = 20;
pub const FXTYPE_HEALTHFLY: i16 = 21;
pub const FXTYPE_SPACETHING1: i16 = 22;
pub const FXTYPE_FROGSHIELD: i16 = 23;
pub const FXTYPE_GLOW: i16 = 24;
pub const FXTYPE_TWINKLE: i16 = 25;
pub const FXTYPE_WAKE: i16 = 26;
pub const FXTYPE_CROAK: i16 = 27;
pub const FXTYPE_FLASH: i16 = 28;
pub const FXTYPE_NUMTYPES: i16 = 29;

/// `maxSpecfx` / `maxPlanes` (PC).
pub const MAX_SPECFX: usize = 128;
pub const MAX_PLANES: usize = 64;
/// `NUM_RINGSEGS` (`specfx.h:29`).
pub const NUM_RINGSEGS: usize = 16;
/// `sizeof(SPECFX)` on the PC build (used for the swarm's pointer-derived phase).
const SIZEOF_SPECFX: u32 = 136;

// `FXUPDATE_*` (`specfx.c:80`): indices into `lastAdded`.
pub const FXUPDATE_DECAL: i8 = 0;
pub const FXUPDATE_RING: i8 = 1;
pub const FXUPDATE_BOLT: i8 = 2;
pub const FXUPDATE_SMOKE: i8 = 3;
pub const FXUPDATE_SWARM: i8 = 4;
pub const FXUPDATE_EXPLODE: i8 = 5;
pub const FXUPDATE_TRAIL: i8 = 6;
pub const FXUPDATE_LIGHTNING: i8 = 7;
pub const FXUPDATE_FLY: i8 = 8;
pub const FXUPDATE_TWINKLE: i8 = 9;
pub const FXUPDATE_PFX_JET: i8 = 10;
pub const FXUPDATE_PFX_TELEIDLE: i8 = 11;
pub const FXUPDATE_PFX_TELEACTIVE: i8 = 12;
pub const NUM_FXUPDATES: usize = 13;

/// `updateMapping[PTYPE_NUMTYPES]` (`specfx.c:118`).
pub const UPDATE_MAPPING: [i8; PTYPE_NUMTYPES as usize] = [
    FXUPDATE_DECAL,
    FXUPDATE_DECAL,
    FXUPDATE_DECAL,
    FXUPDATE_EXPLODE,
    FXUPDATE_SWARM,
    FXUPDATE_SMOKE,
    FXUPDATE_SMOKE,
    FXUPDATE_DECAL,
    FXUPDATE_EXPLODE,
    FXUPDATE_EXPLODE,
    FXUPDATE_SMOKE,
    FXUPDATE_SWARM,
    FXUPDATE_SMOKE,
    FXUPDATE_EXPLODE,
    FXUPDATE_EXPLODE,
    FXUPDATE_RING,
    FXUPDATE_SWARM,
    FXUPDATE_BOLT,
    FXUPDATE_TRAIL,
    FXUPDATE_TRAIL,
    FXUPDATE_LIGHTNING,
    FXUPDATE_FLY,
    FXUPDATE_SWARM,
    FXUPDATE_RING,
    FXUPDATE_TWINKLE,
    FXUPDATE_TWINKLE,
    FXUPDATE_DECAL,
    FXUPDATE_RING,
    FXUPDATE_TWINKLE,
    FXUPDATE_PFX_JET,
    FXUPDATE_PFX_JET,
    FXUPDATE_PFX_TELEIDLE,
    FXUPDATE_PFX_TELEACTIVE,
];

/// `fx->Update` function pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FxUpdate {
    Decal,
    Ring,
    Bolt,
    Smoke,
    Swarm,
    Explode,
    Lightning,
    Fly,
    Twinkle,
    PfxJet,
}

/// `fx->Draw` function pointer. `Twinkle` is `UpdateFXTwinkle` used as a draw function
/// (health flies): it runs during `DrawSpecialFX` and draws `Random()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FxDraw {
    Decal,
    Ring,
    Lightning,
    Twinkle,
}

/// Index into the `PLANE2` pool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Plane2Id(pub u32);

/// `PLANE2` (`types.h:564`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plane2 {
    pub distance: Fixed,
    pub j: Fixed,
    pub point: SVector,
    pub normal: FVector,
}

/// `SPECFX` (`specfx.h:107`). Texture fields are CRCs (`FindTexture`), 0 = `NULL`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecFx {
    pub normal: FVector,
    pub vel: FVector,
    pub scale: FVector,
    pub origin: SVector,
    pub rebound: Option<Plane2Id>,
    /// First particle of the effect's block (`numP` particles via `next`).
    pub particles: Option<ParticleId>,
    /// First sprite of the effect's block (`numP` sprites via `next`).
    pub sprites: Option<SpriteId>,
    pub tex: u32,
    /// `follow` (`ACTOR*`, identified by its `ACTOR2`).
    pub follow: Option<ActorId>,
    /// `act` (`ACTOR2**`, `numP` entries); `None` = `NULL`.
    pub act: Option<Vec<Option<ActorId>>>,
    pub ty: i16,
    pub start: i16,
    pub end: i16,
    pub angle: i16,
    pub spin: i16,
    pub speed: Fixed,
    pub accn: Fixed,
    pub tilt: Fixed,
    pub gravity: Fixed,
    pub lifetime: i32,
    pub num_p: i32,
    pub fade: i32,
    pub fade_alpha: i32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    pub update_type: i8,
    pub update: Option<FxUpdate>,
    pub draw: Option<FxDraw>,
}

/// The `txtr*` texture handles found by `InitSpecFXList` (CRCs of `name.bmp`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FxTextures {
    pub txtr_water_ripples: u32,
    pub txtr_ripple: u32,
    pub txtr_star: u32,
    pub txtr_solid_ring: u32,
    pub txtr_smoke: u32,
    pub txtr_fire: u32,
    pub txtr_smoke2: u32,
    pub txtr_ring: u32,
    pub txtr_bubble: u32,
    pub txtr_blank: u32,
    pub txtr_trail: u32,
    pub txtr_flash: u32,
    pub txtr_flare: u32,
    pub txtr_electric: u32,
    pub txtr_croak: u32,
    pub txtr_warpflash: u32,
}

/// `FindTexture(name)` (PC/pcmisc.cpp): CRC of the lower-case `name.bmp`.
pub fn find_texture(name: &str) -> u32 {
    f2_formats::crc::crc(&format!("{}.bmp", name.to_ascii_lowercase()))
}

/// The renderer's view transform used by `IsPointVisible` / `OutcodeCheck` (PC):
/// `vMatrix` (row-vector convention of `guMtxXFMF`) plus the projection globals of
/// `mdxRender.cpp`. Set by the host after each drawn frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewXform {
    pub matrix: [[f32; 4]; 4],
    pub x_res: f32,
    pub y_res: f32,
    pub near_clip: f32,
    pub far_clip: f32,
    pub horiz_clip: f32,
    pub vert_clip: f32,
    /// `FOV` (450).
    pub fov: i32,
    /// `DIST`.
    pub dist: i32,
}

impl ViewXform {
    fn xfm(&self, p: &SVector) -> (f32, f32, f32) {
        let (x, y, z) = (p.vx as f32 * 0.1, p.vy as f32 * 0.1, p.vz as f32 * 0.1);
        let m = &self.matrix;
        (
            m[0][0] * x + m[1][0] * y + m[2][0] * z + m[3][0],
            m[0][1] * x + m[1][1] * y + m[2][1] * z + m[3][1],
            m[0][2] * x + m[1][2] * y + m[2][2] * z + m[3][2],
        )
    }

    /// `oozd = -FOV * oneOver[fftol(z) + DIST]` with `oneOver[i] = (xRes/640)/i`.
    fn oozd(&self, z: f32) -> f32 {
        let i = (z as i32).wrapping_add(self.dist);
        if i <= 0 {
            return 0.0;
        }
        -(self.fov as f32) * ((self.x_res / 640.0) / i as f32)
    }

    /// Port of `XfmPoint` (MDX `mdxRender.cpp:331`) for `d == NULL`: screen point, or
    /// `vz == 0` when clipped.
    pub fn xfm_point(&self, p: &SVector) -> (f32, f32, f32) {
        let (x, y, z) = self.xfm(p);
        let d = self.dist as f32;
        if z + d > self.near_clip && z + d < self.far_clip && x > -self.horiz_clip && x < self.horiz_clip && y > -self.vert_clip && y < self.vert_clip {
            let o = self.oozd(z);
            (self.x_res / 2.0 + x * o, self.y_res / 2.0 + y * o, z)
        } else {
            (x, y, 0.0)
        }
    }

    /// Port of `IsPointVisible` (Common/multi.c:1694, PC).
    pub fn is_point_visible(&self, p: &SVector) -> bool {
        let (x, y, z) = self.xfm_point(p);
        !(z == 0.0 || x < 0.0 || x > self.x_res || y < 0.0 || y > self.y_res)
    }

    fn outcode(&self, p: &SVector) -> u32 {
        let (mut x, mut y, z) = self.xfm(p);
        let mut oc = 0;
        if z < self.near_clip {
            oc |= 0x10;
        } else if z > self.far_clip {
            oc |= 0x20;
        } else {
            let o = self.oozd(z);
            x = self.x_res / 2.0 + x * o;
            y = self.y_res / 2.0 + y * o;
            if x < 0.0 {
                oc |= 0x4;
            } else if x > self.x_res {
                oc |= 0x8;
            }
            if y < 0.0 {
                oc |= 0x1;
            } else if y > self.y_res {
                oc |= 0x2;
            }
        }
        oc
    }

    /// Port of `OutcodeCheck` (PC/pcmisc.cpp:439): 1 when both points are off the same
    /// side of the screen.
    pub fn outcode_check(&self, p1: &SVector, p2: &SVector) -> bool {
        let oc1 = self.outcode(p1);
        if oc1 == 0 {
            return false;
        }
        let oc2 = self.outcode(p2);
        if oc2 == 0 {
            return false;
        }
        oc1 & oc2 != 0
    }
}

const NIL: u32 = u32::MAX;
const HEAD: u32 = MAX_SPECFX as u32;

/// Globals of `specfx.c` (`sfxList`, `p2List`, `ringVtx`, `specFXAlpha`, `txtr*`, ...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpecFxState {
    /// `sfxList.array` (+ the head at index `MAX_SPECFX`); empty = not allocated.
    pub array: Vec<SpecFx>,
    next: Vec<u32>,
    prev: Vec<u32>,
    stack: Vec<u32>,
    pub stack_ptr: i32,
    pub count: i32,
    /// `sfxList.lastAdded[NUM_FXUPDATES]` (may hold the head index).
    last_added: Vec<u32>,
    /// `p2List`.
    pub p2: Vec<Plane2>,
    p2_stack: Vec<u32>,
    pub p2_stack_ptr: i32,
    pub p2_count: i32,
    /// `ringVtx` (`(NUM_RINGSEGS+1)*2` vertices once created).
    pub ring_vtx: Option<Vec<FVector>>,
    /// `specFXAlpha`.
    pub spec_fx_alpha: i32,
    /// `static char rot` of `CreateSpecialEffectDirect`.
    pub rot: i8,
    /// `pauseMode`.
    pub pause_mode: i8,
    pub textures: FxTextures,
    /// The host's view transform for `IsPointVisible`/`OutcodeCheck` (None = all visible).
    pub view: Option<ViewXform>,
}

impl Default for SpecFxState {
    fn default() -> Self {
        SpecFxState {
            array: Vec::new(),
            next: Vec::new(),
            prev: Vec::new(),
            stack: Vec::new(),
            stack_ptr: 0,
            count: 0,
            last_added: Vec::new(),
            p2: Vec::new(),
            p2_stack: Vec::new(),
            p2_stack_ptr: 0,
            p2_count: 0,
            ring_vtx: None,
            spec_fx_alpha: 255,
            rot: 0,
            pause_mode: 0,
            textures: FxTextures::default(),
            view: None,
        }
    }
}

impl SpecFxState {
    pub fn fx(&self, id: SpecFxId) -> &SpecFx {
        &self.array[id.idx()]
    }

    pub fn fx_mut(&mut self, id: SpecFxId) -> &mut SpecFx {
        &mut self.array[id.idx()]
    }

    pub(crate) fn p2(&self, id: Plane2Id) -> &Plane2 {
        &self.p2[id.0 as usize]
    }

    pub(crate) fn p2_mut(&mut self, id: Plane2Id) -> &mut Plane2 {
        &mut self.p2[id.0 as usize]
    }

    /// Whether `id` is a live effect (linked into `sfxList`).
    pub fn is_live(&self, id: SpecFxId) -> bool {
        self.next.get(id.idx()).is_some_and(|&n| n != NIL) && id.0 != HEAD
    }

    /// Live effects in list order (`sfxList.head.next` → ...).
    pub fn iter_list(&self) -> impl Iterator<Item = (SpecFxId, &SpecFx)> + '_ {
        let mut c = if self.array.is_empty() { HEAD } else { self.next[HEAD as usize] };
        std::iter::from_fn(move || {
            if self.array.is_empty() || c == HEAD || c == NIL {
                return None;
            }
            let id = c;
            c = self.next[c as usize];
            Some((SpecFxId(id), &self.array[id as usize]))
        })
    }

    fn ids(&self) -> Vec<u32> {
        self.iter_list().map(|(i, _)| i.0).collect()
    }

    /// Port of `AllocateP2` (Common/specfx.c:1651).
    pub fn allocate_p2(&mut self) -> Option<Plane2Id> {
        if self.p2.is_empty() || self.p2_stack_ptr == 0 || self.p2_count >= MAX_PLANES as i32 {
            return None;
        }
        self.p2_stack_ptr -= 1;
        self.p2_count += 1;
        Some(Plane2Id(self.p2_stack[(self.p2_stack_ptr + 1) as usize]))
    }

    /// Port of `DeallocateP2` (Common/specfx.c:1733).
    pub fn deallocate_p2(&mut self, p: Option<Plane2Id>) {
        let Some(p) = p else { return };
        if self.p2.is_empty() || self.p2_stack_ptr >= MAX_PLANES as i32 {
            return;
        }
        self.p2_count -= 1;
        self.p2_stack_ptr += 1;
        // BUG(original): with stackPtr == maxPlanes-1 this writes stack[maxPlanes], one
        // past the array; the port ignores that write.
        if let Some(slot) = self.p2_stack.get_mut(self.p2_stack_ptr as usize) {
            *slot = p.0;
        }
        self.p2[p.0 as usize] = Plane2::default();
    }

    /// Port of `AllocateFX` (Common/specfx.c:1615).
    pub fn allocate_fx(&mut self, number: i32, ty: i32) -> Option<SpecFxId> {
        if self.array.is_empty() || number <= 0 || self.stack_ptr - number < 0 || number >= MAX_SPECFX as i32 - self.count {
            return None;
        }
        // updateMapping[type] reads out of bounds for type >= PTYPE_NUMTYPES.
        let update = *UPDATE_MAPPING.get(usize::try_from(ty).ok()?)?;
        let mut n = number;
        while n > 0 {
            n -= 1;
            let s = self.stack[self.stack_ptr as usize];
            self.stack_ptr -= 1;
            let la = self.last_added[update as usize];
            let ptr = if la != NIL { la } else { self.next[HEAD as usize] };
            self.last_added[update as usize] = s;
            self.array[s as usize].update_type = update;
            let pp = self.prev[ptr as usize];
            self.prev[s as usize] = pp;
            self.next[s as usize] = ptr;
            self.next[pp as usize] = s;
            self.prev[ptr as usize] = s;
            self.count += 1;
        }
        Some(SpecFxId(self.stack[(self.stack_ptr + 1) as usize]))
    }
}

/// `Random(30)-10` style velocity jitter helper: `v += (Random(30)-10)*k` on x, y, z.
fn jitter(g: &mut Game, v: &mut FVector, k: i32) {
    v.vx = v.vx.wrapping_add((g.rng.random(30) - 10).wrapping_mul(k));
    v.vy = v.vy.wrapping_add((g.rng.random(30) - 10).wrapping_mul(k));
    v.vz = v.vz.wrapping_add((g.rng.random(30) - 10).wrapping_mul(k));
}

impl Game {
    // ------------------------------------------------------------------ lists

    /// Port of `InitSpecFXList` (Common/specfx.c:1494).
    pub fn init_spec_fx_list(&mut self) {
        let s = &mut self.specfx;
        let n = MAX_SPECFX;
        s.last_added = vec![NIL; NUM_FXUPDATES];
        s.array = vec![SpecFx::default(); n + 1];
        s.next = vec![NIL; n + 1];
        s.prev = vec![NIL; n + 1];
        s.next[n] = HEAD;
        s.prev[n] = HEAD;
        s.stack = (0..n as u32).collect();
        s.count = 0;
        s.stack_ptr = n as i32 - 1;

        s.p2 = vec![Plane2::default(); MAX_PLANES];
        s.p2_stack = (0..MAX_PLANES as u32).collect();
        s.p2_count = 0;
        s.p2_stack_ptr = MAX_PLANES as i32 - 1;

        s.textures = FxTextures {
            txtr_water_ripples: find_texture("AI_RIPPLE"),
            txtr_ripple: find_texture("AI_RIPPLE2"),
            txtr_star: find_texture("STAR_OUTLINE"),
            txtr_solid_ring: find_texture("AI_CIRCLE"),
            txtr_smoke: find_texture("00SMOK07"),
            txtr_fire: find_texture("00SMOK07"),
            txtr_smoke2: find_texture("AI_SMOKE"),
            txtr_ring: find_texture("AI_RING"),
            txtr_bubble: find_texture("WATDROP"),
            txtr_blank: find_texture("AI_FULLWHITE"),
            txtr_trail: find_texture("AI_TRAIL"),
            txtr_flash: find_texture("FLASH"),
            txtr_flare: find_texture("FLASH2"),
            txtr_electric: find_texture("ELECTRIC"),
            txtr_croak: find_texture("AI_CROAK"),
            txtr_warpflash: find_texture("AI_WARPFLASH"),
        };
    }

    /// Port of `FreeSpecFXList` (Common/specfx.c:1565).
    pub fn free_spec_fx_list(&mut self) {
        if !self.specfx.array.is_empty() {
            let first = self.specfx.next[HEAD as usize];
            let count = self.specfx.count;
            self.deallocate_fx(Some(SpecFxId(first)), count);
            self.specfx.array.clear();
            self.specfx.next.clear();
            self.specfx.prev.clear();
        }
        self.specfx.stack.clear();
        self.specfx.last_added.clear();
        self.specfx.count = 0;
        self.specfx.stack_ptr = 0;
        self.specfx.p2.clear();
        self.specfx.p2_stack.clear();
        self.specfx.p2_count = 0;
        self.specfx.p2_stack_ptr = 0;
    }

    /// Port of `DeallocateFX` (Common/specfx.c:1671).
    pub fn deallocate_fx(&mut self, head: Option<SpecFxId>, number: i32) {
        let Some(h) = head else { return };
        let mut fx = h.0;
        {
            let s = &self.specfx;
            if s.array.is_empty() || fx == HEAD || s.next[fx as usize] == NIL || number <= 0 || s.stack_ptr + number >= MAX_SPECFX as i32 {
                return;
            }
        }
        let mut n = number;
        while n > 0 {
            n -= 1;
            if fx == HEAD || fx == NIL {
                return;
            }
            let t = self.specfx.next[fx as usize];
            {
                let s = &mut self.specfx;
                let ut = s.array[fx as usize].update_type as usize;
                if s.last_added[ut] == fx {
                    let nx = s.next[fx as usize];
                    // The static head has updateType 0 (FXUPDATE_DECAL).
                    let nut = if nx == HEAD { 0 } else { s.array[nx as usize].update_type as usize };
                    s.last_added[ut] = if nut == ut { nx } else { NIL };
                }
                let (p, nx) = (s.prev[fx as usize], s.next[fx as usize]);
                s.next[p as usize] = nx;
                s.prev[nx as usize] = p;
            }
            let e = self.specfx.array[fx as usize].clone();
            if e.sprites.is_some() {
                self.sprite.deallocate_sprites(e.sprites, e.num_p);
            }
            if e.particles.is_some() {
                self.particle.deallocate_particles(e.particles, e.num_p);
            }
            if e.rebound.is_some() {
                self.specfx.deallocate_p2(e.rebound);
            }
            if let Some(acts) = &e.act {
                for i in 0..e.num_p.max(0) as usize {
                    if let Some(Some(a)) = acts.get(i) {
                        self.actor2_free(*a);
                    }
                }
            }
            let s = &mut self.specfx;
            s.count -= 1;
            s.stack_ptr += 1;
            s.stack[s.stack_ptr as usize] = fx;
            s.array[fx as usize] = SpecFx::default();
            s.next[fx as usize] = NIL;
            s.prev[fx as usize] = NIL;
            fx = t;
        }
    }

    /// The `numP` sprites of an effect (renderer helper).
    pub fn fx_sprites(&self, fx: SpecFxId) -> Vec<SpriteId> {
        let e = self.specfx.fx(fx);
        self.sprite.chain(e.sprites, e.num_p)
    }

    /// The `numP` particles of an effect (renderer helper; lightning polys, ...).
    pub fn fx_particles(&self, fx: SpecFxId) -> Vec<ParticleId> {
        let e = self.specfx.fx(fx);
        self.particle.chain(e.particles, e.num_p)
    }

    fn follow_pos(&self, fx: SpecFxId) -> Option<SVector> {
        self.specfx.fx(fx).follow.and_then(|a| self.actor2.actors.get(a)).map(|a| a.actor.position)
    }

    /// `if( fx->follow ) SetVectorSS( &fx->origin, &fx->follow->position );`
    fn follow_origin(&mut self, fx: SpecFxId) {
        if let Some(p) = self.follow_pos(fx) {
            self.specfx.fx_mut(fx).origin = p;
        }
    }

    // ------------------------------------------------------------------ creation

    /// `IsPointVisible(pos)` through the host's view transform (always visible without one).
    pub fn is_point_visible(&self, pos: &SVector) -> bool {
        self.specfx.view.as_ref().is_none_or(|v| v.is_point_visible(pos))
    }

    /// Port of `CreateSpecialEffect` (Common/specfx.c:167).
    #[allow(clippy::too_many_arguments)]
    pub fn create_special_effect(
        &mut self,
        ty: i16,
        origin: &SVector,
        normal: &FVector,
        size: Fixed,
        speed: Fixed,
        accn: Fixed,
        lifetime: Fixed,
    ) -> Option<SpecFxId> {
        if !self.is_point_visible(origin) {
            return None;
        }
        self.create_special_effect_direct(ty, origin, normal, size, speed, accn, lifetime)
    }

    /// Port of `CreateSpecialEffectDirect` (Common/specfx.c:181).
    #[allow(clippy::too_many_arguments)]
    pub fn create_special_effect_direct(
        &mut self,
        ty: i16,
        origin: &SVector,
        normal: &FVector,
        size: Fixed,
        speed: Fixed,
        accn: Fixed,
        lifetime: Fixed,
    ) -> Option<SpecFxId> {
        let life = ((lifetime.wrapping_mul(60)) >> 12).max(1);
        let id = self.specfx.allocate_fx(1, ty as i32)?;
        let afc = self.act_frame_count;
        let tx = self.specfx.textures;
        {
            let alpha = self.specfx.spec_fx_alpha as u8;
            let e = self.specfx.fx_mut(id);
            e.ty = ty;
            set_vector_ss(&mut e.origin, origin);
            set_vector_ff(&mut e.normal, normal);
            e.scale.vx = size.wrapping_mul(SCALE);
            e.scale.vy = size.wrapping_mul(SCALE);
            e.scale.vz = size.wrapping_mul(SCALE);
            e.speed = speed.wrapping_mul(SCALE);
            e.accn = accn.wrapping_mul(SCALE);
            e.lifetime = (afc as i32).wrapping_add(life);
            e.r = 255;
            e.g = 255;
            e.b = 255;
            e.a = alpha;
        }

        // Particle system type
        if ty >= FXTYPE_NUMTYPES {
            return self.create_particle_effect(id);
        }

        match ty {
            FXTYPE_WAKE | FXTYPE_DECAL | FXTYPE_GARIBCOLLECT => {
                let e = self.specfx.fx_mut(id);
                e.fade = e.a as i32 / life;
                offset_origin(e);
                if e.ty == FXTYPE_WAKE {
                    e.tex = tx.txtr_ripple;
                } else if e.ty == FXTYPE_WATERRIPPLE {
                    e.r = 200;
                    e.g = 200;
                    e.tex = tx.txtr_water_ripples;
                } else if e.ty == FXTYPE_GARIBCOLLECT {
                    e.b = 0;
                    e.spin = 5;
                    e.tex = tx.txtr_star;
                } else {
                    e.tex = tx.txtr_solid_ring;
                }
                e.update = Some(FxUpdate::Decal);
                e.draw = Some(FxDraw::Decal);
            }
            FXTYPE_POLYRING | FXTYPE_FROGSHIELD | FXTYPE_CROAK => {
                if self.specfx.ring_vtx.is_none() {
                    self.create_blast_ring();
                }
                let e = self.specfx.fx_mut(id);
                e.fade = ((e.a as i32) << 12) / life;
                e.fade_alpha = (e.a as i32) << 12;
                offset_origin(e);
                e.scale.vy >>= 3;
                e.tilt = 3686;
                e.tex = if e.ty == FXTYPE_FROGSHIELD {
                    tx.txtr_flash
                } else if e.ty == FXTYPE_CROAK {
                    tx.txtr_croak
                } else {
                    tx.txtr_blank
                };
                e.update = Some(FxUpdate::Ring);
                e.draw = Some(FxDraw::Ring);
            }
            FXTYPE_BATSWARM | FXTYPE_BUTTERFLYSWARM | FXTYPE_SPACETHING1 => {
                let n = lifetime >> 12; // Nasty Nasty Nasty
                self.specfx.fx_mut(id).num_p = n;
                let Some(spr) = self.sprite.allocate_sprites(n) else {
                    self.deallocate_fx(Some(id), 1);
                    return None;
                };
                self.specfx.fx_mut(id).sprites = Some(spr);
                let Some(parts) = self.particle.allocate_particles(n) else {
                    self.deallocate_fx(Some(id), 1);
                    return None;
                };
                self.specfx.fx_mut(id).particles = Some(parts);
                self.specfx.fx_mut(id).tex = tx.txtr_flash;
                let e = self.specfx.fx(id).clone();
                let ss = self.sprite.chain(Some(spr), n);
                let ps = self.particle.chain(Some(parts), n);
                for (s, p) in ss.into_iter().zip(ps) {
                    {
                        let sp = self.sprite.get_mut(s);
                        sp.texture = e.tex;
                        set_vector_ss(&mut sp.pos, &e.origin);
                        sp.scale_x = ((e.scale.vx >> 12) / SCALE) as i16;
                        sp.scale_y = ((e.scale.vy >> 12) / SCALE) as i16;
                        sp.r = e.r;
                        sp.g = e.g;
                        sp.b = e.b;
                        sp.a = e.a;
                        sp.offset_x = -16;
                        sp.offset_y = -16;
                        sp.flags = SPRITE_TRANSLUCENT | SPRITE_ADDITIVE;
                    }
                    let px = (-80 + self.rng.random(160)) as i16;
                    let py = (-60 + self.rng.random(120)) as i16;
                    let pz = (-80 + self.rng.random(160)) as i16;
                    let vx = -4096 + self.rng.random(8192);
                    let vy = -4096 + self.rng.random(8192);
                    let vz = -4096 + self.rng.random(8192);
                    let pp = self.particle.get_mut(p);
                    pp.pos = SVector::new(px, py, pz);
                    pp.vel = FVector::new(vx, vy, vz);
                }
                let e = self.specfx.fx_mut(id);
                e.update = Some(FxUpdate::Swarm);
                e.draw = None;
            }
            FXTYPE_HEALTHFLY => {
                self.specfx.fx_mut(id).num_p = 1;
                self.specfx.fx_mut(id).act = Some(vec![None]);
                let name = self.collect.pickup_obj_name.clone();
                let Some(a) = self.create_and_add_actor(&name, 0, 0, 0, crate::mdx_actor::INIT_ANIMATION) else {
                    self.deallocate_fx(Some(id), 1);
                    return None;
                };
                self.specfx.fx_mut(id).act = Some(vec![Some(a)]);
                let Some(spr) = self.sprite.allocate_sprites(1) else {
                    self.deallocate_fx(Some(id), 1);
                    return None;
                };
                self.specfx.fx_mut(id).sprites = Some(spr);
                self.actor_animate(a, 0, true, false, 256, false);
                {
                    let act = &mut self.actor2.actors[a].actor;
                    act.size = FVector::new(size, size, size);
                }
                let vx = (-80 + self.rng.random(160)) << 12;
                let vy = (-60 + self.rng.random(120)) << 12;
                let vz = (-80 + self.rng.random(160)) << 12;
                let e = self.specfx.fx_mut(id);
                e.vel = FVector::new(vx, vy, vz);
                let (origin, vel, normal, espeed) = (e.origin, e.vel, e.normal, e.speed);
                {
                    let act = &mut self.actor2.actors[a].actor;
                    add_vector_ssf(&mut act.position, &origin, &vel);
                    set_vector_sf(&mut act.rotaim, &normal);
                    scale_vector_s(&mut act.rotaim, 50);
                    set_vector_ff(&mut act.vel, &f2_math::IN_VEC);
                    scale_vector_ff(&mut act.vel, espeed);
                }
                let e = self.specfx.fx_mut(id);
                e.update = Some(FxUpdate::Fly);
                e.scale.vx /= SCALE;
                e.scale.vy /= SCALE;
                e.scale.vz /= SCALE;
                e.fade = 0;
                e.tilt = 4096;
                e.follow = Some(a);
                // bwahahahahahaha
                e.draw = Some(FxDraw::Twinkle);
                let flare = tx.txtr_flare;
                let s = self.sprite.get_mut(spr);
                s.a = 128;
                s.texture = flare;
                s.r = 255;
                s.g = 255;
                s.b = 255;
                s.offset_x = -20;
                s.offset_y = -20;
                s.flags = SPRITE_TRANSLUCENT | SPRITE_ADDITIVE;
                s.scale_x = 80;
                s.scale_y = 80;
                set_vector_ss(&mut s.pos, &origin);
            }
            FXTYPE_SMOKE_STATIC | FXTYPE_SMOKE_GROWS | FXTYPE_BUBBLES => {
                // Velocity is normal scaled by speed, plus a random offset scaled by speed
                let (n, e_speed) = {
                    let e = self.specfx.fx_mut(id);
                    e.accn /= SCALE;
                    set_vector_ff(&mut e.vel, &e.normal.clone());
                    scale_vector_ff(&mut e.vel, e.speed);
                    (e.normal, e.speed)
                };
                let _ = n;
                let tmp = e_speed >> 5;
                let mut vel = self.specfx.fx(id).vel;
                jitter(self, &mut vel, tmp);
                let n = {
                    let e = self.specfx.fx_mut(id);
                    e.vel = vel;
                    e.fade = e.a as i32 / life;
                    e.num_p = if e.ty == FXTYPE_BUBBLES { 1 } else { 2 };
                    e.num_p
                };
                let Some(spr) = self.sprite.allocate_sprites(n) else {
                    self.deallocate_fx(Some(id), 1);
                    return None;
                };
                self.specfx.fx_mut(id).sprites = Some(spr);
                let e = self.specfx.fx(id).clone();
                for s in self.sprite.chain(Some(spr), n) {
                    {
                        let sp = self.sprite.get_mut(s);
                        sp.texture = if e.ty == FXTYPE_BUBBLES { tx.txtr_bubble } else { tx.txtr_smoke };
                        set_vector_ss(&mut sp.pos, &e.origin);
                        sp.r = e.r;
                        sp.g = e.g;
                        sp.b = e.b;
                        sp.a = e.a;
                        sp.offset_x = -16;
                        sp.offset_y = -16;
                        sp.flags = SPRITE_TRANSLUCENT;
                        if ty == FXTYPE_BUBBLES {
                            sp.flags |= SPRITE_ADDITIVE;
                        }
                    }
                    if e.ty == FXTYPE_SMOKE_GROWS || e.ty == FXTYPE_SMOKE_STATIC {
                        let sx = ((e.scale.vx / SCALE) >> 12).wrapping_add(self.rng.random(21) - 10) as i16;
                        let sy = ((e.scale.vy / SCALE) >> 12).wrapping_add(self.rng.random(21) - 10) as i16;
                        let inc = 25 + self.rng.random(40);
                        let rot = self.specfx.rot;
                        let sp = self.sprite.get_mut(s);
                        sp.scale_x = sx;
                        sp.scale_y = sy;
                        sp.flags |= SPRITE_FLAGS_ROTATE | SPRITE_ADDITIVE;
                        sp.angle = 0;
                        sp.angle_inc = inc as i16;
                        if rot != 0 {
                            sp.angle_inc = sp.angle_inc.wrapping_neg();
                        }
                    } else {
                        let sp = self.sprite.get_mut(s);
                        sp.scale_x = ((e.scale.vx / SCALE) >> 12) as i16;
                        sp.scale_y = ((e.scale.vy / SCALE) >> 12) as i16;
                    }
                    self.specfx.rot = (self.specfx.rot == 0) as i8;
                }
                let e = self.specfx.fx_mut(id);
                e.update = Some(FxUpdate::Smoke);
                e.draw = None;
            }
            FXTYPE_SMOKEBURST | FXTYPE_SPLASH | FXTYPE_SPARKBURST | FXTYPE_SPARKLYTRAIL | FXTYPE_FIERYSMOKE => {
                let n = 5;
                {
                    let e = self.specfx.fx_mut(id);
                    e.num_p = n;
                    e.accn /= SCALE;
                }
                let Some(spr) = self.sprite.allocate_sprites(n) else {
                    self.deallocate_fx(Some(id), 1);
                    return None;
                };
                self.specfx.fx_mut(id).sprites = Some(spr);
                let Some(parts) = self.particle.allocate_particles(n) else {
                    self.deallocate_fx(Some(id), 1);
                    return None;
                };
                self.specfx.fx_mut(id).particles = Some(parts);
                if let Some(p2) = self.specfx.allocate_p2() {
                    let e = self.specfx.fx(id).clone();
                    let pl = self.specfx.p2_mut(p2);
                    set_vector_ss(&mut pl.point, &e.origin);
                    set_vector_ff(&mut pl.normal, &e.normal);
                    self.specfx.fx_mut(id).rebound = Some(p2);
                }
                {
                    let e = self.specfx.fx_mut(id);
                    match e.ty {
                        FXTYPE_SPLASH => e.tex = tx.txtr_bubble,
                        FXTYPE_SPARKBURST => e.tex = tx.txtr_solid_ring,
                        FXTYPE_SPARKLYTRAIL => e.tex = tx.txtr_flash,
                        FXTYPE_SMOKEBURST => e.tex = tx.txtr_smoke,
                        FXTYPE_FIERYSMOKE => e.tex = tx.txtr_fire,
                        _ => {}
                    }
                    e.fade = e.a as i32 / life.max(1);
                }
                let e = self.specfx.fx(id).clone();
                let tmp = e.speed / 30;
                let ss = self.sprite.chain(Some(spr), n);
                let ps = self.particle.chain(Some(parts), n);
                let mut i = n;
                for (s, p) in ss.into_iter().zip(ps) {
                    i -= 1;
                    {
                        let sp = self.sprite.get_mut(s);
                        set_vector_ss(&mut sp.pos, &e.origin);
                        sp.r = e.r;
                        sp.g = e.g;
                        sp.b = e.b;
                        sp.a = e.a;
                        sp.offset_x = -16;
                        sp.offset_y = -16;
                        sp.flags = SPRITE_TRANSLUCENT | SPRITE_ADDITIVE;
                    }
                    let pp = self.particle.get_mut(p);
                    pp.bounce = (e.ty == FXTYPE_SPARKBURST || e.ty == FXTYPE_SPARKLYTRAIL) as u8;
                    set_vector_ff(&mut pp.vel, &e.normal);
                    scale_vector_ff(&mut pp.vel, e.speed);
                    let mut vel = pp.vel;

                    if e.ty == FXTYPE_SMOKEBURST || e.ty == FXTYPE_FIERYSMOKE {
                        self.sprite.get_mut(s).flags |= SPRITE_FLAGS_ROTATE;
                        let sx = ((e.scale.vx / SCALE) >> 12).wrapping_add(self.rng.random(21)).wrapping_sub(10) as i16;
                        let sy = ((e.scale.vy / SCALE) >> 12).wrapping_add(self.rng.random(21)).wrapping_sub(10) as i16;
                        jitter(self, &mut vel, tmp >> 1);
                        let inc = 25 + self.rng.random(40);
                        let sp = self.sprite.get_mut(s);
                        sp.scale_x = sx;
                        sp.scale_y = sy;
                        sp.texture = e.tex;
                        sp.angle = 0;
                        sp.angle_inc = inc as i16;
                        if i & 1 != 0 {
                            sp.angle_inc = sp.angle_inc.wrapping_neg();
                        }
                    } else {
                        jitter(self, &mut vel, tmp);
                        let sp = self.sprite.get_mut(s);
                        sp.scale_x = ((e.scale.vx >> 12) / SCALE) as i16;
                        sp.scale_y = ((e.scale.vy >> 12) / SCALE) as i16;
                        sp.texture = e.tex;
                    }
                    self.particle.get_mut(p).vel = vel;

                    if e.ty == FXTYPE_FIERYSMOKE {
                        {
                            let sp = self.sprite.get_mut(s);
                            sp.g = 180;
                            sp.b = 0;
                        }
                        if self.rng.random(3) == 0 {
                            let sp = self.sprite.get_mut(s);
                            sp.flags &= !SPRITE_ADDITIVE;
                            sp.texture = tx.txtr_smoke2;
                        }
                    }
                }
                let e = self.specfx.fx_mut(id);
                e.update = Some(FxUpdate::Explode);
                e.draw = None;
            }
            FXTYPE_LIGHTNING => {
                let n = {
                    let e = self.specfx.fx_mut(id);
                    e.speed /= SCALE;
                    e.num_p = ((e.speed / 240) >> 12).max(4);
                    e.num_p
                };
                let Some(parts) = self.particle.allocate_particles(n) else {
                    self.deallocate_fx(Some(id), 1);
                    return None;
                };
                self.specfx.fx_mut(id).particles = Some(parts);
                self.specfx.fx_mut(id).tex = tx.txtr_electric;
                let e = self.specfx.fx(id).clone();
                for p in self.particle.chain(Some(parts), n) {
                    let pp = self.particle.get_mut(p);
                    set_vector_ss(&mut pp.pos, &e.origin);
                    pp.r = e.r;
                    pp.g = e.g;
                    pp.b = e.b;
                    pp.a = e.a;
                }
                let e = self.specfx.fx_mut(id);
                e.update = Some(FxUpdate::Lightning);
                e.draw = Some(FxDraw::Lightning);
            }
            FXTYPE_GLOW | FXTYPE_TWINKLE | FXTYPE_FLASH => {
                self.specfx.fx_mut(id).num_p = 1;
                let Some(spr) = self.sprite.allocate_sprites(1) else {
                    self.deallocate_fx(Some(id), 1);
                    return None;
                };
                let e = self.specfx.fx_mut(id);
                e.sprites = Some(spr);
                e.scale.vx /= SCALE;
                e.scale.vy /= SCALE;
                e.scale.vz /= SCALE;
                e.fade = if life != 0 { e.a as i32 / life } else { 0 };
                if e.ty == FXTYPE_FLASH {
                    e.tilt = afc as i32;
                }
                e.update = Some(FxUpdate::Twinkle);
                e.draw = None;
                let (ety, origin) = (e.ty, e.origin);
                let s = self.sprite.get_mut(spr);
                s.a = 200;
                if ety == FXTYPE_GLOW {
                    s.texture = tx.txtr_flare;
                    s.a = 128;
                } else if ety == FXTYPE_FLASH {
                    s.texture = tx.txtr_warpflash;
                } else {
                    s.texture = tx.txtr_flash;
                }
                s.r = 255;
                s.g = 255;
                s.b = 255;
                s.offset_x = -16;
                s.offset_y = -16;
                s.flags = SPRITE_TRANSLUCENT | SPRITE_ADDITIVE;
                s.scale_x = (size >> 12) as i16;
                s.scale_y = (size >> 12) as i16;
                set_vector_ss(&mut s.pos, &origin);
                if ety == FXTYPE_FLASH {
                    s.scale_y = s.scale_y.wrapping_mul(5);
                }
            }
            FXTYPE_GREENGLOOP => {
                // Create a bubble, but the effect persists after the bubbles are gone, to
                // create a wait time.
                let mut vel = FVector::default();
                set_vector_ff(&mut vel, normal);
                scale_vector_ff(&mut vel, speed);
                let k = fmul(speed, 1638);
                vel.vx = vel.vx.wrapping_add(fmul(-4096 + (self.rng.random(30) << 12), k));
                vel.vy = vel.vy.wrapping_add(fmul(-4096 + (self.rng.random(30) << 12), k));
                vel.vz = vel.vz.wrapping_add(fmul(-4096 + (self.rng.random(30) << 12), k));
                {
                    let e = self.specfx.fx_mut(id);
                    e.vel = vel;
                    e.fade = e.a as i32 / life;
                    e.num_p = 1;
                }
                let Some(spr) = self.sprite.allocate_sprites(1) else {
                    self.deallocate_fx(Some(id), 1);
                    return None;
                };
                let e = self.specfx.fx_mut(id);
                e.sprites = Some(spr);
                e.update = Some(FxUpdate::Smoke);
                e.draw = None;
                let e = e.clone();
                let s = self.sprite.get_mut(spr);
                s.texture = tx.txtr_bubble;
                set_vector_ss(&mut s.pos, &e.origin);
                s.scale_x = 22;
                s.scale_y = 22;
                s.r = e.r;
                s.g = e.g;
                s.b = e.b;
                s.a = e.a;
                s.offset_x = -16;
                s.offset_y = -16;
                s.flags = SPRITE_TRANSLUCENT;
            }
            _ => {
                // FXTYPE_CUSTOM, WATERRIPPLE, FROGSTUN, LASER, TRAIL, BILLBOARDTRAIL: their
                // cases are commented out in the final source.
                self.deallocate_fx(Some(id), 1);
                return None;
            }
        }
        Some(id)
    }

    // ------------------------------------------------------------------ update

    /// Port of `UpdateSpecialEffects` (Common/specfx.c:814).
    pub fn update_special_effects(&mut self) {
        if self.specfx.array.is_empty() {
            return;
        }
        let mut fx1 = self.specfx.next[HEAD as usize];
        while fx1 != HEAD && fx1 != NIL {
            let fx2 = self.specfx.next[fx1 as usize];
            if let Some(u) = self.specfx.array[fx1 as usize].update {
                self.run_fx_update(SpecFxId(fx1), u);
            }
            fx1 = fx2;
        }
    }

    fn run_fx_update(&mut self, fx: SpecFxId, u: FxUpdate) {
        match u {
            FxUpdate::Decal => self.update_fx_decal(fx),
            FxUpdate::Ring => self.update_fx_ring(fx),
            FxUpdate::Bolt => self.update_fx_bolt(fx),
            FxUpdate::Smoke => self.update_fx_smoke(fx),
            FxUpdate::Swarm => self.update_fx_swarm(fx),
            FxUpdate::Explode => self.update_fx_explode(fx),
            FxUpdate::Lightning => self.update_fx_lightning(fx),
            FxUpdate::Fly => self.update_fx_fly(fx),
            FxUpdate::Twinkle => self.update_fx_twinkle(fx),
            FxUpdate::PfxJet => self.update_pfx_jet(fx),
        }
    }

    /// The state part of `DrawSpecialFX` (PC/pcgfx.c:45): calls every effect's `Draw`
    /// function in list order. Only `UpdateFXTwinkle` (health flies) changes state.
    pub fn draw_special_fx_state(&mut self) {
        if self.specfx.count == 0 {
            return;
        }
        for id in self.specfx.ids() {
            if !self.specfx.is_live(SpecFxId(id)) {
                continue;
            }
            match self.specfx.array[id as usize].draw {
                Some(FxDraw::Twinkle) => self.update_fx_twinkle(SpecFxId(id)),
                Some(FxDraw::Ring) => self.draw_fx_ring_state(SpecFxId(id)),
                _ => {}
            }
        }
    }

    /// The state part of `DrawFXRing` (PC/pcgfx.c:375): the ring is transformed segment by
    /// segment, and on odd frames every odd, fully on-screen segment drops a
    /// `FXTYPE_TWINKLE` trail at its fourth vertex (`CreateSpecialEffect`, so `Random` and
    /// the effect list are touched while drawing). Float maths follows the x87
    /// single-precision evaluation of the original (see `view.rs`).
    fn draw_fx_ring_state(&mut self, id: SpecFxId) {
        use crate::view::{mtx_cat, mtx_xfm, quaternion_to_matrix, IDENTITY};
        const ONEOVERFIXED: f64 = 0.00024414; // pcgfx.h
        fn rf(v: i32) -> f32 {
            (v as f32 as f64 * ONEOVERFIXED) as f32
        }
        fn scl(v: f32, k: f64) -> f32 {
            (v as f64 * k) as f32
        }
        fn diag(x: f32, y: f32, z: f32) -> f2_formats::objbank::Mat4 {
            let mut m = IDENTITY;
            m[0][0] = x;
            m[1][1] = y;
            m[2][2] = z;
            m
        }

        let fx = self.specfx.fx(id).clone();
        if fx.tex == 0 {
            return;
        }
        let Some(ring) = self.specfx.ring_vtx.clone() else { return };
        let view = self.specfx.view;

        let scale = [scl(rf(fx.scale.vx), 0.1), scl(rf(fx.scale.vy), 0.1), scl(rf(fx.scale.vz), 0.1)];
        let normal = [rf(fx.normal.vx), rf(fx.normal.vy), rf(fx.normal.vz)];
        let pos = [scl(fx.origin.vx as f32, 0.1), scl(fx.origin.vy as f32, 0.1), scl(fx.origin.vz as f32, 0.1)];

        // Translate to current fx pos and push
        let mut t = IDENTITY;
        t[3] = [pos[0], pos[1], pos[2], 1.0];
        let s1 = mtx_cat(&t, &IDENTITY);

        // Rotate to be around normal: axis q1 x upV, angle -acos(q1 . upV)
        let q1 = normal;
        let up = [0.0f32, 1.0, 0.0];
        let mut cross = [q1[1] * up[2] - q1[2] * up[1], q1[2] * up[0] - q1[0] * up[2], q1[0] * up[1] - q1[1] * up[0]];
        let m = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
        if m != 0.0 {
            cross = [cross[0] / m, cross[1] / m, cross[2] / m];
        }
        let tt = q1[0] * up[0] + q1[1] * up[1] + q1[2] * up[2];
        let w = -((tt as f64).acos() as f32);
        // GetQuaternionFromRotation (mdxMath.cpp:223)
        let half = w / 2.0;
        let sh = half.sin();
        let q3 = [sh * cross[0], sh * cross[1], sh * cross[2], half.cos()];
        let r = quaternion_to_matrix(&q3);
        let s2 = mtx_cat(&r, &s1);

        let tilt2 = scl(fx.tilt as f32, 0.000244);
        let nseg = NUM_RINGSEGS;
        let mut prev_valid = false; // vTPrev[0].sz && vTPrev[1].sz
        let afc_odd = self.act_frame_count & 1 != 0;
        for i in 0..nseg {
            let vx = i * 2;
            let mut zero_z = 0;
            let mut fxpos = SVector::default();
            for j in 0..4 {
                if i != 0 && j < 2 && prev_valid {
                    continue; // memcpy( vT, vTPrev, ... )
                }
                let vxj = (vx + j) % (nseg << 1);
                let rv = ring[vxj];
                let v = [scl(rv.vx as f32, 0.000244), scl(rv.vy as f32, 0.000244), scl(rv.vz as f32, 0.000244)];
                // Slant the polys
                let tilt = if if i & 1 == 0 { j == 0 || j == 3 } else { j == 1 || j == 2 } { 1.0 } else { tilt2 };
                let sm = diag(tilt * scale[0], tilt * scale[1], tilt * scale[2]);
                let d = mtx_cat(&sm, &s2);
                let temp = mtx_xfm(&d, v);
                if j == 3 {
                    // SetVectorSR: (short) via __ftol (truncation)
                    fxpos = SVector { vx: temp[0] as i32 as i16, vy: temp[1] as i32 as i16, vz: temp[2] as i32 as i16, ..SVector::default() };
                }
                let visible = view.as_ref().is_none_or(|vw| vw.xfm_point_f(temp)[2] != 0.0);
                if !visible {
                    zero_z += 1;
                }
            }

            if zero_z == 0 {
                prev_valid = true;
                if i & 1 != 0 && afc_odd {
                    fxpos.vx = fxpos.vx.wrapping_mul(10);
                    fxpos.vy = fxpos.vy.wrapping_mul(10);
                    fxpos.vz = fxpos.vz.wrapping_mul(10);
                    let trail = self.create_special_effect(FXTYPE_TWINKLE, &fxpos, &fx.normal, 81920, 0, 0, 4096);
                    if let Some(tr) = trail {
                        self.specfx.fx_mut(tr).tilt = 8192;
                        if i & 2 != 0 {
                            self.set_fx_colour(trail, 180, 220, 180);
                        } else {
                            self.set_fx_colour(trail, fx.r, fx.g, fx.b);
                        }
                    }
                }
            } else {
                prev_valid = false;
            }
        }
    }

    /// Port of `UpdateFXDecal` (Common/specfx.c:835).
    fn update_fx_decal(&mut self, fx: SpecFxId) {
        let gs = self.game_speed;
        self.follow_origin(fx);
        let afc = self.act_frame_count;
        let e = self.specfx.fx_mut(fx);
        let fo = e.fade.wrapping_mul(gs) >> 12;
        if (e.a as i32) > fo {
            e.a = (e.a as i32 - fo) as u8;
        } else {
            e.a = 0;
        }
        e.speed = e.speed.wrapping_add(fmul(e.accn, gs));
        let spd = fmul(e.speed, gs);
        e.scale.vx = e.scale.vx.wrapping_add(spd);
        e.scale.vz = e.scale.vz.wrapping_add(spd);
        if e.spin != 0 {
            e.angle = (e.angle as i32).wrapping_add((e.spin as i32).wrapping_mul(gs) >> 12) as i16;
        }
        if e.a == 0 || afc > e.lifetime as u32 {
            self.deallocate_fx(Some(fx), 1);
        }
    }

    /// Port of `UpdateFXRing` (Common/specfx.c:867).
    fn update_fx_ring(&mut self, fx: SpecFxId) {
        let gs = self.game_speed;
        self.follow_origin(fx);
        let afc = self.act_frame_count;
        let e = self.specfx.fx_mut(fx);
        let fo = e.fade.wrapping_mul(gs) >> 12;
        if e.fade_alpha > fo {
            e.fade_alpha -= fo;
        } else {
            e.fade_alpha = 0;
        }
        e.a = (e.fade_alpha >> 12) as u8;
        e.speed = e.speed.wrapping_add(fmul(e.accn, gs));
        let speed = fmul(e.speed, gs);
        e.scale.vx = e.scale.vx.wrapping_add(speed);
        e.scale.vy = e.scale.vy.wrapping_add(speed >> 2);
        e.scale.vz = e.scale.vz.wrapping_add(speed);
        e.angle = (e.angle as i32).wrapping_add((e.spin as i32).wrapping_mul(gs) >> 12) as i16;
        if e.fade_alpha == 0 || afc > e.lifetime as u32 {
            self.deallocate_fx(Some(fx), 1);
        }
    }

    /// Port of `UpdateFXBolt` (Common/specfx.c:902) (no preset creates bolts in the final game).
    fn update_fx_bolt(&mut self, fx: SpecFxId) {
        let gs = self.game_speed;
        let accn = fmul(self.specfx.fx(fx).accn, gs);
        self.follow_origin(fx);
        let afc = self.act_frame_count;
        let e = self.specfx.fx_mut(fx);
        let fo = e.fade.wrapping_mul(gs) >> 12;
        if (e.a as i32) > fo {
            e.a = (e.a as i32 - fo) as u8;
        } else {
            e.a = 0;
        }
        e.vel.vx = e.vel.vx.wrapping_add(accn);
        e.vel.vy = e.vel.vy.wrapping_add(accn);
        e.vel.vz = e.vel.vz.wrapping_add(accn);
        e.origin.vx = (e.origin.vx as i32).wrapping_add(fmul(e.vel.vx, gs) >> 12) as i16;
        e.origin.vy = (e.origin.vy as i32).wrapping_add(fmul(e.vel.vy, gs) >> 12) as i16;
        e.origin.vz = (e.origin.vz as i32).wrapping_add(fmul(e.vel.vz, gs) >> 12) as i16;
        e.angle = (e.angle as i32).wrapping_add((e.spin as i32).wrapping_mul(gs) >> 12) as i16;
        if afc > e.lifetime as u32 {
            self.deallocate_fx(Some(fx), 1);
        }
    }

    /// Port of `UpdateFXSmoke` (Common/specfx.c:934).
    fn update_fx_smoke(&mut self, fx: SpecFxId) {
        let gs = self.game_speed;
        self.follow_origin(fx);
        let e = self.specfx.fx(fx).clone();
        let mut dead = false;
        let mut s = e.sprites;
        let mut i = e.num_p;
        while i > 0 {
            i -= 1;
            let Some(sid) = s else { break };
            let e = self.specfx.fx(fx).clone();
            let fo = e.fade.wrapping_mul(gs) >> 12;
            {
                let sp = self.sprite.get_mut(sid);
                if (sp.a as i32) > fo {
                    sp.a = (sp.a as i32 - fo) as u8;
                } else {
                    sp.draw = 0;
                    sp.a = 0;
                    dead = true;
                }
                sp.pos.vx = (sp.pos.vx as i32).wrapping_add(fmul(e.vel.vx, gs) >> 12) as i16;
                sp.pos.vy = (sp.pos.vy as i32).wrapping_add(fmul(e.vel.vy, gs) >> 12) as i16;
                sp.pos.vz = (sp.pos.vz as i32).wrapping_add(fmul(e.vel.vz, gs) >> 12) as i16;
            }
            // Slow down gameSpeed times
            let vs = 4096 - gs / 50;
            scale_vector_ff(&mut self.specfx.fx_mut(fx).vel, vs);

            if e.ty == FXTYPE_SMOKE_GROWS {
                let d = fmul(e.accn, gs) >> 12;
                let sp = self.sprite.get_mut(sid);
                sp.scale_x = (sp.scale_x as i32).wrapping_add(d) as i16;
                sp.scale_y = (sp.scale_y as i32).wrapping_add(d) as i16;
            } else if e.ty == FXTYPE_BUBBLES {
                if let Some(rb) = e.rebound {
                    let pl = *self.specfx.p2(rb);
                    let mut frp = FVector::default();
                    set_vector_fs(&mut frp, &pl.point);
                    let j = dot_product_ff(&frp, &pl.normal).wrapping_neg();
                    self.specfx.p2_mut(rb).j = j;
                    let mut fsp = FVector::default();
                    // NB: tests the effect's first sprite, not `s`.
                    let first = e.sprites.map(|f| self.sprite.get(f).pos).unwrap_or_default();
                    set_vector_fs(&mut fsp, &first);
                    let dist = dot_product_ff(&fsp, &pl.normal).wrapping_add(j).wrapping_neg();
                    if dist > 0 && dist < 409600 {
                        let pos = self.sprite.get(sid).pos;
                        self.create_special_effect(FXTYPE_WAKE, &pos, &pl.normal, 20480, 410, 205, 1280);
                        let rb = self.specfx.fx(fx).rebound;
                        self.specfx.deallocate_p2(rb);
                        self.specfx.fx_mut(fx).rebound = None;
                    }
                }
            }
            s = self.sprite.next_of(sid);
        }

        if self.act_frame_count > self.specfx.fx(fx).lifetime as u32 || dead {
            // Spawn some bubbles, some smoke and a splash
            if self.specfx.fx(fx).ty == FXTYPE_GREENGLOOP {
                self.create_gloop_effects(fx);
            }
            self.deallocate_fx(Some(fx), 1);
        }
    }

    /// Port of `UpdateFXSwarm` (Common/specfx.c:1008).
    ///
    /// DEVIATION: the phase `j = ((int)fx) & (40+64+128+256)` uses the effect's heap
    /// address; the port uses `index * sizeof(SPECFX)` (array base 0). Visual only.
    fn update_fx_swarm(&mut self, fx: SpecFxId) {
        let gs = self.game_speed;
        self.follow_origin(fx);
        let e = self.specfx.fx(fx).clone();
        let _speed = fmul(e.speed, gs);
        let limit = e.accn;
        let j = (fx.0.wrapping_mul(SIZEOF_SPECFX) & (40 + 64 + 128 + 256)) as i32;
        let afc = self.act_frame_count as i32;
        let ss = self.sprite.chain(e.sprites, e.num_p);
        let ps = self.particle.chain(e.particles, e.num_p);
        let mut i = e.num_p;
        for (s, p) in ss.into_iter().zip(ps) {
            i -= 1;
            let k = i.wrapping_add(j);
            let px = fmul(fmul(rsin(afc.wrapping_add(k.wrapping_mul(10)).wrapping_mul(43)), limit), 4);
            let py = fmul(fmul(rsin(afc.wrapping_sub(k.wrapping_mul(15)).wrapping_mul(57)), limit), 4);
            let pz = fmul(fmul(rsin(afc.wrapping_add(k.wrapping_mul(20)).wrapping_mul(71)), limit), 4);
            let pp = self.particle.get_mut(p);
            pp.pos = SVector::new(px as i16, py as i16, pz as i16);
            let ppos = pp.pos;
            // Add local particle pos to swarm origin to get world coords for sprite
            add_vector_sss(&mut self.sprite.get_mut(s).pos, &e.origin, &ppos);
        }
    }

    /// Port of `UpdateFXExplode` (Common/specfx.c:1054).
    fn update_fx_explode(&mut self, fx: SpecFxId) {
        let gs = self.game_speed;
        self.follow_origin(fx);
        let vs = 4096 - gs / 50;
        let e = self.specfx.fx(fx).clone();
        let mut alive = 0;
        let mut s = e.sprites;
        let mut p = e.particles;
        let mut i = e.num_p;
        while i > 0 {
            i -= 1;
            let (Some(sid), Some(pid)) = (s, p) else { break };
            // BUG(original): `continue` without advancing s/p: after the first dead
            // particle the remaining iterations all skip.
            if self.particle.get(pid).bounce == 2 {
                continue;
            }
            alive += 1;
            scale_vector_ff(&mut self.particle.get_mut(pid).vel, vs);
            if e.gravity != 0 {
                let mut up = FVector::default();
                set_vector_ff(&mut up, &e.normal);
                scale_vector_ff(&mut up, fmul(e.gravity, gs));
                sub_from_vector_ff(&mut self.particle.get_mut(pid).vel, &up);
            }
            if let Some(rb) = self.specfx.fx(fx).rebound {
                let pl = *self.specfx.p2(rb);
                let mut frp = FVector::default();
                set_vector_fs(&mut frp, &pl.point);
                let j = dot_product_ff(&frp, &pl.normal).wrapping_neg();
                self.specfx.p2_mut(rb).j = j;
                let mut fsp = FVector::default();
                set_vector_fs(&mut fsp, &self.sprite.get(sid).pos);
                let dist = dot_product_ff(&fsp, &pl.normal).wrapping_add(j).wrapping_neg();
                // check if particle has hit (or passed through) the plane
                if dist > 0 {
                    let pp = self.particle.get_mut(pid);
                    if pp.bounce != 0 {
                        pp.vel.vy = fmul(pp.vel.vy, -3891); // Should be relative to the normal, but it'll do for now
                    } else {
                        pp.vel = FVector::default();
                        pp.bounce = 2;
                        self.sprite.get_mut(sid).draw = 0;
                    }
                    // check if this exploding particle type triggers some other effect or event
                    if e.ty == FXTYPE_SPLASH && dist < 204800 {
                        let pos = self.sprite.get(sid).pos;
                        self.create_special_effect(FXTYPE_WAKE, &pos, &pl.normal, 20480, 819, 410, 1229);
                    }
                }
            }
            let vel = self.particle.get(pid).vel;
            {
                let sp = self.sprite.get_mut(sid);
                sp.pos.vx = (sp.pos.vx as i32).wrapping_add(fmul(vel.vx, gs) >> 12) as i16;
                sp.pos.vy = (sp.pos.vy as i32).wrapping_add(fmul(vel.vy, gs) >> 12) as i16;
                sp.pos.vz = (sp.pos.vz as i32).wrapping_add(fmul(vel.vz, gs) >> 12) as i16;
            }
            let fo = (self.rng.random(4).wrapping_add(e.fade)).wrapping_mul(gs) >> 12;
            let sp = self.sprite.get_mut(sid);
            if e.ty == FXTYPE_SMOKEBURST || e.ty == FXTYPE_FIERYSMOKE {
                let d = fmul(e.accn, gs) >> 12;
                sp.scale_x = (sp.scale_x as i32).wrapping_add(d) as i16;
                sp.scale_y = (sp.scale_y as i32).wrapping_add(d) as i16;
            }
            // For fiery smoke, fade to black then fade out if not the additive component
            if e.ty == FXTYPE_FIERYSMOKE && (sp.r != 0 || sp.g != 0 || sp.b != 0) {
                sp.r = if (sp.r as i32) > fo { (sp.r as i32 - fo) as u8 } else { 0 };
                sp.g = if (sp.g as i32) > fo { (sp.g as i32 - fo) as u8 } else { 0 };
            }
            sp.a = if (sp.a as i32) > fo { (sp.a as i32 - fo) as u8 } else { 0 };
            if sp.a == 0 {
                sp.draw = 0;
                self.particle.get_mut(pid).bounce = 2;
            }
            s = self.sprite.next_of(sid);
            p = self.particle.next_of(pid);
        }
        if self.act_frame_count > self.specfx.fx(fx).lifetime as u32 || alive == 0 {
            self.deallocate_fx(Some(fx), 1);
        }
    }

    /// Port of `UpdateFXLightning` (Common/specfx.c:1279).
    fn update_fx_lightning(&mut self, fx: SpecFxId) {
        let e = self.specfx.fx(fx).clone();
        let mut target = SVector::default();
        let mut aim = FVector::default();
        if let Some(fp) = self.follow_pos(fx) {
            set_vector_ss(&mut target, &fp);
            sub_vector_fss(&mut aim, &target, &e.origin);
        } else {
            set_vector_ff(&mut aim, &e.normal);
            scale_vector_ff(&mut aim, e.speed);
            add_vector_sfs(&mut target, &aim, &e.origin);
        }
        let ps = self.particle.chain(e.particles, e.num_p);
        let num_p = e.num_p;
        for (i, &p) in ps.iter().enumerate() {
            let i = i as i32;
            let mut to = FVector::default();
            if i != 0 {
                let scale = 4096 / (num_p - i);
                let mut source = SVector::default();
                let prev = self.particle.prev_of(p).map(|q| self.particle.get(q).pos).unwrap_or_default();
                set_vector_ss(&mut source, &prev);
                sub_vector_fss(&mut to, &target, &source);
                scale_vector_ff(&mut to, scale);

                // Get a random direction, and then modify by the "unit" aim direction
                let rx = self.rng.random(201) - 100;
                let ry = self.rng.random(201) - 100;
                let rz = self.rng.random(201) - 100;
                let mut ran = FVector::new(rx, ry, rz);
                make_unit(&mut ran);
                let r = e.accn.wrapping_mul(if i > (num_p >> 1) { num_p - i } else { i.wrapping_mul(magnitude_f(&to) >> 12) });
                scale_vector_ff(&mut ran, r);
                add_to_vector_ff(&mut to, &ran);
                add_vector_ssf(&mut self.particle.get_mut(p).pos, &source, &to);
            } else {
                let source = self.particle.get(p).pos;
                sub_vector_fss(&mut to, &target, &source);
            }
            // Push the polys out from the position
            make_unit(&mut to);
            let mut cross = FVector::default();
            cross_product_fff(&mut cross, &to, &f2_math::UP_VEC);
            make_unit(&mut cross);
            scale_vector_ff(&mut cross, e.scale.vx);
            let pp = self.particle.get_mut(p);
            let pos = pp.pos;
            add_vector_ssf(&mut pp.poly[0], &pos, &cross);
            sub_vector_ssf(&mut pp.poly[1], &pos, &cross);
        }
        if self.act_frame_count > e.lifetime as u32 {
            self.deallocate_fx(Some(fx), 1);
        }
    }

    /// Port of `UpdateFXFly` (Common/specfx.c:1353): health flies (fx->vel is a local
    /// space position).
    fn update_fx_fly(&mut self, fx: SpecFxId) {
        let gs = self.game_speed;
        let Some(a) = self.specfx.fx(fx).act.as_ref().and_then(|v| v.first().copied().flatten()) else { return };
        if self.actor2.actors.get(a).is_none() {
            return;
        }
        let vel = self.specfx.fx(fx).vel;
        // Randomly go to places
        if distance_between_points_sf(&self.actor2.actors[a].actor.rotaim, &vel) < (100 << 12) {
            let x = (self.rng.random(500) - 250) as i16;
            let y = (self.rng.random(500) - 250) as i16;
            let z = (self.rng.random(500) - 250) as i16;
            self.actor2.actors[a].actor.rotaim = SVector::new(x, y, z);
        }
        let mut q1 = IQuaternion::default();
        set_quaternion(&mut q1, &self.actor2.actors[a].actor.q_rot);
        let mut fwd = FVector::default();
        sub_vector_fsf(&mut fwd, &self.actor2.actors[a].actor.rotaim, &vel);
        make_unit(&mut fwd);

        let mut axis = FVector::default();
        cross_product_fff(&mut axis, &f2_math::IN_VEC, &fwd);
        make_unit(&mut axis);
        let mut t = dot_product_ff(&f2_math::IN_VEC, &fwd);
        t = t.clamp(-4091, 4092);
        if t < 4 && t > -4 {
            t = 4;
        }
        let q3 = IQuaternion { x: axis.vx, y: axis.vy, z: axis.vz, w: arccos(t) };
        let mut q2 = IQuaternion::default();
        fixed_get_quaternion_from_rotation(&mut q2, &q3);

        let e = self.specfx.fx(fx).clone();
        let mut speed = fmul(e.accn, gs);
        if speed > 4092 {
            speed = 4092;
        }
        iquat_slerp(&q1, &q2, speed, &mut self.actor2.actors[a].actor.q_rot);

        // Forward motion
        scale_vector_ff(&mut fwd, fmul(e.speed, gs));
        add_to_vector_ff(&mut self.specfx.fx_mut(fx).vel, &fwd);

        // World coordinates
        let t0 = &self.tongue.tongue[0];
        let held = t0.ty == crate::tongue::TONGUE_GET_PICKUP
            && matches!(t0.thing, crate::tongue::TongueThing::Garib(g) if self.collect.garib_list.array.get(g.idx()).is_some_and(|gb| gb.fx == Some(fx)));
        if !held {
            let (vel, origin) = (self.specfx.fx(fx).vel, self.specfx.fx(fx).origin);
            add_vector_sfs(&mut self.actor2.actors[a].actor.position, &vel, &origin);
        }
    }

    /// Port of `UpdateFXTwinkle` (Common/specfx.c:1413): glows around fire, candles etc
    /// and twinkles from gold. Also the `Draw` function of health flies.
    fn update_fx_twinkle(&mut self, fx: SpecFxId) {
        let gs = self.game_speed;
        let afc = self.act_frame_count;
        let Some(s) = self.specfx.fx(fx).sprites else { return };
        if let Some(p) = self.follow_pos(fx) {
            self.sprite.get_mut(s).pos = p;
        }
        let ty = self.specfx.fx(fx).ty;
        if ty == FXTYPE_FLASH {
            let e = self.specfx.fx_mut(fx);
            if afc.wrapping_sub(e.tilt as u32) > (e.lifetime.wrapping_sub(e.tilt) / 2) as u32 {
                e.speed = e.speed.wrapping_add(fmul(e.accn, gs));
                let speed = fmul(e.speed, gs) >> 12;
                let sp = self.sprite.get_mut(s);
                sp.scale_x = (sp.scale_x as i32).wrapping_sub(speed) as i16;
                sp.scale_y = (sp.scale_y as i32).wrapping_sub(speed) as i16;
            } else {
                e.speed = e.speed.wrapping_sub(fmul(e.accn, gs));
                if e.speed < 0 {
                    e.speed = 0;
                }
                let speed = fmul(e.speed, gs) >> 12;
                let sp = self.sprite.get_mut(s);
                sp.scale_x = (sp.scale_x as i32).wrapping_add(speed) as i16;
                sp.scale_y = (sp.scale_y as i32).wrapping_add(speed) as i16;
            }
            if afc > self.specfx.fx(fx).lifetime as u32 {
                self.deallocate_fx(Some(fx), 1);
            }
            return;
        }
        if ty == FXTYPE_TWINKLE {
            let fade = self.specfx.fx(fx).fade;
            let fo = self.rng.random(fade as u32).wrapping_mul(gs >> 12);
            let mut dead = false;
            let sp = self.sprite.get_mut(s);
            if (sp.a as i32) > fo {
                sp.a = (sp.a as i32 - fo) as u8;
            } else {
                sp.a = 0;
                sp.draw = 0;
                dead = true;
            }
            if afc > self.specfx.fx(fx).lifetime as u32 || dead {
                // After this the effect is zeroed (memset): tilt reads 0 below.
                self.deallocate_fx(Some(fx), 1);
            }
        }
        // Use tilt to specify twinkliness - urgh
        let (tilt, scale) = {
            let e = self.specfx.fx(fx);
            (e.tilt, e.scale)
        };
        if tilt >= 8192 {
            let size = self.rng.random(50) - 20;
            let sp = self.sprite.get_mut(s);
            sp.scale_x = (sp.scale_x as i32).wrapping_add(size) as i16;
            sp.scale_y = (sp.scale_y as i32).wrapping_add(size) as i16;
            if ((sp.scale_x as i32) << 12) > (scale.vx << 1) {
                sp.scale_x = (scale.vx >> 11) as i16;
            } else if ((sp.scale_x as i32) << 12) < (scale.vx >> 1) {
                sp.scale_x = (scale.vx >> 13) as i16;
            }
            if ((sp.scale_y as i32) << 12) > (scale.vy << 1) {
                sp.scale_y = (scale.vy >> 11) as i16;
            } else if ((sp.scale_y as i32) << 12) < (scale.vy >> 1) {
                sp.scale_y = (scale.vy >> 13) as i16;
            }
        }
        if tilt != 0 {
            let d = self.rng.random(50) - 25;
            let sp = self.sprite.get_mut(s);
            sp.a = ((sp.a as i32).wrapping_add(d) as u8).clamp(50, 200);
        }
    }

    // ------------------------------------------------------------------ helpers

    /// Port of `SetFXColour` (Common/specfx.c:1751).
    pub fn set_fx_colour(&mut self, fx: Option<SpecFxId>, r: u8, g: u8, b: u8) {
        let Some(fx) = fx else { return };
        let e = self.specfx.fx_mut(fx);
        e.r = r;
        e.g = g;
        e.b = b;
        let e = e.clone();
        if e.sprites.is_some() {
            for s in self.sprite.chain(e.sprites, e.num_p) {
                let sp = self.sprite.get_mut(s);
                sp.r = r;
                sp.g = g;
                sp.b = b;
            }
        }
        if e.particles.is_some() {
            // (FXUPDATE_TRAIL effects index a particle array; no trails are created.)
            for p in self.particle.chain(e.particles, e.num_p) {
                let pp = self.particle.get_mut(p);
                pp.r = r;
                pp.g = g;
                pp.b = b;
            }
        }
    }

    /// `fx->tilt = tilt` on a created effect.
    pub fn set_fx_tilt(&mut self, fx: Option<SpecFxId>, tilt: Fixed) {
        if let Some(fx) = fx {
            self.specfx.fx_mut(fx).tilt = tilt;
        }
    }

    /// `fx->follow = act->actor` on a created effect.
    pub fn set_fx_follow(&mut self, fx: Option<SpecFxId>, follow: Option<ActorId>) {
        if let Some(fx) = fx {
            self.specfx.fx_mut(fx).follow = follow;
        }
    }

    /// `fx->gravity = g` on a created effect.
    pub fn set_fx_gravity(&mut self, fx: Option<SpecFxId>, gravity: Fixed) {
        if let Some(fx) = fx {
            self.specfx.fx_mut(fx).gravity = gravity;
        }
    }

    /// `if(fx->rebound) { SetVectorSS(&fx->rebound->point, point); SetVectorFF(&fx->rebound->normal, normal); }`
    /// (`normal = None` leaves the plane normal unchanged).
    pub fn set_fx_rebound(&mut self, fx: Option<SpecFxId>, point: &SVector, normal: Option<&FVector>) {
        let Some(rb) = fx.and_then(|f| self.specfx.fx(f).rebound) else { return };
        let pl = self.specfx.p2_mut(rb);
        pl.point = *point;
        if let Some(n) = normal {
            pl.normal = *n;
        }
    }

    /// `fx->rebound = AllocateP2()` on a created effect (any plane it already had is
    /// leaked, as in the original). Returns the new plane.
    pub fn fx_alloc_rebound(&mut self, fx: SpecFxId) -> Option<Plane2Id> {
        let p2 = self.specfx.allocate_p2();
        self.specfx.fx_mut(fx).rebound = p2;
        p2
    }

    /// `fx->sprites->draw = v` (and `fx->act[0]->draw`), used when hiding garibs.
    pub fn set_fx_draw(&mut self, fx: Option<SpecFxId>, v: i8) {
        let Some(fx) = fx else { return };
        if let Some(s) = self.specfx.fx(fx).sprites {
            self.sprite.get_mut(s).draw = v;
        }
        if let Some(Some(a)) = self.specfx.fx(fx).act.as_ref().and_then(|v| v.first().copied()) {
            if let Some(a2) = self.actor2.actors.get_mut(a) {
                a2.draw = v;
            }
        }
    }

    /// Port of `SetAttachedFXColour` (Common/specfx.c:2106).
    pub fn set_attached_fx_colour(&mut self, fx: Option<SpecFxId>, effects: u32) {
        let mut r = 60;
        let mut g = 60;
        let mut b = 60;
        if effects & EF_TINTRED != 0 {
            r = 255;
        }
        if effects & EF_TINTGREEN != 0 {
            g = 255;
        }
        if effects & EF_TINTBLUE != 0 {
            b = 255;
        }
        self.set_fx_colour(fx, r, g, b);
    }

    /// Port of `ProcessAttachedEffects` (Common/specfx.c:1802).
    pub fn process_attached_effects(&mut self, entity: FxEntity) {
        let (act, tile, flags, path, is_enemy) = match entity {
            FxEntity::Enemy(id) => {
                let n = &self.enemies.enemy_list.array[id.idx()];
                (n.nme_actor, n.in_tile, n.flags, n.path, true)
            }
            FxEntity::Platform(id) => {
                let p = &self.platform.platform_list.array[id.idx()];
                (p.plt_actor, p.in_tile[0], p.flags, p.path, false)
            }
        };
        let Some(act) = act else { return };
        let Some(tile) = tile else { return };
        let Some(frog0) = self.frogger.frog[0] else { return };
        let (tnormal, tcentre) = {
            let t = self.map.tile(tile);
            (t.normal, t.centre)
        };
        let mut normal = FVector::default();
        new_set_vector_fs(&mut normal, &tnormal);

        let node = |g: &Game, i: i32| g.path.nodes.get(path.node_index(i)).cloned().unwrap_or_default();
        let gs_of = |g: &Game| {
            let o2 = node(g, path.from_node as i32).offset2;
            if o2 != 0 {
                o2 / SCALE
            } else {
                4096
            }
        };

        let apos = self.actor2.actors[act].actor.position;
        let fx_dist = distance_between_points_ss(&self.actor2.actors[frog0].actor.position, &apos);
        let afc = self.act_frame_count;
        let mut fx: Option<SpecFxId> = None;

        if fx_dist < (ACTOR_DRAWDISTANCEOUTER << 1) && afc > self.actor2.actors[act].fx_count {
            let a2 = &self.actor2.actors[act];
            let (value1, effects0) = (a2.value1, a2.effects);
            // Restart effect timer
            let r = if is_enemy && flags & crate::enemies::ENEMY_NEW_BABYFROG != 0 {
                57
            } else if value1 != 0 {
                if effects0 & EF_RANDOMCREATE != 0 {
                    let ran = self.rng.random((if value1 != 0 { value1 } else { 4096 }) as u32);
                    fdiv(245760, ran.max(1)) >> 12
                } else {
                    fdiv(245760, value1.max(1)) >> 12
                }
            } else {
                60
            };
            self.actor2.actors[act].fx_count = afc.wrapping_add(r as u32);

            let effects = self.actor2.actors[act].effects;
            let radius = self.actor2.actors[act].radius;
            if effects & EF_RIPPLE_RINGS != 0 {
                let mut rpos = apos;
                rpos.vy = tcentre.vy;
                fx = if effects & EF_FAST != 0 {
                    self.create_special_effect(FXTYPE_WAKE, &rpos, &normal, 40960, 1229, 410, 2048)
                } else if effects & EF_SLOW != 0 {
                    self.create_special_effect(FXTYPE_WAKE, &rpos, &normal, 81920, 410, 205, 2048)
                } else {
                    self.create_special_effect(FXTYPE_WAKE, &rpos, &normal, 81920, 820, 410, 2048)
                };
                self.set_attached_fx_colour(fx, effects);
            }
            if effects & EF_SMOKE_STATIC != 0 {
                let sp = if effects & EF_FAST != 0 {
                    6144
                } else if effects & EF_SLOW != 0 {
                    2048
                } else {
                    4915
                };
                fx = self.create_special_effect(FXTYPE_SMOKE_STATIC, &apos, &normal, 262144, sp, 0, 4096);
                self.set_attached_fx_colour(fx, effects);
            }
            if effects & EF_SMOKE_GROWS != 0 {
                let gs = gs_of(self);
                let sp = if effects & EF_FAST != 0 {
                    6144
                } else if effects & EF_SLOW != 0 {
                    2048
                } else {
                    4915
                };
                fx = self.create_special_effect(FXTYPE_SMOKE_GROWS, &apos, &normal, 172032, sp, gs, 4096);
                self.set_attached_fx_colour(fx, effects);
            }
            if effects & EF_SPARKLYTRAIL != 0 {
                let sp = if effects & EF_FAST != 0 {
                    16384
                } else if effects & EF_SLOW != 0 {
                    2048
                } else {
                    8192
                };
                fx = self.create_special_effect(FXTYPE_SPARKLYTRAIL, &apos, &normal, 81920, sp, 0, 20480);
                if let Some(f) = fx {
                    if let Some(rb) = self.specfx.fx(f).rebound {
                        let pl = self.specfx.p2_mut(rb);
                        pl.point = tcentre;
                        new_set_vector_fs(&mut pl.normal, &tnormal);
                    }
                    self.specfx.fx_mut(f).gravity = radius.wrapping_mul(SCALE);
                    self.set_attached_fx_colour(fx, effects);
                }
            }
            if effects & EF_SPARKBURST != 0 {
                if effects & EF_FAST != 0 {
                    let _fast = self.create_special_effect(FXTYPE_SPARKBURST, &apos, &normal, 61440, 16384, 0, 20480);
                }
                // (no `else`: a fast burst is followed by a medium one)
                fx = if effects & EF_SLOW != 0 {
                    self.create_special_effect(FXTYPE_SPARKBURST, &apos, &normal, 61440, 2048, 0, 20480)
                } else {
                    self.create_special_effect(FXTYPE_SPARKBURST, &apos, &normal, 61440, 8192, 0, 20480)
                };
                if let Some(f) = fx {
                    self.specfx.fx_mut(f).gravity = radius.wrapping_mul(SCALE);
                    self.set_attached_fx_colour(fx, effects);
                }
            }
            if effects & EF_SMOKEBURST != 0 {
                let gs = gs_of(self);
                let sp = if effects & EF_FAST != 0 {
                    20480
                } else if effects & EF_SLOW != 0 {
                    2458
                } else {
                    8192
                };
                fx = self.create_special_effect(FXTYPE_SMOKEBURST, &apos, &normal, 204800, sp, gs, 6963);
                self.set_attached_fx_colour(fx, effects);
            }
            if effects & EF_FIERYSMOKE != 0 {
                let gs = gs_of(self);
                let sp = if effects & EF_FAST != 0 {
                    16384
                } else if effects & EF_SLOW != 0 {
                    4096
                } else {
                    10240
                };
                fx = self.create_special_effect(FXTYPE_FIERYSMOKE, &apos, &normal, 204800, sp, gs, 10240);
            }
            if effects & EF_BUBBLES != 0 {
                let sp = if effects & EF_FAST != 0 {
                    4096
                } else if effects & EF_SLOW != 0 {
                    810
                } else {
                    2048
                };
                fx = self.create_special_effect(FXTYPE_BUBBLES, &apos, &normal, 48192, sp, 0, 2048);
                if let Some(f) = fx {
                    self.attach_rebound_up(f, &path, act);
                    self.set_attached_fx_colour(fx, effects);
                }
            }
            if effects & EF_TWINKLE != 0 {
                let life = (self.rng.random(3) + 1) << 12;
                fx = self.create_special_effect(FXTYPE_TWINKLE, &apos, &normal, 81920, 0, 0, life);
                if let Some(f) = fx {
                    self.set_attached_fx_colour(fx, effects);
                    let e = self.specfx.fx_mut(f);
                    e.follow = Some(act);
                    if effects & EF_FAST != 0 {
                        e.tilt = 12288;
                    } else if effects & EF_MEDIUM != 0 {
                        e.tilt = 8192;
                    } else if effects & EF_SLOW != 0 {
                        e.tilt = 4096;
                    }
                }
            }
            if effects & EF_GREENGLOOP != 0 {
                let sp = if effects & EF_FAST != 0 {
                    6144
                } else if effects & EF_SLOW != 0 {
                    1229
                } else {
                    2867
                };
                fx = self.create_special_effect(FXTYPE_GREENGLOOP, &apos, &normal, radius, sp, 0, 2048);
                if let Some(f) = fx {
                    self.attach_rebound_up(f, &path, act);
                    self.set_attached_fx_colour(fx, effects);
                }
            }
        }
        let effects = self.actor2.actors[act].effects;
        let radius = self.actor2.actors[act].radius;
        if effects & EF_GLOW != 0 {
            fx = self.create_special_effect(FXTYPE_GLOW, &apos, &normal, 491520, 0, 0, 0);
            self.set_attached_fx_colour(fx, effects);
            // BUG(original): fx is dereferenced without a NULL check (crash when the glow
            // could not be created); the port skips the writes instead.
            if let Some(f) = fx {
                let e = self.specfx.fx_mut(f);
                e.follow = Some(act);
                if effects & EF_FAST != 0 {
                    e.tilt = 8192;
                } else if effects & EF_MEDIUM != 0 {
                    e.tilt = 4096;
                } else if effects & EF_SLOW != 0 {
                    e.tilt = 0;
                }
                self.actor2.actors[act].effects &= !EF_GLOW;
            }
        }
        let effects = self.actor2.actors[act].effects;
        if effects & (EF_BUTTERFLYSWARM | EF_BATSWARM | EF_SPACETHING1) != 0 {
            let n0 = node(self, 0);
            let value1 = self.actor2.actors[act].value1;
            fx = self.create_special_effect_direct(FXTYPE_BATSWARM, &apos, &normal, radius, n0.speed, n0.offset2, value1);
            // BUG(original): only EF_BATSWARM is cleared, so butterfly/space swarms are
            // re-created every frame until the pool runs out.
            self.actor2.actors[act].effects &= !EF_BATSWARM;
            if let Some(f) = fx {
                self.specfx.fx_mut(f).follow = Some(act);
            }
        }
        let effects = self.actor2.actors[act].effects;
        let value1 = self.actor2.actors[act].value1;
        if effects & EF_TRAIL != 0 {
            let life = if effects & EF_FAST != 0 {
                2458
            } else if effects & EF_SLOW != 0 {
                12288
            } else {
                4096
            };
            fx = self.create_special_effect(FXTYPE_TRAIL, &apos, &normal, value1, 3891, 0, life);
            if let Some(f) = fx {
                if !is_enemy {
                    self.specfx.fx_mut(f).gravity = -1228800;
                }
                self.specfx.fx_mut(f).follow = Some(act);
                self.set_attached_fx_colour(fx, effects);
                self.actor2.actors[act].effects &= !EF_TRAIL;
            }
        } else if effects & EF_BILLBOARDTRAIL != 0 {
            let life = if effects & EF_FAST != 0 {
                2458
            } else if effects & EF_SLOW != 0 {
                12288
            } else {
                8192
            };
            fx = self.create_special_effect(FXTYPE_BILLBOARDTRAIL, &apos, &normal, value1, 3891, 0, life);
            if let Some(f) = fx {
                self.specfx.fx_mut(f).follow = Some(act);
                self.set_attached_fx_colour(fx, effects);
                self.actor2.actors[act].effects &= !EF_BILLBOARDTRAIL;
            }
        }
        let _ = fx;
    }

    /// Bubble/gloop rebound plane: `up = path->nodes[0].worldTile->normal`, plane through
    /// `actor position + up*radius`.
    fn attach_rebound_up(&mut self, f: SpecFxId, path: &crate::path::Path, act: ActorId) {
        let wt = self.path.nodes.get(path.node_index(0)).and_then(|n| n.world_tile);
        let mut up = FVector::default();
        if let Some(t) = wt {
            new_set_vector_fs(&mut up, &self.map.tile(t).normal);
        }
        if let Some(p2) = self.specfx.allocate_p2() {
            self.specfx.fx_mut(f).rebound = Some(p2);
            let (radius, pos) = (self.actor2.actors[act].radius, self.actor2.actors[act].actor.position);
            self.specfx.p2_mut(p2).normal = up;
            scale_vector_ff(&mut up, radius);
            add_vector_ssf(&mut self.specfx.p2_mut(p2).point, &pos, &up);
        }
    }

    /// Port of `CreateGloopEffects` (Common/specfx.c:2128): some bubbles, some smoke and a splash.
    fn create_gloop_effects(&mut self, parent: SpecFxId) {
        let p = self.specfx.fx(parent).clone();
        let up = p.normal;
        let mut surface = up;
        scale_vector_ff(&mut surface, p.scale.vx);
        add_to_vector_fs(&mut surface, &p.origin);
        let mut ssurf = SVector::default();
        set_vector_sf(&mut ssurf, &surface);

        // Create the second wave of bubbles
        let fx = self.create_special_effect(FXTYPE_BUBBLES, &p.origin, &up, 90112, 2867, 0, 2048);
        if let Some(f) = fx {
            if let Some(p2) = self.specfx.allocate_p2() {
                self.specfx.fx_mut(f).rebound = Some(p2);
                let pl = self.specfx.p2_mut(p2);
                pl.normal = up;
                set_vector_sf(&mut pl.point, &surface);
            }
            self.set_fx_colour(fx, p.r, p.g, p.b);
        }
        // Green smoke created at the _surface_ of the water.
        // NB: the original passes the FVECTOR as an SVECTOR* ((SVECTOR*)&surface), so the
        // "position" is the low/high halves of surface.vx and surface.vy.
        let cast = fvector_as_svector(&surface);
        let _ = ssurf;
        let fx = self.create_special_effect(FXTYPE_SMOKEBURST, &cast, &up, 204800, 4096, 0, 6963);
        self.set_fx_colour(fx, p.r, p.g, p.b);

        // Ditto for splash
        let fx = self.create_special_effect(FXTYPE_SPLASH, &cast, &up, 57344, 10240, 0, 8192);
        if let Some(f) = fx {
            self.specfx.fx_mut(f).gravity = 4100;
            if let Some(p2) = self.specfx.allocate_p2() {
                self.specfx.fx_mut(f).rebound = Some(p2);
                let pl = self.specfx.p2_mut(p2);
                pl.normal = up;
                set_vector_sf(&mut pl.point, &surface);
            }
            self.set_fx_colour(fx, p.r, p.g, p.b);
        }
    }

    /// Port of `CreateLightningEffect` (Common/specfx.c:2176): wiggly electric line
    /// between two points.
    pub fn create_lightning_effect(&mut self, p1: &SVector, p2: &SVector, effects: u32, life: i32, _z_depth_off: i32) {
        if self.specfx.view.as_ref().is_some_and(|v| v.outcode_check(p1, p2)) {
            return;
        }
        let mut dir = FVector::default();
        sub_vector_fss(&mut dir, p1, p2);
        if dir.vx == 0 && dir.vy == 0 && dir.vz == 0 {
            return;
        }
        let distance = magnitude_f(&dir);
        dir.vx = fdiv(dir.vx, distance);
        dir.vy = fdiv(dir.vy, distance);
        dir.vz = fdiv(dir.vz, distance);

        let mut i = if effects & EF_SLOW != 0 {
            2
        } else if effects & EF_MEDIUM != 0 {
            3
        } else if effects & EF_FAST != 0 {
            4
        } else {
            1
        };
        let accn = fdiv(25 << 12, distance);
        let lifetime = (life << 12) / 60;
        self.specfx.spec_fx_alpha = 180;
        while i != 0 {
            self.prep_for_priority_effect();
            let fx = self.create_special_effect_direct(FXTYPE_LIGHTNING, p2, &dir, 20480, distance, accn, lifetime);
            if fx.is_some() {
                self.set_attached_fx_colour(fx, effects);
            }
            i -= 1;
        }
        self.specfx.spec_fx_alpha = 255;
    }

    /// Port of `CreateBlastRing` (Common/specfx.c:2230): a generic ring of polys.
    pub fn create_blast_ring(&mut self) {
        let arc_step = 4096 / NUM_RINGSEGS as i32;
        let mut v = vec![FVector::default(); (NUM_RINGSEGS + 1) * 2];
        let mut pb = 0;
        for i in 0..NUM_RINGSEGS {
            let rs = rsin(pb);
            let rc = rcos(pb);
            let k = i * 2;
            v[k] = FVector::new(rs, if i & 1 != 0 { 2048 } else { -2048 }, rc);
            v[k + 1] = FVector::new(rs, if i & 1 != 0 { -2048 } else { 2048 }, rc);
            pb += arc_step;
        }
        self.specfx.ring_vtx = Some(v);
    }

    /// Port of `CreatePickupEffect` (Common/specfx.c:2259): loadsa sparklies.
    #[allow(clippy::too_many_arguments)]
    pub fn create_pickup_effect(&mut self, pl: i32, r1: u8, g1: u8, b1: u8, r2: u8, g2: u8, b2: u8) {
        let p = pl as usize;
        let (Some(t), Some(f)) = (self.frogmove.curr_tile[p], self.frogger.frog[p]) else { return };
        let tn = self.map.tile(t).normal;
        let mut se_up = FVector::default();
        new_set_vector_fs(&mut se_up, &tn);
        scale_vector(&mut se_up, 200);
        let mut pos = SVector::default();
        add_vector_sfs(&mut pos, &se_up, &self.actor2.actors[f].actor.position);
        let mut up = FVector::default();
        new_set_vector_fs(&mut up, &tn);
        for (size, speed, life, c) in [
            (204800, 12288, 20480, (r1, g1, b1)),
            (163480, 10240, 24576, (r2, g2, b2)),
            (122880, 8192, 28672, (r1, g1, b1)),
            (81920, 6144, 32768, (r2, g2, b2)),
        ] {
            let fx = self.create_special_effect_direct(FXTYPE_SPARKLYTRAIL, &pos, &up, size, speed, 0, life);
            if let Some(id) = fx {
                self.set_fx_colour(fx, c.0, c.1, c.2);
                self.specfx.fx_mut(id).gravity = 2870;
            }
        }
    }

    /// Port of `CreateRingEffect` (Common/specfx.c:2299): teleport or invulnerability ring
    /// on a player.
    pub fn create_ring_effect(&mut self, i: i32) {
        let p = i as usize;
        let (Some(t), Some(f)) = (self.frogmove.curr_tile[p], self.frogger.frog[p]) else { return };
        let tn = self.map.tile(t).normal;
        let mut up = FVector::default();
        new_set_vector_fs(&mut up, &tn);
        scale_vector(&mut up, (rsin((self.act_frame_count << 6) as i32) >> 4) + 250);
        let mut pos = SVector::default();
        set_vector_sf(&mut pos, &up);
        add_to_vector_ss(&mut pos, &self.actor2.actors[f].actor.position);
        new_set_vector_fs(&mut up, &tn);
        let fx = self.create_special_effect_direct(FXTYPE_WAKE, &pos, &up, 123840, 1024, 0, 1024);
        let c = self.frogger.frog_pool((self.frogger.player[p].character as usize).min(crate::frogger::FROG_NUMFROGS - 1));
        self.set_fx_colour(fx, c.r, c.g, c.b);
    }

    /// Port of `BabyCollectEffect` (Common/specfx.c:2323): stack of decals at different speeds.
    pub fn baby_collect_effect(&mut self, baby: Option<ActorId>, tile: Option<TileId>, i: i32) {
        // (NULL baby/tile would crash the original.)
        let (Some(baby), Some(tile)) = (baby, tile) else { return };
        if tile.idx() >= self.map.tiles.len() || self.actor2.actors.get(baby).is_none() {
            return;
        }
        let tn = self.map.tile(tile).normal;
        let mut up = FVector::default();
        new_set_vector_fs(&mut up, &tn);
        scale_vector(&mut up, 200);
        let mut pos = SVector::default();
        add_vector_sfs(&mut pos, &up, &self.actor2.actors[baby].actor.position);
        let mut normal = FVector::default();
        new_set_vector_fs(&mut normal, &tn);
        let c = self.babyfrog.baby_list.get(i as usize).map(|b| b.fx_colour).unwrap_or_default();
        let rings = [(65536, 4096, 4096), (32768, 2048, 4915), (16384, 1229, 5734), (8192, 409, 6554), (4096, 205, 7373)];
        for (k, (size, speed, life)) in rings.into_iter().enumerate() {
            if k > 0 {
                add_to_vector_sf(&mut pos, &up);
            }
            let fx = self.create_special_effect_direct(FXTYPE_WAKE, &pos, &normal, size, speed, 82, life);
            self.set_fx_colour(fx, c[0], c[1], c[2]);
        }
        self.actor_animate(baby, crate::babyfrog::BABY_ANIM_COLLECT, false, false, 180, false);
        self.actor_animate(baby, crate::babyfrog::BABY_ANIM_COLLECTHOLD, true, true, 64, false);
        if let Some(f) = self.frogger.frog[0] {
            self.actor_animate(f, crate::frogger::FROG_ANIM_DANCE4, false, false, 77, false);
            self.actor_animate(f, crate::frogger::FROG_ANIM_BREATHE, true, true, 102, false);
        }
    }

    /// Port of `CreateExplodeEffect` (Common/specfx.c:2371): a modest explosion of smoke,
    /// blast and sparks, and shakes the camera.
    pub fn create_explode_effect(&mut self, pos: &SVector, normal: &FVector) {
        let mut p = SVector::default();
        set_vector_sf(&mut p, normal);
        scale_vector_s(&mut p, 200);
        add_to_vector_ss(&mut p, pos);

        let fx = self.create_special_effect(FXTYPE_POLYRING, &p, normal, 81920, 410, 256, 3072);
        self.set_fx_colour(fx, 255, 50, 50);
        self.create_special_effect(FXTYPE_FIERYSMOKE, &p, normal, 102400, 16384, 2048, 10240);
        self.create_special_effect(FXTYPE_FIERYSMOKE, &p, normal, 204800, 12268, 2048, 10240);
        self.create_special_effect(FXTYPE_FIERYSMOKE, &p, normal, 409600, 8191, 2048, 10240);
        let fx = self.create_special_effect(FXTYPE_SPARKBURST, &p, normal, 61440, 2048, 0, 20480);
        if let Some(f) = fx {
            self.set_fx_colour(fx, 255, 80, 80);
            self.specfx.fx_mut(f).gravity = 2048;
        }
        self.cam.cam_shakiness = 500;
        self.cam.cam_shake_falloff = 10;
    }

    /// Port of `CreateRespawnEffect` (Common/specfx.c:2405): for multiplayer and swampy.
    pub fn create_respawn_effect(&mut self, tile: TileId, pl: i32) {
        let (tn, tc) = {
            let t = self.map.tile(tile);
            (t.normal, t.centre)
        };
        let mut pos = FVector::default();
        new_set_vector_fs(&mut pos, &tn);
        scale_vector(&mut pos, 50);
        add_to_vector_fs(&mut pos, &tc);
        let mut tp = SVector::default();
        set_vector_sf(&mut tp, &pos);
        let mut up = FVector::default();
        new_set_vector_fs(&mut up, &tn);
        let c = self.frogger.frog_pool((self.frogger.player[pl as usize].character as usize).min(crate::frogger::FROG_NUMFROGS - 1));
        let fx = self.create_special_effect_direct(FXTYPE_SMOKEBURST, &tp, &up, 262144, 8192, 410, 6963);
        self.set_fx_colour(fx, c.r, c.g, c.b);
        let fx = self.create_special_effect_direct(FXTYPE_SMOKEBURST, &tp, &up, 327680, 8192, 410, 6963);
        self.set_fx_colour(fx, c.r, c.g, c.b);
        let fx = self.create_special_effect_direct(FXTYPE_WAKE, &tp, &up, 81920, 410, 205, 2048);
        self.set_fx_colour(fx, c.r, c.g, c.b);
        self.create_special_effect(FXTYPE_FLASH, &tp, &up, 0, 12384, 512, 2048);
    }

    /// Port of `PrepForPriorityEffect` (Common/specfx.c:2435): remove a low priority
    /// effect if there is not enough room.
    pub fn prep_for_priority_effect(&mut self) {
        // NB: the test is `if( (sfxList.stackPtr-1) ) return;` - only frees at stackPtr == 1.
        if self.specfx.stack_ptr - 1 != 0 || self.specfx.array.is_empty() {
            return;
        }
        for id in self.specfx.ids() {
            let t = self.specfx.array[id as usize].ty;
            if matches!(
                t,
                FXTYPE_TWINKLE
                    | FXTYPE_SMOKE_STATIC
                    | FXTYPE_SMOKE_GROWS
                    | FXTYPE_BUBBLES
                    | FXTYPE_SPARKBURST
                    | FXTYPE_SPARKLYTRAIL
                    | FXTYPE_DECAL
                    | FXTYPE_SPLASH
                    | FXTYPE_WAKE
            ) {
                self.deallocate_fx(Some(SpecFxId(id)), 1);
                break;
            }
        }
    }
}

/// `effect->origin += (normal * SCALE) >> 12` (short arithmetic).
fn offset_origin(e: &mut SpecFx) {
    e.origin.vx = (e.origin.vx as i32).wrapping_add(e.normal.vx.wrapping_mul(SCALE) >> 12) as i16;
    e.origin.vy = (e.origin.vy as i32).wrapping_add(e.normal.vy.wrapping_mul(SCALE) >> 12) as i16;
    e.origin.vz = (e.origin.vz as i32).wrapping_add(e.normal.vz.wrapping_mul(SCALE) >> 12) as i16;
}

/// `(SVECTOR*)&fvector`: reinterprets the first 8 bytes of an `FVECTOR` (little endian).
fn fvector_as_svector(f: &FVector) -> SVector {
    SVector { vx: f.vx as i16, vy: (f.vx >> 16) as i16, vz: f.vy as i16, pad: (f.vy >> 16) as i16 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        let mut g = Game::new();
        g.sprite.init_sprite_list();
        g.init_spec_fx_list();
        g.particle.init_particle_list();
        g
    }

    fn up() -> FVector {
        FVector::new(0, 4096, 0)
    }

    #[test]
    fn decal_fades_and_dies_at_lifetime() {
        let mut g = game();
        let fx = g.create_special_effect(FXTYPE_WAKE, &SVector::new(0, 0, 0), &up(), 81920, 410, 205, 2048).unwrap();
        let e = g.specfx.fx(fx).clone();
        // life = (2048*60)>>12 = 30 frames; fade = 255/30
        assert_eq!(e.lifetime, 30);
        assert_eq!(e.fade, 8);
        assert_eq!(e.origin.vy, 10);
        assert_eq!(e.update, Some(FxUpdate::Decal));
        let mut frames = 0;
        while g.specfx.is_live(fx) {
            g.act_frame_count += 1;
            g.update_special_effects();
            frames += 1;
            assert!(frames < 100);
        }
        assert_eq!(frames, 31);
        assert_eq!(g.specfx.count, 0);
        assert_eq!(*g.specfx.fx(fx), SpecFx::default());
    }

    #[test]
    fn list_groups_by_update_type() {
        let mut g = game();
        let o = SVector::new(0, 0, 0);
        let a = g.create_special_effect(FXTYPE_WAKE, &o, &up(), 4096, 0, 0, 4096).unwrap();
        let b = g.create_special_effect(FXTYPE_POLYRING, &o, &up(), 4096, 0, 0, 4096).unwrap();
        let c = g.create_special_effect(FXTYPE_DECAL, &o, &up(), 4096, 0, 0, 4096).unwrap();
        let order: Vec<SpecFxId> = g.specfx.iter_list().map(|(i, _)| i).collect();
        // c (decal) is inserted before the last decal a; b (ring) at the front.
        assert_eq!(order, vec![b, c, a]);
        assert!(g.specfx.ring_vtx.is_some());
        g.deallocate_fx(Some(a), 1);
        g.deallocate_fx(Some(c), 1);
        let d = g.create_special_effect(FXTYPE_WAKE, &o, &up(), 4096, 0, 0, 4096).unwrap();
        // a was lastAdded[DECAL] and the last in the list: lastAdded[DECAL] became the
        // static head (updateType 0), so new decals go to the end of the list.
        let order: Vec<SpecFxId> = g.specfx.iter_list().map(|(i, _)| i).collect();
        assert_eq!(order, vec![b, d]);
    }

    #[test]
    fn unported_types_allocate_then_free() {
        let mut g = game();
        let sp = g.specfx.stack_ptr;
        assert!(g.create_special_effect(FXTYPE_TRAIL, &SVector::default(), &up(), 4096, 0, 0, 4096).is_none());
        assert!(g.create_special_effect(FXTYPE_WATERRIPPLE, &SVector::default(), &up(), 4096, 0, 0, 4096).is_none());
        assert_eq!(g.specfx.stack_ptr, sp);
        assert_eq!(g.specfx.count, 0);
    }

    #[test]
    fn explode_creation_draws_random_in_order() {
        let mut g = game();
        let seed = g.rng.rseed;
        let fx = g.create_special_effect(FXTYPE_SMOKEBURST, &SVector::new(0, 100, 0), &up(), 204800, 8192, 0, 6963).unwrap();
        // 5 particles x (2 scale + 3 vel + 1 angle) = 30 Random() calls
        let mut r = f2_math::Rng { rseed: seed };
        for _ in 0..30 {
            r.random(1);
        }
        assert_eq!(g.rng.rseed, r.rseed);
        let sprites = g.fx_sprites(fx);
        assert_eq!(sprites.len(), 5);
        assert!(g.specfx.fx(fx).rebound.is_some());
        // runs until every particle faded
        let mut n = 0;
        while g.specfx.is_live(fx) {
            g.act_frame_count += 1;
            g.update_special_effects();
            n += 1;
            assert!(n < 1000);
        }
        assert_eq!(g.sprite.count, 0);
        assert_eq!(g.particle.count, 0);
        assert_eq!(g.specfx.p2_count, 0);
    }

    #[test]
    fn twinkle_lives_and_dies() {
        let mut g = game();
        let fx = g.create_special_effect(FXTYPE_TWINKLE, &SVector::default(), &up(), 81920, 0, 0, 3 << 12);
        g.set_fx_tilt(fx, 12288);
        let fx = fx.unwrap();
        assert_eq!(g.specfx.fx(fx).lifetime, 180);
        for _ in 0..400 {
            g.act_frame_count += 1;
            g.update_special_effects();
        }
        assert!(!g.specfx.is_live(fx));
        assert_eq!(g.sprite.count, 0);
    }

    #[test]
    fn lightning_and_priority() {
        let mut g = game();
        g.create_lightning_effect(&SVector::new(0, 0, 0), &SVector::new(0, 0, 3000), EF_MEDIUM | EF_TINTBLUE, 30, 0);
        assert_eq!(g.specfx.count, 3);
        let (id, e) = g.specfx.iter_list().next().unwrap();
        assert_eq!(e.a, 180);
        assert_eq!((e.r, e.g, e.b), (60, 60, 255));
        assert_eq!(g.specfx.spec_fx_alpha, 255);
        g.act_frame_count += 1;
        g.update_special_effects();
        let ps = g.fx_particles(id);
        assert!(ps.len() >= 4);
        // fill the pool, then a priority effect frees a low-priority one
        while g.create_special_effect(FXTYPE_WAKE, &SVector::default(), &up(), 4096, 0, 0, 40960).is_some() {}
        assert_eq!(g.specfx.stack_ptr, 0);
        let last = g.specfx.iter_list().map(|(i, _)| i).last();
        g.deallocate_fx(last, 1);
        assert_eq!(g.specfx.stack_ptr, 1);
        let before = g.specfx.count;
        g.prep_for_priority_effect();
        assert_eq!(g.specfx.count, before - 1);
    }

    #[test]
    fn view_culling() {
        let v = ViewXform {
            matrix: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 100.0, 1.0]],
            x_res: 640.0,
            y_res: 480.0,
            near_clip: 10.0,
            far_clip: 1200.0,
            horiz_clip: 3000.0,
            vert_clip: 2500.0,
            fov: 450,
            dist: 0,
        };
        assert!(v.is_point_visible(&SVector::new(0, 0, 0)));
        assert!(!v.is_point_visible(&SVector::new(0, 0, -2000)));
        assert!(!v.outcode_check(&SVector::new(0, 0, 0), &SVector::new(0, 0, 100)));
        assert!(v.outcode_check(&SVector::new(0, 0, -2000), &SVector::new(100, 0, -3000)));
    }
}
