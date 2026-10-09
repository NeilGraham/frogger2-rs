//! Port of `PC/actor.cpp`: the PC glue between game actors and MDX actors
//! (`actorAnimate`, `actorResetAnimation`, `ChangeModel`, `UndoChangeModel`,
//! `StartAnimateActor`), plus the per-frame `UpdateAnims` call that the original
//! makes from `XformActor` while drawing, and the `StartAnim` hook
//! (`PlaySfxMappedSample`, `PC/pcaudio.c:425`).
//!
//! Deviation (logged in the report): the original only advances an actor's animation
//! when `ActorListDraw` transforms it, i.e. when it passes the view-frustum test. The
//! simulation has no camera frustum, so [`Game::update_actor_anims`] advances every
//! actor once per tick. TODO(port): revisit once the camera port can provide the
//! on-screen test deterministically.

use crate::ids::ActorId;
use crate::mdx_actor::{self, init_anims, AnimEvent};
use crate::Game;

/// `FROG_ANIM_BREATHE` (`Common/frogger.h:44`).
pub const FROG_ANIM_BREATHE: i32 = 8;
/// `FROG_BREATHE_SPEED` (`Common/frogger.h:30`).
pub const FROG_BREATHE_SPEED: i32 = 50;

impl Game {
    /// `timeInfo.speed` for the current tick (`gameSpeed / 4096`, 1.0 at 60 Hz).
    #[inline]
    pub fn time_info_speed(&self) -> f32 {
        self.game_speed as f32 / 4096.0
    }

    /// The `StartAnim` callback: `PlaySfxMappedSample` (`PC/pcaudio.c:425`). Whether a
    /// sample loops (and must become an ambient sound, tracked with `loopFlags`) is a
    /// property of the loaded sample, so the decision is left to the audio layer.
    fn start_anim(&mut self, id: ActorId) {
        let a = &self.actor2.actors[id].actor;
        let Some(anim) = a.actual_actor.animation.as_ref() else {
            return;
        };
        let Some(map) = anim.sfx_mapping else { return };
        let uid = self.general.sfx_mapped_sample(map, anim.current_animation);
        if uid == 0 {
            return;
        }
        // SetVectorSR(&pos, &act->pos); ScaleVector(&pos, 10): the MDX position is the
        // game position / 10, so this is the actor position (to within rounding).
        let (cur, pos) = (anim.current_animation, a.position);
        self.play_sfx_mapped_sample(id, uid, cur, pos); // ambient
    }

    /// Port of `actorAnimate` (PC/actor.cpp:38): speed is 8.8 fixed (`speed/256.0f`).
    pub fn actor_animate(&mut self, id: ActorId, anim_num: i32, looped: bool, queue: bool, speed: i32, _skip_end_frame: bool) {
        let a = &mut self.actor2.actors[id].actor.actual_actor;
        let Some(anim) = a.animation.as_mut() else {
            return;
        };
        if mdx_actor::animate(anim, anim_num, looped, queue, speed as f32 / 256.0) == AnimEvent::StartAnim {
            self.start_anim(id);
        }
    }

    /// Port of `actorResetAnimation` (PC/actor.cpp:44).
    pub fn actor_reset_animation(&mut self, id: ActorId) {
        let a = &mut self.actor2.actors[id].actor.actual_actor;
        if let Some(anim) = a.animation.as_mut() {
            if anim.has_anims() {
                anim.anim_time = anim.anim(anim.current_animation as i32).anim_start as f32;
            }
        }
    }

    /// Port of `StartAnimateActor` (PC/actor.cpp:121): animate unless already playing.
    pub fn start_animate_actor(&mut self, id: ActorId, anim_num: i32, looped: bool, queue: bool, speed: i32, skip: bool) {
        let cur = self.actor2.actors[id].actor.actual_actor.animation.as_ref().map_or(-1, |a| a.current_animation as i32);
        if cur != anim_num {
            self.actor_animate(id, anim_num, looped, queue, speed, skip);
        }
    }

    /// Port of `ChangeModel` (PC/actor.cpp:72): swap the actor's model, remembering
    /// the old one in `LODObjectController`.
    pub fn change_model(&mut self, id: ActorId, model: &str) {
        // newName[16]: copy up to the first '.', then append ".obe".
        let base: String = model.chars().take_while(|&c| c != '.' && c != '\0').collect();
        let new_name = format!("{base}.obe");
        let found = self.models.find_object(&new_name).cloned();
        let a = &mut self.actor2.actors[id].actor.actual_actor;
        a.lod_object_controller = a.object_controller;
        a.object_controller = found.as_ref().map(|m| m.id);
        if let Some(m) = found {
            a.animation = Some(init_anims(&m));
        }
    }

    /// Port of `UndoChangeModel` (PC/actor.cpp:99). Returns 1 if a model was restored.
    pub fn undo_change_model(&mut self, id: ActorId) -> i32 {
        let a = &mut self.actor2.actors[id].actor.actual_actor;
        let Some(lod) = a.lod_object_controller else {
            return 0;
        };
        a.object_controller = Some(lod);
        a.lod_object_controller = None;
        if let Some(m) = self.models.get(lod).cloned() {
            self.actor2.actors[id].actor.actual_actor.animation = Some(init_anims(&m));
        }
        self.actor_animate(id, FROG_ANIM_BREATHE, true, false, FROG_BREATHE_SPEED, false);
        1
    }

    /// `UpdateAnims` (`mdxActor.cpp:375`) for one actor, as `XformActor` does.
    pub fn update_actor_anim(&mut self, id: ActorId) {
        let speed = self.time_info_speed();
        let a = &mut self.actor2.actors[id].actor.actual_actor;
        if a.object_controller.is_none() {
            return;
        }
        let Some(anim) = a.animation.as_mut() else {
            return;
        };
        if mdx_actor::update_anims(anim, speed) == AnimEvent::StartAnim {
            self.start_anim(id);
        }
    }

    /// Per-frame animation step of `ActorListDraw(0)` then `ActorListDraw(1)`
    /// (`PC/drawloop.cpp`), without frustum culling (see module docs).
    pub fn update_actor_anims(&mut self) {
        for list in 0..2 {
            for id in self.actor_draw_list(list) {
                self.update_actor_anim(id);
            }
        }
    }

    /// `FindActorSubObject(actor, name)` (`mdxActor.cpp:1003`): whether the actor's
    /// current model has a sub-object of that name.
    pub fn find_actor_sub_object(&self, id: ActorId, sub_name: &str) -> bool {
        let a = &self.actor2.actors[id].actor.actual_actor;
        a.object_controller.and_then(|m| self.models.get(m)).is_some_and(|m| m.find_sub_object(sub_name).is_some())
    }

    // TODO(port): FindFrogSubObjects (PC/actor.cpp:129) - head/breast post-matrices are
    // renderer state; needs player[].character (frogger.c wave).
    // TODO(port): UpdateFrogCroak (PC/actor.cpp:168) - needs player/specfx/babyfrog state.
}
