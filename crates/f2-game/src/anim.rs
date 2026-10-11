//! Port of `Common/anim.c` / `anim.h`: frog death animations, dispatched through the
//! `deathAnims[]` table that `InitEnemyLinkedList` fills (and called directly by
//! `frogmove.c`).
//!
//! Delayed follow-ups (`MakeTrigger(OnTimeout, ...)` + `AttachEvent(..., DeathAnim, pl,
//! DeathX)`) go through [`Game::make_timeout_event`]; [`Game::run_death_anim`] is the
//! `DeathAnim` event's target.

use crate::prelude::*;

use f2_math::{add_to_vector_fs, new_set_vector_fs, scale_vector, set_vector_sf, FVector, SVector};

use crate::effects::Effect;
use crate::frogger::{
    FANIM_ALL, FROG_ANIM_ASSONFIRE, FROG_ANIM_BOWLEDOVER, FROG_ANIM_DROWN, FROG_ANIM_ELECTROCUTE, FROG_ANIM_SCREENSLIDE, FROG_ANIM_SCREENSPLAT, FROG_ANIM_STUNG,
};
use crate::frogmove::*;
use crate::layout::worldid;
use crate::specfx::{FXTYPE_DECAL, FXTYPE_SMOKEBURST, FXTYPE_SPARKBURST, FXTYPE_SPARKLYTRAIL};
use crate::Game;

/// `SAMPLE_VOLUME` (`PC/pcaudio.c:44`, an options-controlled global; default 255).
pub const SAMPLE_VOLUME: i32 = 255;

/// `ANIM_RANDOM_START` (`anim.h:16`).
pub const ANIM_RANDOM_START: u32 = 1 << 0;

/// `LEVELID_SUBTERRANEAN3` (`layout.h:297`).
const LEVELID_SUBTERRANEAN3: u32 = 2;

// `genSfx[GEN_DEATH*]` (`PC/pcaudio.c:214`): sample names (uid = CRC of the name).
const GEN_DEATHDROWN: &str = "frogdrown1";
const GEN_DEATHCRUSH: &str = "frogcrush";
const GEN_DEATHEXPLODE: &str = "frogexplode";
const GEN_DEATHMOWED: &str = "frogmowed";
const GEN_DEATHGIB: &str = "frog_gib";
const GEN_DEATHCHOP: &str = "frogchop";
const GEN_DEATHELECTRIC: &str = "electrocute";
const GEN_DEATHFIRE: &str = "burnbum";

/// `loopElecAnim[]` (`anim.c:133`), per character.
pub const LOOP_ELEC_ANIM: [i8; 8] = [1, 1, 1, 1, 1, 1, 0, 1];

/// Death animation functions (`anim.h:23`): the `deathAnims[]` entries plus
/// `DeathVanish`/`DeathMulti`, which triggers and multiplayer use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeathAnim {
    Normal,
    RunOver,
    Drowning,
    Squashed,
    Fire,
    Electric,
    Falling,
    Whacking,
    Inflation,
    Poison,
    Slicing,
    Explosion,
    Gibbing,
    Incinerate,
    Bursting,
    Spiked,
    Lightning,
    Multi,
    Vanish,
}

/// The `deathAnims[NUM_DEATHTYPES]` table as `InitEnemyLinkedList` (Common/enemies.c:2541)
/// fills it. `DEATHBY_FALLINGFOREVER` has no entry (NULL in the original).
pub const DEATH_ANIMS: [Option<DeathAnim>; NUM_DEATHTYPES] = [
    Some(DeathAnim::Normal),
    Some(DeathAnim::RunOver),
    Some(DeathAnim::Drowning),
    Some(DeathAnim::Squashed),
    Some(DeathAnim::Fire),
    Some(DeathAnim::Electric),
    Some(DeathAnim::Falling),
    Some(DeathAnim::Whacking),
    Some(DeathAnim::Inflation),
    Some(DeathAnim::Poison),
    Some(DeathAnim::Slicing),
    Some(DeathAnim::Explosion),
    Some(DeathAnim::Gibbing),
    Some(DeathAnim::Incinerate),
    Some(DeathAnim::Bursting),
    Some(DeathAnim::Spiked),
    Some(DeathAnim::Lightning),
    None,
];

