//! Game audio on the libdragon mixer: the simulation's sound [`Effect`]s become wav64
//! voices (VADPCM, streamed from `sfx/<bank>/<uid>.wav64`, see `tools/f2-n64prep`) with the
//! PC's positional volume / pan / pitch maths, and CD tracks become `music/trackNN.wav64`.
//!
//! The maths is a copy of `f2-audio`'s `positional` / `pcaudio.c` tables (that crate needs
//! `std`): `PlaySample` (pcaudio.c:296) and `UpdateLoopingSample` (pcaudio.c:464).
//!
//! [`Effect`]: f2_game::Effect

use crate::ffi::Voice;

/// `DEFAULT_SFX_DIST` (pcaudio.c:37).
pub const DEFAULT_SFX_DIST: i32 = 7000;
/// `MAX_SOUND_VOL` (menus.h:57).
pub const MAX_SOUND_VOL: i32 = 100;
/// `SAMPLE_VOLUME` (pcaudio.c:43).
pub const SAMPLE_VOLUME: i32 = 255;
const DSBFREQUENCY_MIN: u32 = 100;
const DSBFREQUENCY_MAX: u32 = 100_000;
const PITCH_STEP: u32 = DSBFREQUENCY_MAX / 256;
/// The mixer channels' frequency limit (csrc/main.c `mixer_ch_set_limits`).
pub const MAX_FREQ: u32 = 128_000;

/// `bytetoDB` (pcaudio.c:91), see `f2_audio::tables::BYTE_TO_DB` (incl. the BUG(original)
/// `100` at 238).
#[rustfmt::skip]
const BYTE_TO_DB: [i16; 256] = [
    -10000,-7992,-6992,-6407,-5992,-5670,-5407,-5185,-4992,-4823,-4671,-4533,-4408,-4292,-4185,-4086,
    -3993,-3905,-3823,-3745,-3671,-3601,-3533,-3470,-3408,-3350,-3293,-3239,-3186,-3135,-3086,-3039,
    -2993,-2949,-2906,-2864,-2823,-2784,-2745,-2708,-2671,-2636,-2601,-2567,-2534,-2501,-2470,-2439,
    -2408,-2378,-2349,-2321,-2293,-2265,-2238,-2212,-2186,-2160,-2135,-2111,-2086,-2063,-2039,-2016,
    -1993,-1971,-1949,-1927,-1906,-1885,-1864,-1844,-1823,-1804,-1785,-1765,-1745,-1727,-1708,-1690,
    -1671,-1654,-1636,-1618,-1601,-1584,-1567,-1550,-1534,-1518,-1502,-1486,-1470,-1454,-1439,-1424,
    -1408,-1394,-1379,-1364,-1350,-1335,-1321,-1307,-1293,-1279,-1266,-1252,-1239,-1225,-1212,-1199,
    -1186,-1173,-1161,-1148,-1136,-1123,-1111,-1099,-1087,-1075,-1063,-1051,-1039,-1028,-1016,-1005,
    -994,-982,-971,-960,-950,-938,-928,-917,-906,-896,-885,-875,-864,-854,-844,-834,
    -824,-814,-804,-794,-784,-774,-765,-755,-746,-736,-727,-718,-708,-699,-690,-681,
    -672,-663,-654,-645,-636,-627,-619,-610,-601,-593,-585,-576,-568,-560,-551,-543,
    -535,-527,-518,-510,-502,-494,-486,-478,-471,-463,-455,-447,-440,-432,-424,-417,
    -409,-402,-394,-387,-380,-372,-365,-358,-350,-343,-336,-329,-322,-315,-308,-301,
    -294,-287,-280,-273,-266,-260,-253,-246,-240,-233,-226,-220,-213,-206,-200,-193,
    -187,-181,-174,-168,-161,-155,-149,-142,-136,-130,-124,-118,-112,-106,100,-93,
    -87,-81,-75,-70,-64,-58,-52,-46,-40,-34,-28,-22,-17,-11,-5,0,
];

fn byte_to_db(vol: i32) -> i32 {
    BYTE_TO_DB[vol.clamp(0, 255) as usize] as i32
}

fn ds_pan(pan: i32) -> i32 {
    let pan = pan.clamp(-255, 255);
    if pan <= 0 {
        -byte_to_db(255 + pan)
    } else {
        byte_to_db(255 - pan)
    }
}

fn db_to_gain(db: i32) -> f32 {
    if db <= -10000 {
        0.0
    } else {
        libm::powf(10.0, db.min(0) as f32 / 2000.0)
    }
}

