//! Simulation side of `PC/pcaudio.c`: the sample table (`LoadSfx`/`FindSample`, loop
//! flags only), `genSfx`, the ambient sound list (`AddAmbientSound`, `SubAmbientSound`,
//! `UpdateAmbientSounds`, `FreeAmbientSoundList`), `PlaySfxMappedSample` and
//! `PrepareSong`'s track selection.
//!
//! Everything that consumes `Random()` or decides *what* is played lives here so the game
//! RNG advances in the original order; the host audio layer only executes
//! [`Effect::PlaySample`] / [`Effect::UpdateHandle`] / [`Effect::StopHandle`] /
//! [`Effect::PlayMusic`].
//!
//! Ambient instances: an ambient sound owns a nonzero `handle`. Starting an instance is
//! `Effect::PlaySample { handle, .. }` (the host binds the started voice to the handle);
//! `UpdateLoopingSample` is `Effect::UpdateHandle`; stopping is `Effect::StopHandle`.
//! Whether `PlaySample` returned a buffer (`amb->lpdsBuffer`) is reproduced here from the
//! same inputs the original used (sample present, `globalSoundVol`, distance to
//! `currCamSource`).

use std::collections::BTreeMap;

use f2_formats::crc::crc;
use f2_formats::wus::SoundBank;
use f2_math::{magnitude_s, scale_vector_s, sub_vector_ssf, SVector};
use serde::{Deserialize, Serialize};

use crate::assets::DataSource;
use crate::effects::Effect;
use crate::ids::ActorId;
use crate::Game;

/// `DEFAULT_SFX_DIST` (PC/pcaudio.c:52).
pub const DEFAULT_SFX_DIST: i32 = 7000;
/// `MAX_SOUND_VOL` (PC/pcaudio.h).
pub const MAX_SOUND_VOL: i32 = 100;
/// `SAMPLE_VOLUME` (PC/pcaudio.c:54).
pub const SAMPLE_VOLUME: i32 = 255;
/// `AMBIENT_VOLUME` (PC/pcaudio.c:55).
pub const AMBIENT_VOLUME: i32 = 255;
/// `SFX_BASE` (PC/Main.h:26).
pub const SFX_BASE: &str = "sfx\\";

// `genSfx[]` indices (PC/pcaudio.h:39).
pub const GEN_FROG_HOP: usize = 0;
pub const GEN_SUPER_HOP: usize = 1;
pub const GEN_DOUBLE_HOP: usize = 2;
pub const GEN_COLLECT_BABY: usize = 3;
pub const GEN_COLLECT_COIN: usize = 4;
pub const GEN_BABYHAPPY: usize = 5;
pub const GEN_BABYSAD: usize = 6;
pub const GEN_BABYCRY: usize = 7;
pub const GEN_BABYREPLY: usize = 8;
pub const GEN_TELEPORT: usize = 9;
pub const GEN_POWERUP: usize = 10;
pub const GEN_CLOCKTICK: usize = 11;
pub const GEN_POWERTICK: usize = 12;
pub const GEN_DEATHDROWN: usize = 13;
pub const GEN_DEATHCRUSH: usize = 14;
pub const GEN_DEATHEXPLODE: usize = 15;
pub const GEN_DEATHFALL: usize = 16;
pub const GEN_DEATHGIB: usize = 17;
pub const GEN_DEATHMOWED: usize = 18;
pub const GEN_DEATHCHOP: usize = 19;
pub const GEN_DEATHELECTRIC: usize = 20;
pub const GEN_DEATHFIRE: usize = 21;
pub const NUM_GENERIC_SFX: usize = 22;

/// Sample names `LoadSfx` (PC/pcaudio.c:200) assigns to `genSfx[GEN_*]`.
pub const GEN_SFX_NAMES: [&str; NUM_GENERIC_SFX] = [
    "hopongrass",
    "hop2",
    "doublehop",
    "getbabyfrog",
    "pickupcoin",
    "babyhappy",
    "babysad",
    "babycry",
    "babyreply",
    "teleport",
    "powerup",
    "clocktick",
    "puptick",
    "frogdrown1",
    "frogcrush",
    "frogexplode",
    "frogfall",
    "frog_gib",
    "frogmowed",
    "frogchop",
    "electrocute",
    "burnbum",
];