impl Game {
    /// `deathAnims[n](pl)`. An index without a function (NULL / out of range) would
    /// crash the original; the port logs it.
    pub fn call_death_anim(&mut self, n: i32, pl: i32) {
        match usize::try_from(n).ok().and_then(|i| DEATH_ANIMS.get(i).copied().flatten()) {
            Some(d) => self.run_death_anim(d, pl),
            None => self.effects.push(Effect::Log { text: format!("deathAnims[{n}] is NULL (player {pl})") }),
        }
    }

    /// Calls one death function (the `DeathAnim` event of `evenfunc.c` calls a
    /// `void (*)(int)` taken from its trigger arguments).
    pub fn run_death_anim(&mut self, d: DeathAnim, pl: i32) {
        match d {
            DeathAnim::Normal => self.death_normal(pl),
            DeathAnim::RunOver => self.death_run_over(pl),
            DeathAnim::Drowning => self.death_drowning(pl),
            DeathAnim::Squashed => self.death_squashed(pl),
            DeathAnim::Fire => self.death_fire(pl),
            DeathAnim::Electric => self.death_electric(pl),
            DeathAnim::Falling => self.death_falling(pl),
            DeathAnim::Whacking => self.death_whacking(pl),
            DeathAnim::Inflation => self.death_inflation(pl),
            DeathAnim::Poison => self.death_poison(pl),
            DeathAnim::Slicing => self.death_slicing(pl),
            DeathAnim::Explosion => self.death_explosion(pl),
            DeathAnim::Gibbing => self.death_gibbing(pl),
            DeathAnim::Incinerate => self.death_incinerate(pl),
            DeathAnim::Bursting => self.death_bursting(pl),
            DeathAnim::Spiked => self.death_spiked(pl),
            DeathAnim::Lightning => self.death_lightning(pl),
            DeathAnim::Multi => self.death_multi(pl),
            DeathAnim::Vanish => self.death_vanish(pl),
        }
    }

    // ----- helpers -----

    /// `actorAnimate(frog[pl]->actor, ...)` (no-op without a frog actor).
    fn frog_animate(&mut self, pl: i32, anim: i32, looped: bool, queue: bool, speed: i32) {
        if let Some(f) = self.frogger.frog[pl as usize] {
            if self.actor2.actors.get(f).is_some() {
                self.actor_animate(f, anim, looped, queue, speed, false);
            }
        }
    }

    /// `ChangeModel(frog[pl]->actor, name)`.
    fn frog_change_model(&mut self, pl: i32, name: &str) {
        if let Some(f) = self.frogger.frog[pl as usize] {
            if self.actor2.actors.get(f).is_some() {
                self.change_model(f, name);
            }
        }
    }

    /// `frogPool[player[pl].character].anim < FANIM_ALL`.
    fn frog_lacks_death_anims(&self, pl: i32) -> bool {
        let ch = self.frogger.player[pl as usize].character as usize;
        self.frogger.frog_pool(ch.min(crate::frogger::FROG_NUMFROGS - 1)).anim < FANIM_ALL
    }

    /// `frogPool[player[pl].character].{r,g,b}`.
    fn frog_colour(&self, pl: i32) -> (u8, u8, u8) {
        let ch = self.frogger.player[pl as usize].character as usize;
        let f = self.frogger.frog_pool(ch.min(crate::frogger::FROG_NUMFROGS - 1));
        (f.r, f.g, f.b)
    }

    /// `if( frog[pl]->actor->shadow ) frog[pl]->actor->shadow->draw = 0;`
    fn frog_hide_shadow(&mut self, pl: i32) {
        if let Some(f) = self.frogger.frog[pl as usize] {
            if let Some(a) = self.actor2.actors.get_mut(f) {
                if let Some(s) = a.actor.shadow.as_mut() {
                    s.draw = 0;
                }
            }
        }
    }

