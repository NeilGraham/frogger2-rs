//! Port of `PC/banks.cpp`: object/texture bank file names (`LoadObjectBank`,
//! `LoadTextureBank`) and scenic actor creation (`CreateLevelObjects`).
//! The `switch` tables themselves are generated into [`crate::bank_tables`].
//!
//! Dropped: `LoadGame`/`SaveGame` (savegame wave), texture/object decoding (renderer).

use f2_formats::objbank::ObjectBank;

use crate::actor2::{ACTOR_ADDITIVE, ACTOR_DRAW_ALWAYS, ACTOR_LEAVES, ACTOR_SLIDYTEX, ACTOR_SLUDGE, ACTOR_WATER};
use crate::assets::DataSource;
use crate::bank_tables::{object_bank_name, texture_bank_name};
use crate::general::{gstrcmp, string_change};
use crate::mdx_actor::{mdx_flags, object_flags, INIT_ANIMATION};
use crate::Game;

/// `OBJECT_BASE` (`PC/Main.h:22`).
pub const OBJECT_BASE: &str = "objects\\";
/// `TEXTURE_BASE` (`PC/Main.h:23`).
pub const TEXTURE_BASE: &str = "textures\\";
/// `compressedTexBanks` is 1 under `FINAL_MASTER`, so `LoadTexBankFile` (`.fla`) is used.
pub const COMPRESSED_TEX_BANKS: bool = true;

/// File loaded by `LoadObjectBank(objectBank)` → `LoadObjBank` (`mdxObject.cpp:792`):
/// `objects\<name>.dll`; `None` for the `default:` case ("Didn't load object bank").
pub fn object_bank_file(object_bank: i32) -> Option<String> {
    object_bank_name(object_bank).map(|n| format!("{OBJECT_BASE}{n}.dll"))
}

/// File loaded by `LoadTextureBank(textureBank)` → `LoadTexBankFile`
/// (`mdxTexture.cpp:679`): `textures\<name>.fla`; `None` for `default:`.
pub fn texture_bank_file(texture_bank: i32) -> Option<String> {
    texture_bank_name(texture_bank).map(|n| format!("{TEXTURE_BASE}{n}.fla"))
}

/// Result of one `LoadObjectBank` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BankLoad {
    /// The bank id has no file (switch `default:`).
    NoFile,
    /// The file does not exist (`LoadLibrary` failed; the original only logs).
    Missing(String),
    /// Loaded; models added to the model table.
    Loaded(String),
}

impl Game {
    /// Port of `LoadObjectBank` (PC/banks.cpp:35): loads the bank's models into
    /// `self.models` (first loaded bank wins on duplicate ids, like `FindObject`).
    pub fn load_object_bank(&mut self, object_bank: i32, data: &dyn DataSource) -> Result<BankLoad, String> {
        let Some(file) = object_bank_file(object_bank) else {
            return Ok(BankLoad::NoFile);
        };
        let Some(bytes) = data.read(&file) else {
            return Ok(BankLoad::Missing(file));
        };
        let bank = ObjectBank::parse(&bytes).map_err(|e| format!("{file}: {e}"))?;
        self.models.add_bank(&bank);
        Ok(BankLoad::Loaded(file))
    }

