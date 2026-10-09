//! Requests from the simulation to the platform layer.
//!
//! Where the original called platform services directly (`PlaySample`,
//! `PrepareSong`, `StartVideoPlayback`, `SaveGame`, vibration, ...), the port
//! pushes an [`Effect`] onto `Game::effects`. The host drains the queue after
//! every tick. Effects never feed back into simulation state, which keeps
//! `f2-game` deterministic and headless.

use f2_math::SVector;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    /// `PlaySample(sample, pos, radius, volume, pitch)` (`PC/pcaudio.c`). `uid` = sample CRC.
    /// `handle != 0` (ambient sounds, `crate::pcaudio`): bind the started voice to
    /// `handle` for later `UpdateHandle`/`StopHandle`.
    PlaySample {
        uid: u32,
        pos: Option<SVector>,
        radius: i32,
        volume: i32,
        pitch: i32,
        handle: u32,
    },
    /// `StopSample`.
    StopSample {
        uid: u32,
    },
    /// No longer emitted: `PlaySfxMappedSample` is simulated in `crate::pcaudio` (kept for
    /// host compatibility). // ambient
    /// `StartAnim` → `PlaySfxMappedSample(act)` (`PC/pcaudio.c:425`): an actor started
    /// animation `anim` whose sfx mapping names sample `uid`. If the sample loops the
    /// audio layer adds it as an ambient sound once per (actor, anim) (`loopFlags`),
    /// otherwise plays it at `pos` with `DEFAULT_SFX_DIST`/`SAMPLE_VOLUME`.
    PlaySfxMapped {
        actor: crate::ids::ActorId,
        uid: u32,
        anim: i16,
        pos: SVector,
    },
    /// `PlayVoice(pl, name)` (`PC/pcaudio.h:210`): `PlaySample(FindVoice(UpdateCRC(name), pl),
    /// NULL, 0, SAMPLE_VOLUME, -1)`; the audio layer picks the player character's voice bank.
    PlayVoice {
        player: u8,
        uid: u32,
    },
    /// Stop a specific playing voice (looping/ambient sounds).
    StopHandle {
        handle: u32,
    },
    /// Change volume/pitch/position of a playing voice (`UpdateLoopingSample`;
    /// `radius` 0 = `DEFAULT_SFX_DIST`).
    UpdateHandle {
        handle: u32,
        volume: i32,
        pitch: i32,
        pos: Option<SVector>,
        radius: i32, // ambient
    },
    /// `PrepareSong(track, loop)`: CD track number (2–12), see `f2_import::MUSIC_TRACKS`.
    PlayMusic {
        track: u32,
        looped: bool,
    },
    StopMusic,
    /// `StartVideoPlayback(name)` (`Video\<name>.bik`).
    PlayVideo {
        name: String,
    },
    /// Persist `savegame.dat` (2112-byte block, `Common/savegame.c`).
    SaveGame {
        data: Vec<u8>,
    },
    /// Pad vibration request (PSX code paths; optional on PC).
    Rumble {
        player: u8,
        strength: u8,
        frames: u16,
    },
    /// Debug text the original printed with `utilPrintf`.
    Log {
        text: String,
    },
}

impl crate::Game {
    /// `PlaySample(sample, pos, radius, volume, pitch)` (`PC/pcaudio.c`). A NULL sample
    /// (uid 0) plays nothing.
    pub fn play_sample(&mut self, uid: u32, pos: Option<SVector>, radius: i32, volume: i32, pitch: i32) {
        if uid != 0 {
            self.effects.push(Effect::PlaySample { uid, pos, radius, volume, pitch, handle: 0 });
        }
    }
}
