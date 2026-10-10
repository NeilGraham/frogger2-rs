//! Booting the real game flow: `CommonInit` (Common/layout.c:1287) for the PC build, the
//! game text and save loading the PC `WinMain` did, and the menu actions that start story
//! and arcade games (`BookStart`/`ArcadeStart`, Common/options.c) as host API.
//!
//! `InitLevel` is called from inside the simulation (level complete, game over, the end,
//! ...). The session keeps the [`DataSource`] so [`Game::init_level`] can load levels, and
//! queues the resulting [`LevelLoad`] for the host (renderer/audio reload).

use std::sync::Arc;

use crate::assets::DataSource;
use crate::define::mode::{FRONTEND_MODE, INGAME_MODE, STARTUP_MODE};
use crate::define::{ARCADE_MODE, SINGLEPLAYER, STORY_MODE};
use crate::effects::Effect;
use crate::frontend::NUM_LIVES;
use crate::language::LANG_DEFAULT;
use crate::layout::{worldid, LevelAssets, LEVELID_FRONTEND1, LEVELID_FRONTEND4};
use crate::story::{FMV_NONE, NUM_STORY_LEVELS, STORY_SEQUENCE};
use crate::Game;

/// A level `InitLevel` loaded; the host rebuilds its scene from it.
#[derive(Clone, Debug)]
pub struct LevelLoad {
    pub world: u32,
    pub level: u32,
    pub assets: LevelAssets,
}

/// Host configuration for [`Game::new_session`].
#[derive(Clone)]
pub struct SessionConfig {
    /// Game files (`MAPS\...`, `OBJECTS\...`); kept for every `InitLevel`.
    pub data: Arc<dyn DataSource + Send + Sync>,
    /// `gameTextLang` (`LANG_*`).
    pub language: i32,
    /// Content of `SaveData\savegame.dat`, if present (`LoadGame`).
    pub save_data: Option<Vec<u8>>,
    /// P7-1 (mods): levels/strings added by mods; empty = authentic.
    pub extra_levels: crate::levels::LevelRegistry,
    /// Gameplay options (P6-1; authentic by default).
    pub options: crate::enhance::GameOptions,
    /// P7b (mods): behaviour scripts; empty = authentic.
    pub scripts: Vec<crate::modscript::ScriptSource>,
}

impl SessionConfig {
    pub fn new(data: Arc<dyn DataSource + Send + Sync>) -> SessionConfig {
        SessionConfig { data, language: LANG_DEFAULT, save_data: None, extra_levels: Default::default(), options: Default::default(), scripts: Vec::new() }
    }
}

/// Session state (not a C global: the port's link between the simulation and level files).
#[derive(Clone, Default)]
pub struct SessionState {
    pub data: Option<Arc<dyn DataSource + Send + Sync>>,
    /// Levels loaded since the host last called [`Game::take_level_loads`].
    pub level_loads: Vec<LevelLoad>,
    /// P7-1 (mods): installed level registry ([`Game::install_level_registry`]).
    pub levels: crate::levels::LevelRegistry,
}

impl Game {
    /// Creates a game the way the PC executable starts: game text, save game, then
    /// `CommonInit` (STARTUP_MODE with the title backdrop; the logo FMVs follow).
    pub fn new_session(config: SessionConfig) -> Game {
        let mut g = Game::new();
        g.enhance.opts = config.options.clone(); // P6-1
        g.load_game_text(&*config.data, config.language);
        g.session.data = Some(config.data.clone());
        g.load_game(config.save_data.as_deref());
        if !config.extra_levels.is_empty() {
            g.install_level_registry(config.extra_levels.clone()); // P7-1
        }
        if !config.scripts.is_empty() {
            for e in g.install_mod_scripts(&config.scripts) {
                g.effects.push(crate::Effect::Log { text: format!("modscript: {e}") });
                // P7b
            }
        }
        g.common_init();
        g
    }

    /// Port of `CommonInit` (Common/layout.c:1287), PC non-demo build.
    pub fn common_init(&mut self) {
        // frame = 0; quitMainLoop = 0; InitMatrixStack() (host/renderer).
        self.frogger.player[0].world_num = worldid::FRONTEND as u8;
        self.frogger.player[0].level_num = LEVELID_FRONTEND1 as u8;
        self.game_state.multi = SINGLEPLAYER;
        self.game.mode_timer.init(1);

        self.game_state.mode = STARTUP_MODE;
        self.screen_fade(0, 255, 30);
        self.init_backdrop("TITLE");
        self.game.mode_timer.init(3);
    }

    /// Skips the start-up logos: straight into the frontend level (what `STARTUP_MODE` ends
    /// with, minus the FMVs).
    pub fn start_frontend(&mut self) {
        self.free_backdrop();
        self.game_state.mode = FRONTEND_MODE;
        self.game_state.multi = SINGLEPLAYER;
        self.frogger.num_frogs = 1;
        self.act_frame_count = 0;
        self.game.mode_timer.init(1);
        self.init_level(worldid::FRONTEND, LEVELID_FRONTEND1);
        self.frame_count = 0;
    }

