//! Port of `Common/levplay.c` (attract-mode demo playback) plus the `padData`
//! bookkeeping of `PC/controll.cpp` (`ProcessUserInput`) that gameplay reads.
//!
//! The host passes the live pad bits ([`crate::TickInput::pads`], PSX layout from
//! `PC/islpad.h`) to [`Game::tick`], which calls [`Game::process_user_input`] before
//! `GameLoop`, exactly where `LoopFunc` (PC/Main.cpp) calls `ProcessUserInput`.

use crate::prelude::*;

use f2_formats::misc::{parse_key_file, KeyRecord};
use serde::{Deserialize, Serialize};

use crate::assets::DataSource;
use crate::define::mode::{DEMO_MODE, FRONTEND_MODE};
use crate::define::{ARCADE_MODE, SINGLEPLAYER};
use crate::effects::Effect;
use crate::layout::{worldid, LevelAssets};
use crate::timer::Timer;
use crate::Game;

// ----- pad bits (`PC/islpad.h`) -----
pub const PAD_SELECT: u16 = 1 << 0;
pub const PAD_L3: u16 = 1 << 1;
pub const PAD_R3: u16 = 1 << 2;
pub const PAD_START: u16 = 1 << 3;
pub const PAD_UP: u16 = 1 << 4;
pub const PAD_RIGHT: u16 = 1 << 5;
pub const PAD_DOWN: u16 = 1 << 6;
pub const PAD_LEFT: u16 = 1 << 7;
pub const PAD_L2: u16 = 1 << 8;
pub const PAD_R2: u16 = 1 << 9;
pub const PAD_L1: u16 = 1 << 10;
pub const PAD_R1: u16 = 1 << 11;
pub const PAD_TRIANGLE: u16 = 1 << 12;
pub const PAD_CIRCLE: u16 = 1 << 13;
pub const PAD_CROSS: u16 = 1 << 14;
pub const PAD_SQUARE: u16 = 1 << 15;

/// `DEMO_TIMEOUT` (`levplay.c:34`), seconds.
pub const DEMO_TIMEOUT: i32 = 25;
/// `NUM_DEMOS` (`levplay.c:35`).
pub const NUM_DEMOS: usize = 3;
/// `demoLevels[NUM_DEMOS][2]` (`levplay.c:43`): Garden 1, Ancient 1, Halloween 2.
pub const DEMO_LEVELS: [[u32; 2]; NUM_DEMOS] = [[worldid::GARDEN, 0], [worldid::ANCIENT, 0], [worldid::HALLOWEEN, 1]];

/// `PadDataType padData` (`PC/controll.cpp:46`), the part gameplay reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadData {
    pub digital: [u16; 4],
    pub debounce: [u16; 4],
}

/// Globals of `levplay.c`, `padData`, and the fade state `RunDemoMode` waits on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevPlayState {
    /// `padData`.
    pub pad_data: PadData,
    /// `playKeyList` (NULL = `None`), as `(frame, buttons, player)` triplets.
    pub play_key_list: Option<Vec<[u32; 3]>>,
    /// `playKeyCount`.
    pub play_key_count: u32,
    /// `curPlayKey`.
    pub cur_play_key: u32,
    /// `demoTimeout`.
    pub demo_timeout: Timer,
    /// `nextDemo`.
    pub next_demo: i32,
    /// `quittingDemo`.
    pub quitting_demo: bool,
    /// `fadingOut` / `fadeoutStart` / `fadeoutLength` (`PC/fadeout.cpp`).
    pub fading_out: bool,
    pub fadeout_start: u32,
    pub fadeout_length: i32,
    /// Set when `RunDemoMode` returned to the frontend (the port does not load the
    /// frontend level; the host decides what to do next).
    pub demo_finished: bool,
}

impl Game {
    /// Port of the `padData` update of `ProcessUserInput` (PC/controll.cpp:950):
    /// `oldDigital = digital; digital = live; debounce = ~oldDigital & digital`.
    /// `oldDigital` is whatever the previous `GameLoop` left in `padData.digital`
    /// (demo playback overwrites it).
    pub fn process_user_input(&mut self, live: [u16; 4]) {
        let pd = &mut self.levplay.pad_data;
        for (i, &l) in live.iter().enumerate() {
            let old = pd.digital[i];
            pd.debounce[i] = 0;
            pd.digital[i] = l;
            pd.debounce[i] |= !old & pd.digital[i];
        }
    }

