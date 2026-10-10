//! P7-1 (mods): the level registry — levels added or overridden by mods
//! (`levels/<id>/level.toml`, parsed by the host) without patching the switch tables.
//!
//! Not part of the original game. With an empty registry (the authentic game) nothing here
//! has any effect: `load_level` falls back to the generated `bank_tables` switches and
//! `origWorldVisualData` exactly as before.
//!
//! An [`ExtraLevel`] names a `worldVisualData[world].levelVisualData[level]` slot, the full
//! `LEVEL_VISUAL_DATA` row to put there and (optionally) the file names `InitLevel` loads
//! for it, which otherwise come from `collBank`/`objectBankToUse`/`masterTextureBank` and the
//! `ENTITY-%d-%d.DAT`/`SCRIPT-%d-%d.FEV`/`SFXANIM%d%c.SAM` patterns. File names are passed
//! verbatim to the [`DataSource`] (the host's mod VFS).

use crate::assets::DataSource;
use crate::effects::Effect;
use crate::language::LANG_NUM_STRINGS;
use crate::layout::{LevelVisualData, MAX_LEVELS, MAX_WORLDS};
use crate::Game;

/// Files of an [`ExtraLevel`]; `None` = what the original tables give for the row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LevelFileNames {
    /// Collision tile graph (`COLLISION\*.COL`).
    pub collision: Option<String>,
    /// Scenics (`SCENICS\*.SCE`).
    pub scenics: Option<String>,
    /// Entities (`MAPS\ENTITY-w-l.DAT`).
    pub entities: Option<String>,
    /// Event script (`MAPS\SCRIPT-w-l.FEV`, compiled bytes).
    pub script: Option<String>,
    /// Sfx/ambient mapping (`MAPS\SFXANIMwl.SAM`).
    pub sfx_mapping: Option<String>,
    /// World texture bank (`textures\*.fla`, replaces `masterTextureBank`).
    pub texture_bank: Option<String>,
    /// World object bank (`objects\*.dll`, replaces `masterObjectBank`).
    pub world_object_bank: Option<String>,
    /// Level object bank (`objects\*.dll`, replaces `objectBankToUse`).
    pub object_bank: Option<String>,
    /// World sample bank (`sfx\*.wus`, replaces the world's bank in `LoadSfx`).
    pub sfx_bank: Option<String>,
}

/// World-level fields a registry entry may change (`WORLD_VISUAL_DATA`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorldPatch {
    pub world_id: Option<u32>,
    pub description_str: Option<i32>,
    /// Literal world name (becomes an extra string; overrides `description_str`).
    pub name: Option<String>,
    pub world_open: Option<u32>,
}

/// One added/overridden level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtraLevel {
    /// `worldVisualData` index (`< MAX_WORLDS`).
    pub world: u32,
    /// `levelVisualData` index (`< MAX_LEVELS`).
    pub level: u32,
    /// The row written into the slot. On every later `InitLevel` of the slot the row is
    /// written again, keeping the progress fields (`levelOpen`, `levelCompleted`,
    /// `levelBeaten`, `maxCoins`, `parTime`, `parName`) the game changed.
    pub data: LevelVisualData,
    /// Literal level name (becomes an extra string; overrides `data.description_str`).
    pub name: Option<String>,
    pub files: LevelFileNames,
    /// CD track for `PrepareSong` (else the world's track).
    pub music_track: Option<u32>,
    pub world_patch: Option<WorldPatch>,
}

/// Levels and strings injected by the host at session start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LevelRegistry {
    pub levels: Vec<ExtraLevel>,
    /// Extra strings; `strings[i]` is `GAMESTRING(LANG_NUM_STRINGS + i)` (see
    /// [`extra_string_index`]). Literal level/world names are appended after these.
    pub strings: Vec<String>,
}

impl LevelRegistry {
    pub fn is_empty(&self) -> bool {
        self.levels.is_empty() && self.strings.is_empty()
    }

    pub fn get(&self, world: u32, level: u32) -> Option<&ExtraLevel> {
        self.levels.iter().rev().find(|e| e.world == world && e.level == level)
    }
}

/// `STR_*` index of extra string `i`.
pub fn extra_string_index(i: usize) -> i32 {
    (LANG_NUM_STRINGS + i) as i32
}

impl Game {
    /// Installs `reg` (host, at session start): registers its strings and writes every
    /// entry into `worldVisualData`. Replaces any earlier registry.
    pub fn install_level_registry(&mut self, mut reg: LevelRegistry) {
        reg.levels.retain(|e| (e.world as usize) < MAX_WORLDS && (e.level as usize) < MAX_LEVELS);
        let mut strings = reg.strings.clone();
        for e in reg.levels.iter_mut() {
            if let Some(n) = &e.name {
                strings.push(n.clone());
                e.data.description_str = extra_string_index(strings.len() - 1);
            }
            if let Some(w) = e.world_patch.as_mut() {
                if let Some(n) = &w.name {
                    strings.push(n.clone());
                    w.description_str = Some(extra_string_index(strings.len() - 1));
                }
            }
        }
        self.language.extra_strings = strings;
        for e in &reg.levels {
            self.apply_extra_level(e, false);
        }
        self.session.levels = reg;
    }

