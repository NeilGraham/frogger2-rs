//! Port of `Common/platform.c` / `platform.h`: moving platforms.
//!
//! `platformList`, `currPlatform[]` and `destPlatform[]` are defined in `frogger.c`
//! in the original; they live here with the code that owns them.

use crate::prelude::*;

use f2_math::{
    add_to_vector_sf, add_vector_sfs, add_vector_ssf, dot_product_ff, fmul, fsqrt_f, iquat_slerp, make_unit, new_set_vector_fs, scale_vector_ff,
    sub_from_vector_sf, sub_vector_fss, FVector, Fixed, IQuaternion, SVector,
};
use serde::{Deserialize, Serialize};

use crate::actor2::ACTOR_DRAWDISTANCEOUTER;
use crate::anim::SAMPLE_VOLUME;
use crate::define::SINGLEPLAYER;
use crate::effects::Effect;
use crate::frogger::MAX_FROGS;
use crate::frogmove::{FROGSTATUS_ISDEAD, FROGSTATUS_ISJUMPINGTOTILE};
use crate::ids::{ActorId, PlatformId, TileId};
use crate::list::{HasLinks, Head, Links};
use crate::map::{GameTile, SCALE};
use crate::path::{get_position_for_path_node, get_position_for_path_node_offset2, Path, PathNode};
use crate::specfx::{FxEntity, FXTYPE_SMOKEBURST};
use crate::Game;

// ----- platform flags (`platform.h:20`) -----
pub const PLATFORM_NEW_NONE: u32 = 0;
pub const PLATFORM_NEW_FOLLOWPATH: u32 = 1 << 0;
pub const PLATFORM_NEW_FORWARDS: u32 = 1 << 1;
pub const PLATFORM_NEW_BACKWARDS: u32 = 1 << 2;
pub const PLATFORM_NEW_PINGPONG: u32 = 1 << 3;
pub const PLATFORM_NEW_CYCLE: u32 = 1 << 4;
pub const PLATFORM_NEW_MOVEUP: u32 = 1 << 5;
pub const PLATFORM_NEW_MOVEDOWN: u32 = 1 << 6;
pub const PLATFORM_NEW_STEPONACTIVATED: u32 = 1 << 7;
pub const PLATFORM_NEW_SINKWITHFROG: u32 = 1 << 8;
pub const PLATFORM_NEW_RISEWITHFROG: u32 = 1 << 9;
pub const PLATFORM_NEW_CARRYINGFROG: u32 = 1 << 10;
pub const PLATFORM_NEW_DISAPPEARWITHFROG: u32 = 1 << 11;
pub const PLATFORM_NEW_CRUMBLES: u32 = 1 << 12;
pub const PLATFORM_NEW_NONMOVING: u32 = 1 << 13;
pub const PLATFORM_NEW_REGENERATES: u32 = 1 << 14;
pub const PLATFORM_NEW_NOWALKUNDER: u32 = 1 << 15;
pub const PLATFORM_NEW_KILLSFROG: u32 = 1 << 16;
pub const PLATFORM_NEW_FACEFORWARDS: u32 = 1 << 17;
pub const PLATFORM_NEW_SHAKABLESCENIC: u32 = 1 << 18;
pub const PLATFORM_NEW_SHADOW: u32 = 1 << 19;
pub const PLATFORM_NEW_SLERPPATH: u32 = 1 << 20;
pub const PLATFORM_NEW_NOREORIENTATE: u32 = 1 << 21;

/// Platform update functions (`PLATFORM::Update`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlatformUpdate {
    /// `UpdatePathPlatform`.
    Path,
    /// `UpdateUpDownPlatform`.
    UpDown,
    /// `UpdateStepOnActivatedPlatform`.
    StepOnActivated,
}

///
/// Phase 3S: `repr(C)` with the fields `UpdatePlatforms` touches every tick first (D-cache
/// lines on the N64); `ser_order!` keeps the serialized field order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Platform {
    pub links: Links<PlatformId>,
    pub plt_actor: Option<ActorId>,
    pub carrying: Option<ActorId>,
    pub active: u8,
    pub visible: i8,
    pub is_waiting: i16,
    pub flags: u32,
    pub update: Option<PlatformUpdate>,
    pub path: Path,
    pub in_tile: [Option<TileId>; 2],
    pub curr_speed: Fixed,
    pub src_orientation: IQuaternion,
    pub dest_orientation: IQuaternion,
    pub countdown: Fixed,
    pub uid: i32,
    pub facing: u8,
    /// `next != NULL`: the platform is in `platformList`.
    pub linked: bool,
    /// `pltPtrs[4]`: adjacent platforms (never set).
    pub plt_ptrs: [Option<PlatformId>; 4],
    pub dir_vector: [FVector; 4],
    pub init_dir_vector: [FVector; 4],
}

crate::ser_order!(Platform, "Platform", PlatformWire {
    links: Links<PlatformId>,
    linked: bool,
    plt_ptrs: [Option<PlatformId>; 4],
    uid: i32,
    dir_vector: [FVector; 4],
    init_dir_vector: [FVector; 4],
    plt_actor: Option<ActorId>,
    active: u8,
    flags: u32,
    curr_speed: Fixed,
    is_waiting: i16,
    src_orientation: IQuaternion,
    dest_orientation: IQuaternion,
    countdown: Fixed,
    facing: u8,
    visible: i8,
    in_tile: [Option<TileId>; 2],
    path: Path,
    carrying: Option<ActorId>,
    update: Option<PlatformUpdate>,
});

