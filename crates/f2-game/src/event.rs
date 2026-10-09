//! Port of `Common/event.c` / `event.h`: the trigger/event lists.
//!
//! A `TRIGGER` has a condition function (checked every frame by `UpdateEvents`) and a
//! single attached `EVENT` whose action runs when the condition holds. The C function
//! pointers + `void *data[4]` become the typed enums [`TriggerFunc`] and [`EventFunc`]
//! (the functions themselves live in [`crate::evenfunc`] and [`crate::script`]).
//!
//! Storage mirrors the original allocator: a base array of `TRIGGER_NUMINITIAL`
//! triggers plus `TRIGGER_BLOCKSIZE` blocks allocated on demand, a LIFO free list
//! (`avail`) and the active list (`head`), into which `AttachEvent` inserts at the
//! front. Triggers that never get an event (the operands of AND/OR/NOT, the trigger
//! of `EV_IF`) are allocated but not linked, exactly like the C.

use std::sync::Arc;

use f2_formats::fev::Script;
use f2_math::{Fixed, SVector, Vector};

use crate::anim::DeathAnim;
use crate::ids::{ActorId, EnemyId, PlatformId, TileId, TriggerId};
use crate::list::{HasLinks, Head, Links};
use crate::Game;

// TRIGGER flags (`event.h:17`).
pub const TRIGGER_ONCE: u16 = 1 << 0;
pub const TRIGGER_DELAY: u16 = 1 << 1;
pub const TRIGGER_ALWAYS: u16 = 1 << 2;
pub const TRIGGER_FIRED: u16 = 1 << 3;
pub const TRIGGER_RISING: u16 = 1 << 4;
/// Argument 1 is a trigger and must be freed!
pub const TRIGGER_LOGIC: u16 = 1 << 8;
/// Argument 2 is a trigger and must be freed!
pub const TRIGGER_LOGIC2: u16 = 1 << 9;
/// This trigger is on the free list.
pub const TRIGGER_FREE: u16 = 1 << 15;

pub const MAX_EVENTDATA: usize = 4;
pub const TRIGGER_BLOCKSIZE: i16 = 16;
pub const TRIGGER_NUMINITIAL: i16 = 64;

/// The owner of a `PATH*` passed to a trigger (`PathAtFlag`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathOwner {
    Enemy(EnemyId),
    Platform(PlatformId),
}

/// An `int*` / `fixed*` passed to `BitCheck`, `AssignIntToInt` or
/// `AssignFloatToFloat`. No PC code creates these triggers/events; the variables
/// below are the script-visible ones, extend as needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntVar {
    /// `scriptCounters[n]` (stored as `short`).
    ScriptCounter(u8),
    /// `cam_shakiness`.
    CamShakiness,
    /// `scaleV`.
    ScaleV,
}

/// Sample of a `PlaySFX` event (`SAMPLE*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SfxSample {
    /// `FindSample(crc)`: uid = CRC of the sample name (0 = NULL).
    Sample(u32),
    /// `FindVoice(crc, pl)`: a voice of player `pl`'s character.
    Voice { player: u8, uid: u32 },
}

/// The `SVECTOR*` position of a `PlaySFX` event; read when the event fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SfxPoint {
    /// `&actor->actor->position`.
    ActorPosition(ActorId),
    /// A fixed point.
    Point(SVector),
}