fn pan_gains(ds_pan: i32) -> (f32, f32) {
    let p = ds_pan.clamp(-10000, 10000);
    if p > 0 {
        (db_to_gain(-p), 1.0)
    } else {
        (1.0, db_to_gain(p))
    }
}

/// `SetFrequency` for a game pitch (pcaudio.c:375): -1/0 = native, out of range = keep.
pub fn pitch_to_freq(pitch: i32, original: u32, current: u32) -> u32 {
    if pitch == -1 {
        return original;
    }
    let f = pitch.wrapping_mul(PITCH_STEP as i32) as u32;
    if f == 0 {
        original
    } else if (DSBFREQUENCY_MIN..=DSBFREQUENCY_MAX).contains(&f) {
        f
    } else {
        current
    }
}

/// `Fsqrt` (newpsx.h:10) via `utilSqrt` (islutil.cpp:38).
fn fsqrt(num: u32) -> i32 {
    let n = num as f32;
    let bits = (n.to_bits() as i32).wrapping_sub(0x3F80_0000) >> 1;
    let r = f32::from_bits(bits.wrapping_add(0x3F80_0000) as u32);
    (((r * 65536.0) as i64 as u32) >> 4) as i32
}

/// The camera: `currCamSource` (20.12) and the view matrix (row vectors).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub cam_source: [i32; 3],
    pub view: [[f32; 4]; 4],
}

impl Default for Listener {
    fn default() -> Self {
        let mut view = [[0.0; 4]; 4];
        for (i, r) in view.iter_mut().enumerate() {
            r[i] = 1.0;
        }
        Listener { cam_source: [0; 3], view }
    }
}

/// Distance, attenuation radius and pan (-255..=255) of a sound at `pos` (pcaudio.c:323).
pub fn positional(pos: [i16; 3], radius: i32, l: &Listener) -> (f32, f32, i32) {
    let att = if radius != 0 { radius as f32 } else { DEFAULT_SFX_DIST as f32 };
    let d: [i32; 3] = core::array::from_fn(|i| (pos[i] as i32).wrapping_sub(l.cam_source[i] >> 12) as i16 as i32);
    let sum = d.iter().fold(0i32, |a, &x| a.wrapping_add(x.wrapping_mul(x)));
    let dist = fsqrt(sum as u32) as f32 / 4096.0;
    let p = [(pos[0] as f64 / 10.0) as f32, (pos[1] as f64 / 10.0) as f32, (pos[2] as f64 / 10.0) as f32];
    let m = &l.view;
    let sx = m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0];
    let sy = m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1];
    let sz = m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2];
    let sd = libm::sqrtf(sx * sx + sy * sy + sz * sz);
    let pan = if sd != 0.0 { ((sx * 255.0) / sd) as i32 } else { 0 };
    (dist, att, pan)
}

/// Mixer voice for game volume 0..=255 (already scaled by the sfx volume), optional
/// position, and frequency. `None` = inaudible (out of range / zero volume).
pub fn voice(vol: i32, pos: Option<[i16; 3]>, radius: i32, l: &Listener, freq: u32) -> Option<Voice> {
    let mut vol = vol;
    let mut pan = 0;
    if let Some(p) = pos.filter(|p| *p != [0; 3]) {
        let (dist, att, pn) = positional(p, radius, l);
        if dist > att {
            return None;
        }
        vol = (vol as f32 * ((att - dist) / att)) as i32;
        pan = pn;
    }
    if vol <= 0 {
        return None;
    }
    let g = db_to_gain(byte_to_db(vol));
    let (lg, rg) = pan_gains(ds_pan(pan));
    Some(Voice { lvol: lg * g, rvol: rg * g, freq: freq as f32 })
}

#[cfg(target_vendor = "nintendo64")]
pub use mixer::Audio;

#[cfg(target_vendor = "nintendo64")]
mod mixer {
    use super::*;
    use crate::rt;
    use alloc::collections::BTreeMap;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use crate::data::SfxRef;

    #[derive(Clone, Copy)]
    struct Sfx {
        handle: u32,
        rate: u32,
    }

    #[derive(Clone, Copy)]
    struct Chan {
        id: u32,
        uid: u32,
        rate: u32,
        freq: u32,
    }

