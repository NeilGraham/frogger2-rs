//! Port of `Common/memload.c` `MemLoadEntities`: creates the level's enemies,
//! platforms, placeholders, garibs and transition cameras from the entity file.
//! The byte stream is decoded by `f2_formats::entity` (same field order and the
//! same `>>4` / `>>16` / `>>8` scalings as the loader); this module does the
//! creation half in the original order.

use f2_formats::entity::{ActorKind, Entities, Item, Place};
use f2_math::{add_vector_ssf, fmul, new_set_vector_fs, scale_vector_ff, FVector, SVector, Vector};

use crate::babyfrog::BABY_SCALE;
use crate::cam::TransCameraDef;
use crate::collect::EXTRALIFE_GARIB;
use crate::define::DIFFICULTY_EASY;
use crate::effects::Effect;
use crate::enemies::ENEMY_NEW_BABYFROG;
use crate::general::{find_nearest_tile_s, find_nearest_tile_v, gstrcmp};
use crate::ids::{ActorId, EnemyId, TileId};
use crate::map::SCALE;
use crate::path::{Path, PathNode};
use crate::Game;

/// `MEMLOAD_ENTITY_VERSION`.
pub const MEMLOAD_ENTITY_VERSION: u8 = 16;

/// Globals of `memload.c`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemLoadState {
    /// `memload_fast`: entity places are tile indices.
    pub memload_fast: bool,
    /// `createExtraLife`: every other extra-life garib is skipped unless on easy.
    pub create_extra_life: i16,
}

/// `v * SCALE` on a `short` (the loader stores `>>16` into a `short` first).
fn scaled_s(p: [i32; 3]) -> SVector {
    let s = |x: i32| (x as i16 as i32).wrapping_mul(SCALE) as i16;
    SVector { vx: s(p[0]), vy: s(p[1]), vz: s(p[2]), pad: 0 }
}

impl Game {
    /// `&firstTile[index]` for a "fast" entity file (`None` when out of range: the
    /// original does not check).
    fn fast_tile(&self, index: i32) -> Option<TileId> {
        (index >= 0 && (index as usize) < self.map.tiles.len()).then_some(TileId(index as u32))
    }