impl HasLinks<PlatformId> for Platform {
    fn links(&self) -> &Links<PlatformId> {
        &self.links
    }
    fn links_mut(&mut self) -> &mut Links<PlatformId> {
        &mut self.links
    }
}

/// `PLATFORMLIST`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformList {
    /// `array` (block of `blocks` zeroed platforms; `None` = not allocated).
    pub array: Vec<Platform>,
    pub count: i16,
    pub blocks: i16,
    pub head: Head<PlatformId>,
}

/// Platform globals (`platformList`, `currPlatform`, `destPlatform`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformState {
    pub platform_list: PlatformList,
    /// `destPlatform[MAX_FROGS]`: platform the frog is about to attempt to jump to.
    pub dest_platform: [Option<PlatformId>; MAX_FROGS],
    /// `currPlatform[MAX_FROGS]`: platform the frog is currently on.
    pub curr_platform: [Option<PlatformId>; MAX_FROGS],
}

/// Unsigned `a / b` as the C `unsigned long` expressions compute it. A zero divisor
/// (zero speed) faults in the original; the port yields 0.
#[inline]
pub(crate) fn udiv(a: u32, b: u32) -> i32 {
    a.checked_div(b).unwrap_or(0) as i32
}

/// Iteration cap for the original's catch-up `do { } while` loops, which never end
/// when a path's speed and wait times are all zero (the original hangs).
pub(crate) const CATCHUP_GUARD: u32 = 1_000_000;

impl Game {
    /// `&firstTile[...]` for a possibly NULL tile pointer: the tile, or a zeroed tile
    /// where the original would dereference NULL.
    pub(crate) fn tile_or_default(&self, t: Option<TileId>) -> GameTile {
        t.map_or_else(GameTile::default, |t| self.map.tile(t).clone())
    }

    /// `path->nodes[i]` by value. Indices outside the node block (the original reads
    /// whatever precedes/follows it) give a zeroed node.
    pub(crate) fn path_node(&self, path: &Path, i: i32) -> PathNode {
        match self.path.nodes.get(path.node_index(i)) {
            Some(n) => *n,
            None => PathNode::default(),
        }
    }

    /// `GetPositionForPathNode(&v, &path->nodes[i])`.
    pub(crate) fn node_position(&self, path: &Path, i: i32) -> SVector {
        let n = self.path_node(path, i);
        if n.world_tile.is_none() {
            return SVector::default();
        }
        get_position_for_path_node(&self.map, &n)
    }

    /// `GetPositionForPathNodeOffset2(&v, &path->nodes[i])`.
    pub(crate) fn node_position_offset2(&self, path: &Path, i: i32) -> SVector {
        let n = self.path_node(path, i);
        if n.world_tile.is_none() {
            return SVector::default();
        }
        get_position_for_path_node_offset2(&self.map, &n)
    }

    /// Tile of `path->nodes[i].worldTile`.
    pub(crate) fn node_tile(&self, path: &Path, i: i32) -> GameTile {
        self.tile_or_default(self.path_node(path, i).world_tile)
    }

    fn plt(&self, id: PlatformId) -> &Platform {
        &self.platform.platform_list.array[id.idx()]
    }

    fn plt_mut(&mut self, id: PlatformId) -> &mut Platform {
        &mut self.platform.platform_list.array[id.idx()]
    }

    fn plt_actor_mut(&mut self, id: PlatformId) -> &mut crate::actor2::Actor2 {
        let a = self.plt(id).plt_actor.expect("platform without actor");
        &mut self.actor2.actors[a]
    }

    /// Port of `GetPlatformFromUID` (Common/platform.c:84).
    pub fn get_platform_from_uid(&self, uid: i32) -> Option<PlatformId> {
        let l = &self.platform.platform_list;
        let mut c = l.head.first;
        while let Some(id) = c {
            if l.array[id.idx()].uid == uid {
                return Some(id);
            }
            c = l.array[id.idx()].links.next;
        }
        None
    }

    /// Port of `UpdatePlatforms` (Common/platform.c:105).
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_p")]
    pub fn update_platforms(&mut self) {
        if self.platform.platform_list.count == 0 {
            return;
        }
        let mut cur = self.platform.platform_list.head.first;
        while let Some(id) = cur {
            let next = self.plt(id).links.next;
            cur = next;
            self.update_platform(id);
        }
    }

