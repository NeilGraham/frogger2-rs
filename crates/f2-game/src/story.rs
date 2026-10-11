//! Port of `Common/story.c` / `story.h`: story-mode level order and the FMV list.

use crate::prelude::*;

use crate::define::mode::FRONTEND_MODE;
use crate::define::{SINGLEPLAYER, STORY_MODE};
use crate::frogger::{FROG_BABYFROG, FROG_FROGGER, FROG_LILLIE};
use crate::layout::worldid::*;
use crate::Game;

/// `NUM_STORY_LEVELS` (`story.h:18`).
pub const NUM_STORY_LEVELS: usize = 17;

// FMV sequences (`story.h:20`).
pub const FMV_NONE: i32 = 0;
pub const FMV_HASBRO_LOGO: i32 = 1;
pub const FMV_BLITZ_LOGO: i32 = 2;
pub const FMV_INTRO: i32 = 3;
pub const FMV_FROGGER_LILLIE_BABIES: i32 = 4;
pub const FMV_SWAMPY_PLAN: i32 = 5;
pub const FMV_SWAMPY_ESCAPES: i32 = 6;
pub const FMV_LILLIE_TO_GATE: i32 = 7;
pub const FMV_BOTH_AT_GATE: i32 = 8;
pub const FMV_LILLIE_COMPUTER: i32 = 9;
pub const FMV_SWAMPY_COMMERCIAL: i32 = 10;
pub const FMV_TELEPORT_TO_EARTH: i32 = 11;
pub const FMV_ENTER_HAUNTED_HOUSE: i32 = 12;
pub const FMV_LILLIE_IN_CAGE: i32 = 13;
pub const FMV_VICTORY: i32 = 14;
pub const NUM_FMV_SEQUENCES: usize = 15;

/// `FMV_DATA` (`story.h:40`): video file base name (`Video\<name>.bik`) and length in frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FmvData {
    pub name: &'static str,
    pub len: i32,
}

/// `fmv[NUM_FMV_SEQUENCES]` (`story.c:26`).
pub static FMV: [FmvData; NUM_FMV_SEQUENCES] = [
    FmvData { name: "", len: 0 },
    FmvData { name: "HASBROS", len: 273 }, // FMV_HASBRO_LOGO
    FmvData { name: "BLITZS", len: 118 },  // FMV_BLITZ_LOGO
    FmvData { name: "01S", len: 973 },     // FMV_INTRO
    FmvData { name: "02S", len: 1299 },    // FMV_FROGGER_LILLIE_BABIES
    FmvData { name: "03S", len: 859 },     // FMV_SWAMPY_PLAN
    FmvData { name: "04S", len: 836 },     // FMV_SWAMPY_ESCAPES
    FmvData { name: "05S", len: 371 },     // FMV_LILLIE_TO_GATE
    FmvData { name: "06S", len: 589 },     // FMV_BOTH_AT_GATE
    FmvData { name: "07S", len: 779 },     // FMV_LILLIE_COMPUTER
    FmvData { name: "08S", len: 1188 },    // FMV_SWAMPY_COMMERCIAL
    FmvData { name: "09S", len: 1007 },    // FMV_TELEPORT_TO_EARTH
    FmvData { name: "10S", len: 553 },     // FMV_ENTER_HAUNTED_HOUSE
    FmvData { name: "11S", len: 1304 },    // FMV_LILLIE_IN_CAGE
    FmvData { name: "12S", len: 1565 },    // FMV_VICTORY
];

/// `storySequenceLevelToChapter[]` (`story.c:46`).
pub static STORY_SEQUENCE_LEVEL_TO_CHAPTER: [i16; NUM_STORY_LEVELS] = [0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 8, 9];

/// `GAME_LEVEL` (`story.h:52`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameLevel {
    pub world_num: u32,
    pub level_num: u32,
    pub another: i32,
    pub character: u8,
    pub fmv: i32,
    pub icon_name: &'static str,
}

const fn gl(world_num: u32, level_num: u32, another: i32, character: u8, fmv: i32, icon_name: &'static str) -> GameLevel {
    GameLevel { world_num, level_num, another, character, fmv, icon_name }
}