    /// Starts story mode at `storySequence[story_level]` (`BookSelect` + `BookStart`,
    /// Common/options.c:2607).
    pub fn start_story(&mut self, story_level: u32) {
        self.free_backdrop();
        self.frogger.player[0].lives = NUM_LIVES[self.game_state.difficulty as usize % 3];
        self.frogger.num_frogs = 1;
        self.game_state.multi = SINGLEPLAYER;
        self.game_state.single = STORY_MODE;
        self.game_state.story_sequence_level = story_level.min(NUM_STORY_LEVELS as u32 - 1);
        let s = STORY_SEQUENCE[self.game_state.story_sequence_level as usize];
        self.frogger.player[0].world_num = s.world_num as u8;
        self.frogger.player[0].level_num = s.level_num as u8;
        self.last_act_frame_count = self.act_frame_count;
        self.game_state.mode = INGAME_MODE;
        self.init_level(s.world_num, s.level_num);
    }

    /// Starts an arcade level (`ArcadeStart`, Common/options.c:2910) with the current
    /// character. Garden level 0 chosen on the arcade machine is the training level.
    pub fn start_arcade(&mut self, world: u32, level: u32) {
        self.free_backdrop();
        self.frogger.num_frogs = 1;
        self.game_state.multi = SINGLEPLAYER;
        self.game_state.single = ARCADE_MODE;
        self.last_act_frame_count = self.act_frame_count;
        self.game_state.mode = INGAME_MODE;
        self.frogger.player[0].world_num = world as u8;
        self.frogger.player[0].level_num = level as u8;
        self.init_level(world, level);
    }

    /// `InitLevel(world, level)` (Common/layout.c:670) with the session's data source. Without
    /// a session (tests, tools) this only logs; the caller loads levels itself.
    pub fn init_level(&mut self, world: u32, level: u32) {
        let Some(data) = self.session.data.clone() else {
            self.effects.push(Effect::Log { text: format!("InitLevel({world}, {level}): no data source") });
            return;
        };
        match self.load_level(world, level, &*data) {
            Ok(assets) => {
                if self.game_state.mode == crate::define::mode::DEMO_MODE {
                    self.load_demo_file(world, level, &*data);
                }
                self.session.level_loads.push(LevelLoad { world, level, assets });
            }
            Err(e) => self.effects.push(Effect::Log { text: format!("InitLevel({world}, {level}) failed: {e}") }),
        }
    }

    /// Levels `InitLevel` loaded since the last call (the host reloads its scene).
    pub fn take_level_loads(&mut self) -> Vec<LevelLoad> {
        std::mem::take(&mut self.session.level_loads)
    }

    /// `InitLevel` steps between `FreeAllLists` and the data loads: the story FMV
    /// (`FreeAllLists`, Common/layout.c:1158), `restartingLevel = NO`, the loading screen and
    /// `StoryStartLevel`.
    pub fn init_level_after_free(&mut self) {
        if self.game_state.single == STORY_MODE && !self.frontend.restarting_level {
            let s = STORY_SEQUENCE[(self.game_state.story_sequence_level as usize).min(NUM_STORY_LEVELS - 1)];
            if self.frogger.player[0].world_num as u32 != worldid::FRONTEND && s.fmv != FMV_NONE {
                self.start_video_playback(s.fmv);
            }
        }
        self.frontend.restarting_level = false;
        self.init_loading_screen("LOADING");
        self.update_loading_screen(10);
        self.update_loading_screen(10);
        self.init_level_story_start();
    }

    /// `InitLevel`'s lives reset (Common/layout.c:902).
    pub fn init_level_lives(&mut self) {
        use crate::define::mode::DEMO_MODE;
        if self.game_state.mode != DEMO_MODE && self.game_state.single != STORY_MODE && self.game_state.mode != FRONTEND_MODE {
            self.frogger.player[0].lives = NUM_LIVES[self.game_state.difficulty as usize % 3];
        }
    }

    /// The tail of `InitLevel` (Common/layout.c:944): landscape on, loading screen gone,
    /// fades, training mode, the level's music.
    pub fn init_level_finish(&mut self, world: u32, level: u32) {
        self.backdrop.draw_landscape = true;
        self.update_loading_screen(100);
        self.free_loading_screen();
        self.screen_fade(0, 0, 1);
        self.fadeout.keep_fade = true;

        if world == worldid::FRONTEND && level == LEVELID_FRONTEND4 {
            self.init_training_mode();
        } else {
            self.training.train = None;
        }
        // LoadDemoFile for DEMO_MODE: done by `init_level` / `init_demo_level` (needs data).
        self.prepare_level_song(world, level, true); // PrepareSong (+ P7-1 registry track)
                                                     // UnPauseAudio(); SetMusicVolume(): host audio state.

        self.screen_fade(0, 255, 30);
        self.fadeout.keep_fade = false;
    }
}