    /// The loop body of `UpdatePlatforms` (`continue` = return).
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_p")]
    fn update_platform(&mut self, id: PlatformId) {
        let gs = self.game_speed;
        let act2 = self.plt(id).plt_actor;

        // check if platform is active
        if self.plt(id).active == 0 {
            // update regenerating platforms
            if self.plt(id).flags & PLATFORM_NEW_REGENERATES != 0 {
                let p = self.plt_mut(id);
                p.countdown = p.countdown.wrapping_sub(gs);
                if p.countdown < 0 {
                    p.active = 1;
                    p.countdown = -4096;
                    if let Some(a) = act2 {
                        self.actor2.actors[a].draw = 1;
                    }
                } else {
                    return;
                }
            } else {
                return;
            }
        }

        // If enemy has an actor2 and it's active and not miles away
        let Some(act2) = act2 else { return };
        if self.actor2.actors[act2].distance_from_frog >= (ACTOR_DRAWDISTANCEOUTER << 2) {
            return;
        }

        {
            let a = &self.actor2.actors[act2];
            let vis = (a.draw != 0 && a.clipped == 0) as i8;
            self.plt_mut(id).visible = vis;
        }

        // call update function for individual platform type
        if self.plt(id).update.is_some() && self.plt(id).is_waiting == 0 {
            self.call_platform_update(id);
        }

        // update frog if on the current platform
        if let Some(carrying) = self.plt(id).carrying {
            let pos = self.actor2.actors[act2].actor.position;
            self.actor2.actors[carrying].actor.position = pos;

            if self.game_state.multi != SINGLEPLAYER {
                let nf = self.frogger.num_frogs.max(0) as usize;
                let i = (0..nf).find(|&i| self.frogger.frog[i] == Some(carrying));
                if let Some(i) = i {
                    let cpos = self.actor2.actors[carrying].actor.position;
                    let mut j = self.frogger.player[i].frogunder;
                    while j != -1 {
                        let ju = j as usize;
                        if self.frogger.player[ju].is_super_hopping == 0 {
                            if let Some(f) = self.frogger.frog[ju] {
                                self.actor2.actors[f].actor.position = cpos;
                            }
                        }
                        j = self.frogger.player[ju].frogunder;
                    }
                    j = self.frogger.player[i].frogon;
                    while j != -1 {
                        let ju = j as usize;
                        if let Some(f) = self.frogger.frog[ju] {
                            self.actor2.actors[f].actor.position = cpos;
                        }
                        j = self.frogger.player[ju].frogon;
                    }
                }
            }
        }

        // check if this platform is currently 'waiting' at a node
        if self.plt(id).is_waiting != 0 {
            let p = self.plt(id);
            if (p.is_waiting != -1 && self.act_frame_count > p.path.start_frame) || (p.flags & PLATFORM_NEW_STEPONACTIVATED != 0 && p.carrying.is_some()) {
                self.plt_mut(id).is_waiting = 0;
            } else {
                return;
            }
        }

        let path = self.plt(id).path;
        let node0 = self.path_node(&path, 0);

        // check if this is a disappearing or crumbling platform
        if self.plt(id).flags & (PLATFORM_NEW_DISAPPEARWITHFROG | PLATFORM_NEW_CRUMBLES) != 0 {
            if self.plt(id).countdown < 0 {
                if self.plt(id).flags & PLATFORM_NEW_CARRYINGFROG != 0 {
                    self.plt_mut(id).countdown = node0.speed;
                }
            } else {
                // we're counting down... Wibble platform
                let mut pos = self.node_position(&path, 0);
                pos.vx = (pos.vx as i32).wrapping_add(-24 + self.rng.random(50)) as i16;
                pos.vy = (pos.vy as i32).wrapping_add(-24 + self.rng.random(50)) as i16;
                pos.vz = (pos.vz as i32).wrapping_add(-24 + self.rng.random(50)) as i16;
                self.actor2.actors[act2].actor.position = pos;

                let p = self.plt_mut(id);
                p.countdown = p.countdown.wrapping_sub(gs);

                if p.countdown <= 0 {
                    // .. timer has reached zero; collapse
                    if self.plt(id).flags & PLATFORM_NEW_CARRYINGFROG != 0 {
                        // What frog are we carrying?
                        for i in 0..self.frogger.num_frogs.max(0) as usize {
                            if self.platform.curr_platform[i] == Some(id) {
                                let t = fsqrt_f(node0.offset >> 2) >> 12;
                                let in0 = self.plt(id).in_tile[0];
                                self.frogmove.curr_tile[i] = in0;
                                self.frogmove.dest_tile[i] = in0;
                                self.platform.curr_platform[i] = None;
                                self.frogger.player[i].frog_state |= FROGSTATUS_ISJUMPINGTOTILE;
                                let p = self.plt_mut(id);
                                p.flags &= !PLATFORM_NEW_CARRYINGFROG;
                                p.carrying = None;

                                if self.frogger.player[i].frog_state & FROGSTATUS_ISDEAD == 0 {
                                    let dt = self.tile_or_default(in0);
                                    let fpos = self.frog_position(i);
                                    self.calculate_frog_jump_s(&fpos, &dt.centre, &dt.normal, 0, t.wrapping_add(1), i as i32);
                                }
                            }
                        }
                    }

                    self.actor2.actors[act2].draw = 0;
                    let p = self.plt_mut(id);
                    p.active = 0;
                    p.carrying = None;
                    p.countdown = (node0.wait_time as i32) << 12;
                    // Matt E3bodge
                    if p.countdown < -5000 {
                        p.countdown = 245760;
                    }

                    let apos = self.actor2.actors[act2].actor.position;
                    self.play_sample(node0.sample, Some(apos), 0, SAMPLE_VOLUME, -1);

                    if self.plt(id).flags & PLATFORM_NEW_CRUMBLES != 0 {
                        let mut n = FVector::default();
                        new_set_vector_fs(&mut n, &self.tile_or_default(self.plt(id).in_tile[0]).normal);
                        self.create_special_effect(FXTYPE_SMOKEBURST, &apos, &n, 204800, 8192, 4096, 4096);
                        self.create_special_effect(FXTYPE_SMOKEBURST, &apos, &n, 163840, 4096, 4096, 6144);
                    }
                }
            }
        } else {
            // platform is not carrying a frog
            let p = self.plt_mut(id);
            if p.countdown < node0.speed {
                p.countdown = p.countdown.wrapping_add(gs);
            }
        }

        // special fx associated with platform
        if self.actor2.actors[act2].effects != 0 && self.actor2.actors[act2].clipped == 0 {
            self.process_attached_effects(FxEntity::Platform(id));
        }
    }