    fn apply_extra_level(&mut self, e: &ExtraLevel, keep_progress: bool) {
        let w = &mut self.layout.world_visual_data[e.world as usize];
        if let Some(p) = &e.world_patch {
            if let Some(v) = p.world_id {
                w.world_id = v;
            }
            if let Some(v) = p.description_str {
                w.description_str = v;
            }
            if let Some(v) = p.world_open {
                if !keep_progress {
                    w.world_open = v;
                }
            }
        }
        let slot = &mut w.level_visual_data[e.level as usize];
        let mut row = e.data;
        if keep_progress {
            row.level_open = slot.level_open;
            row.level_completed = slot.level_completed;
            row.level_beaten = slot.level_beaten;
            row.max_coins = slot.max_coins;
            row.par_time = slot.par_time;
            row.par_name = slot.par_name;
        }
        *slot = row;
    }

    /// Re-applies the registry entry of a slot (start of `load_level`).
    pub fn apply_level_registry(&mut self, world: u32, level: u32) {
        if let Some(e) = self.session.levels.get(world, level).cloned() {
            self.apply_extra_level(&e, true);
        }
    }

    /// A file name the registry sets for `world`/`level`.
    pub fn registry_file(&self, world: u32, level: u32, pick: impl Fn(&LevelFileNames) -> &Option<String>) -> Option<String> {
        self.session.levels.get(world, level).and_then(|e| pick(&e.files).clone())
    }

    /// `LoadSfx(worldID)` (`load_sfx`) with the registry's world sample bank, if any.
    pub fn load_level_sfx(&mut self, world: u32, level: u32, data: &dyn DataSource) {
        let Some(bank) = self.registry_file(world, level, |f| &f.sfx_bank) else {
            self.load_sfx(world, data);
            return;
        };
        let load = |g: &mut Game, path: &str| match data.sound_bank(path) {
            Some(Ok(b)) => g.pcaudio.load_sfx_set(&b),
            _ => g.effects.push(Effect::Log { text: format!("Failed to load sample bank {path}") }),
        };
        load(self, "sfx\\generic.wus");
        if self.game_state.multi != crate::define::SINGLEPLAYER {
            load(self, "sfx\\multi.wus");
        }
        load(self, &bank);
    }

    /// `PrepareSong(worldID, looped)` with the registry's music track, if any.
    pub fn prepare_level_song(&mut self, world: u32, level: u32, looped: bool) {
        match self.session.levels.get(world, level).and_then(|e| e.music_track) {
            Some(track) => {
                self.effects.push(Effect::PlayMusic { track, looped });
                self.pcaudio.cd_track = if looped { track as i32 } else { -1 };
            }
            None => self.prepare_song(world as i16, looped),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::ORIG_WORLD_VISUAL_DATA;

    #[test]
    fn install_and_reapply() {
        let mut g = Game::new();
        let mut data = ORIG_WORLD_VISUAL_DATA[0].level_visual_data[0];
        data.fog = [10, 20, 30];
        let reg = LevelRegistry {
            levels: vec![ExtraLevel { world: 0, level: 1, data, name: Some("Hello".into()), files: Default::default(), music_track: Some(5), world_patch: None }],
            strings: vec!["first".into()],
        };
        g.install_level_registry(reg);
        let l = g.layout.world_visual_data[0].level_visual_data[1];
        assert_eq!(l.fog, [10, 20, 30]);
        assert_eq!(l.description_str, extra_string_index(1));
        assert_eq!(g.gamestring(extra_string_index(0)), "first");
        assert_eq!(g.gamestring(extra_string_index(1)), "Hello");
        g.layout.world_visual_data[0].level_visual_data[1].level_completed = 1;
        g.layout.world_visual_data[0].level_visual_data[1].fog = [0, 0, 0];
        g.apply_level_registry(0, 1);
        let l = g.layout.world_visual_data[0].level_visual_data[1];
        assert_eq!((l.fog, l.level_completed), ([10, 20, 30], 1));
        // Untouched slots and the authentic table.
        assert_eq!(g.layout.world_visual_data[1], ORIG_WORLD_VISUAL_DATA[1]);
        g.prepare_level_song(0, 1, true);
        assert_eq!(g.pcaudio.cd_track, 5);
    }

    #[test]
    fn empty_registry_is_authentic() {
        let mut g = Game::new();
        g.install_level_registry(LevelRegistry::default());
        assert_eq!(g.layout.world_visual_data, ORIG_WORLD_VISUAL_DATA);
        assert!(g.language.extra_strings.is_empty());
    }
}