/// `storySequence[]` (`story.c:51`).
pub static STORY_SEQUENCE: [GameLevel; NUM_STORY_LEVELS] = [
    gl(FRONTEND, 3, 1, FROG_FROGGER, 0, ""),
    gl(GARDEN, 0, 0, FROG_FROGGER, FMV_FROGGER_LILLIE_BABIES, "ICONRETROP"),
    gl(ANCIENT, 0, 1, FROG_LILLIE, FMV_SWAMPY_PLAN, "ICONRETROP"),
    gl(SUBTERRANEAN, 0, 0, FROG_FROGGER, 0, "ICONRETROP"),
    gl(ANCIENT, 2, 1, FROG_LILLIE, FMV_SWAMPY_ESCAPES, "ICONRETROP"),
    gl(ANCIENT, 1, 0, FROG_LILLIE, 0, "ICONRETROP"),
    gl(SUBTERRANEAN, 1, 1, FROG_FROGGER, FMV_LILLIE_TO_GATE, "ICONRETROP"),
    gl(SUBTERRANEAN, 2, 0, FROG_FROGGER, 0, "ICONRETROP"),
    gl(LABORATORY, 0, 1, FROG_LILLIE, FMV_BOTH_AT_GATE, "ICONRETROP"),
    gl(LABORATORY, 1, 0, FROG_LILLIE, 0, "ICONRETROP"),
    gl(SPACE, 1, 1, FROG_FROGGER, FMV_LILLIE_COMPUTER, "ICONMULTOP"),
    gl(SPACE, 0, 0, FROG_FROGGER, 0, "ICONMULTOP"),
    gl(LABORATORY, 2, 1, FROG_LILLIE, FMV_SWAMPY_COMMERCIAL, "ICONMULTOP"),
    gl(SPACE, 2, 0, FROG_FROGGER, 0, "ICONMULTOP"),
    gl(HALLOWEEN, 0, 0, FROG_FROGGER, FMV_TELEPORT_TO_EARTH, "ICONCHEATOP"),
    gl(HALLOWEEN, 1, 0, FROG_FROGGER, FMV_ENTER_HAUNTED_HOUSE, "ICONCHEATOP"),
    gl(HALLOWEEN, 2, 0, FROG_BABYFROG, FMV_LILLIE_IN_CAGE, "ICONCHEATOP"),
];

impl Game {
    /// Port of `StoryStartLevel` (Common/story.c:88).
    pub fn story_start_level(&mut self) {
        let i = (self.game_state.story_sequence_level as usize).min(NUM_STORY_LEVELS - 1);
        self.frogger.player[0].character = STORY_SEQUENCE[i].character;
    }

    /// `StartVideoPlayback(fmvNum)` (`PC/video.cpp`): the host plays `Video\<name>.bik`
    /// (blocking in the original; `quitAllVideo` is host state).
    pub fn start_video_playback(&mut self, fmv_num: i32) {
        if let Some(f) = FMV.get(fmv_num as usize) {
            if !f.name.is_empty() {
                self.effects.push(crate::Effect::PlayVideo { name: f.name.to_string() });
            }
        }
    }

    /// The story-mode test of `InitLevel` (Common/layout.c:741): `StoryStartLevel()` when not
    /// in the frontend, single player, story mode.
    pub fn init_level_story_start(&mut self) {
        if self.game_state.mode != FRONTEND_MODE && self.game_state.multi == SINGLEPLAYER && self.game_state.single == STORY_MODE {
            self.story_start_level();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_order_and_fmvs() {
        assert_eq!(STORY_SEQUENCE[0].world_num, FRONTEND);
        assert_eq!(STORY_SEQUENCE[NUM_STORY_LEVELS - 1].fmv, FMV_LILLIE_IN_CAGE);
        assert_eq!(FMV[FMV_VICTORY as usize].name, "12S");
        let mut g = Game::new();
        g.game_state.story_sequence_level = 2;
        g.story_start_level();
        assert_eq!(g.frogger.player[0].character, FROG_LILLIE);
        g.start_video_playback(FMV_NONE);
        assert!(g.effects.is_empty());
        g.start_video_playback(FMV_INTRO);
        assert_eq!(g.effects, vec![crate::Effect::PlayVideo { name: "01S".into() }]);
    }
}