    /// Voices on the mixer's sound-effect channels plus the music stream.
    pub struct Audio {
        first: u32,
        chans: Vec<Option<Chan>>,
        samples: BTreeMap<u32, Sfx>,
        voice_banks: [BTreeMap<u32, Sfx>; 4],
        open: Vec<u32>,
        by_path: BTreeMap<String, u32>,
        pub listener: Listener,
        pub sfx_volume: i32,
        pub music_volume: i32,
        next_id: u32,
        /// Effect handle (ambient sounds) → voice id.
        handles: BTreeMap<u32, u32>,
        pub plays: u32,
        pub missing: u32,
        paused: bool,
        /// CD track last started.
        pub music_track: Option<u32>,
    }

    impl Default for Audio {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Audio {
        pub fn new() -> Audio {
            let (first, n) = rt::sfx_channels();
            Audio {
                first,
                chans: alloc::vec![None; n as usize],
                samples: BTreeMap::new(),
                voice_banks: Default::default(),
                open: Vec::new(),
                by_path: BTreeMap::new(),
                listener: Listener::default(),
                sfx_volume: MAX_SOUND_VOL,
                music_volume: MAX_SOUND_VOL,
                next_id: 1,
                handles: BTreeMap::new(),
                plays: 0,
                missing: 0,
                paused: false,
                music_track: None,
            }
        }

        /// Stops every voice and closes every sample (level change).
        pub fn free_samples(&mut self) {
            for (i, c) in self.chans.iter_mut().enumerate() {
                if c.take().is_some() {
                    rt::ch_stop(self.first + i as u32);
                }
            }
            for h in self.open.drain(..) {
                rt::sfx_close(h);
            }
            self.by_path.clear();
            self.samples.clear();
            for v in &mut self.voice_banks {
                v.clear();
            }
            self.handles.clear();
        }

        fn open_bank(&mut self, refs: &[SfxRef]) -> BTreeMap<u32, Sfx> {
            let mut out = BTreeMap::new();
            for s in refs {
                // A sample shared by several banks (the pack dedupes them) is opened once.
                let h = match self.by_path.get(&s.path) {
                    Some(&h) => h,
                    None => {
                        let h = rt::sfx_open(&s.path, s.looped);
                        rt::audio_poll();
                        if h == 0 {
                            crate::log!("audio: {} missing", s.path);
                            continue;
                        }
                        self.open.push(h);
                        self.by_path.insert(s.path.clone(), h);
                        h
                    }
                };
                // The file's own rate (Opus is stored at 48 kHz whatever the source rate).
                let rate = match rt::sfx_freq(h) {
                    0 => s.rate,
                    f => f,
                };
                out.entry(s.uid).or_insert(Sfx { handle: h, rate });
            }
            out
        }

        /// `LoadSfxSet` of a bank.
        pub fn load_bank(&mut self, refs: &[SfxRef]) {
            let m = self.open_bank(refs);
            for (k, v) in m {
                self.samples.entry(k).or_insert(v);
            }
        }

        pub fn load_voice_bank(&mut self, player: usize, refs: &[SfxRef]) {
            let m = self.open_bank(refs);
            if let Some(v) = self.voice_banks.get_mut(player) {
                *v = m;
            }
        }

        pub fn knows(&self, uid: u32) -> bool {
            self.samples.contains_key(&uid)
        }

        fn refresh(&mut self) {
            for (i, c) in self.chans.iter_mut().enumerate() {
                if c.is_some() && !rt::ch_playing(self.first + i as u32) {
                    *c = None;
                }
            }
        }

        fn start(&mut self, s: Sfx, uid: u32, v: &mut Voice, pitch: i32) -> Option<u32> {
            self.refresh();
            // A free channel, else steal the oldest voice.
            let i = match self.chans.iter().position(|c| c.is_none()) {
                Some(i) => i,
                None => self.chans.iter().enumerate().min_by_key(|(_, c)| c.map_or(0, |c| c.id)).map(|(i, _)| i)?,
            };
            let freq = pitch_to_freq(pitch, s.rate, s.rate).min(MAX_FREQ);
            v.freq = freq as f32;
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            rt::sfx_play(s.handle, self.first + i as u32, v);
            self.chans[i] = Some(Chan { id, uid, rate: s.rate, freq });
            self.plays += 1;
            Some(id)
        }

        /// `PlaySample`; returns the voice id.
        pub fn play(&mut self, uid: u32, volume: i32, pitch: i32, pos: Option<[i16; 3]>, radius: i32) -> Option<u32> {
            let Some(&s) = self.samples.get(&uid) else {
                self.missing += 1;
                return None;
            };
            let vol = volume.wrapping_mul(self.sfx_volume) / MAX_SOUND_VOL;
            let mut v = voice(vol, pos, radius, &self.listener, 0)?;
            self.start(s, uid, &mut v, pitch)
        }

