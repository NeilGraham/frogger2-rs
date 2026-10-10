//! Mod level manifests (`levels/<id>/level.toml`, parsed by `f2_modkit::levels`) → the
//! game's level registry (`f2_game::levels`), installed at session start.

use f2_game::assets::DataSource;
use f2_game::bank_tables::{collision_file_for_coll_bank, scenics_file_for_coll_bank};
use f2_game::layout::{entity_file_name, script_file_name, LEVEL_CLOSED, LEVEL_OPEN, ORIG_WORLD_VISUAL_DATA, WORLD_CLOSED, WORLD_OPEN};
use f2_game::levels::{ExtraLevel, LevelFileNames, LevelRegistry, WorldPatch};
use f2_game::pc_banks::{object_bank_file, texture_bank_file};
use f2_modkit::levels::{find_levels, level_path, FoundLevel};

use crate::data::Data;

/// The registry for the enabled mods, plus warnings (bad manifests, unknown text ids,
/// missing files). Manifests apply in directory order; a later one wins a shared slot.
pub fn build_registry(data: &Data) -> (LevelRegistry, Vec<String>) {
    let (found, mut warnings) = find_levels(&data.vfs);
    warnings.extend(f2_modkit::levels::duplicate_slots(&found));
    let table = match found.iter().any(|l| l.manifest.text_id.is_some() || l.manifest.world_info.as_ref().is_some_and(|w| w.text_id.is_some())) {
        true => data.string_table(f2_game::language::LANGUAGE_FILE).and_then(|r| r.ok()),
        false => None,
    };
    let text_index = |id: &str, warnings: &mut Vec<String>| -> Option<i32> {
        let n = table.as_ref().and_then(|t| (0..t.num_strings()).find(|&i| t.id_by_index(i).is_some_and(|s| s.eq_ignore_ascii_case(id))));
        if n.is_none() {
            warnings.push(format!("unknown text id {id}"));
        }
        n.map(|n| n as i32)
    };
    let mut reg = LevelRegistry::default();
    for l in &found {
        warnings.extend(f2_modkit::levels::check_files(&data.vfs, l));
        let mut e = extra_level(l);
        if let Some(id) = &l.manifest.text_id {
            if let Some(n) = text_index(id, &mut warnings) {
                e.data.description_str = n;
            }
        }
        if let (Some(w), Some(wi)) = (e.world_patch.as_mut(), l.manifest.world_info.as_ref()) {
            if let Some(id) = &wi.text_id {
                w.description_str = text_index(id, &mut warnings);
            }
        }
        reg.levels.push(e);
    }
    (reg, warnings)
}

/// One manifest → registry entry: the base row and its files, then the manifest's fields.
pub fn extra_level(l: &FoundLevel) -> ExtraLevel {
    let m = &l.manifest;
    let (w, lv) = (m.world, m.level);
    let [bw, bl] = m.base.unwrap_or([w, lv]);
    let bwvd = &ORIG_WORLD_VISUAL_DATA[bw as usize];
    let mut row = bwvd.level_visual_data[bl as usize];

    // Files of the base slot, spelled out so a copy into another world keeps them.
    let mut files = LevelFileNames::default();
    if m.base.is_some() {
        files.collision = collision_file_for_coll_bank(row.coll_bank).map(String::from);
        files.scenics = scenics_file_for_coll_bank(row.coll_bank).map(String::from);
        files.entities = Some(entity_file_name(bwvd.world_id, row.level_id));
        files.script = Some(script_file_name(bwvd.world_id, row.level_id));
        files.sfx_mapping = Some(f2_game::general::sfx_mapping_file_name(bw, bl));
        files.texture_bank = texture_bank_file(bwvd.master_texture_bank as i32);
        files.world_object_bank = object_bank_file(bwvd.master_object_bank as i32);
        if row.coll_bank > -1 && row.object_bank_to_use > 0 {
            files.object_bank = object_bank_file(row.object_bank_to_use);
        }
        if bw != w {
            files.sfx_bank = f2_game::pcaudio::world_bank(bw).map(|b| format!("{}{b}", f2_game::pcaudio::SFX_BASE));
        }
        if [bw, bl] != [w, lv] {
            row.level_completed = 0;
            row.level_beaten = 0;
            row.max_coins = 0;
        }
    }
    let f = &m.files;
    let set = |slot: &mut Option<String>, v: &Option<String>| {
        if let Some(v) = v {
            *slot = Some(level_path(&l.dir, v));
        }
    };
    set(&mut files.collision, &f.collision);
    set(&mut files.scenics, &f.scenics);
    set(&mut files.entities, &f.entities);
    set(&mut files.script, &f.script);
    set(&mut files.sfx_mapping, &f.sfx_mapping);
    set(&mut files.texture_bank, &f.texture_bank);
    set(&mut files.world_object_bank, &f.world_object_bank);
    set(&mut files.object_bank, &f.object_bank);
    set(&mut files.sfx_bank, &f.sfx_bank);

    if let Some(v) = m.level_id {
        row.level_id = v;
    }
    if let Some([nw, nl]) = m.next {
        row.next_world_id = nw;
        row.next_level_id = nl as u32;
    }
    if let Some(c) = m.fog {
        row.fog = c.map(|x| x as i8);
    }
    if let Some(v) = m.fog_near {
        row.fog_near_dist = v;
    }
    if let Some(v) = m.fog_far {
        row.fog_far_dist = v;
    }
    if let Some(v) = m.far_clip {
        row.far_clip = v;
    }
    if let Some(v) = m.par_time {
        row.par_time = v;
    }
    row.level_open = match m.open {
        Some(false) => LEVEL_CLOSED,
        _ => LEVEL_OPEN,
    };
    let world_patch = m.world_info.as_ref().map(|wi| WorldPatch {
        world_id: wi.world_id,
        description_str: None,
        name: wi.name.clone(),
        world_open: wi.open.map(|o| if o { WORLD_OPEN } else { WORLD_CLOSED }),
    });
    ExtraLevel { world: w, level: lv, data: row, name: m.name.clone(), files, music_track: m.music, world_patch }
}
