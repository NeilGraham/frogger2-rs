//! P6-1: gameplay options of the enhanced profile (`docs/enhancements.md`).
//!
//! Not a port of anything: the retail game has no such options. Everything here is behind
//! [`GameOptions`]; with [`GameProfile::Authentic`] (the default, used by every parity test)
//! none of the hooks changes the simulation, so the demo traces stay identical to the
//! oracle. Each toggle only takes effect in [`GameProfile::Enhanced`].
//!
//! Hooks (all small, marked `// P6-1` at the call sites):
//! * `KillFrog` (frogmove.rs): [`GameOptions::infinite_lives`] skips `lives--`;
//! * `NMEDamageFrog` (enemies.rs): [`GameOptions::invulnerable`] ignores enemy hits;
//! * `RunGameLoop` (game.rs): [`Game::enhance_enemy_tick`] slows enemies and platforms
//!   ([`GameOptions::enemy_speed`]);
//! * `FrogOnPlatform` (evenfunc.rs): [`GameOptions::fix_tube_softlock`] (known bug #11).

use serde::{Deserialize, Serialize};

use crate::Game;

/// Behaviour profile of the simulation (mirrors the host's display profile).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameProfile {
    /// Retail behaviour, bit-identical (parity).
    #[default]
    Authentic,
    /// Fixes and assists, each individually toggleable.
    Enhanced,
}

/// Retail enemy/platform speed in percent.
pub const ENEMY_SPEED_RETAIL: u32 = 100;
/// Slowest selectable enemy speed in percent.
pub const ENEMY_SPEED_MIN: u32 = 25;

/// Gameplay options (part of [`crate::session::SessionConfig`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameOptions {
    pub profile: GameProfile,
    /// Assist: dying never costs a life.
    pub infinite_lives: bool,
    /// Assist: enemies cannot hurt the frog (hazard tiles, falls and scripted deaths still
    /// kill; combine with `infinite_lives`).
    pub invulnerable: bool,
    /// Assist: enemy and platform speed in percent ([`ENEMY_SPEED_MIN`]..=100).
    pub enemy_speed: u32,
    /// Fix #11 (`research/known-bugs.md`): a dead frog does not count as riding a platform
    /// in script triggers, so the Swampy 3 vacuum tubes cannot fire out a dead frog that
    /// then respawns behind the parked tube (softlock).
    pub fix_tube_softlock: bool,
}

impl Default for GameOptions {
    fn default() -> Self {
        GameOptions::AUTHENTIC
    }
}

impl GameOptions {
    /// Retail behaviour.
    pub const AUTHENTIC: GameOptions = GameOptions { profile: GameProfile::Authentic, infinite_lives: false, invulnerable: false, enemy_speed: ENEMY_SPEED_RETAIL, fix_tube_softlock: false };
    /// Enhanced profile defaults: fixes on, assists off.
    pub const ENHANCED: GameOptions = GameOptions { profile: GameProfile::Enhanced, fix_tube_softlock: true, ..GameOptions::AUTHENTIC };

    fn enhanced(&self) -> bool {
        self.profile == GameProfile::Enhanced
    }

    /// Infinite lives in effect.
    pub fn infinite_lives_on(&self) -> bool {
        self.enhanced() && self.infinite_lives
    }

    /// Invulnerability (to enemies) in effect.
    pub fn invulnerable_on(&self) -> bool {
        self.enhanced() && self.invulnerable
    }

    /// Fix #11 in effect.
    pub fn fix_tube_softlock_on(&self) -> bool {
        self.enhanced() && self.fix_tube_softlock
    }

    /// Enemy/platform speed in effect, percent (100 in the authentic profile).
    pub fn enemy_speed_pct(&self) -> u32 {
        if self.enhanced() {
            self.enemy_speed.clamp(ENEMY_SPEED_MIN, ENEMY_SPEED_RETAIL)
        } else {
            ENEMY_SPEED_RETAIL
        }
    }
}

/// Simulation-side state of the enhancements (lives in [`Game::enhance`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnhanceState {
    pub opts: GameOptions,
    /// Enemy-speed accumulator (percent units): enemies/platforms update on the ticks where
    /// it reaches 100.
    pub enemy_acc: u32,
}

impl Game {
    /// Sets the gameplay options (host menu, config).
    pub fn set_game_options(&mut self, opts: GameOptions) {
        self.enhance.opts = opts;
    }

    /// The options in effect: recorded demos (`DEMO_MODE`) always replay with retail
    /// behaviour, whatever the profile.
    pub fn game_options(&self) -> &GameOptions {
        static AUTHENTIC: GameOptions = GameOptions::AUTHENTIC;
        if self.game_state.mode == crate::define::mode::DEMO_MODE {
            &AUTHENTIC
        } else {
            &self.enhance.opts
        }
    }