    /// `cur->Update(cur)`.
    pub(crate) fn call_platform_update(&mut self, id: PlatformId) {
        match self.plt(id).update {
            Some(PlatformUpdate::Path) => self.update_path_platform(id),
            Some(PlatformUpdate::UpDown) => self.update_up_down_platform(id),
            Some(PlatformUpdate::StepOnActivated) => self.update_step_on_activated_platform(id),
            None => {}
        }
    }

    /// Port of `ResetPlatformFlags` (Common/platform.c:345).
    pub fn reset_platform_flags(&mut self) {
        if self.platform.platform_list.count == 0 {
            return;
        }
        let mut cur = self.platform.platform_list.head.first;
        while let Some(id) = cur {
            let p = self.plt_mut(id);
            cur = p.links.next;
            p.flags &= !PLATFORM_NEW_CARRYINGFROG;
            p.carrying = None;
        }
    }

    /// Port of `InitPlatformLinkedList` (Common/platform.c:370).
    pub fn init_platform_linked_list(&mut self) {
        let l = &mut self.platform.platform_list;
        l.array = Vec::new();
        l.count = 0;
        l.head = Head::default();
    }

    /// Port of `AddPlatform` (Common/platform.c:385): a new platform goes before the
    /// first listed platform with the same update function, else at the head.
    pub fn add_platform(&mut self, plat: PlatformId) {
        if self.plt(plat).linked {
            return;
        }
        let upd = self.plt(plat).update;
        let l = &mut self.platform.platform_list;
        let mut found = None;
        let mut c = l.head.first;
        while let Some(cur) = c {
            if l.array[cur.idx()].update == upd {
                found = Some(cur);
                break;
            }
            c = l.array[cur.idx()].links.next;
        }
        match found {
            Some(cur) => l.head.insert_before(&mut l.array, cur, plat),
            None => l.head.push_front(&mut l.array, plat),
        }
        l.array[plat.idx()].linked = true;
        l.count += 1;
    }

    /// Port of `SubPlatform` (Common/platform.c:425).
    pub fn sub_platform(&mut self, plat: PlatformId) {
        if !self.plt(plat).linked {
            return;
        }
        if let Some(a) = self.plt(plat).plt_actor {
            self.actor2_free(a);
        }
        let l = &mut self.platform.platform_list;
        l.head.remove(&mut l.array, plat);
        l.array[plat.idx()].linked = false;
        l.array[plat.idx()].links.next = None;
        l.count -= 1;
    }

    /// Port of `FreePlatformLinkedList` (Common/platform.c:452).
    pub fn free_platform_linked_list(&mut self) {
        let count = self.platform.platform_list.count;
        if count != 0 {
            self.effects.push(Effect::Log { text: format!("Freeing linked list : PLATFORM : ({count} elements)") });
        }
        while let Some(first) = self.platform.platform_list.head.first {
            self.sub_platform(first);
        }
        self.free_platform_block();
        self.init_platform_linked_list();
    }

    /// Port of `AllocPlatformBlock` (Common/platform.c:474).
    pub fn alloc_platform_block(&mut self, num: i32) {
        let l = &mut self.platform.platform_list;
        if l.array.is_empty() {
            l.array = vec![Platform::default(); num.max(0) as usize];
        }
        l.count = 0;
        l.blocks = num as i16;
    }

    /// Port of `FreePlatformBlock` (Common/platform.c:489).
    pub fn free_platform_block(&mut self) {
        let l = &mut self.platform.platform_list;
        l.array = Vec::new();
        l.count = 0;
        l.blocks = 0;
    }

