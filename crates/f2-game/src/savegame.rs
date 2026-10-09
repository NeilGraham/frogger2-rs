//! Port of `Common/savegame.c` (`MakeSaveGameBlock` / `LoadSaveGameBlock`) and the
//! `LoadGame` / `SaveGame` glue of `PC/banks.cpp:440`. The 2112-byte layout is
//! [`f2_formats::misc::SaveGame`]; persisting the block is an [`Effect::SaveGame`], loading
//! takes the bytes of `SaveData\savegame.dat` from the host.

use f2_formats::misc::{SaveGame, SAVE_LEVELS, SAVE_SIZE};
use serde::{Deserialize, Serialize};

use crate::define::{DIFFICULTY_EASY, DIFFICULTY_HARD};
use crate::effects::Effect;
use crate::layout::{worldid, LEVELID_FRONTEND4, MAX_WORLDS, WORLD_OPEN};
use crate::Game;

/// `SAVEGAME_VERSION` (`savegame.h:19`).
pub const SAVEGAME_VERSION: u8 = 1;
/// `SAVEGAME_SIZE` (`savegame.h:63`) on PC (no PSX card header).
pub const SAVEGAME_SIZE: usize = SAVE_SIZE;

/// Option globals the save block carries (`Common/menus.c:844`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveGameState {
    /// `globalSoundVol`.
    pub global_sound_vol: i32,
    /// `globalMusicVol`.
    pub global_music_vol: i32,
}

impl Default for SaveGameState {
    fn default() -> Self {
        SaveGameState { global_sound_vol: 100, global_music_vol: 70 }
    }
}

/// `strcpy` of a NUL-terminated name into an 8-byte field.
///
/// BUG(original): both directions `strcpy` between 8-byte fields; a name without a NUL in
/// its 8 bytes runs over into the following struct fields. The port copies at most 8 bytes.
fn strcpy8(dst: &mut [u8; 8], src: &[u8]) {
    let n = src.iter().position(|&c| c == 0).unwrap_or(src.len()).min(8);
    dst[..n].copy_from_slice(&src[..n]);
    if n < 8 {
        dst[n] = 0;
    }
}

/// `strncpy(dst, src, 8)`: copy up to the NUL, then pad with zeros to 8 bytes.
fn strncpy8(dst: &mut [u8], src: &[u8]) {
    let n = src.iter().position(|&c| c == 0).unwrap_or(src.len()).min(8);
    dst[..n].copy_from_slice(&src[..n]);
    for b in dst.iter_mut().take(8).skip(n) {
        *b = 0;
    }
}

impl Game {
    /// Port of `MakeSaveGameBlock` (Common/savegame.c:34): the 2112-byte save block.
    pub fn make_save_game_block(&self) -> Vec<u8> {
        let mut s = SaveGame {
            version: SAVEGAME_VERSION,
            version_check: 0,
            story_world: 0,
            story_level: 0,
            name: [0; 8],
            language: 0,
            controller: 0,
            music_vol: 0,
            sfx_vol: 0,
            char_unlock: 0,
            lives: 0,
            res: [0; 2],
            flags: [0; 32],
            res2: [0; 12],
            levels: [Default::default(); SAVE_LEVELS],
        };
        strcpy8(&mut s.name, &self.frogger.player[0].name);
        // TODO: language, controller setup & audio settings here
        s.lives = self.frogger.player[0].lives as u8;
        s.music_vol = self.savegame.global_music_vol as u8;
        s.sfx_vol = self.savegame.global_sound_vol as u8;

        let wvd = &self.layout.world_visual_data;
        let training_done = wvd[worldid::FRONTEND as usize].level_visual_data[LEVELID_FRONTEND4 as usize].level_completed != 0;
        let mut count = 0;
        for wv in wvd.iter().take(MAX_WORLDS - 1) {
            for lv in wv.level_visual_data.iter().take(wv.num_levels as usize) {
                let Some(li) = s.levels.get_mut(count) else { break };
                //sb horrible bodge to remember if training level has been completed!!!
                li.flags = (if lv.level_open != 0 { 1 } else { 0 })
                    + (if lv.level_completed != 0 { 2 } else { 0 })
                    + (if training_done { 4 } else { 0 })
                    + (if lv.level_beaten != 0 { 8 } else { 0 })
                    + (if self.game_state.difficulty == DIFFICULTY_HARD { 16 } else { 0 })
                    + (if self.game_state.difficulty == DIFFICULTY_EASY { 32 } else { 0 });
                li.best_time = lv.par_time;
                li.garibs = lv.max_coins;
                strncpy8(&mut li.winner, &lv.par_name);
                count += 1;
            }
        }
        s.to_bytes()
    }