    /// `frog[pl]->draw = 0`.
    fn frog_hide(&mut self, pl: i32) {
        if let Some(f) = self.frogger.frog[pl as usize] {
            if let Some(a) = self.actor2.actors.get_mut(f) {
                a.draw = 0;
            }
        }
    }

    /// `currTile[pl]->normal` copied field-wise into an `FVECTOR` (`SetVectorFF` on
    /// the `SVECTOR` tile normal).
    fn curr_tile_up(&self, pl: i32) -> FVector {
        let mut up = FVector::default();
        if let Some(t) = self.frogmove.curr_tile[pl as usize] {
            new_set_vector_fs(&mut up, &self.map.tiles[t.idx()].normal);
        }
        up
    }

    /// `PlayVoice(pl, name)`.
    fn death_voice(&mut self, pl: i32, name: &str) {
        self.effects.push(Effect::PlayVoice { player: pl as u8, uid: f2_formats::crc::crc(name) });
    }

    /// `PlaySample(genSfx[GEN_x], NULL, 0, SAMPLE_VOLUME, -1)`.
    fn death_sample(&mut self, name: &str) {
        self.play_sample(f2_formats::crc::crc(name), None, 0, SAMPLE_VOLUME, -1);
    }

    /// `t = MakeTrigger(OnTimeout, actFrameCount + delay); AttachEvent(t, TRIGGER_ONCE,
    /// 0, DeathAnim, pl, func)`.
    fn death_anim_trigger(&mut self, pl: i32, delay: u32, func: DeathAnim) {
        self.make_timeout_event(delay, crate::event::EventFunc::DeathAnim { pl, func });
    }

    /// The `SPARKBURST, SPARKBURST, DECAL` trio of `DeathSlicing`/`Explosion`/`Gibbing`.
    fn death_gib_effects(&mut self, pl: i32) {
        let up = self.curr_tile_up(pl);
        let pos = self.frog_position(pl as usize);
        let fx = self.create_special_effect(FXTYPE_SPARKBURST, &pos, &up, 81920, 16384, 0, 20480);
        if fx.is_some() {
            self.set_fx_colour(fx, 255, 255, 0);
            self.set_fx_gravity(fx, 4100);
        }
        let fx = self.create_special_effect(FXTYPE_SPARKBURST, &pos, &up, 81920, 12288, 0, 20480);
        if fx.is_some() {
            self.set_fx_colour(fx, 255, 255, 130);
            self.set_fx_gravity(fx, 4100);
        }
        let fx = self.create_special_effect(FXTYPE_DECAL, &pos, &up, 163840, 0, 0, 8192);
        if fx.is_some() {
            let (r, g, b) = self.frog_colour(pl);
            self.set_fx_colour(fx, r, g, b);
            let t = self.specfx.textures.txtr_solid_ring;
            if let Some(f) = fx {
                self.specfx.fx_mut(f).tex = t;
            }
        }
    }

    // ----- death functions -----

    /// Port of `DeathVanish` (Common/anim.c:37).
    pub fn death_vanish(&mut self, pl: i32) {
        if self.frog_lacks_death_anims(pl) {
            self.death_normal(pl);
            return;
        }
        self.frog_change_model(pl, "nothing");
    }

    /// Port of `DeathNormal` (Common/anim.c:56).
    pub fn death_normal(&mut self, pl: i32) {
        let p = pl as usize;
        self.frogger.player[p].death_by = DEATHBY_NORMAL as u8;
        self.frog_animate(pl, FROG_ANIM_BOWLEDOVER, false, false, 128);
        self.frogger.player[p].dead.init(3);
        self.death_voice(pl, "frogdeath");
    }

    /// Port of `DeathRunOver` (Common/anim.c:65).
    pub fn death_run_over(&mut self, pl: i32) {
        self.death_normal(pl);
    }

