//! The curated game API of behaviour scripts: natives (functions) and built-in constants.
//! Handles are plain integers (tile index, enemy index, player number); every native checks
//! its arguments, so a script can never index out of range or reach raw memory.

use f2_math::{FVector, SVector};

use super::vm::{Host, Value};
use super::ModScriptState;
use crate::define::tilestate;
use crate::frogmove::FROGSTATUS_ISDEAD;
use crate::ids::{EnemyId, TileId};
use crate::textover::{Font, TEXTOVERLAY_SHADOW};
use crate::Game;

macro_rules! natives {
    ($($v:ident = $name:literal / $arity:literal),* $(,)?) => {
        /// Built-in functions (`name / arity`).
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u16)]
        pub enum Native { $($v),* }
        const NATIVES: &[(Native, &str, usize)] = &[$((Native::$v, $name, $arity)),*];
    };
}

natives! {
    Log = "log" / 1,
    Frame = "frame" / 0,
    LevelFrame = "level_frame" / 0,
    World = "world" / 0,
    Level = "level" / 0,
    NumFrogs = "num_frogs" / 0,
    Random = "random" / 1,
    Abs = "abs" / 1,
    Min = "min" / 2,
    Max = "max" / 2,
    Clamp = "clamp" / 3,
    Str = "str" / 1,
    FrogTile = "frog_tile" / 1,
    FrogLives = "frog_lives" / 1,
    FrogDead = "frog_dead" / 1,
    FrogState = "frog_state" / 1,
    KillFrog = "kill_frog" / 2,
    TileCount = "tile_count" / 0,
    TileState = "tile_state" / 1,
    SetTileState = "set_tile_state" / 2,
    TileNeighbour = "tile_neighbour" / 2,
    TileX = "tile_x" / 1,
    TileY = "tile_y" / 1,
    TileZ = "tile_z" / 1,
    EnemyCount = "enemy_count" / 0,
    EnemyFind = "enemy_find" / 1,
    EnemyUid = "enemy_uid" / 1,
    EnemyModel = "enemy_model" / 1,
    EnemyActive = "enemy_active" / 1,
    EnemySetActive = "enemy_set_active" / 2,
    EnemySpeed = "enemy_speed" / 1,
    EnemySetSpeed = "enemy_set_speed" / 2,
    EnemyFlags = "enemy_flags" / 1,
    EnemySetFlags = "enemy_set_flags" / 2,
    EnemyWaiting = "enemy_waiting" / 1,
    EnemySetWaiting = "enemy_set_waiting" / 2,
    EnemyNode = "enemy_node" / 1,
    EnemyToNode = "enemy_to_node" / 1,
    EnemyNumNodes = "enemy_num_nodes" / 1,
    EnemySetNodes = "enemy_set_nodes" / 3,
    EnemyStartFrame = "enemy_start_frame" / 1,
    EnemyEndFrame = "enemy_end_frame" / 1,
    EnemySetFrames = "enemy_set_frames" / 3,
    EnemyTile = "enemy_tile" / 1,
    EnemyX = "enemy_x" / 1,
    EnemyY = "enemy_y" / 1,
    EnemyZ = "enemy_z" / 1,
    EnemySetPos = "enemy_set_pos" / 4,
    ClaimEnemy = "claim_enemy" / 1,
    ClaimEnemyModel = "claim_enemy_model" / 1,
    EnemyRunDefault = "enemy_run_default" / 1,
    SpawnFx = "spawn_fx" / 2,
    SpawnFxEx = "spawn_fx_ex" / 6,
    PlaySample = "play_sample" / 1,
    PlaySampleAt = "play_sample_at" / 2,
    ShowText = "show_text" / 2,
    ShowTextAt = "show_text_at" / 4,
    HideText = "hide_text" / 1,
    SetFog = "set_fog" / 3,
    SetFogRange = "set_fog_range" / 2,
}

