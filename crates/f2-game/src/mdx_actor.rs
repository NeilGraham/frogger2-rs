//! Gameplay-relevant half of `MDX Library/mdxActor.cpp`: the `MDX_ACTOR` fields the
//! simulation reads/writes and the animation timing (`Animate`, `UpdateAnims`,
//! `InitAnims`, `QueueFlush`). Matrix transforms, bounding boxes and drawing stay in
//! the renderer.
//!
//! Animation time is `float` in the original (`MDX_ACTOR_ANIMATION`), so it is `f32` here.

use serde::{Deserialize, Serialize};

use crate::assets::{MdxAnimation, ModelInfo};

/// `MDX_ANIM_QUEUE_LENGTH` (`mdxObject.h:200`).
pub const MDX_ANIM_QUEUE_LENGTH: usize = 5;

/// `INIT_ANIMATION` / `INIT_SHADOW` (`mdxActor.h:57`).
pub const INIT_ANIMATION: i32 = 1 << 0;
pub const INIT_SHADOW: i32 = 1 << 1;

/// MDX actor flags (`mdxActor.h:60`). Prefixed module because `actor2.h` reuses the
/// `ACTOR_*` names for different bits.
pub mod mdx_flags {
    pub const ACTOR_NOFLAGS: i32 = 0;
    pub const ACTOR_WRAPTC: i32 = 1 << 0;
    pub const ACTOR_ALWAYSDRAW: i32 = 1 << 1;
    pub const ACTOR_SLIDY: i32 = 1 << 2;
}

/// `OBJECT_FLAGS_*` (`mdxObject.h:257`) used by actor creation.
pub mod object_flags {
    pub const OBJECT_FLAGS_XLU: i16 = 1 << 1;
    pub const OBJECT_FLAGS_SHEEN: i16 = 1 << 4;
    pub const OBJECT_FLAGS_MODGE: i16 = 1 << 5;
    pub const OBJECT_FLAGS_ADDITIVE: i16 = 1 << 6;
    pub const OBJECT_FLAGS_WAVE: i16 = 1 << 7;
    pub const OBJECT_FLAGS_PHONG: i16 = 1 << 8;
}

/// `MDX_ACTOR_ANIMATION` (`mdxObject.h:202`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MdxActorAnimation {
    pub num_animations: i16,
    pub current_animation: i16,
    pub reached_end_of_animation: i32,
    pub loop_animation: i32,
    pub animation_speed: f32,
    pub queue_animation: [i16; MDX_ANIM_QUEUE_LENGTH],
    pub queue_loop_animation: [i32; MDX_ANIM_QUEUE_LENGTH],
    pub queue_animation_speed: [f32; MDX_ANIM_QUEUE_LENGTH],
    pub number_queued: i16,
    /// `anims`: copy of the model's `MDX_ANIMATION` table (empty = `NULL`).
    pub anims: Vec<MdxAnimation>,
    /// `anims[numAnimations]` (what follows the table in the bank image), if known.
    #[serde(default)]
    pub anim_past_end: Option<MdxAnimation>,
    pub anim_time: f32,
    /// `loopFlags[5]` (looping sounds already started per animation).
    pub loop_flags: [i8; 5],
    /// `sfxMapping`: index into `GeneralState::sfx_anim_map` of this actor's sample run.
    pub sfx_mapping: Option<usize>,
}

impl MdxActorAnimation {
    /// `anims + i`. Out-of-range reads (the original reads past the table when
    /// `animNum == numAnimations`, see `animate`) yield the bank bytes after the table
    /// (`anim_past_end`), else a zeroed entry.
    // BUG(original): `Animate` only rejects `animNum > numAnimations`; index ==
    // numAnimations reads whatever follows the table in the bank image (e.g. the frog's
    // celebrate anim 42 on the level-complete screen, fin-4-1).
    #[inline]
    pub fn anim(&self, i: i32) -> MdxAnimation {
        if i >= 0 {
            match self.anims.get(i as usize) {
                Some(a) => *a,
                None if i as usize == self.anims.len() => self.anim_past_end.unwrap_or_default(),
                None => MdxAnimation::default(),
            }
        } else {
            MdxAnimation::default()
        }
    }

    /// `anims != NULL`.
    #[inline]
    pub fn has_anims(&self) -> bool {
        !self.anims.is_empty()
    }
}

/// Gameplay-visible part of `MDX_ACTOR` (`mdxActor.h:16`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MdxActor {
    /// `objectController`: model id (`crc("name.obe")`), `None` = `NULL`.
    pub object_controller: Option<u32>,
    /// `LODObjectController` (used by `ChangeModel` to remember the original model).
    pub lod_object_controller: Option<u32>,
    /// `flags` (`mdx_flags::*`).
    pub flags: i32,
    /// `listNum`: which `actorDrawList` the actor is on (0 normal, 1 shadows/additive).
    pub list_num: u32,
    /// `objectController->object->flags` of this actor's (unique) root object.
    pub root_object_flags: i16,
    /// `animation`; `None` when the model has no animation table.
    pub animation: Option<MdxActorAnimation>,
    /// `pos`/`qRot`/`scale`/`visible`/`draw` as `CopyActorList` sets them (`view.rs`).
    pub placement: crate::view::MdxPlacement,
}

