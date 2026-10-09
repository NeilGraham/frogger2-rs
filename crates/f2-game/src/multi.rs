//! `Common/multi.c`: multiplayer. **Later wave.** Entry points enemies call.

use serde::{Deserialize, Serialize};

use crate::ids::EnemyId;
use crate::Game;

/// `MULTIMODE_*` (`multi.h:28`).
pub const MULTIMODE_RACE: i32 = 1;
pub const MULTIMODE_BATTLE: i32 = 2;
pub const MULTIMODE_COLLECT: i32 = 3;

/// Globals of `multi.c` used so far.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiState {
    /// `multiplayerMode`.
    pub multiplayer_mode: i32,
}

impl Game {
    /// `KillMPFrog` (Common/multi.c).
    pub fn kill_mp_frog(&mut self, _pl: i32) {
        // TODO(port): multi.c
    }

    /// `PickupBabyFrogMulti` (Common/multi.c).
    pub fn pickup_baby_frog_multi(&mut self, _baby: EnemyId, _pl: i32) {
        // TODO(port): multi.c
    }

    /// `RunMultiplayer` (Common/multi.c).
    pub fn run_multiplayer(&mut self) {
        // TODO(port): multi.c
    }
}