    /// Port of `DeathDrowning` (Common/anim.c:70).
    pub fn death_drowning(&mut self, pl: i32) {
        let p = pl as usize;
        self.frogger.player[p].dead.init(3);
        self.frogger.player[p].death_by = DEATHBY_DROWNING as u8;
        self.frogger.player[p].frog_state |= FROGSTATUS_ISDEAD;
        self.frogger.player[p].is_sinking = 0;

        self.frog_animate(pl, FROG_ANIM_DROWN, false, false, 64);

        // (The `if( Random(2) )` choice is commented out: both sounds play.)
        self.death_sample(GEN_DEATHDROWN);
        self.death_voice(pl, "frogdrown2");
    }

    /// Port of `DeathSquashed` (Common/anim.c:95).
    pub fn death_squashed(&mut self, pl: i32) {
        if self.frog_lacks_death_anims(pl) {
            self.death_normal(pl);
            return;
        }
        let p = pl as usize;
        self.frogger.player[p].death_by = DEATHBY_SQUASHED as u8;
        self.frogger.player[p].idle_enable = 0;

        // Drop in replacement model and animate gib explosion
        self.frog_change_model(pl, "dth-flat");

        self.frog_animate(pl, 0, false, false, 51);
        self.frog_animate(pl, 1, true, true, 51);

        self.frogger.player[p].dead.init(3);

        self.death_sample(GEN_DEATHCRUSH);
    }

    /// Port of `DeathFire` (Common/anim.c:117).
    pub fn death_fire(&mut self, pl: i32) {
        let p = pl as usize;
        self.frogger.player[p].death_by = DEATHBY_FIRE as u8;
        self.frog_animate(pl, FROG_ANIM_ASSONFIRE, false, false, 128);
        self.death_sample(GEN_DEATHFIRE);

        self.bounce_frog(pl, 4096000, 40);

        self.frogger.player[p].dead.init(3);

        // t = MakeTrigger( OnTimeout, (void *)(actFrameCount + 120), NULL, NULL, NULL );
        // AttachEvent( t, TRIGGER_ONCE, 0, DeathAnim, (void *)pl, (void *)DeathNormal, NULL, NULL );
        self.death_anim_trigger(pl, 120, DeathAnim::Normal);
    }

    /// Port of `DeathElectric` (Common/anim.c:137).
    pub fn death_electric(&mut self, pl: i32) {
        if self.frog_lacks_death_anims(pl) {
            self.death_normal(pl);
            return;
        }
        let p = pl as usize;
        let ch = (self.frogger.player[p].character as usize).min(crate::frogger::FROG_NUMFROGS - 1);

        // So the changing thing works right
        self.frog_change_model(pl, self.frogger.frog_pool(ch).single_model);

        self.frogger.player[p].death_by = DEATHBY_ELECTRIC as u8;
        self.frog_animate(pl, FROG_ANIM_ELECTROCUTE, LOOP_ELEC_ANIM[ch] != 0, false, 128);
        self.frogger.player[p].dead.init(4);

        // t = MakeTrigger( OnTimeout, (void *)(actFrameCount + 80), NULL, NULL, NULL );
        // AttachEvent( t, TRIGGER_ONCE, 0, DeathAnim, (void *)pl, (void *)DeathIncinerate, NULL, NULL );
        self.death_anim_trigger(pl, 80, DeathAnim::Incinerate);

        let mut pos = self.curr_tile_up(pl);
        scale_vector(&mut pos, 50);
        if let Some(t) = self.frogmove.curr_tile[p] {
            add_to_vector_fs(&mut pos, &self.map.tiles[t.idx()].centre);
        }
        let mut tp = SVector::default();
        set_vector_sf(&mut tp, &pos);
        let up = self.curr_tile_up(pl);

        let fx = self.create_special_effect_direct(FXTYPE_SPARKLYTRAIL, &tp, &up, 163840, 8192, 0, 20480);
        if fx.is_some() {
            self.set_fx_gravity(fx, 4096);
            let (r, g, b) = self.frog_colour(pl);
            self.set_fx_colour(fx, r, g, b);
        }
        let fx = self.create_special_effect_direct(FXTYPE_SPARKLYTRAIL, &tp, &up, 163840, 8192, 0, 20480);
        if fx.is_some() {
            self.set_fx_gravity(fx, 4096);
            self.set_fx_colour(fx, 220, 60, 60);
        }

        self.death_sample(GEN_DEATHELECTRIC);
    }