/// `genSfx[i]` as a sample uid (`UpdateCRC(name)`).
pub fn gen_sfx(i: usize) -> u32 {
    crc(GEN_SFX_NAMES[i])
}

// `PrepareSong` ids (Common/layout.h world ids, PC/pcaudio.h:87).
pub const AUDIOTRK_GAMEOVER: i16 = 100;
pub const AUDIOTRK_LEVELCOMPLETE: i16 = 101;
pub const AUDIOTRK_LEVELCOMPLETELOOP: i16 = 102;

/// CD tracks (PC/pcaudio.c:128).
pub mod cdaudio {
    pub const GARDEN: u32 = 2;
    pub const ANCIENTS: u32 = 3;
    pub const SPACE: u32 = 4;
    pub const SUBTERRANEAN: u32 = 5;
    pub const LABORATORY: u32 = 6;
    pub const HALLOWEEN: u32 = 7;
    pub const SUPERRETRO: u32 = 8;
    pub const FRONTEND: u32 = 9;
    pub const LEVELCOMPLETE: u32 = 10;
    pub const GAMEOVER: u32 = 11;
    pub const LEVELCOMPLETELOOP: u32 = 12;
}

/// Track selection of `PrepareSong` (PC/pcaudio.c:949). City deliberately uses the Super
/// Retro track.
pub fn song_track(world: i16) -> Option<u32> {
    use crate::layout::worldid as w;
    if world < 0 {
        return None;
    }
    Some(match world as u32 {
        w::GARDEN => cdaudio::GARDEN,
        w::ANCIENT => cdaudio::ANCIENTS,
        w::SPACE => cdaudio::SPACE,
        w::CITY => cdaudio::SUPERRETRO,
        w::SUBTERRANEAN => cdaudio::SUBTERRANEAN,
        w::LABORATORY => cdaudio::LABORATORY,
        w::HALLOWEEN => cdaudio::HALLOWEEN,
        w::SUPERRETRO => cdaudio::SUPERRETRO,
        w::FRONTEND => cdaudio::FRONTEND,
        100 => cdaudio::GAMEOVER,
        101 => cdaudio::LEVELCOMPLETE,
        102 => cdaudio::LEVELCOMPLETELOOP,
        _ => return None,
    })
}

/// World sound bank loaded by `LoadSfx` (PC/pcaudio.c:232).
pub fn world_bank(world: u32) -> Option<&'static str> {
    use crate::layout::worldid as w;
    Some(match world {
        w::GARDEN => "garden.wus",
        w::ANCIENT => "ancients.wus",
        w::SPACE => "space.wus",
        w::CITY => "city.wus",
        w::SUBTERRANEAN => "sub.wus",
        w::LABORATORY => "lab.wus",
        w::HALLOWEEN => "halloween.wus",
        w::SUPERRETRO => "superretro.wus",
        w::FRONTEND => "frontend.wus",
        w::TEST => "test.wus",
        _ => return None,
    })
}

/// One `AMBIENT_SOUND` (PC/pcaudio.h:138).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AmbientSound {
    /// Identity of this ambient (the C pointer); also the host voice handle.
    pub handle: u32,
    pub pos: SVector,
    pub follow: Option<ActorId>,
    /// `sample->uid`.
    pub sample: u32,
    /// `sample->flags & SFXFLAGS_LOOP`.
    pub looped: bool,
    /// `lpdsBuffer != NULL`: the last `PlaySample` returned a buffer.
    pub has_buffer: bool,
    pub volume: i16,
    pub pitch: i16,
    pub radius: i32,
    /// Pre-multiplied by 60.
    pub freq: i32,
    pub rand_freq: i32,
    pub counter: i32,
}

/// Globals of `PC/pcaudio.c` that the simulation needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PcAudioState {
    /// `soundList`: loaded sample uid -> `SFXFLAGS_LOOP`.
    pub sound_list: BTreeMap<u32, bool>,
    /// `ambientSoundList`, head first.
    pub ambient_sound_list: Vec<AmbientSound>,
    /// Next ambient handle (never 0).
    pub next_handle: u32,
    /// `globalSoundVol` (0..=100), set by the host from the options.
    pub global_sound_vol: i32,
    /// `cdTrack`: >0 looping track, -1 playing once, 0 stopped.
    pub cd_track: i32,
}