        pub fn play_voice(&mut self, player: usize, uid: u32) -> Option<u32> {
            let s = *self.voice_banks.get(player)?.get(&uid)?;
            let vol = SAMPLE_VOLUME * self.sfx_volume / MAX_SOUND_VOL;
            let mut v = voice(vol, None, 0, &self.listener, 0)?;
            self.start(s, uid, &mut v, -1)
        }

        fn chan_of(&self, id: u32) -> Option<usize> {
            self.chans.iter().position(|c| c.is_some_and(|c| c.id == id))
        }

        pub fn bind(&mut self, handle: u32, id: u32) {
            self.handles.insert(handle, id);
        }

        pub fn stop_handle(&mut self, handle: u32) {
            if let Some(id) = self.handles.remove(&handle) {
                if let Some(i) = self.chan_of(id) {
                    rt::ch_stop(self.first + i as u32);
                    self.chans[i] = None;
                }
            }
        }

        pub fn stop_sample(&mut self, uid: u32) {
            for (i, c) in self.chans.iter_mut().enumerate() {
                if c.is_some_and(|c| c.uid == uid) {
                    rt::ch_stop(self.first + i as u32);
                    *c = None;
                }
            }
        }

        /// `UpdateLoopingSample`.
        pub fn update_handle(&mut self, handle: u32, volume: i32, pitch: i32, pos: Option<[i16; 3]>, radius: i32) {
            let Some(&id) = self.handles.get(&handle) else { return };
            let Some(i) = self.chan_of(id) else { return };
            let c = self.chans[i].unwrap();
            let vol = volume.wrapping_mul(self.sfx_volume) / MAX_SOUND_VOL;
            let freq = pitch_to_freq(pitch, c.rate, c.freq).min(MAX_FREQ);
            let v = voice(vol, pos, radius, &self.listener, freq).unwrap_or(Voice { lvol: 0.0, rvol: 0.0, freq: freq as f32 });
            self.chans[i] = Some(Chan { freq, ..c });
            rt::ch_set(self.first + i as u32, &v);
        }

        pub fn play_music(&mut self, track: u32, looped: bool) {
            let p: String = format!("rom:/music/track{track:02}.wav64");
            if rt::music_play(Some(&p), looped) {
                self.music_track = Some(track);
            } else {
                crate::log!("audio: {p} missing");
                self.music_track = None;
            }
            rt::music_vol(self.music_volume as f32 / MAX_SOUND_VOL as f32);
        }

        pub fn stop_music(&mut self) {
            rt::music_play(None, false);
            self.music_track = None;
        }

        /// The music channel is producing samples.
        pub fn music_playing(&self) -> bool {
            rt::ch_playing(0)
        }

        pub fn set_music_volume(&mut self, v: i32) {
            self.music_volume = v.clamp(0, MAX_SOUND_VOL);
            rt::music_vol(self.music_volume as f32 / MAX_SOUND_VOL as f32);
        }

        pub fn pause(&mut self, on: bool) {
            // PauseAudio pauses samples; libdragon has no per-channel pause: stop them.
            if on && !self.paused {
                for (i, c) in self.chans.iter_mut().enumerate() {
                    if c.take().is_some() {
                        rt::ch_stop(self.first + i as u32);
                    }
                }
            }
            self.paused = on;
        }

        pub fn active(&self) -> usize {
            self.chans.iter().filter(|c| c.is_some()).count()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_and_pan() {
        let l = Listener::default();
        let v = voice(255, None, 0, &l, 22050).unwrap();
        assert!((v.lvol - 1.0).abs() < 1e-6 && (v.rvol - 1.0).abs() < 1e-6);
        assert!(voice(0, None, 0, &l, 0).is_none());
        // Far away: inaudible.
        assert!(voice(255, Some([30000, 0, 0]), 1000, &l, 0).is_none());
        // +x in MDX view space is screen left (the row-vector view matrix mirrors x), and the
        // PC maps a positive game pan to a negative DirectSound pan: the left channel wins.
        let v = voice(255, Some([500, 0, 0]), 0, &l, 0).unwrap();
        assert!(v.lvol > v.rvol);
        assert_eq!(pitch_to_freq(-1, 22050, 1), 22050);
        assert_eq!(pitch_to_freq(100, 22050, 1), 100 * 390);
    }
}