    /// Port of `CreateAndAddPlatform` (Common/platform.c:500).
    pub fn create_and_add_platform(&mut self, actor_name: &str, flags: i32, id: i32, path: Path, anim_speed: Fixed, facing: u8) -> Option<PlatformId> {
        let mut init_flags = crate::mdx_actor::INIT_ANIMATION;
        let mut shadow_radius = 0;
        if flags as u32 & PLATFORM_NEW_SHADOW != 0 {
            init_flags |= crate::mdx_actor::INIT_SHADOW;
            shadow_radius = 819200;
        }

        // create and add platform actor
        let actor = self.create_and_add_actor(actor_name, 0, 0, 0, init_flags)?;

        let l = &self.platform.platform_list;
        if l.count >= l.blocks || l.count as usize >= l.array.len() {
            return None;
        }
        let pid = PlatformId(l.count as u32);

        {
            let p = self.plt_mut(pid);
            p.plt_actor = Some(actor);
            p.flags = flags as u32;
            p.facing = facing;
        }
        self.actor2.actors[actor].anim_speed = anim_speed;
        self.actor_animate(actor, 0, true, false, anim_speed, false);

        // set shadow radius (if applicable)
        if shadow_radius != 0 {
            if let Some(s) = self.actor2.actors[actor].actor.shadow.as_mut() {
                s.radius = shadow_radius;
            }
        }

        {
            let p = self.plt_mut(pid);
            p.plt_ptrs = [None; 4];
            p.active = 1;
            p.path = Path::default();
            p.in_tile = [None; 2];
            p.uid = id;
            p.curr_speed = 4096;
            p.carrying = None;
        }

        self.assign_path_to_platform(pid, path, 0);

        {
            let p = self.plt_mut(pid);
            if p.flags & PLATFORM_NEW_STEPONACTIVATED != 0 {
                if p.flags & PLATFORM_NEW_MOVEUP != 0 {
                    p.flags &= !PLATFORM_NEW_MOVEUP;
                    p.flags |= PLATFORM_NEW_RISEWITHFROG;
                } else if p.flags & PLATFORM_NEW_MOVEDOWN != 0 {
                    p.flags &= !PLATFORM_NEW_MOVEDOWN;
                    p.flags |= PLATFORM_NEW_SINKWITHFROG;
                }
            }
        }

        // determine relevant platform update function
        let flags = self.plt(pid).flags;
        let path = self.plt(pid).path;
        let t0 = self.node_tile(&path, 0);
        let facing = self.plt(pid).facing as usize;
        let orient = |g: &mut Game| {
            let q = &mut g.actor2.actors[actor].actor.q_rot;
            crate::actor2::orientate_ss(q, &t0.dir_vector[facing], &t0.normal);
        };
        if flags & PLATFORM_NEW_FOLLOWPATH != 0 {
            self.plt_mut(pid).update = Some(PlatformUpdate::Path);
            orient(self);
            let q = self.actor2.actors[actor].actor.q_rot;
            self.plt_mut(pid).dest_orientation = q;
        } else if flags & (PLATFORM_NEW_MOVEUP | PLATFORM_NEW_MOVEDOWN) != 0 {
            self.plt_mut(pid).update = Some(PlatformUpdate::UpDown);
            orient(self);
        } else if flags & PLATFORM_NEW_STEPONACTIVATED != 0 {
            self.plt_mut(pid).update = Some(PlatformUpdate::StepOnActivated);
            orient(self);
        } else {
            // (flags & NONMOVING) || (actor && path && path->nodes): always true here.
            orient(self);
        }

        self.add_platform(pid);
        Some(pid)
    }

    /// Port of `AssignPathToPlatform` (Common/platform.c:589). `PATH_MAKENODETILEPTRS`
    /// is not supported (node tiles are already resolved in the port).
    pub fn assign_path_to_platform(&mut self, pid: PlatformId, path: Path, _path_flags: u32) {
        let mut path = path;
        // set the start position for the platform
        path.from_node = path.start_node;
        let mut flags = self.plt(pid).flags;

        if flags & PLATFORM_NEW_FORWARDS != 0 {
            // this platform moves forward thru path nodes
            flags |= PLATFORM_NEW_FOLLOWPATH;
            path.to_node = path.from_node.wrapping_add(1);
            if path.to_node > path.last_node() {
                path.to_node = 0;
            }
            if flags & PLATFORM_NEW_FACEFORWARDS != 0 && crate::enemies::check_path_orientation_safe(self, &path) != 0 {
                flags |= PLATFORM_NEW_NOREORIENTATE;
            }
        } else if flags & PLATFORM_NEW_BACKWARDS != 0 {
            // this platform moves backward thru path nodes
            flags |= PLATFORM_NEW_FOLLOWPATH;
            path.to_node = path.from_node.wrapping_sub(1);
            if path.to_node < 0 {
                // BUG(original): sets fromNode instead of toNode, leaving toNode == -1
                // (reads the node before this path's block until the first node update).
                path.from_node = path.last_node();
            }
            if flags & PLATFORM_NEW_FACEFORWARDS != 0 && crate::enemies::check_path_orientation_safe(self, &path) != 0 {
                flags |= PLATFORM_NEW_NOREORIENTATE;
            }
        } else {
            // moves up or down, or does not move
            path.from_node = 0;
            path.to_node = 0;
        }
        self.plt_mut(pid).flags = flags;
        self.plt_mut(pid).path = path;

        // set platform position to relevant point on path
        let start = if flags & PLATFORM_NEW_MOVEDOWN != 0 {
            self.node_position_offset2(&path, path.from_node as i32)
        } else {
            self.node_position(&path, path.from_node as i32)
        };
        let actor = self.plt(pid).plt_actor.expect("platform actor");
        self.actor2.actors[actor].actor.position = start;

        let to_t = self.node_tile(&path, path.to_node as i32);
        let from_t = self.node_tile(&path, path.from_node as i32);
        let mut fwd = FVector::default();
        sub_vector_fss(&mut fwd, &to_t.centre, &from_t.centre);
        make_unit(&mut fwd);

        let facing = self.plt(pid).facing as usize;
        let t0 = self.node_tile(&path, 0);
        let q = &mut self.actor2.actors[actor].actor.q_rot;
        if flags & PLATFORM_NEW_FACEFORWARDS == 0 {
            crate::actor2::orientate_fs(q, &fwd, &from_t.normal);
        } else {
            crate::actor2::orientate_ss(q, &t0.dir_vector[facing], &t0.normal);
        }

        // set platform current 'in' tile and speeds and pause times
        let from = self.path_node(&path, path.from_node as i32);
        let afc = self.act_frame_count;
        let p = self.plt_mut(pid);
        p.in_tile[0] = from.world_tile;
        p.curr_speed = from.speed;
        p.is_waiting = from.wait_time;
        p.path.start_frame = afc;
        p.path.end_frame = afc.wrapping_add((p.curr_speed.wrapping_mul(60) >> 12) as u32);
        p.countdown = -1 << 12;

        self.calc_platform_normal_interps(pid);
    }