impl Default for PcAudioState {
    fn default() -> Self {
        PcAudioState { sound_list: BTreeMap::new(), ambient_sound_list: Vec::new(), next_handle: 1, global_sound_vol: MAX_SOUND_VOL, cd_track: 0 }
    }
}

impl PcAudioState {
    /// Port of `FindSample` (PC/pcaudio.c:1226): `Some(looped)` when the sample is loaded.
    pub fn find_sample(&self, uid: u32) -> Option<bool> {
        if uid == 0 {
            return None;
        }
        self.sound_list.get(&uid).copied()
    }

    /// `LoadSfxSet` (PC/pcaudio.c:155) on a parsed bank: `AddSample` inserts at the head,
    /// so a later bank's sample wins `FindSample`.
    pub fn load_sfx_set(&mut self, bank: &SoundBank) {
        for s in &bank.samples {
            self.sound_list.insert(s.uid, s.looped);
        }
    }

    /// Port of `InitSampleList` (PC/pcaudio.c:1258) (sample part).
    pub fn init_sample_list(&mut self) {
        self.sound_list.clear();
    }
}

/// `SetVectorSR(&v, &act->pos); ScaleVector(&v, 10)`: the MDX actor position is the game
/// position / 10 (float), truncated to `short`, then scaled back by 10.
pub fn mdx_pos_x10(p: SVector) -> SVector {
    let mut v = SVector { vx: (p.vx as f32 / 10.0) as i16, vy: (p.vy as f32 / 10.0) as i16, vz: (p.vz as f32 / 10.0) as i16, pad: 0 };
    scale_vector_s(&mut v, 10);
    v
}

impl Game {
    /// Port of `LoadSfx` (PC/pcaudio.c:186), sample-table part: generic, multi (when not
    /// single player) and the world bank. Voice banks (`InitVoices`) stay host-side.
    pub fn load_sfx(&mut self, world_id: u32, data: &dyn DataSource) {
        let load = |g: &mut Game, name: &str| {
            let path = format!("{SFX_BASE}{name}");
            match data.sound_bank(&path) {
                Some(Ok(bank)) => g.pcaudio.load_sfx_set(&bank),
                _ => g.effects.push(Effect::Log { text: format!("Failed to load sample bank {path}") }),
            }
        };
        load(self, "generic.wus");
        if self.game_state.multi != crate::define::SINGLEPLAYER {
            load(self, "multi.wus");
        }
        if let Some(b) = world_bank(world_id) {
            load(self, b);
        }
    }

    /// Port of `AddAmbientSound` (PC/pcaudio.c:638). `uid` is the sample (`FindSample`
    /// is applied here; a missing sample adds nothing). Returns the ambient handle.
    #[allow(clippy::too_many_arguments)]
    pub fn add_ambient_sound(
        &mut self,
        uid: u32,
        pos: Option<SVector>,
        radius: i32,
        vol: i16,
        pitch: i16,
        freq: i32,
        rand_freq: i32,
        follow: Option<ActorId>,
    ) -> Option<u32> {
        let looped = self.pcaudio.find_sample(uid)?;
        let p = &mut self.pcaudio;
        let handle = p.next_handle;
        p.next_handle = p.next_handle.wrapping_add(1).max(1);
        let a = AmbientSound {
            handle,
            pos: pos.map(|v| SVector { pad: 0, ..v }).unwrap_or_default(),
            follow,
            sample: uid,
            looped,
            has_buffer: false,
            volume: vol,
            pitch,
            radius,
            freq: freq.wrapping_mul(60),
            rand_freq: rand_freq.wrapping_mul(60),
            counter: 0,
        };
        p.ambient_sound_list.insert(0, a);
        Some(handle)
    }

    /// Port of `SubAmbientSound` (PC/pcaudio.c:1412). Like the original, a playing
    /// instance is left alone.
    pub fn sub_ambient_sound(&mut self, handle: u32) {
        self.pcaudio.ambient_sound_list.retain(|a| a.handle != handle);
    }

    /// Port of `FreeAmbientSoundList` (PC/pcaudio.c:1425): stops each ambient's buffer and
    /// empties the list. (Stopping other looping buffered samples is host-side.)
    pub fn free_ambient_sound_list(&mut self) {
        let list = std::mem::take(&mut self.pcaudio.ambient_sound_list);
        for a in list {
            if a.has_buffer {
                self.effects.push(Effect::StopHandle { handle: a.handle });
            }
        }
    }