    /// Port of `MemLoadEntities` (Common/memload.c:107). Returns 1 (the version check
    /// happens when the file is parsed).
    pub fn mem_load_entities(&mut self, ents: &Entities) -> i32 {
        self.memload.create_extra_life = 1;
        self.memload.memload_fast = ents.fast;
        self.effects.push(Effect::Log {
            text: if ents.fast { "MemLoadEntities() --- SUPERFAST VERSION ---".into() } else { "MemLoadEntities() ---------------------".into() },
        });

        self.path.allocate_path_list(ents.total_path_nodes as i32);
        self.effects.push(Effect::Log { text: format!("Loading {} paths..", ents.paths.len()) });

        let mut path_list: Vec<Path> = Vec::with_capacity(ents.paths.len());
        for p in &ents.paths {
            let num_nodes = p.nodes.len() as i32;
            let first = self.path.get_path_nodes(num_nodes, &mut self.effects);
            for (k, n) in p.nodes.iter().enumerate() {
                let world_tile = match n.place {
                    Place::Tile(index) => self.fast_tile(index),
                    Place::Pos(v) => {
                        let v = Vector::new(v[0].wrapping_mul(SCALE), v[1].wrapping_mul(SCALE), v[2].wrapping_mul(SCALE));
                        find_nearest_tile_v(&self.map, v)
                    }
                };
                self.path.nodes[first as usize + k] = PathNode {
                    world_tile,
                    offset: n.offset,
                    offset2: n.offset2,
                    speed: n.speed,
                    wait_time: n.wait_time as i16,
                    // FindSample(uid): the audio layer resolves the uid.
                    sample: n.sample_uid,
                };
            }
            path_list.push(Path { nodes: first, num_nodes: num_nodes as i16, ..Default::default() });
        }

        self.alloc_nme_block(ents.total_enemies as i32 + ents.total_placeholders as i32);
        self.alloc_platform_block(ents.total_platforms as i32);
        self.alloc_cam_block(ents.total_camera_cases as i32);
        self.alloc_garib_block(ents.total_garibs as i32);

        for item in &ents.items {
            match item {
                Item::Actor(a) => {
                    let mut effects = a.effects;
                    // mike frig to get rid of butterfly trails!?!?
                    if gstrcmp(&a.model, "bfly.obe") == 0 && effects & 4 != 0 {
                        effects &= !4;
                    }
                    let mut scale = a.scale;
                    let mut path = path_list[a.path_index as usize];
                    path.start_node = a.start_node as i16;

                    let mut act: Option<ActorId> = None;
                    let mut enemy: Option<EnemyId> = None;
                    match a.kind {
                        ActorKind::Enemy => {
                            self.effects.push(Effect::Log { text: format!("Enemy: {} (ID {})..", a.model, a.uid) });
                            enemy = self.create_and_add_enemy(&a.model, a.flags, a.uid, path, a.anim_speed, a.facing);
                            match enemy {
                                None => self.effects.push(Effect::Log { text: "failed!".into() }),
                                Some(e) => act = self.enemies.enemy_list.array[e.idx()].nme_actor,
                            }
                        }
                        ActorKind::Placeholder => {
                            self.effects.push(Effect::Log { text: format!("Placeholder ID {}..", a.uid) });
                            let l = &mut self.enemies.enemy_list;
                            let idx = l.count.max(0) as usize;
                            if idx < l.array.len() {
                                let e = &mut l.array[idx];
                                e.path = path;
                                e.active = 0;
                                e.uid = a.uid;
                                e.nme_actor = None;
                                self.add_enemy(EnemyId(idx as u32));
                            } else {
                                // The original writes past the enemy block here.
                                self.effects.push(Effect::Log { text: format!("Placeholder {} does not fit the enemy block", a.uid) });
                            }
                        }
                        ActorKind::Platform => {
                            if gstrcmp(&a.model, "blank.obe") != 0 {
                                self.effects.push(Effect::Log { text: format!("Platform: {} (ID {})..", a.model, a.uid) });
                                match self.create_and_add_platform(&a.model, a.flags, a.uid, path, a.anim_speed, a.facing) {
                                    None => self.effects.push(Effect::Log { text: "failed!".into() }),
                                    Some(p) => act = self.platform.platform_list.array[p.idx()].plt_actor,
                                }
                            }
                        }
                    }

                    if let Some(act) = act {
                        if a.kind == ActorKind::Enemy {
                            let e = enemy.expect("enemy actor without enemy");
                            if self.enemies.enemy_list.array[e.idx()].flags & ENEMY_NEW_BABYFROG == 0 {
                                self.actor2.actors[act].effects = effects as u32;
                            } else {
                                scale = fmul(scale, BABY_SCALE);
                            }
                        }
                        let x = &mut self.actor2.actors[act];
                        x.radius = a.radius;
                        x.actor.size = FVector { vx: scale, vy: scale, vz: scale };
                        x.anim_speed = a.anim_speed;
                        x.value1 = a.value1;
                        x.depth_shift = a.depth_shift;
                        x.dff_clipping = a.dff_clipping;
                        // PC `!rHardware` sort hacks (sortHackNames) dropped: the port models
                        // the hardware renderer.
                    }
                    self.effects.push(Effect::Log { text: "ok".into() });
                }
                Item::Garib(gd) => {
                    let n = gd.garib_type as i32;
                    let tile = match gd.place {
                        Place::Tile(index) => self.fast_tile(index),
                        Place::Pos(p) => find_nearest_tile_s(&self.map, scaled_s(p)),
                    };
                    let Some(tile) = tile else {
                        // The original dereferences the NULL tile.
                        self.effects.push(Effect::Log { text: "garib without tile".into() });
                        continue;
                    };
                    let t = self.map.tile(tile).clone();
                    let mut w = FVector::default();
                    new_set_vector_fs(&mut w, &t.normal);
                    scale_vector_ff(&mut w, gd.height_offset.wrapping_mul(SCALE));
                    let mut v = SVector::default();
                    add_vector_ssf(&mut v, &t.centre, &w);

                    if n != EXTRALIFE_GARIB || self.game_state.difficulty == DIFFICULTY_EASY || self.memload.create_extra_life != 0 {
                        self.create_new_garib(v, n);
                    }
                    if n == EXTRALIFE_GARIB {
                        self.memload.create_extra_life = 1 - self.memload.create_extra_life;
                    }
                }
                Item::CameraCase(c) => {
                    let v = scaled_s(c.pos);
                    for place in &c.nodes {
                        let tile = match *place {
                            Place::Tile(index) => self.fast_tile(index),
                            Place::Pos(p) => find_nearest_tile_s(&self.map, scaled_s(p)),
                        };
                        self.create_and_add_trans_camera(TransCameraDef {
                            tile,
                            dir_cam_must_face: (c.flags >> 16) as u8,
                            pos: v,
                            flags: (c.flags & 0xFF) as u8,
                            fov: c.fov,
                            speed: c.speed << 12,
                            target: c.target,
                        });
                    }
                }
            }
        }

        self.effects.push(Effect::Log { text: "Success!".into() });
        1
    }
}