    /// Slower enemies (enhanced assist): whether `UpdatePlatforms`/`UpdateEnemies` run this
    /// tick. Always `true` at 100 % (authentic). On a skipped tick every enemy and platform
    /// is frozen in time: their path time stamps (`path.startFrame/endFrame`, compared with
    /// `actFrameCount`) move forward by one frame, so they resume exactly where they were;
    /// baby frogs (updated by `UpdateEnemies`) still update.
    pub fn enhance_enemy_tick(&mut self) -> bool {
        let pct = self.game_options().enemy_speed_pct();
        if pct >= ENEMY_SPEED_RETAIL {
            return true;
        }
        self.enhance.enemy_acc += pct;
        if self.enhance.enemy_acc >= ENEMY_SPEED_RETAIL {
            self.enhance.enemy_acc -= ENEMY_SPEED_RETAIL;
            return true;
        }
        for e in self.enemies.enemy_list.array.iter_mut().filter(|e| e.linked) {
            e.path.start_frame = e.path.start_frame.wrapping_add(1);
            e.path.end_frame = e.path.end_frame.wrapping_add(1);
        }
        for p in self.platform.platform_list.array.iter_mut().filter(|p| p.linked) {
            p.path.start_frame = p.path.start_frame.wrapping_add(1);
            p.path.end_frame = p.path.end_frame.wrapping_add(1);
        }
        self.update_babies();
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentic_ignores_toggles() {
        let o = GameOptions { infinite_lives: true, invulnerable: true, enemy_speed: 50, ..GameOptions::AUTHENTIC };
        assert!(!o.infinite_lives_on());
        assert!(!o.invulnerable_on());
        assert_eq!(o.enemy_speed_pct(), 100);
        let e = GameOptions { profile: GameProfile::Enhanced, ..o };
        assert!(e.infinite_lives_on());
        assert!(e.invulnerable_on());
        assert_eq!(e.enemy_speed_pct(), 50);
        assert_eq!(GameOptions { enemy_speed: 1, ..e }.enemy_speed_pct(), ENEMY_SPEED_MIN);
        assert_eq!(GameOptions::default(), GameOptions::AUTHENTIC);
    }

    #[test]
    fn invulnerable_ignores_enemy_hits() {
        let mut g = Game::new();
        g.game_state.mode = crate::define::mode::INGAME_MODE;
        g.enemies.enemy_list.array.push(crate::enemies::Enemy { linked: true, ..Default::default() });
        g.enhance.opts = GameOptions { profile: GameProfile::Enhanced, invulnerable: true, ..GameOptions::AUTHENTIC };
        g.frogger.player[0].health_points = 1;
        g.nme_damage_frog(0, Some(crate::ids::EnemyId(0)));
        assert_eq!(g.frogger.player[0].frog_state & crate::frogmove::FROGSTATUS_ISDEAD, 0);
        assert_eq!(g.frogger.player[0].health_points, 1);
    }

    #[test]
    fn dead_frog_is_not_on_a_platform() {
        let mut g = Game::new();
        g.platform.platform_list.array.push(crate::platform::Platform { linked: true, uid: 3, ..Default::default() });
        g.platform.curr_platform[0] = Some(crate::ids::PlatformId(0));
        g.frogger.player[0].frog_state |= crate::frogmove::FROGSTATUS_ISDEAD;
        assert!(g.frog_on_platform(0, 3), "authentic: KillFrog leaves currPlatform set");
        g.enhance.opts = GameOptions::ENHANCED;
        assert!(!g.frog_on_platform(0, 3));
        g.frogger.player[0].frog_state = 0;
        assert!(g.frog_on_platform(0, 3));
        g.enhance.opts.fix_tube_softlock = false;
        g.frogger.player[0].frog_state |= crate::frogmove::FROGSTATUS_ISDEAD;
        assert!(g.frog_on_platform(0, 3));
    }

    #[test]
    fn demos_replay_authentic() {
        let mut g = Game::new();
        g.enhance.opts = GameOptions { profile: GameProfile::Enhanced, infinite_lives: true, enemy_speed: 50, ..GameOptions::AUTHENTIC };
        assert!(g.game_options().infinite_lives_on());
        g.game_state.mode = crate::define::mode::DEMO_MODE;
        assert_eq!(*g.game_options(), GameOptions::AUTHENTIC);
        assert!((0..4).all(|_| g.enhance_enemy_tick()));
    }

    #[test]
    fn enemy_speed_skips_ticks_and_shifts_paths() {
        let mut g = Game::new();
        g.enemies.enemy_list.array.push(crate::enemies::Enemy { linked: true, ..Default::default() });
        g.enemies.enemy_list.array[0].path.start_frame = 10;
        g.enemies.enemy_list.array[0].path.end_frame = 70;
        // Authentic: always runs, no shifting.
        g.enhance.opts = GameOptions { enemy_speed: 50, ..GameOptions::AUTHENTIC };
        assert!((0..10).all(|_| g.enhance_enemy_tick()));
        assert_eq!(g.enemies.enemy_list.array[0].path.start_frame, 10);
        // Enhanced 50 %: every other tick runs; frozen ticks shift the path.
        g.enhance.opts.profile = GameProfile::Enhanced;
        let runs: Vec<bool> = (0..10).map(|_| g.enhance_enemy_tick()).collect();
        assert_eq!(runs.iter().filter(|&&r| r).count(), 5);
        let p = g.enemies.enemy_list.array[0].path;
        assert_eq!((p.start_frame, p.end_frame), (15, 75));
        // 75 %: 3 of 4.
        g.enhance.opts.enemy_speed = 75;
        g.enhance.enemy_acc = 0;
        assert_eq!((0..40).filter(|_| g.enhance_enemy_tick()).count(), 30);
    }
}