    /// `PlaySample`'s return value (PC/pcaudio.c:295) for a positioned call: whether a
    /// buffer is created. Mirrors the early-outs (no sample, zero volume, out of range).
    fn play_sample_would_play(&self, volume: i32, pos: &SVector, radius: i32) -> bool {
        let vol = volume.wrapping_mul(self.pcaudio.global_sound_vol) / MAX_SOUND_VOL;
        if vol == 0 {
            return false;
        }
        let att = if radius != 0 { radius as f32 } else { DEFAULT_SFX_DIST as f32 };
        let mut diff = SVector::default();
        sub_vector_ssf(&mut diff, pos, &self.cam.curr_cam_source);
        let dist = magnitude_s(&diff) as f32 / 4096.0;
        dist <= att
    }

    /// Port of `UpdateAmbientSounds` (PC/pcaudio.c:676), called from `RunGameLoop`
    /// (Common/game.c:711) when not paused.
    pub fn update_ambient_sounds(&mut self) {
        let afc = self.act_frame_count as i32;
        for i in 0..self.pcaudio.ambient_sound_list.len() {
            let mut a = self.pcaudio.ambient_sound_list[i].clone();

            // Timeout, so play a new sound
            if afc < a.counter {
                continue;
            }

            // If it is attached to a platform, make ambient follow that platform
            if let Some(f) = a.follow {
                if let Some(act) = self.actor2.actors.get(f) {
                    a.pos = mdx_pos_x10(act.actor.position);
                }
            }

            if a.looped && a.has_buffer {
                // UpdateLoopingSample(amb)
                self.effects.push(Effect::UpdateHandle {
                    handle: a.handle,
                    volume: a.volume as i32,
                    pitch: a.pitch as i32,
                    pos: Some(a.pos),
                    radius: a.radius,
                });
            } else {
                // BUG(original): `PlaySample` gets `&amb->pos` even when the computed `pos`
                // is NULL, so a sound "without a source" is attenuated relative to the origin.
                a.has_buffer = self.play_sample_would_play(a.volume as i32, &a.pos, a.radius);
                self.effects.push(Effect::PlaySample {
                    uid: a.sample,
                    pos: Some(a.pos),
                    radius: a.radius,
                    volume: a.volume as i32,
                    pitch: a.pitch as i32,
                    handle: a.handle,
                });
            }

            // Freq and randFreq are cunningly pre-multiplied by 60
            let r = if a.rand_freq != 0 { self.rng.random(a.rand_freq as u32) } else { 0 };
            a.counter = afc.wrapping_add(a.freq).wrapping_add(r);
            self.pcaudio.ambient_sound_list[i] = a;
        }
    }

    /// Port of `PlaySfxMappedSample` (PC/pcaudio.c:425): looping samples become an ambient
    /// sound following the actor (once per animation, `loopFlags`), others play at `pos`.
    /// `position` is the game actor's position.
    pub fn play_sfx_mapped_sample(&mut self, id: ActorId, uid: u32, current_animation: i16, position: SVector) {
        let Some(looped) = self.pcaudio.find_sample(uid) else { return };
        let pos = mdx_pos_x10(position);
        if looped {
            let Some(anim) = self.actor2.actors.get_mut(id).and_then(|a| a.actor.actual_actor.animation.as_mut()) else { return };
            let Some(flag) = anim.loop_flags.get_mut(current_animation as usize) else { return };
            if *flag == 0 {
                *flag = 1;
                self.add_ambient_sound(uid, Some(pos), DEFAULT_SFX_DIST, SAMPLE_VOLUME as i16, -1, 0, 0, Some(id));
            }
            return;
        }
        self.play_sample(uid, Some(pos), DEFAULT_SFX_DIST, SAMPLE_VOLUME, -1);
    }

    /// Port of `PauseAudio` (PC/pcaudio.c:735), simulation part.
    pub fn pause_audio(&mut self) {
        for a in &mut self.pcaudio.ambient_sound_list {
            a.has_buffer = false;
        }
    }

    /// Port of `UnPauseAudio` (PC/pcaudio.c:764), simulation part.
    pub fn un_pause_audio(&mut self) {
        for a in &mut self.pcaudio.ambient_sound_list {
            a.has_buffer = false;
        }
    }