impl Native {
    pub fn by_name(name: &str) -> Option<Native> {
        NATIVES.iter().find(|n| n.1 == name).map(|n| n.0)
    }
    pub fn from_id(id: u16) -> Native {
        NATIVES[id as usize].0
    }
    pub fn name(self) -> &'static str {
        NATIVES[self as usize].1
    }
    pub fn arity(self) -> usize {
        NATIVES[self as usize].2
    }
    /// Every native `(name, arity)` (docs/tests).
    pub fn all() -> impl Iterator<Item = (&'static str, usize)> {
        NATIVES.iter().map(|n| (n.1, n.2))
    }
}

/// Built-in constants (`TILE_*`, `DEATH_*`, `PICKUP_*`, `FX_*`, `ENEMY_*`, `FIXED_ONE`).
pub const CONSTS: &[(&str, i32)] = &[
    ("FIXED_ONE", 4096),
    ("TILE_NORMAL", tilestate::NORMAL as i32),
    ("TILE_DEADLY", tilestate::DEADLY as i32),
    ("TILE_SINK", tilestate::SINK as i32),
    ("TILE_ICE", tilestate::ICE as i32),
    ("TILE_SUPERHOP", tilestate::SUPERHOP as i32),
    ("TILE_JOIN", tilestate::JOIN as i32),
    ("TILE_SAFE", tilestate::SAFE as i32),
    ("TILE_FINISHLINE", tilestate::FINISHLINE as i32),
    ("TILE_GRAPPLE", tilestate::GRAPPLE as i32),
    ("TILE_SMASH", tilestate::SMASH as i32),
    ("TILE_BARRED", tilestate::BARRED as i32),
    ("TILE_FALL", tilestate::FALL as i32),
    ("TILE_DEADLYFALL", tilestate::DEADLYFALL as i32),
    ("TILE_HOT", tilestate::HOT as i32),
    ("TILE_ELECTRIC", tilestate::ELECTRIC as i32),
    ("TILE_NOSUPER", tilestate::NOSUPER as i32),
    ("TILE_PANTS", tilestate::PANTS as i32),
    ("TILE_NOSUPERHOT", tilestate::NOSUPERHOT as i32),
    ("TILE_OCCUPIED", tilestate::OCCUPIED as i32),
    ("TILE_CONVEYOR", tilestate::CONVEYOR as i32),
    ("TILE_CONVEYOR_MED", tilestate::CONVEYOR_MED as i32),
    ("TILE_CONVEYOR_FAST", tilestate::CONVEYOR_FAST as i32),
    ("TILE_CONVEYOR_ONEWAY", tilestate::CONVEYOR_ONEWAY as i32),
    ("DEATH_NORMAL", crate::frogmove::DEATHBY_NORMAL),
    ("DEATH_RUNOVER", crate::frogmove::DEATHBY_RUNOVER),
    ("DEATH_DROWNING", crate::frogmove::DEATHBY_DROWNING),
    ("DEATH_SQUASHED", crate::frogmove::DEATHBY_SQUASHED),
    ("DEATH_FIRE", crate::frogmove::DEATHBY_FIRE),
    ("DEATH_ELECTRIC", crate::frogmove::DEATHBY_ELECTRIC),
    ("DEATH_FALLING", crate::frogmove::DEATHBY_FALLING),
    ("DEATH_WHACKING", crate::frogmove::DEATHBY_WHACKING),
    ("DEATH_INFLATION", crate::frogmove::DEATHBY_INFLATION),
    ("DEATH_POISON", crate::frogmove::DEATHBY_POISON),
    ("DEATH_SLICING", crate::frogmove::DEATHBY_SLICING),
    ("DEATH_EXPLOSION", crate::frogmove::DEATHBY_EXPLOSION),
    ("DEATH_GIBBING", crate::frogmove::DEATHBY_GIBBING),
    ("DEATH_INCINERATE", crate::frogmove::DEATHBY_INCINERATE),
    ("DEATH_BURSTING", crate::frogmove::DEATHBY_BURSTING),
    ("DEATH_SPIKED", crate::frogmove::DEATHBY_SPIKED),
    ("DEATH_LIGHTNING", crate::frogmove::DEATHBY_LIGHTNING),
    ("DEATH_FALLINGFOREVER", crate::frogmove::DEATHBY_FALLINGFOREVER),
    ("PICKUP_SILVERCOIN", crate::collect::SILVERCOIN_GARIB),
    ("PICKUP_EXTRALIFE", crate::collect::EXTRALIFE_GARIB),
    ("PICKUP_AUTOHOP", crate::collect::AUTOHOP_GARIB),
    ("PICKUP_LONGTONGUE", crate::collect::LONGTONGUE_GARIB),
    ("PICKUP_QUICKHOP", crate::collect::QUICKHOP_GARIB),
    ("PICKUP_INVULNERABILITY", crate::collect::INVULNERABILITY_GARIB),
    ("PICKUP_GOLDCOIN", crate::collect::GOLDCOIN_GARIB),
    ("PICKUP_SLOWHOP", crate::collect::SLOWHOP_GARIB),
    ("FX_WATERRIPPLE", crate::specfx::FXTYPE_WATERRIPPLE as i32),
    ("FX_GARIBCOLLECT", crate::specfx::FXTYPE_GARIBCOLLECT as i32),
    ("FX_SPARKLYTRAIL", crate::specfx::FXTYPE_SPARKLYTRAIL as i32),
    ("FX_FROGSTUN", crate::specfx::FXTYPE_FROGSTUN as i32),
    ("FX_SMOKE_STATIC", crate::specfx::FXTYPE_SMOKE_STATIC as i32),
    ("FX_SMOKE_GROWS", crate::specfx::FXTYPE_SMOKE_GROWS as i32),
    ("FX_DECAL", crate::specfx::FXTYPE_DECAL as i32),
    ("FX_SPLASH", crate::specfx::FXTYPE_SPLASH as i32),
    ("FX_SMOKEBURST", crate::specfx::FXTYPE_SMOKEBURST as i32),
    ("FX_GREENGLOOP", crate::specfx::FXTYPE_GREENGLOOP as i32),
    ("FX_BUBBLES", crate::specfx::FXTYPE_BUBBLES as i32),
    ("FX_SPARKBURST", crate::specfx::FXTYPE_SPARKBURST as i32),
    ("FX_FIERYSMOKE", crate::specfx::FXTYPE_FIERYSMOKE as i32),
    ("FX_POLYRING", crate::specfx::FXTYPE_POLYRING as i32),
    ("FX_GLOW", crate::specfx::FXTYPE_GLOW as i32),
    ("FX_TWINKLE", crate::specfx::FXTYPE_TWINKLE as i32),
    ("FX_CROAK", crate::specfx::FXTYPE_CROAK as i32),
    ("FX_FLASH", crate::specfx::FXTYPE_FLASH as i32),
    ("ENEMY_FOLLOWPATH", crate::enemies::ENEMY_NEW_FOLLOWPATH as i32),
    ("ENEMY_PINGPONG", crate::enemies::ENEMY_NEW_PINGPONG as i32),
    ("ENEMY_CYCLE", crate::enemies::ENEMY_NEW_CYCLE as i32),
    ("ENEMY_NODAMAGE", crate::enemies::ENEMY_NEW_NODAMAGE as i32),
    ("ENEMY_ONEHITKILL", crate::enemies::ENEMY_NEW_ONEHITKILL as i32),
    ("ENEMY_FACEFORWARDS", crate::enemies::ENEMY_NEW_FACEFORWARDS as i32),
    ("ENEMY_DONOTMOVE", crate::enemies::ENEMY_NEW_DONOTMOVE as i32),
    ("ENEMY_RADIUSCOLLISION", crate::enemies::ENEMY_NEW_RADIUSBASEDCOLLISION as i32),
];