    /// Port of `DeathFalling` (Common/anim.c:180).
    pub fn death_falling(&mut self, pl: i32) {
        self.death_normal(pl);
    }

    /// Port of `DeathWhacking` (Common/anim.c:185).
    pub fn death_whacking(&mut self, pl: i32) {
        let p = pl as usize;
        self.frog_hide_shadow(pl);

        self.frogger.player[p].death_by = DEATHBY_WHACKING as u8;

        self.throw_frog_at_screen(pl);
        self.frog_animate(pl, FROG_ANIM_SCREENSPLAT, false, false, 64);
        self.frog_animate(pl, FROG_ANIM_SCREENSLIDE, false, true, 64);

        self.frogger.player[p].dead.init(5);

        self.death_voice(pl, "frogdeath");

        // t = MakeTrigger( OnTimeout, (void *)(actFrameCount + 40), NULL, NULL, NULL );
        // AttachEvent( t, TRIGGER_ONCE, 0, PlaySFX, (void *)FindSample(utilStr2CRC("frogcrush")), NULL, NULL, NULL );
        self.make_timeout_event(
            40,
            crate::event::EventFunc::PlaySfx { sample: crate::event::SfxSample::Sample(f2_formats::crc::crc("frogcrush")), point: None },
        );
    }

    /// Port of `DeathInflation` (Common/anim.c:205).
    pub fn death_inflation(&mut self, pl: i32) {
        if self.frog_lacks_death_anims(pl) {
            self.death_normal(pl);
            return;
        }
        let p = pl as usize;
        self.frogger.player[p].death_by = DEATHBY_INFLATION as u8;
        self.frogger.player[p].idle_enable = 0;

        // Drop in replacement model and animate gib explosion
        self.frog_change_model(pl, "dth-ball");

        // Randomly play different type of inflation anim
        if self.rng.random(2) != 0 {
            self.frog_animate(pl, 0, false, false, 51);
            self.frog_animate(pl, 2, true, true, 51);
        } else {
            self.frog_animate(pl, 1, false, false, 51);
            self.frog_animate(pl, 3, true, true, 51);
        }

        self.frogger.player[p].dead.init(3);

        self.death_voice(pl, "frogdeath");
    }

    /// Port of `DeathPoison` (Common/anim.c:236).
    pub fn death_poison(&mut self, pl: i32) {
        let p = pl as usize;
        self.frogger.player[p].death_by = DEATHBY_POISON as u8;
        self.frog_animate(pl, FROG_ANIM_STUNG, false, false, 128);
        let up = self.curr_tile_up(pl);

        let pos = self.frog_position(p);
        let fx = self.create_special_effect(FXTYPE_SPARKBURST, &pos, &up, 81920, 16384, 0, 20480);
        if fx.is_some() {
            self.set_fx_colour(fx, 255, 255, 0);
            self.set_fx_gravity(fx, 4100);
        }

        self.frogger.player[p].dead.init(3);

        self.death_voice(pl, "frogdeath");
    }

    /// Port of `DeathSlicing` (Common/anim.c:256).
    pub fn death_slicing(&mut self, pl: i32) {
        if self.frog_lacks_death_anims(pl) {
            self.death_normal(pl);
            return;
        }
        let p = pl as usize;
        self.frogger.player[p].death_by = DEATHBY_SLICING as u8;
        self.frogger.player[p].idle_enable = 0;

        self.frog_hide_shadow(pl);

        // Drop in replacement model and animate halves falling apart
        self.frog_change_model(pl, "dth-half");

        self.frog_animate(pl, 0, false, false, 64);
        self.death_gib_effects(pl);
        self.frogger.player[p].dead.init(3);

        self.death_sample(GEN_DEATHCHOP);
    }