    /// Port of `PrepareSong` (PC/pcaudio.c:942).
    pub fn prepare_song(&mut self, world: i16, looped: bool) {
        let Some(track) = song_track(world) else { return };
        self.effects.push(Effect::PlayMusic { track, looped });
        self.pcaudio.cd_track = if looped { track as i32 } else { -1 };
    }

    /// Port of `StopSong` (PC/pcaudio.c:988).
    pub fn stop_song(&mut self) {
        self.effects.push(Effect::StopMusic);
        self.pcaudio.cd_track = 0;
    }

    /// Port of `IsSongPlaying` (PC/pcaudio.c:995).
    pub fn is_song_playing(&self) -> bool {
        self.pcaudio.cd_track != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game_with(samples: &[(u32, bool)]) -> Game {
        let mut g = Game::new();
        for &(u, l) in samples {
            g.pcaudio.sound_list.insert(u, l);
        }
        g
    }

    #[test]
    fn gen_sfx_names() {
        assert_eq!(gen_sfx(GEN_COLLECT_COIN), crc("pickupcoin"));
        assert_eq!(gen_sfx(GEN_DEATHGIB), crc("frog_gib"));
        assert_eq!(gen_sfx(GEN_DEATHFIRE), crc("burnbum"));
    }

    #[test]
    fn song_tracks() {
        assert_eq!(song_track(3), Some(cdaudio::SUPERRETRO));
        assert_eq!(song_track(8), Some(cdaudio::FRONTEND));
        assert_eq!(song_track(AUDIOTRK_LEVELCOMPLETE), Some(cdaudio::LEVELCOMPLETE));
        assert_eq!(song_track(9), None);
        let mut g = Game::new();
        g.prepare_song(0, true);
        assert_eq!(g.effects, vec![Effect::PlayMusic { track: 2, looped: true }]);
        assert!(g.is_song_playing());
    }

    #[test]
    fn ambient_list_order_and_random() {
        let mut g = game_with(&[(1, false), (2, true)]);
        assert_eq!(g.add_ambient_sound(99, None, 0, 255, -1, 0, 0, None), None);
        let h1 = g.add_ambient_sound(1, None, 12000, 255, -1, 1, 2, None).unwrap();
        let h2 = g.add_ambient_sound(2, None, 12000, 255, -1, 0, 0, None).unwrap();
        // Head insertion: newest first.
        assert_eq!(g.pcaudio.ambient_sound_list.iter().map(|a| a.handle).collect::<Vec<_>>(), vec![h2, h1]);

        let mut rng = g.rng;
        g.act_frame_count = 5;
        g.update_ambient_sounds();
        let r = rng.random(120);
        assert_eq!(g.rng, rng, "one Random(randFreq*60) per due ambient with randFreq");
        assert_eq!(g.pcaudio.ambient_sound_list[1].counter, 5 + 60 + r);
        assert_eq!(g.pcaudio.ambient_sound_list[0].counter, 5);
        assert!(matches!(g.effects[0], Effect::PlaySample { uid: 2, handle, .. } if handle == h2));
        assert!(matches!(g.effects[1], Effect::PlaySample { uid: 1, handle, .. } if handle == h1));

        // The looping one now has a buffer (origin is within range of the camera at 0).
        g.effects.clear();
        g.act_frame_count = 6;
        g.update_ambient_sounds();
        assert!(matches!(g.effects[0], Effect::UpdateHandle { handle, .. } if handle == h2));
        assert_eq!(g.effects.len(), 1);

        g.effects.clear();
        g.free_ambient_sound_list();
        assert!(g.pcaudio.ambient_sound_list.is_empty());
        assert!(g.effects.contains(&Effect::StopHandle { handle: h2 }));
    }

    #[test]
    fn out_of_range_loop_retries() {
        let mut g = game_with(&[(2, true)]);
        let far = SVector { vx: 30000, vy: 0, vz: 0, pad: 0 };
        g.add_ambient_sound(2, Some(far), 12000, 255, -1, 0, 0, None).unwrap();
        g.update_ambient_sounds();
        g.update_ambient_sounds();
        assert!(g.effects.iter().all(|e| matches!(e, Effect::PlaySample { .. })));
        assert!(!g.pcaudio.ambient_sound_list[0].has_buffer);
    }
}