    /// Port of `CreateLevelObjects` (PC/banks.cpp:502): one actor per scenic, except
    /// `world`; `backdrop` becomes `backGnd`.
    pub fn create_level_objects(&mut self, _world_id: u32, _level_id: u32) {
        let mut act_count = 0;
        for i in 0..self.map.scenics.len() {
            let ts = self.map.scenics[i].clone();
            // stringChange(ts->name) rewrites the scenic's name in place.
            let name = string_change(&ts.name);
            self.map.scenics[i].name = name.clone();

            if gstrcmp(&name, "backdrop.obe") == 0 || gstrcmp(&name, "backdrop.psi") == 0 {
                let bg = self.create_and_add_actor(&name, ts.pos.vx, ts.pos.vy, ts.pos.vz, INIT_ANIMATION);
                self.actor2.back_gnd = bg;
                if let Some(bg) = bg {
                    let s = &mut self.actor2.actors[bg].actor.size;
                    s.vx = 5 << 12;
                    s.vy = 5 << 12;
                    s.vz = 5 << 12;
                }
            } else if gstrcmp(&name, "world.obe") != 0 && gstrcmp(&name, "world.psi") != 0 {
                // tmp = first four chars of the name.
                let tmp: String = name.chars().take(4).collect();

                let Some(id) = self.create_and_add_actor(&name, ts.pos.vx, ts.pos.vy, ts.pos.vz, INIT_ANIMATION) else {
                    continue;
                };
                let fifth = name.as_bytes().get(4).copied().unwrap_or(0);
                let a = &mut self.actor2.actors[id];

                // If a water object, draw always
                if gstrcmp(&tmp, "wat_") == 0 {
                    a.flags = ACTOR_WATER | ACTOR_DRAW_ALWAYS | ACTOR_ADDITIVE;
                    a.actor.actual_actor.root_object_flags =
                        object_flags::OBJECT_FLAGS_WAVE | object_flags::OBJECT_FLAGS_MODGE | object_flags::OBJECT_FLAGS_ADDITIVE;
                    a.actor.actual_actor.flags |= mdx_flags::ACTOR_SLIDY;
                    if fifth == b'f' {
                        a.flags |= ACTOR_SLIDYTEX;
                    }
                } else if gstrcmp(&tmp, "slu_") == 0 {
                    a.flags = ACTOR_WATER | ACTOR_SLUDGE | ACTOR_DRAW_ALWAYS;
                    a.actor.actual_actor.root_object_flags = object_flags::OBJECT_FLAGS_MODGE;
                    a.actor.actual_actor.flags |= mdx_flags::ACTOR_SLIDY;
                    if fifth == b'f' {
                        a.flags |= ACTOR_SLIDYTEX;
                    }
                } else if gstrcmp(&tmp, "lea_") == 0 {
                    a.flags = ACTOR_WATER | ACTOR_LEAVES | ACTOR_DRAW_ALWAYS;
                    a.actor.actual_actor.root_object_flags = object_flags::OBJECT_FLAGS_WAVE;
                    a.actor.actual_actor.flags |= mdx_flags::ACTOR_SLIDY;
                    if fifth == b'f' {
                        a.flags |= ACTOR_SLIDYTEX;
                    }
                } else if gstrcmp(&tmp, "xxa_") == 0 {
                    a.flags = ACTOR_ADDITIVE;
                    a.actor.actual_actor.root_object_flags = object_flags::OBJECT_FLAGS_ADDITIVE;
                }

                a.actor.q_rot = ts.rot; // rot is a quat! Yay!

                self.actor_animate(id, 0, true, false, 90, false);

                let s = &mut self.actor2.actors[id].actor.size;
                s.vx = 4096;
                s.vy = 4096;
                s.vz = 4096;
                act_count += 1;
            }
        }
        self.effects.push(crate::Effect::Log { text: format!("** ADDED {act_count} ACTORS **") });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank_tables::*;

    #[test]
    fn file_names() {
        assert_eq!(object_bank_file(INGAMEGENERIC_OBJ_BANK).unwrap(), "objects\\gamegeneric.dll");
        assert_eq!(object_bank_file(FROGGER_OBJ_BANK).unwrap(), "objects\\frogger.dll");
        assert_eq!(object_bank_file(SYSTEM_OBJ_BANK), None);
        assert_eq!(texture_bank_file(INGAMEGENERIC_TEX_BANK).unwrap(), "textures\\System.fla");
        assert_eq!(texture_bank_file(GARDEN_TEX_BANK).unwrap(), "textures\\Garden.fla");
        assert_eq!(texture_bank_file(TITLESGENERIC_TEX_BANK).unwrap(), "textures\\titles.fla");
        assert_eq!(collision_file_for_coll_bank(GARDENLEV1_COL).unwrap(), "COLLISION\\GARDEN1.COL");
        assert_eq!(scenics_file_for_coll_bank(GARDENLEV1_COL).unwrap(), "SCENICS\\GARDEN1.SCE");
    }
}