    /// Port of `UpdatePlatformPathNodes` (Common/platform.c:675).
    pub fn update_platform_path_nodes(&mut self, pid: PlatformId) {
        {
            let p = self.plt_mut(pid);
            p.path.from_node = p.path.to_node;
        }
        self.calc_next_platform_dest(pid);

        let path = self.plt(pid).path;
        let from = self.path_node(&path, path.from_node as i32);
        {
            let p = self.plt_mut(pid);
            p.curr_speed = from.speed;
            p.is_waiting = from.wait_time;
        }

        // Stop overshoot when waiting on a path node
        if self.plt(pid).is_waiting != 0 {
            let pos = if self.plt(pid).flags & PLATFORM_NEW_MOVEDOWN != 0 {
                self.node_position_offset2(&path, path.from_node as i32)
            } else {
                self.node_position(&path, path.from_node as i32)
            };
            self.plt_actor_mut(pid).actor.position = pos;
        }

        self.calc_platform_normal_interps(pid);
    }

    /// Port of `CalcPlatformNormalInterps` (Common/platform.c:713).
    pub fn calc_platform_normal_interps(&mut self, pid: PlatformId) {
        let path = self.plt(pid).path;
        if path.num_nodes < 2 {
            return;
        }
        let from_t = self.node_tile(&path, path.from_node as i32);
        let to_t = self.node_tile(&path, path.to_node as i32);
        let p = self.plt(pid);
        if p.visible != 0 && p.flags & PLATFORM_NEW_NOREORIENTATE == 0 {
            let q = self.actor2.actors[p.plt_actor.expect("platform actor")].actor.q_rot;
            let flags = p.flags;
            let facing = p.facing as usize;
            let t0 = self.node_tile(&path, 0);
            let p = self.plt_mut(pid);
            p.src_orientation = q;
            if flags & PLATFORM_NEW_FACEFORWARDS == 0 {
                let mut fwd = FVector::default();
                sub_vector_fss(&mut fwd, &to_t.centre, &from_t.centre);
                make_unit(&mut fwd);
                crate::actor2::orientate_fs(&mut p.dest_orientation, &fwd, &to_t.normal);
            } else {
                crate::actor2::orientate_ss(&mut p.dest_orientation, &t0.dir_vector[facing], &to_t.normal);
            }
        }
    }

    /// Port of `CalcNextPlatformDest` (Common/platform.c:752).
    pub fn calc_next_platform_dest(&mut self, pid: PlatformId) {
        let flags = self.plt(pid).flags;
        let mut path = self.plt(pid).path;
        let mut new_flags = flags;
        let mut sample = 0;

        if flags & PLATFORM_NEW_FORWARDS != 0 {
            path.to_node = path.from_node.wrapping_add(1);
            if path.to_node > path.last_node() {
                if flags & PLATFORM_NEW_PINGPONG != 0 {
                    new_flags ^= PLATFORM_NEW_FORWARDS | PLATFORM_NEW_BACKWARDS;
                    path.from_node = path.last_node();
                    path.to_node = path.last_node() - 1;
                } else if flags & PLATFORM_NEW_CYCLE != 0 {
                    // platform has cyclic movement
                    path.from_node = path.last_node();
                    path.to_node = 0;
                } else {
                    path.from_node = 0;
                    path.to_node = 1;
                }
            }
            sample = self.path_node(&path, path.from_node as i32).sample;
        } else if flags & PLATFORM_NEW_BACKWARDS != 0 {
            path.to_node = path.from_node.wrapping_sub(1);
            if path.to_node < 0 {
                if flags & PLATFORM_NEW_PINGPONG != 0 {
                    new_flags ^= PLATFORM_NEW_FORWARDS | PLATFORM_NEW_BACKWARDS;
                    path.from_node = 0;
                    path.to_node = 1;
                } else if flags & PLATFORM_NEW_CYCLE != 0 {
                    path.from_node = 0;
                    path.to_node = path.last_node();
                } else {
                    path.from_node = path.last_node();
                    path.to_node = path.last_node() - 1;
                }
            }
            sample = self.path_node(&path, path.from_node as i32).sample;
        } else if flags & (PLATFORM_NEW_MOVEUP | PLATFORM_NEW_MOVEDOWN) != 0 && flags & PLATFORM_NEW_PINGPONG != 0 {
            new_flags ^= PLATFORM_NEW_MOVEUP | PLATFORM_NEW_MOVEDOWN; // invert flags
            sample = self.path_node(&path, 0).sample;
        }
        let p = self.plt_mut(pid);
        p.flags = new_flags;
        p.path = path;
        if sample != 0 {
            let pos = self.plt_actor_mut(pid).actor.position;
            self.play_sample(sample, Some(pos), 0, SAMPLE_VOLUME, -1);
        }
    }

    /// Port of `FrogLeavePlatform` (Common/platform.c:817).
    pub fn frog_leave_platform(&mut self, pl: i32) {
        if let Some(plt) = self.platform.curr_platform[pl as usize] {
            let p = self.plt_mut(plt);
            p.flags &= !PLATFORM_NEW_CARRYINGFROG; // plat is NOT carrying a frog
            p.carrying = None;
            self.platform.curr_platform[pl as usize] = None;
        }
    }