/// A trigger condition function plus its `data[]` (`int (*func)(TRIGGER*)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerFunc {
    /// No function (only on free/never-made triggers; never true).
    None,
    /// `EnemyOnTile(nme, tile)`; the tile is `&firstTile[n]` (kept as the raw number).
    EnemyOnTile { nme: EnemyId, tile: i32 },
    /// `FrogOnTile(frog, tile)`.
    FrogOnTile { frog: i32, tile: i32 },
    /// `FrogOnPlatform(pl, uid)`.
    FrogOnPlatform { pl: i32, id: i32 },
    /// `ActorWithinRadius(actor, &pos, &radius)`.
    ActorWithinRadius { actor: ActorId, pos: Vector, radius: Fixed },
    /// `OnTimeout(time, interval)`: `data[0]` is advanced by `data[1]` when it fires.
    OnTimeout { time: u32, interval: u32 },
    /// `OnRandomTimeout(time, start, end)`.
    OnRandomTimeout { time: u32, start: u32, end: u32 },
    /// `OnTrigger(t)`: whether `t` fired.
    OnTrigger(TriggerId),
    /// `LogicalAND(numT, t...)`.
    LogicalAnd(Vec<TriggerId>),
    /// `LogicalOR(numT, t...)`.
    LogicalOr(Vec<TriggerId>),
    /// `EnemyAtFlag(uid, flag)`.
    EnemyAtFlag { id: i32, flag: i32 },
    /// `PlatformAtFlag(uid, flag)`.
    PlatformAtFlag { id: i32, flag: i32 },
    /// `PathAtFlag(path, &flag)`.
    PathAtFlag { path: PathOwner, flag: i32 },
    /// `BitCheck(&var, mask)`.
    BitCheck { var: IntVar, mask: i32 },
    /// `FrogIsDead(pl)`.
    FrogIsDead(i32),
    /// `LevelIsOpen(world, level)`.
    LevelIsOpen { world: i32, level: i32 },
    /// `ANDtrigger(a, b)` (script.c).
    And(TriggerId, TriggerId),
    /// `ORtrigger(a, b)` (script.c).
    Or(TriggerId, TriggerId),
    /// `NOTtrigger(a)` (script.c).
    Not(TriggerId),
    /// `AlwaysTrigger` (script.c).
    Always,
    /// `OnFlagSet(flag)` (script.c).
    OnFlagSet(i32),
    /// `OnCounterEquals(counter, value)` (script.c).
    OnCounterEquals { counter: i32, value: i32 },
    /// `FrogOnPath(pl, enemy->path)` (script.c); the path is read from the enemy.
    FrogOnPath { pl: i32, nme: EnemyId },
}

/// A reference into the level script (`UBYTE*` into `scriptBuffer`): the block of
/// the `EV_ON` reached by following `path` (command indices) from the outer block.
#[derive(Clone, Debug)]
pub struct ScriptBlock {
    pub script: Arc<Script>,
    pub path: Arc<[u32]>,
}

/// An event action function plus its `data[]` (`void (*func)(EVENT*)`).
#[derive(Clone, Debug, Default)]
pub enum EventFunc {
    /// `func == NULL`.
    #[default]
    None,
    /// `InterpretEvent(block)` (script.c).
    InterpretEvent(ScriptBlock),
    /// `ChangeActorScale` (body commented out in the original).
    ChangeActorScale,
    /// `AssignFloatToFloat(&var, val)`.
    AssignFloatToFloat { var: IntVar, val: Fixed },
    /// `AssignIntToInt(&var, val)`.
    AssignIntToInt { var: IntVar, val: i32 },
    /// `TogglePlatformMove(plt)`.
    TogglePlatformMove(PlatformId),
    /// `ToggleEnemyMove(nme)`.
    ToggleEnemyMove(EnemyId),
    /// `ToggleTileLink(tile)`.
    ToggleTileLink(TileId),
    /// `PlaySFX(sample, point)`.
    PlaySfx { sample: SfxSample, point: Option<SfxPoint> },
    /// `EvAnimateActor(actor, &anim, &flags, &speed)`.
    EvAnimateActor { actor: ActorId, anim: i32, flags: i32, speed: i32 },
    /// `ChangeLevel(&world, &level)`.
    ChangeLevel { world: i32, level: i32 },
    /// `TeleportFrog(frog, tile)`.
    TeleportFrog { frog: i32, tile: TileId },
    /// `DeathAnim(pl, func)`.
    DeathAnim { pl: i32, func: DeathAnim },
}

/// `TRIGGER` (`event.h:42`).
#[derive(Clone, Debug)]
pub struct Trigger {
    pub links: Links<TriggerId>,
    /// `next != NULL`: the trigger is in the active list.
    pub linked: bool,
    pub func: TriggerFunc,
    pub event: EventFunc,
    pub flags: u16,
    pub count: u32,
    pub delay: u32,
}

impl Default for Trigger {
    fn default() -> Self {
        Trigger { links: Links::default(), linked: false, func: TriggerFunc::None, event: EventFunc::None, flags: 0, count: 0, delay: 0 }
    }
}