/// Port of `QueueFlush` (`MDX Library/mdxActor.cpp:288`).
pub fn queue_flush(anim: &mut MdxActorAnimation) {
    anim.number_queued = 0;
    for q in anim.queue_animation.iter_mut() {
        *q = -1;
    }
}

/// Port of `InitAnims` (`MDX Library/mdxActor.cpp:597`). The `memcpy` from the
/// controller's template copies `numAnimations` and `anims`; every other field of the
/// template is zero in the banks, so they start zeroed (incl. `sfxMapping`).
pub fn init_anims(model: &ModelInfo) -> MdxActorAnimation {
    let mut a = MdxActorAnimation { num_animations: model.animations.len() as i16, anims: model.animations.clone(), anim_past_end: model.anim_past_end, ..Default::default() };
    a.current_animation = -1;
    // Only the first five entries are reset (all of them, as the queue length is 5).
    a.queue_animation = [-1; MDX_ANIM_QUEUE_LENGTH];
    a.loop_animation = 0;
    a.number_queued = 0;
    a.anim_time = 0.0;
    a.reached_end_of_animation = 0;
    a
}

/// What `Animate`/`UpdateAnims` asked the `StartAnim` hook to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimEvent {
    None,
    /// `StartAnim(actor)` was called (`PlaySfxMappedSample` on PC).
    StartAnim,
}

/// Port of `Animate` (`MDX Library/mdxActor.cpp:307`).
pub fn animate(anim: &mut MdxActorAnimation, mut anim_num: i32, looped: bool, queue: bool, speed: f32) -> AnimEvent {
    if !anim.has_anims() {
        return AnimEvent::None;
    }
    if anim_num > anim.num_animations as i32 {
        anim_num = 0;
    }
    let actual_speed = if speed == 0.0 { anim.anim(anim_num).speed } else { speed };

    if !queue {
        anim.current_animation = anim_num as i16;
        let a = anim.anim(anim.current_animation as i32);
        anim.loop_animation = looped as i32;
        anim.animation_speed = actual_speed;
        anim.reached_end_of_animation = 0;
        anim.anim_time = if actual_speed < 0.0 { a.anim_end as f32 } else { a.anim_start as f32 };
        queue_flush(anim);
        AnimEvent::StartAnim
    } else {
        if anim.current_animation == -1 {
            anim.current_animation = anim_num as i16;
            anim.loop_animation = looped as i32;
            anim.animation_speed = actual_speed;
        } else {
            if anim.number_queued as usize == MDX_ANIM_QUEUE_LENGTH {
                return AnimEvent::None;
            }
            let n = anim.number_queued as usize;
            anim.queue_animation[n] = anim_num as i16;
            anim.queue_loop_animation[n] = looped as i32;
            anim.queue_animation_speed[n] = actual_speed;
            anim.number_queued += 1;
        }
        AnimEvent::None
    }
}