pub fn builtin_const(name: &str) -> Option<i32> {
    CONSTS.iter().find(|c| c.0 == name).map(|c| c.1)
}

/// Text overlays a script may have on screen at once.
pub const MAX_TEXTS: usize = 8;

/// The natives' view of the game during one script run.
pub struct ApiHost<'a> {
    pub g: &'a mut Game,
    pub st: &'a mut ModScriptState,
    pub unit: usize,
}

fn sv_to_fv(s: &SVector) -> FVector {
    FVector { vx: s.vx as i32, vy: s.vy as i32, vz: s.vz as i32 }
}

impl ApiHost<'_> {
    fn int(&self, args: &[Value], i: usize) -> Result<i32, String> {
        match &args[i] {
            Value::Int(v) => Ok(*v),
            Value::Str(_) => Err(format!("argument {} must be a number", i + 1)),
        }
    }
    fn text<'v>(&self, args: &'v [Value], i: usize) -> Result<&'v str, String> {
        match &args[i] {
            Value::Str(s) => Ok(s),
            Value::Int(_) => Err(format!("argument {} must be a string", i + 1)),
        }
    }
    fn player(&self, args: &[Value], i: usize) -> Result<usize, String> {
        let p = self.int(args, i)?;
        if (0..4).contains(&p) {
            Ok(p as usize)
        } else {
            Err(format!("no player {p}"))
        }
    }
    fn tile(&self, args: &[Value], i: usize) -> Result<TileId, String> {
        let t = self.int(args, i)?;
        if t >= 0 && (t as usize) < self.g.map.tiles.len() {
            Ok(TileId(t as u32))
        } else {
            Err(format!("no tile {t}"))
        }
    }
    fn enemy(&self, args: &[Value], i: usize) -> Result<EnemyId, String> {
        let e = self.int(args, i)?;
        match usize::try_from(e).ok().and_then(|i| self.g.enemies.enemy_list.array.get(i)) {
            Some(n) if n.linked => Ok(EnemyId(e as u32)),
            _ => Err(format!("no enemy {e}")),
        }
    }
    fn nme(&mut self, id: EnemyId) -> &mut crate::enemies::Enemy {
        &mut self.g.enemies.enemy_list.array[id.idx()]
    }
    fn enemy_pos(&mut self, args: &[Value]) -> Result<Option<&mut SVector>, String> {
        let e = self.enemy(args, 0)?;
        Ok(self.g.enemies.enemy_list.array[e.idx()].nme_actor.and_then(|a| self.g.actor2.actors.get_mut(a)).map(|a| &mut a.actor.position))
    }
    fn claim(&mut self, e: EnemyId) {
        let e = e.0;
        match self.st.claims.iter_mut().find(|c| c.0 == e) {
            Some(c) => c.1 = self.unit as u32,
            None => self.st.claims.push((e, self.unit as u32)),
        }
    }
    fn show_text(&mut self, text: &str, x: i32, y: i32, frames: i32) -> i32 {
        // Expire our oldest overlay when at the limit.
        let mine: Vec<usize> = self.st.texts.iter().enumerate().filter(|(_, t)| t.unit == self.unit as u32).map(|(i, _)| i).collect();
        if mine.len() >= MAX_TEXTS {
            let t = self.st.texts.remove(mine[0]);
            self.g.textover.sub_text_overlay(Some(crate::ids::TextOverlayId(t.overlay)));
        }
        let Some(id) =
            self.g.textover.create_and_add_text_overlay(x.clamp(0, 4096) as i16, y.clamp(0, 4096) as i16, text, true, 255, Font::Normal, TEXTOVERLAY_SHADOW)
        else {
            return -1;
        };
        let expire = if frames > 0 { self.g.act_frame_count.wrapping_add(frames as u32) } else { 0 };
        self.st.texts.push(super::ScriptText { unit: self.unit as u32, overlay: id.0, expire, live: frames > 0 });
        id.0 as i32
    }
}