impl HasLinks<TriggerId> for Trigger {
    fn links(&self) -> &Links<TriggerId> {
        &self.links
    }
    fn links_mut(&mut self) -> &mut Links<TriggerId> {
        &mut self.links
    }
}

/// `TRIGGERLIST` (`event.h:79`). `triggers` holds the base array followed by every
/// extra block in allocation order; `avail` is the free list (last = `avail.next`).
#[derive(Clone, Debug, Default)]
pub struct TriggerList {
    pub triggers: Vec<Trigger>,
    pub head: Head<TriggerId>,
    pub avail: Vec<TriggerId>,
    /// Current total number of active triggers.
    pub count: i16,
    /// Maximum allocated triggers.
    pub alloced: i16,
    /// Number of extra TRIGGERBLOCKs.
    pub blocks: i16,
    /// Which array we're garbage collecting for this frame.
    pub garbage: i16,
}

/// Globals of `event.c`.
#[derive(Clone, Debug, Default)]
pub struct EventState {
    /// `triggerList`.
    pub trigger_list: TriggerList,
    /// `triggersReset`.
    pub triggers_reset: bool,
}

impl TriggerList {
    pub fn get(&self, t: TriggerId) -> &Trigger {
        &self.triggers[t.idx()]
    }

    pub fn get_mut(&mut self, t: TriggerId) -> &mut Trigger {
        &mut self.triggers[t.idx()]
    }

    /// Active triggers in list order.
    pub fn active(&self) -> Vec<TriggerId> {
        self.head.ids(&self.triggers)
    }

    /// Port of `InitTriggerList` (Common/event.c:96).
    pub fn init_trigger_list(&mut self) {
        self.head = Head::default();
        self.avail.clear();
        self.count = 0;
        self.alloced = TRIGGER_NUMINITIAL;
        self.blocks = 0;
        self.garbage = 0;
        self.triggers = (0..TRIGGER_NUMINITIAL).map(|_| Trigger::default()).collect();
        // Add all initial triggers to free list
        for i in 0..TRIGGER_NUMINITIAL as usize {
            self.avail.push(TriggerId(i as u32));
            self.triggers[i].flags |= TRIGGER_FREE;
        }
    }

    /// Port of `FreeTriggerList` (Common/event.c:130).
    pub fn free_trigger_list(&mut self) -> bool {
        // First, remove all active triggers
        self.kill_all_triggers();
        *self = TriggerList::default();
        true
    }

    /// Port of `KillAllTriggers` (Common/event.c:163). Returns `triggersReset = 1`.
    pub fn kill_all_triggers(&mut self) -> bool {
        while let Some(t) = self.head.first {
            self.deallocate_trigger(Some(t));
        }
        true
    }

    /// Port of `AllocateTrigger` (Common/event.c:180).
    pub fn allocate_trigger(&mut self) -> TriggerId {
        // If we need more triggers then allocate a block
        if self.count >= self.alloced {
            self.alloc_trigger_block();
        }
        // Get the next one off the free list
        let t = self.avail.pop().expect("trigger free list empty");
        // Detach from all lists - will get added to active list if or when an event is
        // attached to it. If no event then it is a logic trigger (AND,OR,etc) and will
        // get freed separately.
        let tr = self.get_mut(t);
        tr.links = Links::default();
        tr.linked = false;
        tr.flags &= !TRIGGER_FREE;
        self.count += 1;
        t
    }

    /// Port of `DeallocateTrigger` (Common/event.c:214).
    pub fn deallocate_trigger(&mut self, t: Option<TriggerId>) {
        let Some(t) = t else { return };
        if self.get(t).flags & TRIGGER_FREE != 0 {
            return;
        }
        // Remove from active list
        if self.get(t).linked {
            self.head.remove(&mut self.triggers, t);
            self.get_mut(t).linked = false;
        }
        // Add to free list
        self.avail.push(t);
        self.get_mut(t).flags |= TRIGGER_FREE;
        self.count -= 1;

        // Do we need to kill extra logic bits?
        let (flags, func) = (self.get(t).flags, self.get(t).func.clone());
        if flags & TRIGGER_LOGIC != 0 {
            self.deallocate_trigger(logic_arg(&func, 0));
        }
        if flags & TRIGGER_LOGIC2 != 0 {
            self.deallocate_trigger(logic_arg(&func, 1));
        }
    }