/// `Bound(a, lo, hi)` on floats.
#[inline]
fn bound(x: f32, lo: f32, hi: f32) -> f32 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// Port of `UpdateAnims` (`MDX Library/mdxActor.cpp:375`). `time_speed` is
/// `timeInfo.speed` (1.0 at the fixed 60 Hz step).
pub fn update_anims(anim: &mut MdxActorAnimation, time_speed: f32) -> AnimEvent {
    if !anim.has_anims() {
        return AnimEvent::None;
    }
    if anim.current_animation < 0 {
        return AnimEvent::None;
    }
    let a = anim.anim(anim.current_animation as i32);
    let (start, end) = (a.anim_start as f32, a.anim_end as f32);

    anim.reached_end_of_animation = 0;
    anim.anim_time += anim.animation_speed * time_speed;

    if (anim.anim_time > end || anim.anim_time < start) && (anim.loop_animation == 0 || anim.number_queued != 0) {
        anim.anim_time = bound(anim.anim_time, start, end);
        anim.reached_end_of_animation = 1;

        if anim.number_queued != 0 {
            anim.current_animation = anim.queue_animation[0];
            anim.loop_animation = anim.queue_loop_animation[0];
            anim.animation_speed = anim.queue_animation_speed[0];

            let q = anim.anim(anim.current_animation as i32);
            anim.anim_time = if anim.animation_speed < 0.0 { q.anim_end as f32 } else { q.anim_start as f32 };

            anim.number_queued -= 1;
            if anim.number_queued != 0 {
                for i in 0..MDX_ANIM_QUEUE_LENGTH - 1 {
                    anim.queue_animation[i] = anim.queue_animation[i + 1];
                    anim.queue_loop_animation[i] = anim.queue_loop_animation[i + 1];
                    anim.queue_animation_speed[i] = anim.queue_animation_speed[i + 1];
                }
                anim.queue_animation[MDX_ANIM_QUEUE_LENGTH - 1] = -1;
                anim.queue_loop_animation[MDX_ANIM_QUEUE_LENGTH - 1] = -1;
                anim.queue_animation_speed[MDX_ANIM_QUEUE_LENGTH - 1] = -1.0;
            } else {
                anim.queue_animation[0] = -1;
                anim.queue_loop_animation[0] = -1;
                anim.queue_animation_speed[0] = -1.0;
            }
            return AnimEvent::StartAnim;
        }
        AnimEvent::None
    } else if a.anim_end == a.anim_start {
        anim.anim_time = end;
        AnimEvent::None
    } else if anim.anim_time > end {
        // (end - start) is an int expression converted to float.
        let len = (a.anim_end as i32 - a.anim_start as i32) as f32;
        // BUG(original): with end < start this loop never terminates in the original;
        // the guard below only stops the port from hanging on such data.
        if len > 0.0 {
            while anim.anim_time > end {
                anim.anim_time -= len;
            }
        }
        AnimEvent::StartAnim
    } else if anim.anim_time < start {
        anim.anim_time += (a.anim_end as i32 - a.anim_start as i32) as f32;
        AnimEvent::None
    } else {
        AnimEvent::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ModelInfo {
        ModelInfo {
            animations: vec![
                MdxAnimation { anim_start: 0, anim_end: 10, speed: 1.0, looped: 1 },
                MdxAnimation { anim_start: 20, anim_end: 25, speed: 0.5, looped: 0 },
                MdxAnimation { anim_start: 30, anim_end: 30, speed: 1.0, looped: 0 },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn looping_wraps() {
        let mut a = init_anims(&model());
        assert_eq!(a.current_animation, -1);
        assert_eq!(update_anims(&mut a, 1.0), AnimEvent::None);
        assert_eq!(animate(&mut a, 0, true, false, 3.0), AnimEvent::StartAnim);
        let mut wraps = 0;
        for _ in 0..20 {
            if update_anims(&mut a, 1.0) == AnimEvent::StartAnim {
                wraps += 1;
            }
            assert!((0.0..=10.0).contains(&a.anim_time));
            assert_eq!(a.reached_end_of_animation, 0);
        }
        // 60 frames of time over a 10-frame loop.
        // Wrapping only happens when strictly past the end, so 60 lands on 10.
        assert_eq!(wraps, 5);
        assert_eq!(a.anim_time, 10.0);
    }

    #[test]
    fn one_shot_then_queue() {
        let mut a = init_anims(&model());
        animate(&mut a, 1, false, false, 0.0);
        assert_eq!(a.animation_speed, 0.5);
        assert_eq!(a.anim_time, 20.0);
        animate(&mut a, 0, true, true, 2.0);
        assert_eq!(a.number_queued, 1);
        let mut t = 0;
        while a.current_animation == 1 {
            update_anims(&mut a, 1.0);
            t += 1;
        }
        // 20 -> 25.5 clamps after 11 frames, then switches to the queued anim.
        assert_eq!(t, 11);
        assert_eq!(a.reached_end_of_animation, 1);
        assert_eq!((a.current_animation, a.anim_time, a.loop_animation), (0, 0.0, 1));
        assert_eq!(a.queue_animation[0], -1);
        update_anims(&mut a, 1.0);
        assert_eq!(a.reached_end_of_animation, 0);
    }

    #[test]
    fn non_loop_stops_at_end_and_reverse() {
        let mut a = init_anims(&model());
        animate(&mut a, 0, false, false, -4.0);
        assert_eq!(a.anim_time, 10.0);
        for _ in 0..3 {
            update_anims(&mut a, 1.0);
        }
        assert_eq!(a.anim_time, 0.0);
        assert_eq!(a.reached_end_of_animation, 1);
        animate(&mut a, 2, false, false, 0.0);
        update_anims(&mut a, 1.0);
        assert_eq!(a.anim_time, 30.0);
    }

    #[test]
    fn queue_full_and_first_queue_without_current() {
        let mut a = init_anims(&model());
        animate(&mut a, 1, false, true, 0.0);
        assert_eq!((a.current_animation, a.number_queued), (1, 0));
        for _ in 0..7 {
            animate(&mut a, 0, false, true, 1.0);
        }
        assert_eq!(a.number_queued, 5);
        // animNum > numAnimations -> 0
        animate(&mut a, 9, false, false, 0.0);
        assert_eq!(a.current_animation, 0);
        assert_eq!(a.number_queued, 0);
    }
}