    /// Port of `DeathExplosion` (Common/anim.c:298).
    pub fn death_explosion(&mut self, pl: i32) {
        if self.frog_lacks_death_anims(pl) {
            self.death_normal(pl);
            return;
        }
        let p = pl as usize;
        self.frog_hide_shadow(pl);

        self.frogger.player[p].death_by = DEATHBY_EXPLOSION as u8;
        self.frogger.player[p].idle_enable = 0;
        self.frogger.player[p].jump_time = -1; // ds- stop moving

        // Drop in replacement model and animate gib explosion
        self.frog_change_model(pl, "dth-gib");

        self.frog_animate(pl, 0, false, false, 51);
        self.frog_animate(pl, 2, true, true, 51);
        self.death_gib_effects(pl);
        self.frogger.player[p].dead.init(3);

        // Make invisibly after explosion
        // t = MakeTrigger( OnTimeout, (void *)(actFrameCount + 25), NULL, NULL, NULL );
        // AttachEvent( t, TRIGGER_ONCE, 0, DeathAnim, (void *)pl, (void *)DeathVanish, NULL, NULL );
        self.death_anim_trigger(pl, 25, DeathAnim::Vanish);

        self.death_sample(GEN_DEATHEXPLODE);
    }

    /// Port of `DeathGibbing` (Common/anim.c:347).
    pub fn death_gibbing(&mut self, pl: i32) {
        if self.frog_lacks_death_anims(pl) {
            self.death_normal(pl);
            return;
        }
        let p = pl as usize;
        self.frog_hide_shadow(pl);

        self.frogger.player[p].death_by = DEATHBY_GIBBING as u8;
        self.frogger.player[p].idle_enable = 0;
        self.frogger.player[p].jump_time = -1; // ds- stop moving

        // Drop in replacement model and animate gib explosion
        self.frog_change_model(pl, "dth-gib");

        self.frog_animate(pl, 1, false, false, 51);
        self.frog_animate(pl, 3, true, true, 51);
        self.death_gib_effects(pl);
        self.frogger.player[p].dead.init(3);

        if self.frogger.player[0].world_num as u32 != worldid::GARDEN {
            self.death_sample(GEN_DEATHGIB);
        } else {
            self.death_sample(GEN_DEATHMOWED);
        }
    }

    /// Port of `DeathIncinerate` (Common/anim.c:394).
    pub fn death_incinerate(&mut self, pl: i32) {
        if self.frog_lacks_death_anims(pl) {
            self.death_normal(pl);
            return;
        }
        let p = pl as usize;
        self.frogger.player[p].death_by = DEATHBY_INCINERATE as u8;
        self.frogger.player[p].idle_enable = 0;

        self.frog_hide_shadow(pl);

        // Drop in replacement model and animate frog becoming a pile of ash
        self.frog_change_model(pl, "dth-ash");

        self.frog_animate(pl, 0, false, false, 80);
        self.frog_animate(pl, 1, true, true, 80);
        let up = self.curr_tile_up(pl);

        let pos = self.frog_position(p);
        self.create_special_effect(FXTYPE_SMOKEBURST, &pos, &up, 204800, 8192, 0, 6963);

        self.frogger.player[p].dead.init(3);

        self.death_voice(pl, "frogdeath");
    }

    /// Port of `DeathBursting` (Common/anim.c:422).
    pub fn death_bursting(&mut self, pl: i32) {
        if self.frog_lacks_death_anims(pl) {
            self.death_normal(pl);
            return;
        }

        self.death_inflation(pl);
        self.frogger.player[pl as usize].dead.init(4);

        // t = MakeTrigger( OnTimeout, (void *)(actFrameCount + 120), NULL, NULL, NULL );
        // AttachEvent( t, TRIGGER_ONCE, 0, DeathAnim, (void *)pl, (void *)DeathExplosion, NULL, NULL );
        self.death_anim_trigger(pl, 120, DeathAnim::Explosion);
    }