    /// Port of `AllocTriggerBlock` (Common/event.c:250).
    pub fn alloc_trigger_block(&mut self) {
        self.blocks += 1;
        self.alloced += TRIGGER_BLOCKSIZE;
        // Add all the new elements to the free list
        for _ in 0..TRIGGER_BLOCKSIZE {
            let id = TriggerId(self.triggers.len() as u32);
            self.triggers.push(Trigger { flags: TRIGGER_FREE, ..Default::default() });
            self.avail.push(id);
        }
    }

    /// Port of `TriggerGarbageCollect` (Common/event.c:309): empty (`// TODO`) in the
    /// original, so `DeallocTriggerBlock` is never called.
    pub fn trigger_garbage_collect(&mut self) {}

    /// Port of `MakeTrigger` (Common/event.c:36).
    pub fn make_trigger(&mut self, func: TriggerFunc) -> TriggerId {
        let t = self.allocate_trigger();
        let tr = self.get_mut(t);
        tr.func = func;
        tr.flags = 0;
        tr.delay = 0;
        tr.count = 0;
        t
    }

    /// Port of `AttachEvent` (Common/event.c:62): sets the trigger's event and adds it
    /// to the front of the active list.
    pub fn attach_event(&mut self, trigger: TriggerId, flags: u16, time: u32, func: EventFunc) {
        let tr = self.get_mut(trigger);
        // Set trigger vars
        tr.flags |= flags;
        if flags & TRIGGER_DELAY != 0 {
            tr.delay = time;
        }
        // Attach the event to the trigger
        tr.event = func;
        // And add it to the active list
        // (A trigger attached twice would corrupt the C list; unlink first.)
        if tr.linked {
            self.head.remove(&mut self.triggers, trigger);
        }
        self.head.push_front(&mut self.triggers, trigger);
        self.get_mut(trigger).linked = true;
    }
}

/// `data[0]` / `data[1]` of a logic trigger (what `TRIGGER_LOGIC`/`LOGIC2` free).
fn logic_arg(f: &TriggerFunc, i: usize) -> Option<TriggerId> {
    match (f, i) {
        (TriggerFunc::And(a, _) | TriggerFunc::Or(a, _) | TriggerFunc::Not(a), 0) => Some(*a),
        (TriggerFunc::And(_, b) | TriggerFunc::Or(_, b), 1) => Some(*b),
        _ => None,
    }
}

impl Game {
    /// Port of `MakeTrigger` (Common/event.c:36).
    pub fn make_trigger(&mut self, func: TriggerFunc) -> TriggerId {
        self.event.trigger_list.make_trigger(func)
    }

    /// Port of `AttachEvent` (Common/event.c:62).
    pub fn attach_event(&mut self, trigger: TriggerId, flags: u16, time: u32, func: EventFunc) {
        self.event.trigger_list.attach_event(trigger, flags, time, func);
    }

    /// `t = MakeTrigger(OnTimeout, actFrameCount + delay, NULL...); AttachEvent(t,
    /// TRIGGER_ONCE, 0, func, ...)`: the delayed one-shot used by `anim.c`/`tongue.c`.
    pub fn make_timeout_event(&mut self, delay: u32, func: EventFunc) -> TriggerId {
        let t = self.make_trigger(TriggerFunc::OnTimeout { time: self.act_frame_count.wrapping_add(delay), interval: 0 });
        self.attach_event(t, TRIGGER_ONCE, 0, func);
        t
    }

    /// Port of `InitTriggerList` (Common/event.c:96).
    pub fn init_trigger_list(&mut self) {
        self.event.trigger_list.init_trigger_list();
    }

    /// Port of `FreeTriggerList` (Common/event.c:130).
    pub fn free_trigger_list(&mut self) {
        if self.event.trigger_list.free_trigger_list() {
            self.event.triggers_reset = true;
        }
    }

    /// Port of `KillAllTriggers` (Common/event.c:163).
    pub fn kill_all_triggers(&mut self) {
        self.event.trigger_list.kill_all_triggers();
        self.event.triggers_reset = true;
    }

    /// Port of `DeallocateTrigger` (Common/event.c:214).
    pub fn deallocate_trigger(&mut self, t: Option<TriggerId>) {
        self.event.trigger_list.deallocate_trigger(t);
    }