    /// Port of `UpdatePathPlatform` (Common/platform.c:840).
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_p")]
    pub fn update_path_platform(&mut self, pid: PlatformId) {
        let afc = self.act_frame_count;
        let ws = self.frogger.wait_scale;
        // check if this platform has arrived at a path node
        if afc >= self.plt(pid).path.end_frame {
            let mut guard = 0;
            loop {
                self.update_platform_path_nodes(pid);
                let p = self.plt_mut(pid);
                p.path.start_frame = p.path.end_frame.wrapping_add(((p.is_waiting as i32).wrapping_mul(ws) >> 12) as u32);
                p.path.end_frame = p.path.start_frame.wrapping_add((60i32.wrapping_mul(p.curr_speed) >> 12) as u32);
                guard += 1;
                if !(afc >= p.path.end_frame && p.is_waiting >= 0) || guard >= CATCHUP_GUARD {
                    break;
                }
            }
            if self.plt(pid).is_waiting != 0 {
                return;
            }
        } else {
            let path = self.plt(pid).path;
            if afc > path.start_frame.wrapping_add(path.end_frame) / 2 {
                let t = self.path_node(&path, path.to_node as i32).world_tile;
                self.plt_mut(pid).in_tile[0] = t;
            }
        }

        // update the platform position
        let path = self.plt(pid).path;
        let to_position = self.node_position(&path, path.to_node as i32);
        let from_position = self.node_position(&path, path.from_node as i32);

        let length = udiv(afc.wrapping_sub(path.start_frame) << 12, path.end_frame.wrapping_sub(path.start_frame));

        let p = self.plt(pid);
        let (visible, flags, src, dest) = (p.visible, p.flags, p.src_orientation, p.dest_orientation);
        let actor = p.plt_actor.expect("platform actor");
        if visible != 0 && flags & PLATFORM_NEW_NOREORIENTATE == 0 {
            if flags & PLATFORM_NEW_SLERPPATH == 0 {
                self.actor2.actors[actor].actor.q_rot = dest;
            } else {
                // mainly for space2 platforms
                iquat_slerp(&src, &dest, length, &mut self.actor2.actors[actor].actor.q_rot);
            }
        }

        let mut fwd = FVector::default();
        sub_vector_fss(&mut fwd, &to_position, &from_position);
        scale_vector_ff(&mut fwd, length);
        add_vector_sfs(&mut self.actor2.actors[actor].actor.position, &fwd, &from_position);
    }

    /// Port of `UpdateUpDownPlatform` (Common/platform.c:898).
    pub fn update_up_down_platform(&mut self, pid: PlatformId) {
        let afc = self.act_frame_count;
        let ws = self.frogger.wait_scale;
        // check if this platform has arrived at a path node
        if afc > self.plt(pid).path.end_frame {
            let mut guard = 0;
            loop {
                self.update_platform_path_nodes(pid);
                let p = self.plt_mut(pid);
                p.path.start_frame = p.path.end_frame.wrapping_add(((p.is_waiting as i32).wrapping_mul(ws) >> 12) as u32);
                p.path.end_frame = p.path.start_frame.wrapping_add((p.curr_speed.wrapping_mul(60) >> 12) as u32);
                guard += 1;
                if afc <= p.path.end_frame || guard >= CATCHUP_GUARD {
                    break;
                }
            }

            for i in 0..self.frogger.num_frogs.max(0) as usize {
                if Some(pid) == self.platform.curr_platform[i] {
                    let t = self.frogmove.curr_tile[i];
                    self.check_tile_for_collectable(t, i as i32);
                }
            }

            if self.plt(pid).is_waiting != 0 {
                return;
            }
        }

        let p = self.plt(pid);
        let path = p.path;
        let node0 = self.path_node(&path, 0);
        let (start_offset, end_offset) = if p.flags & PLATFORM_NEW_MOVEUP != 0 { (node0.offset, node0.offset2) } else { (node0.offset2, node0.offset) };

        // get up vector for this platform
        let t0 = self.node_tile(&path, 0);
        let mut move_vec = FVector::default();
        new_set_vector_fs(&mut move_vec, &t0.normal);

        let t = udiv(((afc.wrapping_sub(path.start_frame) as i32) << 12) as u32, path.end_frame.wrapping_sub(path.start_frame));

        scale_vector_ff(&mut move_vec, fmul(t, end_offset).wrapping_add(fmul((1 << 12) - t, start_offset)));
        let actor = p.plt_actor.expect("platform actor");
        add_vector_ssf(&mut self.actor2.actors[actor].actor.position, &t0.centre, &move_vec);
    }

    /// Port of `UpdateStepOnActivatedPlatform` (Common/platform.c:953).
    pub fn update_step_on_activated_platform(&mut self, pid: PlatformId) {
        let p = self.plt(pid);
        let path = p.path;
        let node0 = self.path_node(&path, 0);
        let start_offs = node0.offset;
        let dest_offs = node0.offset2;
        let t0 = self.node_tile(&path, 0);
        let actor = p.plt_actor.expect("platform actor");
        let carrying = p.flags & PLATFORM_NEW_CARRYINGFROG != 0;

        // get up vector for this platform
        let mut move_vec = FVector::default();
        new_set_vector_fs(&mut move_vec, &t0.normal);

        let mut v = FVector::default();
        sub_vector_fss(&mut v, &self.actor2.actors[actor].actor.position, &t0.centre);
        let offs = dot_product_ff(&v, &move_vec);

        let speed = fmul(p.curr_speed.wrapping_mul(SCALE), self.game_speed);
        scale_vector_ff(&mut move_vec, speed);

        // platform only moves when frog is it - otherwsie returns to start position
        let target = if carrying { dest_offs } else { start_offs };
        let pos = &mut self.actor2.actors[actor].actor.position;
        if offs > target.wrapping_add(speed) {
            sub_from_vector_sf(pos, &move_vec);
        } else if offs < target.wrapping_sub(speed) {
            add_to_vector_sf(pos, &move_vec);
        }
    }