    /// Port of `DeathSpiked` (Common/anim.c:439).
    pub fn death_spiked(&mut self, pl: i32) {
        if self.frog_lacks_death_anims(pl) {
            self.death_normal(pl);
            return;
        }
        let p = pl as usize;
        let ch = (self.frogger.player[p].character as usize).min(crate::frogger::FROG_NUMFROGS - 1);

        // So the changing thing works right
        self.frog_change_model(pl, self.frogger.frog_pool(ch).single_model);

        self.frogger.player[p].death_by = DEATHBY_SPIKED as u8;
        self.frog_animate(pl, FROG_ANIM_ASSONFIRE, false, false, 128);
        self.death_voice(pl, "frogdeath");

        if let Some(t) = self.frogmove.curr_tile[p] {
            let pos = self.frog_position(p);
            let (centre, normal) = (self.map.tiles[t.idx()].centre, self.map.tiles[t.idx()].normal);
            self.calculate_frog_jump_s(&pos, &centre, &normal, 5000000, 40, pl);
        }

        self.frogger.player[p].dead.init(3);

        // t = MakeTrigger( OnTimeout, (void *)(actFrameCount + 35), NULL, NULL, NULL );
        // AttachEvent( t, TRIGGER_ONCE, 0, DeathAnim, (void *)pl, (void *)DeathGibbing, NULL, NULL );
        self.death_anim_trigger(pl, 35, DeathAnim::Gibbing);
    }

    /// Port of `DeathMulti` (Common/anim.c:464): a standard multiplayer death.
    pub fn death_multi(&mut self, pl: i32) {
        if let Some(t) = self.frogmove.curr_tile[pl as usize] {
            self.create_respawn_effect(t, pl);
        }

        self.death_sample(GEN_DEATHEXPLODE);

        let p = pl as usize;
        self.frogger.player[p].frog_state |= FROGSTATUS_ISDEAD;
        self.frogger.player[p].dead.init(3);
        self.frog_hide(pl);
    }