    /// Port of `UpdateEvents` (Common/event.c:322).
    pub fn update_events(&mut self) {
        self.event.triggers_reset = false;

        let mut cur = self.event.trigger_list.head.first;
        while let Some(t) = cur {
            let next = self.event.trigger_list.get(t).links.next;
            let (flags, count) = {
                let tr = self.event.trigger_list.get(t);
                (tr.flags, tr.count)
            };

            // Check if the trigger condition(s) are true and the trigger is allowed to fire
            if !(flags & TRIGGER_DELAY != 0 && count != 0) && self.call_trigger(t) {
                let flags = self.event.trigger_list.get(t).flags;
                // rising-edge triggers fire when the trigger is not previously fired
                if !(flags & TRIGGER_RISING != 0 && flags & TRIGGER_FIRED != 0) {
                    let ev = self.event.trigger_list.get(t).event.clone();
                    self.call_event(&ev);

                    // we need to skip trying to do anything else this gameloop if the
                    // triggers were reset!
                    if self.event.triggers_reset {
                        return;
                    }
                }

                let tr = self.event.trigger_list.get_mut(t);
                tr.flags |= TRIGGER_FIRED; // Flag for other triggers to check if needed

                if tr.flags & TRIGGER_ONCE != 0 {
                    // if "trigger once", kill this trigger
                    self.event.trigger_list.deallocate_trigger(Some(t));
                } else if tr.flags & TRIGGER_DELAY != 0 {
                    tr.count = tr.delay;
                }
            } else {
                let tr = self.event.trigger_list.get_mut(t);
                if tr.count != 0 {
                    tr.count -= 1;
                }
                tr.flags &= !TRIGGER_FIRED;
            }
            cur = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        let mut g = Game::new();
        g.init_trigger_list();
        g
    }

    /// An `AssignIntToInt(&scriptCounters[0], counter0 + 1)`-like observable action:
    /// increments script counter 0 via a script-free event.
    fn bump(g: &Game) -> EventFunc {
        EventFunc::AssignIntToInt { var: IntVar::ScriptCounter(0), val: g.script.script_counters[0] as i32 + 1 }
    }

    #[test]
    fn alloc_order_and_blocks() {
        let mut l = TriggerList::default();
        l.init_trigger_list();
        // avail.next is the last base trigger
        assert_eq!(l.allocate_trigger(), TriggerId(63));
        assert_eq!(l.allocate_trigger(), TriggerId(62));
        for _ in 0..62 {
            l.allocate_trigger();
        }
        assert_eq!(l.count, 64);
        let t = l.allocate_trigger();
        assert_eq!((t, l.blocks, l.alloced), (TriggerId(79), 1, 80));
        l.deallocate_trigger(Some(TriggerId(5)));
        assert_eq!(l.allocate_trigger(), TriggerId(5));
        // double free is ignored
        l.deallocate_trigger(Some(TriggerId(5)));
        l.deallocate_trigger(Some(TriggerId(5)));
        assert_eq!(l.count, 64);
    }

    #[test]
    fn attach_inserts_at_front_and_logic_frees_children() {
        let mut l = TriggerList::default();
        l.init_trigger_list();
        let a = l.make_trigger(TriggerFunc::Always);
        let b = l.make_trigger(TriggerFunc::Always);
        let or = l.make_trigger(TriggerFunc::Or(a, b));
        l.get_mut(or).flags |= TRIGGER_LOGIC | TRIGGER_LOGIC2;
        let x = l.make_trigger(TriggerFunc::Always);
        l.attach_event(or, 0, 0, EventFunc::None);
        l.attach_event(x, 0, 0, EventFunc::None);
        assert_eq!(l.active(), vec![x, or]);
        assert_eq!(l.count, 4);
        l.deallocate_trigger(Some(or));
        assert_eq!(l.count, 1);
        assert!(l.get(a).flags & TRIGGER_FREE != 0 && l.get(b).flags & TRIGGER_FREE != 0);
        assert_eq!(l.active(), vec![x]);
        l.kill_all_triggers();
        assert_eq!((l.count, l.head.first), (0, None));
    }

    #[test]
    fn and_or_not_logic() {
        let mut g = game();
        g.script.script_flags[0] = 0b01; // flag 0 set, flag 1 clear
        let f0 = || TriggerFunc::OnFlagSet(0);
        let f1 = || TriggerFunc::OnFlagSet(1);
        let cases = [
            (TriggerFunc::And as fn(TriggerId, TriggerId) -> TriggerFunc, f0(), f0(), true),
            (TriggerFunc::And, f0(), f1(), false),
            (TriggerFunc::Or, f1(), f0(), true),
            (TriggerFunc::Or, f1(), f1(), false),
        ];
        for (mk, a, b, want) in cases {
            let a = g.make_trigger(a);
            let b = g.make_trigger(b);
            let t = g.make_trigger(mk(a, b));
            assert_eq!(g.call_trigger(t), want);
        }
        let a = g.make_trigger(f1());
        let n = g.make_trigger(TriggerFunc::Not(a));
        assert!(g.call_trigger(n));
    }

    #[test]
    fn and_short_circuits_random_timeout() {
        let mut g = game();
        let a = g.make_trigger(TriggerFunc::OnFlagSet(3));
        let b = g.make_trigger(TriggerFunc::OnRandomTimeout { time: 0, start: 1, end: 10 });
        let t = g.make_trigger(TriggerFunc::And(a, b));
        let seed = g.rng.rseed;
        assert!(!g.call_trigger(t));
        assert_eq!(g.rng.rseed, seed, "second operand must not run");
        let t = g.make_trigger(TriggerFunc::Or(b, a));
        assert!(g.call_trigger(t));
        assert_ne!(g.rng.rseed, seed);
    }

    #[test]
    fn once_fires_once_and_frees() {
        let mut g = game();
        let t = g.make_trigger(TriggerFunc::Always);
        let ev = bump(&g);
        g.attach_event(t, TRIGGER_ONCE, 0, ev);
        g.update_events();
        g.update_events();
        assert_eq!(g.script.script_counters[0], 1);
        assert_eq!(g.event.trigger_list.count, 0);
    }

    #[test]
    fn always_fires_every_frame() {
        let mut g = game();
        for n in 1..=3 {
            let t = g.make_trigger(TriggerFunc::Always);
            g.attach_event(t, TRIGGER_ALWAYS, 0, EventFunc::AssignIntToInt { var: IntVar::ScriptCounter(1), val: n });
        }
        // Front insertion: the last attached runs first, so the first one wins.
        g.update_events();
        assert_eq!(g.script.script_counters[1], 1);
        assert_eq!(g.event.trigger_list.count, 3);
    }

    #[test]
    fn delay_waits_count_frames() {
        let mut g = game();
        let t = g.make_trigger(TriggerFunc::Always);
        g.attach_event(t, TRIGGER_DELAY, 3, EventFunc::None);
        let mut fired = vec![];
        for _ in 0..9 {
            let before = g.event.trigger_list.get(t).count;
            g.update_events();
            fired.push(before == 0);
        }
        // fires, then is blocked while count counts 3 -> 0 (decremented in the else branch)
        assert_eq!(fired, vec![true, false, false, false, true, false, false, false, true]);
        // while blocked FIRED is cleared
        assert_eq!(g.event.trigger_list.get(t).count, 3);
    }

    #[test]
    fn rising_edge() {
        let mut g = game();
        let t = g.make_trigger(TriggerFunc::OnFlagSet(2));
        g.attach_event(t, TRIGGER_RISING, 0, EventFunc::AssignIntToInt { var: IntVar::ScriptCounter(2), val: 7 });
        let mut hits = 0;
        for frame in 0..8 {
            // flag 2 high on frames 1,2,3 and 6
            let high = matches!(frame, 1..=3 | 6);
            g.script.script_flags[0] = if high { 1 << 2 } else { 0 };
            g.script.script_counters[2] = 0;
            g.update_events();
            hits += (g.script.script_counters[2] == 7) as i32;
        }
        assert_eq!(hits, 2);
    }

    #[test]
    fn on_timeout_rearms() {
        let mut g = game();
        g.act_frame_count = 10;
        let t = g.make_trigger(TriggerFunc::OnTimeout { time: 12, interval: 5 });
        let mut fires = vec![];
        for _ in 0..12 {
            if g.call_trigger(t) {
                fires.push(g.act_frame_count);
            }
            g.act_frame_count += 1;
        }
        assert_eq!(fires, vec![12, 17]);
    }
}