impl Host for ApiHost<'_> {
    fn native(&mut self, id: u16, a: &[Value]) -> Result<Value, String> {
        use Native as N;
        let n = Native::from_id(id);
        let i = |v: i32| Ok(Value::Int(v));
        let unit = |_: ()| Ok(Value::Int(0));
        match n {
            N::Log => {
                let name = &self.st.units[self.unit].name;
                let text = format!("modscript {name}: {}", a[0]);
                self.g.effects.push(crate::Effect::Log { text });
                unit(())
            }
            N::Frame => i(self.g.act_frame_count as i32),
            N::LevelFrame => i(self.g.frame_count),
            N::World => i(self.g.frogger.player[0].world_num as i32),
            N::Level => i(self.g.frogger.player[0].level_num as i32),
            N::NumFrogs => i(self.g.frogger.num_frogs),
            N::Random => {
                let n = self.int(a, 0)?;
                i(if n <= 0 { 0 } else { self.g.rng.random(n as u32) })
            }
            N::Abs => i(self.int(a, 0)?.wrapping_abs()),
            N::Min => i(self.int(a, 0)?.min(self.int(a, 1)?)),
            N::Max => i(self.int(a, 0)?.max(self.int(a, 1)?)),
            N::Clamp => {
                let (v, lo, hi) = (self.int(a, 0)?, self.int(a, 1)?, self.int(a, 2)?);
                i(v.max(lo).min(hi))
            }
            N::Str => Ok(Value::Str(a[0].to_string())),
            N::FrogTile => {
                let p = self.player(a, 0)?;
                i(self.g.frogmove.curr_tile[p].map_or(-1, |t| t.idx() as i32))
            }
            N::FrogLives => {
                let p = self.player(a, 0)?;
                i(self.g.frogger.player[p].lives as i32)
            }
            N::FrogDead => {
                let p = self.player(a, 0)?;
                i((self.g.frogger.player[p].frog_state & FROGSTATUS_ISDEAD != 0) as i32)
            }
            N::FrogState => {
                let p = self.player(a, 0)?;
                i(self.g.frogger.player[p].frog_state as i32)
            }
            N::KillFrog => {
                let p = self.player(a, 0)?;
                let cause = self.int(a, 1)?;
                let pl = &self.g.frogger.player[p];
                if (p as i32) >= self.g.frogger.num_frogs || self.g.frogger.frog[p].is_none() || pl.dead.time != 0 || pl.frog_state & FROGSTATUS_ISDEAD != 0 {
                    return i(0);
                }
                if self.g.game_state.multi == crate::define::SINGLEPLAYER {
                    self.g.frogger.player[p].frog_state |= FROGSTATUS_ISDEAD;
                    let cause = if (0..=crate::frogmove::DEATHBY_LIGHTNING).contains(&cause) { cause } else { 0 };
                    self.g.call_death_anim(cause, p as i32);
                } else {
                    self.g.kill_mp_frog(p as i32);
                }
                i(1)
            }
            N::TileCount => i(self.g.map.tiles.len() as i32),
            N::TileState => {
                let t = self.tile(a, 0)?;
                i(self.g.map.tile(t).state as i32)
            }
            N::SetTileState => {
                let t = self.tile(a, 0)?;
                let s = self.int(a, 1)?;
                if !(0..=0xff).contains(&s) {
                    return Err(format!("bad tile state {s}"));
                }
                self.g.map.tile_mut(t).state = s as u32;
                unit(())
            }
            N::TileNeighbour => {
                let t = self.tile(a, 0)?;
                let d = self.int(a, 1)?;
                if !(0..4).contains(&d) {
                    return Err(format!("bad direction {d} (0-3)"));
                }
                i(self.g.map.tile(t).tile_ptrs[d as usize].map_or(-1, |n| n.idx() as i32))
            }
            N::TileX | N::TileY | N::TileZ => {
                let t = self.tile(a, 0)?;
                let c = self.g.map.tile(t).centre;
                i(match n {
                    N::TileX => c.vx,
                    N::TileY => c.vy,
                    _ => c.vz,
                } as i32)
            }
            N::EnemyCount => i(self.g.enemies.enemy_list.array.len() as i32),
            N::EnemyFind => {
                let uid = self.int(a, 0)?;
                i(self.g.enemies.enemy_list.array.iter().position(|e| e.linked && e.uid == uid).map_or(-1, |p| p as i32))
            }
            N::EnemyUid => {
                let e = self.enemy(a, 0)?;
                i(self.nme(e).uid)
            }
            N::EnemyModel => {
                let e = self.enemy(a, 0)?;
                let name = self.nme(e).nme_actor.and_then(|x| self.g.actor2.actors.get(x)).map(|x| x.actor.name.clone()).unwrap_or_default();
                Ok(Value::Str(name))
            }
            N::EnemyActive => {
                let e = self.enemy(a, 0)?;
                i(self.nme(e).active as i32)
            }
            N::EnemySetActive => {
                let e = self.enemy(a, 0)?;
                let v = self.int(a, 1)?;
                self.nme(e).active = (v != 0) as u8;
                unit(())
            }
            N::EnemySpeed => {
                let e = self.enemy(a, 0)?;
                i(self.nme(e).speed)
            }
            N::EnemySetSpeed => {
                let e = self.enemy(a, 0)?;
                let v = self.int(a, 1)?;
                self.nme(e).speed = v;
                unit(())
            }
            N::EnemyFlags => {
                let e = self.enemy(a, 0)?;
                i(self.nme(e).flags as i32)
            }
            N::EnemySetFlags => {
                let e = self.enemy(a, 0)?;
                let v = self.int(a, 1)?;
                self.nme(e).flags = v as u32;
                unit(())
            }
            N::EnemyWaiting => {
                let e = self.enemy(a, 0)?;
                i(self.nme(e).is_waiting as i32)
            }
            N::EnemySetWaiting => {
                let e = self.enemy(a, 0)?;
                let v = self.int(a, 1)?;
                self.nme(e).is_waiting = v.clamp(-1, i16::MAX as i32) as i16;
                unit(())
            }
            N::EnemyNode => {
                let e = self.enemy(a, 0)?;
                i(self.nme(e).path.from_node as i32)
            }
            N::EnemyToNode => {
                let e = self.enemy(a, 0)?;
                i(self.nme(e).path.to_node as i32)
            }
            N::EnemyNumNodes => {
                let e = self.enemy(a, 0)?;
                i(self.nme(e).path.num_nodes as i32)
            }
            N::EnemySetNodes => {
                let e = self.enemy(a, 0)?;
                let (f, t) = (self.int(a, 1)?, self.int(a, 2)?);
                let nn = self.nme(e).path.num_nodes as i32;
                if !(0..nn).contains(&f) || !(0..nn).contains(&t) {
                    return Err(format!("node out of range (path has {nn} nodes)"));
                }
                let p = &mut self.nme(e).path;
                p.from_node = f as i16;
                p.to_node = t as i16;
                unit(())
            }
            N::EnemyStartFrame => {
                let e = self.enemy(a, 0)?;
                i(self.nme(e).path.start_frame as i32)
            }
            N::EnemyEndFrame => {
                let e = self.enemy(a, 0)?;
                i(self.nme(e).path.end_frame as i32)
            }
            N::EnemySetFrames => {
                let e = self.enemy(a, 0)?;
                let (s, en) = (self.int(a, 1)?, self.int(a, 2)?);
                let p = &mut self.nme(e).path;
                p.start_frame = s as u32;
                p.end_frame = en as u32;
                unit(())
            }
            N::EnemyTile => {
                let e = self.enemy(a, 0)?;
                i(self.nme(e).in_tile.map_or(-1, |t| t.idx() as i32))
            }
            N::EnemyX | N::EnemyY | N::EnemyZ => {
                let p = self.enemy_pos(a)?.map(|p| *p).unwrap_or_default();
                i(match n {
                    N::EnemyX => p.vx,
                    N::EnemyY => p.vy,
                    _ => p.vz,
                } as i32)
            }
            N::EnemySetPos => {
                let (x, y, z) = (self.int(a, 1)?, self.int(a, 2)?, self.int(a, 3)?);
                if let Some(p) = self.enemy_pos(a)? {
                    p.vx = x as i16;
                    p.vy = y as i16;
                    p.vz = z as i16;
                }
                unit(())
            }
            N::ClaimEnemy => {
                let e = self.enemy(a, 0)?;
                self.claim(e);
                unit(())
            }
            N::ClaimEnemyModel => {
                let model = self.text(a, 0)?.to_ascii_lowercase();
                let found: Vec<EnemyId> = (0..self.g.enemies.enemy_list.array.len())
                    .filter(|&k| {
                        let e = &self.g.enemies.enemy_list.array[k];
                        e.linked && e.nme_actor.and_then(|x| self.g.actor2.actors.get(x)).is_some_and(|x| x.actor.name.eq_ignore_ascii_case(&model))
                    })
                    .map(|k| EnemyId(k as u32))
                    .collect();
                for &e in &found {
                    self.claim(e);
                }
                i(found.len() as i32)
            }
            N::EnemyRunDefault => {
                let e = self.enemy(a, 0)?;
                if self.nme(e).nme_actor.is_some() {
                    self.g.update_enemy_motion(e);
                }
                unit(())
            }
            N::SpawnFx | N::SpawnFxEx => {
                let ty = self.int(a, 0)?;
                if !(1..crate::specfx::FXTYPE_NUMTYPES as i32).contains(&ty) {
                    return Err(format!("bad effect type {ty}"));
                }
                let t = self.tile(a, 1)?;
                let (size, speed, accn, life) =
                    if n == N::SpawnFxEx { (self.int(a, 2)?, self.int(a, 3)?, self.int(a, 4)?, self.int(a, 5)?) } else { (102400, 4096, 410, 3277) };
                let tt = self.g.map.tile(t);
                let (c, nrm) = (tt.centre, sv_to_fv(&tt.normal));
                i(self.g.create_special_effect(ty as i16, &c, &nrm, size, speed, accn, life).is_some() as i32)
            }
            N::PlaySample | N::PlaySampleAt => {
                let name = self.text(a, 0)?.to_string();
                let pos = if n == N::PlaySampleAt { Some(self.g.map.tile(self.tile(a, 1)?).centre) } else { None };
                self.g.play_sample(f2_formats::crc::crc(&name), pos, 0, crate::anim::SAMPLE_VOLUME, -1);
                unit(())
            }
            N::ShowText => {
                let text = self.text(a, 0)?.to_string();
                let frames = self.int(a, 1)?;
                i(self.show_text(&text, 2048, 1400, frames))
            }
            N::ShowTextAt => {
                let text = self.text(a, 0)?.to_string();
                let (x, y, frames) = (self.int(a, 1)?, self.int(a, 2)?, self.int(a, 3)?);
                i(self.show_text(&text, x, y, frames))
            }
            N::HideText => {
                let id = self.int(a, 0)?;
                if let Some(k) = self.st.texts.iter().position(|t| t.unit == self.unit as u32 && t.overlay as i32 == id) {
                    let t = self.st.texts.remove(k);
                    self.g.textover.sub_text_overlay(Some(crate::ids::TextOverlayId(t.overlay)));
                }
                unit(())
            }
            N::SetFog => {
                let (r, gg, b) = (self.int(a, 0)?, self.int(a, 1)?, self.int(a, 2)?);
                let f = &mut self.g.frogger.fog;
                f.r = r.clamp(0, 255) as u8;
                f.g = gg.clamp(0, 255) as u8;
                f.b = b.clamp(0, 255) as u8;
                unit(())
            }
            N::SetFogRange => {
                let (near, far) = (self.int(a, 0)?, self.int(a, 1)?);
                let f = &mut self.g.frogger.fog;
                f.min = near.clamp(0, i16::MAX as i32) as i16;
                f.max = far.clamp(0, i16::MAX as i32) as i16;
                unit(())
            }
        }
    }
}