    /// Port of `DeathLightning` (Common/anim.c:476).
    pub fn death_lightning(&mut self, pl: i32) {
        let p = pl as usize;
        let up = self.curr_tile_up(pl);
        let mut pos = self.curr_tile_up(pl);
        scale_vector(&mut pos, 50);
        add_to_vector_fs(&mut pos, &self.frog_position(p));
        let mut tp = SVector::default();
        set_vector_sf(&mut tp, &pos);

        let fx = self.create_special_effect_direct(FXTYPE_SMOKEBURST, &tp, &up, 262144, 8192, 410, 6963);
        self.set_fx_colour(fx, 200, 200, 255);
        let fx = self.create_special_effect_direct(FXTYPE_SMOKEBURST, &tp, &up, 327680, 8192, 410, 6963);
        self.set_fx_colour(fx, 255, 255, 255);

        let fx = self.create_special_effect_direct(FXTYPE_SPARKBURST, &tp, &up, 72200, 8192, 0, 20480);
        if fx.is_some() {
            self.set_fx_gravity(fx, 4096);
            let (r, g, b) = self.frog_colour(pl);
            self.set_fx_colour(fx, r, g, b);
        }
        let fx = self.create_special_effect_direct(FXTYPE_SPARKBURST, &tp, &up, 61149, 8192, 0, 20480);
        if fx.is_some() {
            self.set_fx_gravity(fx, 4096);
            self.set_fx_colour(fx, 220, 60, 60);
        }

        if self.frogger.player[0].world_num as u32 == worldid::SUBTERRANEAN && self.frogger.player[0].level_num as u32 == LEVELID_SUBTERRANEAN3 {
            self.death_sample(GEN_DEATHEXPLODE);
        } else {
            self.death_sample(GEN_DEATHELECTRIC);
        }

        self.frogger.player[p].frog_state |= FROGSTATUS_ISDEAD;
        self.frogger.player[p].dead.init(3);
        self.frog_hide(pl);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor2::Actor2;
    use crate::ids::{ActorId, TileId};
    use crate::map::GameTile;

    fn game() -> Game {
        let mut g = Game::new();
        g.map.tiles.push(GameTile { normal: SVector { vx: 0, vy: 4096, vz: 0, pad: 0 }, ..Default::default() });
        g.frogmove.curr_tile[0] = Some(TileId(0));
        let a = Actor2 { draw: 1, ..Default::default() };
        g.actor2.actors.slots.push(Some(a));
        g.frogger.frog[0] = Some(ActorId(0));
        g
    }

    #[test]
    fn table_dispatch_sets_death_by() {
        for (n, d) in DEATH_ANIMS.iter().enumerate() {
            let mut g = game();
            g.call_death_anim(n as i32, 0);
            let pl = &g.frogger.player[0];
            match d {
                None => assert!(matches!(g.effects[0], Effect::Log { .. })),
                // Death functions that defer to DeathNormal (RunOver, Falling).
                Some(DeathAnim::RunOver | DeathAnim::Falling) => assert_eq!(pl.death_by as i32, DEATHBY_NORMAL),
                // Bursting is DeathInflation + a trigger.
                Some(DeathAnim::Bursting) => assert_eq!(pl.death_by as i32, DEATHBY_INFLATION),
                // Lightning doesn't set deathBy (the caller does).
                Some(DeathAnim::Lightning) => assert_eq!(pl.death_by, 0),
                Some(_) => assert_eq!(pl.death_by as usize, n, "{d:?}"),
            }
            if d.is_some() {
                let expect = match d {
                    Some(DeathAnim::Electric | DeathAnim::Bursting) => 4,
                    Some(DeathAnim::Whacking) => 5,
                    _ => 3,
                };
                assert_eq!(pl.dead.time, expect, "{d:?}");
            }
        }
    }

    #[test]
    fn drowning_marks_dead_and_plays_both_sounds() {
        let mut g = game();
        g.frogger.player[0].is_sinking = 1;
        g.death_drowning(0);
        let pl = &g.frogger.player[0];
        assert_eq!(pl.death_by as i32, DEATHBY_DROWNING);
        assert_ne!(pl.frog_state & FROGSTATUS_ISDEAD, 0);
        assert_eq!(pl.is_sinking, 0);
        assert_eq!(pl.dead.time, 3);
        assert_eq!(g.effects.len(), 2);
        assert!(matches!(g.effects[0], Effect::PlaySample { uid, .. } if uid == f2_formats::crc::crc("frogdrown1")));
    }

    #[test]
    fn inflation_draws_one_random() {
        let mut g = game();
        let mut r = g.rng;
        g.death_inflation(0);
        r.random(2);
        assert_eq!(g.rng, r);
        assert_eq!(g.frogger.player[0].idle_enable, 0);
    }

    #[test]
    fn multi_and_lightning_hide_frog() {
        let mut g = game();
        g.death_multi(0);
        assert_eq!(g.actor2.actors[ActorId(0)].draw, 0);
        assert_ne!(g.frogger.player[0].frog_state & FROGSTATUS_ISDEAD, 0);
        let mut g = game();
        g.frogger.player[0].world_num = worldid::SUBTERRANEAN as u8;
        g.frogger.player[0].level_num = LEVELID_SUBTERRANEAN3 as u8;
        g.death_lightning(0);
        assert_eq!(g.actor2.actors[ActorId(0)].draw, 0);
        assert!(matches!(g.effects[0], Effect::PlaySample { uid, .. } if uid == f2_formats::crc::crc("frogexplode")));
    }

    #[test]
    fn explosion_stops_jump() {
        let mut g = game();
        g.death_explosion(0);
        assert_eq!(g.frogger.player[0].jump_time, -1);
        assert_eq!(g.frogger.player[0].death_by as i32, DEATHBY_EXPLOSION);
    }
}