    /// Port of `LoadSaveGameBlock` (Common/savegame.c:110). Returns 1 on success, 0 when the
    /// version does not match (or, unlike the original, when the block is too short).
    pub fn load_save_game_block(&mut self, data: &[u8]) -> i32 {
        let Ok(s) = SaveGame::parse(data) else {
            self.effects.push(Effect::Log { text: format!("Failed loading savegame block: {} bytes", data.len()) });
            return 0;
        };
        if s.version != SAVEGAME_VERSION {
            self.effects.push(Effect::Log { text: "Failed loading savegame block due to incorrect revision number".into() });
            return 0;
        }

        strcpy8(&mut self.frogger.player[0].name, &s.name);
        // TODO: language, controller setup & audio settings here
        self.frogger.player[0].lives = if s.lives != 0 { s.lives as i16 } else { 3 };

        self.savegame.global_music_vol = s.music_vol as i32;
        self.savegame.global_sound_vol = s.sfx_vol as i32;

        let mut count = 0;
        for world in 0..MAX_WORLDS - 1 {
            for level in 0..self.layout.world_visual_data[world].num_levels as usize {
                let Some(li) = s.levels.get(count) else { break };
                let wv = &mut self.layout.world_visual_data[world];
                let num_sp = wv.num_single_player_levels as usize;
                let lv = &mut wv.level_visual_data[level];
                lv.level_open = (li.flags & 1) as u16;
                let open = lv.level_open != 0;
                lv.level_completed = if li.flags & 2 != 0 { 1 } else { 0 };
                lv.level_beaten = if li.flags & 8 != 0 { 1 } else { 0 };
                lv.max_coins = li.garibs;
                lv.par_time = li.best_time;
                strncpy8(&mut lv.par_name, &li.winner);
                lv.par_name[8] = 0;
                if level < num_sp && open {
                    wv.world_open = WORLD_OPEN;
                }
                //sb horrible bodge to remember if training level has been completed!!!
                self.layout.world_visual_data[worldid::FRONTEND as usize].level_visual_data[LEVELID_FRONTEND4 as usize].level_completed =
                    if li.flags & 4 != 0 { 1 } else { 0 };

                if li.flags & 16 != 0 {
                    self.game_state.difficulty = DIFFICULTY_HARD;
                } else if li.flags & 32 != 0 {
                    self.game_state.difficulty = DIFFICULTY_EASY;
                }
                count += 1;
            }
        }
        1
    }

    /// Port of `SaveGame` (PC/banks.cpp:466): writes `SaveData\savegame.dat`.
    pub fn save_game(&mut self) {
        let data = self.make_save_game_block();
        self.effects.push(Effect::SaveGame { data });
    }

    /// Port of `LoadGame` (PC/banks.cpp:440): `data` is the content of
    /// `SaveData\savegame.dat` (`None`: "Couldn't load savegame").
    pub fn load_game(&mut self, data: Option<&[u8]>) {
        match data {
            Some(d) => {
                self.load_save_game_block(d);
            }
            None => self.effects.push(Effect::Log { text: "Couldn't load savegame SaveData\\savegame.dat".into() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::define::DIFFICULTY_NORMAL;
    use crate::layout::{LEVEL_OPEN, WORLD_CLOSED};

    #[test]
    fn save_round_trip_is_identical() {
        let mut g = Game::new();
        g.frogger.player[0].name = *b"ABC\0\0\0\0\0";
        g.frogger.player[0].lives = 4;
        let wv = &mut g.layout.world_visual_data[worldid::ANCIENT as usize];
        wv.level_visual_data[1].level_open = LEVEL_OPEN;
        wv.level_visual_data[1].level_completed = 1;
        wv.level_visual_data[1].par_time = 1234;
        wv.level_visual_data[1].max_coins = 25;
        wv.level_visual_data[1].par_name = *b"ZZZ\0\0\0\0\0\0";
        g.game_state.difficulty = DIFFICULTY_HARD;
        let a = g.make_save_game_block();
        assert_eq!(a.len(), SAVEGAME_SIZE);

        let mut h = Game::new();
        h.layout.world_visual_data[worldid::ANCIENT as usize].world_open = WORLD_CLOSED;
        h.game_state.difficulty = DIFFICULTY_NORMAL;
        assert_eq!(h.load_save_game_block(&a), 1);
        assert_eq!(h.layout.world_visual_data[worldid::ANCIENT as usize].world_open, WORLD_OPEN);
        assert_eq!(h.game_state.difficulty, DIFFICULTY_HARD);
        assert_eq!(h.frogger.player[0].lives, 4);
        assert_eq!(&h.frogger.player[0].name[..4], b"ABC\0");
        let b = h.make_save_game_block();
        assert_eq!(a, b);
        // Load -> Make of the re-made block is still identical.
        let mut k = Game::new();
        k.load_save_game_block(&b);
        assert_eq!(k.make_save_game_block(), a);
    }

    #[test]
    fn wrong_version_rejected() {
        let mut g = Game::new();
        let mut a = g.make_save_game_block();
        a[0] = 2;
        assert_eq!(g.load_save_game_block(&a), 0);
        g.save_game();
        assert!(matches!(g.effects.last(), Some(Effect::SaveGame { data }) if data.len() == SAVEGAME_SIZE));
    }
}