    /// Port of `EnumPlatforms` (Common/platform.c:1004): calls `func(platform, param)`
    /// for every platform with `uid == id` (all when `id == 0`) until it returns 0.
    pub fn enum_platforms(&mut self, id: i32, mut func: impl FnMut(&mut Game, PlatformId, i32) -> i32, param: i32) -> i32 {
        let mut cur = self.platform.platform_list.head.first;
        let mut count = 0;
        let mut down = self.platform.platform_list.count as i32;
        while down != 0 {
            let Some(c) = cur else { break };
            let next = self.plt(c).links.next;
            if (id == 0 || self.plt(c).uid == id) && func(self, c, param) == 0 {
                break;
            }
            cur = next;
            count += 1;
            down -= 1;
        }
        if count == 0 {
            self.effects.push(Effect::Log { text: format!("EnumPlatforms({id}) found 0 enemies?") });
        }
        count
    }

    /// Port of `SetPlatformVisible` (Common/platform.c:1027).
    pub fn set_platform_visible(&mut self, plt: PlatformId, visible: bool) {
        if visible {
            self.plt_mut(plt).active = 1;
            self.plt_actor_mut(plt).draw = 1;
            self.plt_mut(plt).countdown = -1;
            if self.plt(plt).is_waiting != -1 {
                self.plt_mut(plt).is_waiting = -1;
                self.set_platform_moving(plt, true);
            }
            self.move_platform_to_start_of_list(plt);
        } else {
            self.plt_actor_mut(plt).draw = 0;
            self.plt_mut(plt).active = 0;
            self.move_platform_to_end_of_list(plt);
        }
    }

    /// Port of `SetPlatformMoving` (Common/platform.c:1051).
    pub fn set_platform_moving(&mut self, plt: PlatformId, moving: bool) {
        if moving {
            if self.plt(plt).is_waiting != 0 {
                let afc = self.act_frame_count;
                let p = self.plt_mut(plt);
                p.is_waiting = 0;
                p.path.start_frame = afc;
                p.path.end_frame = afc.wrapping_add((60i32.wrapping_mul(p.curr_speed) >> 12) as u32);
            }
            self.call_platform_update(plt);
        } else {
            self.plt_mut(plt).is_waiting = -1;
        }
    }

    /// Port of `MovePlatformToEndOfList` (Common/platform.c:1071).
    pub fn move_platform_to_end_of_list(&mut self, plt: PlatformId) {
        let l = &mut self.platform.platform_list;
        l.head.remove(&mut l.array, plt);
        l.head.push_back(&mut l.array, plt);
    }

    /// Port of `MovePlatformToStartOfList` (Common/platform.c:1081).
    pub fn move_platform_to_start_of_list(&mut self, plt: PlatformId) {
        let l = &mut self.platform.platform_list;
        l.head.remove(&mut l.array, plt);
        l.head.push_front(&mut l.array, plt);
    }

    /// Port of `MovePlatformToNode` (Common/platform.c:1096). Returns 1.
    pub fn move_platform_to_node(&mut self, plt: PlatformId, flag: i32) -> i32 {
        let path = self.plt(plt).path;
        if flag >= 0 && flag < path.num_nodes as i32 {
            let t = self.path_node(&path, flag).world_tile;
            let afc = self.act_frame_count;
            {
                let p = self.plt_mut(plt);
                p.path.to_node = flag as i16;
                p.in_tile[0] = t;
                p.path.end_frame = afc;
            }
            self.call_platform_update(plt);

            let p = self.plt(plt).clone();
            if p.flags & PLATFORM_NEW_NOREORIENTATE == 0 {
                let in0 = self.tile_or_default(p.in_tile[0]);
                let mut dest = p.dest_orientation;
                if p.flags & PLATFORM_NEW_FACEFORWARDS == 0 {
                    let nt = self.node_tile(&p.path, (flag + 1) % p.path.num_nodes as i32);
                    let mut fwd = FVector::default();
                    sub_vector_fss(&mut fwd, &nt.centre, &in0.centre);
                    make_unit(&mut fwd);
                    crate::actor2::orientate_fs(&mut dest, &fwd, &in0.normal);
                } else {
                    let t0 = self.node_tile(&p.path, 0);
                    crate::actor2::orientate_ss(&mut dest, &t0.dir_vector[p.facing as usize], &in0.normal);
                }
                let pm = self.plt_mut(plt);
                pm.dest_orientation = dest;
                pm.src_orientation = dest;
                self.plt_actor_mut(plt).actor.q_rot = dest;
            }
        } else {
            self.effects.push(Effect::Log { text: format!("MoveEnemyToNode(): Flag ({flag}) out of range") });
        }
        1
    }
}