    /// Port of `LoadDemoFile` (Common/levplay.c:51).
    ///
    /// BUG(original): `RunDemoMode` builds the debounce of the first replayed key from
    /// `playKeyList[-2]`, the heap header in front of the `fileLoad` buffer, so whether a
    /// demo's first press registers varied between runs. The oracle (f2fix) replays from
    /// a copy whose two leading words are 0, and so does the port: the record before the
    /// first one reads as buttons 0 (the first press always registers).
    pub fn load_demo_file(&mut self, world: u32, level: u32, data: &dyn DataSource) {
        let filename = format!("DEMOS\\RECORD-{world}-{level}.KEY");
        self.levplay.play_key_list = None;
        let Some(bytes) = data.read(&filename) else { return };
        match parse_key_file(&bytes) {
            Ok(recs) => {
                self.levplay.play_key_count = recs.len() as u32;
                self.levplay.play_key_list = Some(recs.iter().map(|k: &KeyRecord| [k.frame, k.buttons, k.player]).collect());
            }
            Err(e) => self.effects.push(Effect::Log { text: format!("{filename}: {e}") }),
        }
    }

    /// Port of `InitDemoMode` (Common/levplay.c:69) as the frontend runs it
    /// (`goingToDemo`: `gameState.difficulty = DIFFICULTY_NORMAL` first). `InitLevel`
    /// calls `LoadDemoFile` for `DEMO_MODE`; here the level load and the demo file are
    /// both done from `data`.
    pub fn init_demo_mode(&mut self, data: &dyn DataSource) -> Result<LevelAssets, String> {
        let [world, level] = DEMO_LEVELS[self.levplay.next_demo.rem_euclid(NUM_DEMOS as i32) as usize];
        self.init_demo_level(world, level, data)
    }

    /// [`Game::init_demo_mode`] for an explicit level (the oracle's `start.demo=W-L` writes
    /// `demoLevels[0] = {W, L}`, `nextDemo = 0`).
    pub fn init_demo_level(&mut self, world: u32, level: u32, data: &dyn DataSource) -> Result<LevelAssets, String> {
        self.game_state.difficulty = crate::define::DIFFICULTY_NORMAL;
        self.game_state.multi = SINGLEPLAYER;
        self.game_state.single = ARCADE_MODE;
        self.game_state.mode = DEMO_MODE;

        self.levplay.play_key_list = None;
        let assets = self.load_level(world, level, data)?;
        // InitLevel: if (gameState.mode == DEMO_MODE) LoadDemoFile(worldID, levelID)
        self.load_demo_file(world, level, data);
        self.levplay.demo_finished = false;
        self.levplay.quitting_demo = false;

        if self.levplay.play_key_list.is_none() {
            self.effects.push(Effect::Log { text: "DEMO MODE: Failed loading demo file, returning".into() });
            return Ok(assets);
        }
        self.levplay.cur_play_key = 0;
        let s = self.gamestring(crate::language::str_id::STR_DEMO_MODE);
        self.textover.create_and_add_text_overlay(2048, 128, &s, true, 128, crate::textover::Font::Normal, 0);
        let s = self.gamestring(crate::language::str_id::STR_PRESSENTER);
        self.textover.create_and_add_text_overlay(2048, 3500, &s, true, 128, crate::textover::Font::Normal, 0);
        self.levplay.demo_timeout.init(DEMO_TIMEOUT);
        self.disable_hud();
        Ok(assets)
    }

