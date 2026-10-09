//! `Common/specfx.c`: special effects. **Later wave.** This wave provides the
//! constants and the entry points enemies/platforms call, as no-op stubs.
//! (When ported, effect creation draws `Random()` numbers: the call sites below
//! already sit at their original positions.)

use f2_math::{FVector, Fixed, SVector};

use crate::ids::{EnemyId, PlatformId, SpecFxId};
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

impl Game {
    /// `CreateSpecialEffect` (Common/specfx.c:167). Returns `None` until ported.
    #[allow(clippy::too_many_arguments)]
    pub fn create_special_effect(
        &mut self,
        _ty: i16,
        _origin: &SVector,
        _normal: &FVector,
        _size: Fixed,
        _speed: Fixed,
        _accn: Fixed,
        _lifetime: Fixed,
    ) -> Option<SpecFxId> {
        // TODO(port): specfx.c
        None
    }

    /// `CreateSpecialEffectDirect` (Common/specfx.c:181): `CreateSpecialEffect` without the
    /// visibility test. Returns `None` until ported.
    #[allow(clippy::too_many_arguments)]
    pub fn create_special_effect_direct(
        &mut self,
        _ty: i16,
        _origin: &SVector,
        _normal: &FVector,
        _size: Fixed,
        _speed: Fixed,
        _accn: Fixed,
        _lifetime: Fixed,
    ) -> Option<SpecFxId> {
        // TODO(port): specfx.c
        None
    }

    /// `ProcessAttachedEffects` (Common/specfx.c:1802).
    pub fn process_attached_effects(&mut self, _entity: FxEntity) {
        // TODO(port): specfx.c
    }

    /// `SetAttachedFXColour` (Common/specfx.c:2106).
    pub fn set_attached_fx_colour(&mut self, _fx: Option<SpecFxId>, _effects: u32) {
        // TODO(port): specfx.c
    }

    /// `SetFXColour` (Common/specfx.c:1751).
    pub fn set_fx_colour(&mut self, _fx: Option<SpecFxId>, _r: u8, _g: u8, _b: u8) {
        // TODO(port): specfx.c
    }

    /// `SetFX tilt` on a created effect (`fx->tilt = ...`).
    pub fn set_fx_tilt(&mut self, _fx: Option<SpecFxId>, _tilt: Fixed) {
        // TODO(port): specfx.c
    }

    /// `CreateLightningEffect` (Common/specfx.c:2176).
    pub fn create_lightning_effect(&mut self, _p1: &SVector, _p2: &SVector, _effects: u32, _life: i32, _z_depth_off: i32) {
        // TODO(port): specfx.c
    }

    /// `PrepForPriorityEffect` (Common/specfx.c:2435).
    pub fn prep_for_priority_effect(&mut self) {
        // TODO(port): specfx.c
    }

    /// `CreateRingEffect` (Common/specfx.c:2299).
    pub fn create_ring_effect(&mut self, _i: i32) {
        // TODO(port): specfx.c
    }

    /// `UpdateSpecialEffects` (Common/specfx.c).
    pub fn update_special_effects(&mut self) {
        // TODO(port): specfx.c
    }
}