    /// Port of `RunDemoMode` (Common/levplay.c:120).
    pub fn run_demo_mode(&mut self) {
        let afc = self.act_frame_count;
        self.levplay.demo_timeout.update(-1, afc);
        if self.levplay.quitting_demo && !self.levplay.fading_out {
            self.levplay.quitting_demo = false;
            self.levplay.play_key_list = None; // FREE(playKeyList)
            self.levplay.next_demo += 1;
            if self.levplay.next_demo == NUM_DEMOS as i32 {
                self.levplay.next_demo = 0;
            }
            if let Some(f) = self.frogger.frog[0] {
                if self.undo_change_model(f) != 0 {
                    self.frogger.player[0].idle_enable = 1;
                }
            }
            self.game_state.difficulty = self.menus.old_diff_mode as u8;
            self.menus.old_diff_mode = self.game_state.difficulty as i32;
            self.game_state.mode = FRONTEND_MODE;
            self.frogger.player[0].character = crate::frogger::FROG_FROGGER;
            self.levplay.demo_finished = true;
            // InitLevel(WORLDID_FRONTEND, LEVELID_FRONTEND1) through the session's data source
            // (a no-op without one: the host then loads the frontend itself).
            self.init_level(crate::layout::worldid::FRONTEND, crate::layout::LEVELID_FRONTEND1);
            self.frame_count = 0; // why? who can say? (me! me!)
            return;
        }

        if self.levplay.quitting_demo {
            return;
        }

        if self.levplay.pad_data.debounce[0] != 0 || self.levplay.cur_play_key >= self.levplay.play_key_count || self.levplay.demo_timeout.time == 0 {
            self.levplay.quitting_demo = true;
            self.screen_fade(255, 0, 30);
            self.fadeout.keep_fade = false;
        }
        // BUG(original): the loop clears pad 0 four times instead of each pad.
        for _pl in 0..4 {
            self.levplay.pad_data.debounce[0] = 0;
            self.levplay.pad_data.digital[0] = 0;
        }

        if let Some(list) = &self.levplay.play_key_list {
            while self.levplay.cur_play_key < self.levplay.play_key_count && list[self.levplay.cur_play_key as usize][0] < self.act_frame_count {
                let k = self.levplay.cur_play_key as usize;
                let pl = (list[k][2] & 3) as usize;
                let buttons = list[k][1] as u16;
                // playKeyList[((curPlayKey-1)*3)+1]: see the BUG(original) note on
                // `load_demo_file` (the record before the first reads as 0).
                let prev = if k == 0 { 0 } else { list[k - 1][1] as u16 };
                let pd = &mut self.levplay.pad_data;
                pd.digital[pl] |= buttons;
                pd.debounce[pl] |= !prev & pd.digital[pl];
                self.levplay.cur_play_key += 1;
            }
        }

        self.run_game_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_from_previous_digital() {
        let mut g = Game::new();
        g.process_user_input([PAD_UP, 0, 0, 0]);
        assert_eq!(g.levplay.pad_data.debounce[0], PAD_UP);
        g.process_user_input([PAD_UP | PAD_CROSS, 0, 0, 0]);
        assert_eq!(g.levplay.pad_data.debounce[0], PAD_CROSS);
        g.process_user_input([0, 0, 0, 0]);
        assert_eq!(g.levplay.pad_data.debounce[0], 0);
    }

    #[test]
    fn demo_replay_first_press_registers() {
        let mut g = Game::new();
        g.game_state.mode = DEMO_MODE;
        g.levplay.play_key_list = Some(vec![[5, PAD_UP as u32, 0], [6, 0, 0], [9, PAD_UP as u32, 0]]);
        g.levplay.play_key_count = 3;
        g.levplay.demo_timeout.init(DEMO_TIMEOUT);
        let mut seen = vec![];
        for f in 1..=12u32 {
            g.act_frame_count = f;
            g.process_user_input([0; 4]);
            g.run_demo_mode();
            seen.push((g.levplay.pad_data.digital[0], g.levplay.pad_data.debounce[0]));
        }
        // frame 6 replays record 0 (frame 5 < 6): first press registers.
        assert_eq!(seen[5], (PAD_UP, PAD_UP));
        assert_eq!(seen[6], (0, 0));
        assert_eq!(seen[9], (PAD_UP, PAD_UP));
        // all keys consumed -> quitting on the next frame.
        assert!(g.levplay.quitting_demo);
    }
}
