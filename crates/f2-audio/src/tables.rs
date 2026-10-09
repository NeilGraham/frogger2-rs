//! Constants and tables from `PC/pcaudio.c`, plus the DirectSound unit conversions.

/// `MAX_AMBIENT_SFX` (pcaudio.c:36). Not enforced by the original either.
pub const MAX_AMBIENT_SFX: usize = 50;
/// `DEFAULT_SFX_DIST` (pcaudio.c:37): attenuation radius used when a caller passes 0.
pub const DEFAULT_SFX_DIST: i32 = 7000;
/// `MAX_SOUND_VOL` (Common/menus.h:57): range of `globalSoundVol` / `globalMusicVol`.
pub const MAX_SOUND_VOL: i32 = 100;
/// `SAMPLE_VOLUME` / `AMBIENT_VOLUME` defaults (pcaudio.c:43-44).
pub const SAMPLE_VOLUME: i32 = 255;
pub const AMBIENT_VOLUME: i32 = 255;
/// `DSBFREQUENCY_MAX` (dsound.h) and `PITCH_STEP` (pcaudio.c:41).
pub const DSBFREQUENCY_MIN: u32 = 100;
pub const DSBFREQUENCY_MAX: u32 = 100_000;
pub const PITCH_STEP: u32 = DSBFREQUENCY_MAX / 256;
/// DirectSound silence (`DSBVOLUME_MIN`), hundredths of a dB.
pub const DSBVOLUME_MIN: i32 = -10000;

/// `bytetoDB` (pcaudio.c:91): 0..255 linear-ish volume to DirectSound hundredths of dB.
///
/// BUG(original): entry 238 is `100` (a missing minus sign). As a volume DirectSound rejects
/// a positive value (`SetVolume` fails and the buffer keeps its previous volume); as a pan it is
/// a valid, mirrored pan. Both effects are reproduced by [`crate::mixer`] callers.
#[rustfmt::skip]
pub const BYTE_TO_DB: [i32; 256] = [
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

/// `bytetoDB[vol]` with the index clamped to the table (the original indexes unchecked).
pub fn byte_to_db(vol: i32) -> i32 {
    BYTE_TO_DB[vol.clamp(0, 255) as usize]
}

/// The DirectSound pan value the original passes to `SetPan` for a game pan of -255..=255
/// (pcaudio.c:378-381).
pub fn ds_pan(pan: i32) -> i32 {
    let pan = pan.clamp(-255, 255);
    if pan <= 0 {
        -BYTE_TO_DB[(255 + pan) as usize]
    } else {
        BYTE_TO_DB[(255 - pan) as usize]
    }
}

/// DirectSound attenuation (hundredths of dB, <= 0) to a linear amplitude factor.
pub fn db_to_gain(db: i32) -> f32 {
    if db <= DSBVOLUME_MIN {
        0.0
    } else {
        10f32.powf(db.min(0) as f32 / 2000.0)
    }
}

/// DirectSound pan law: a positive pan attenuates the left channel, a negative one the right.
/// Returns linear `(left, right)` factors.
pub fn pan_gains(ds_pan: i32) -> (f32, f32) {
    let ds_pan = ds_pan.clamp(-10000, 10000);
    if ds_pan > 0 {
        (db_to_gain(-ds_pan), 1.0)
    } else {
        (1.0, db_to_gain(ds_pan))
    }
}

/// The playback frequency `SetFrequency` ends up with for a game pitch (pcaudio.c:375).
/// `-1` (and `0`, which equals `DSBFREQUENCY_ORIGINAL`) keep the sample's own rate; an
/// out-of-range value makes `SetFrequency` fail, so the previous rate (`current`) stays.
pub fn pitch_to_freq(pitch: i16, original: u32, current: u32) -> u32 {
    if pitch == -1 {
        return original;
    }
    let f = (pitch as i32).wrapping_mul(PITCH_STEP as i32) as u32;
    if f == 0 {
        original
    } else if (DSBFREQUENCY_MIN..=DSBFREQUENCY_MAX).contains(&f) {
        f
    } else {
        current
    }
}

/// Port of `utilSqrt` (PC/islutil.cpp:38): float exponent-halving approximation, 16.16 result.
pub fn util_sqrt(num: u32) -> u32 {
    let n = num as f32;
    let bits = (n.to_bits() as i32).wrapping_sub(0x3F80_0000) >> 1;
    let r = f32::from_bits(bits.wrapping_add(0x3F80_0000) as u32);
    // MSVC `(unsigned long)float` goes through a 64-bit _ftol; truncates toward zero.
    (r * 65536.0) as i64 as u32
}

/// Port of `Fsqrt` (Common/newpsx.h:10).
pub fn fsqrt(num: u32) -> i32 {
    (util_sqrt(num) >> 4) as i32
}

// ---- CD tracks -------------------------------------------------------------------------------

/// CD audio track numbers (pcaudio.c:128-145). Ripped tracks live at `music/trackNN.wav`.
pub mod track {
    pub const NOTRACK: u32 = 0;
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
    pub const NUM_CD_TRACKS: u32 = 13;
}

/// World ids (Common/layout.h:235) and the extra song ids (pcaudio.h:87-89) `PrepareSong` takes.
pub mod song {
    pub const WORLDID_GARDEN: i16 = 0;
    pub const WORLDID_ANCIENT: i16 = 1;
    pub const WORLDID_SPACE: i16 = 2;
    pub const WORLDID_CITY: i16 = 3;
    pub const WORLDID_SUBTERRANEAN: i16 = 4;
    pub const WORLDID_LABORATORY: i16 = 5;
    pub const WORLDID_HALLOWEEN: i16 = 6;
    pub const WORLDID_SUPERRETRO: i16 = 7;
    pub const WORLDID_FRONTEND: i16 = 8;
    pub const WORLDID_TEST: i16 = 9;
    pub const AUDIOTRK_GAMEOVER: i16 = 100;
    pub const AUDIOTRK_LEVELCOMPLETE: i16 = 101;
    pub const AUDIOTRK_LEVELCOMPLETELOOP: i16 = 102;
}

/// Track selection of `PrepareSong` (pcaudio.c:949). City deliberately uses the Super Retro track.
pub fn song_track(world: i16) -> Option<u32> {
    use song::*;
    Some(match world {
        WORLDID_GARDEN => track::GARDEN,
        WORLDID_ANCIENT => track::ANCIENTS,
        WORLDID_SPACE => track::SPACE,
        WORLDID_CITY => track::SUPERRETRO,
        WORLDID_SUBTERRANEAN => track::SUBTERRANEAN,
        WORLDID_LABORATORY => track::LABORATORY,
        WORLDID_HALLOWEEN => track::HALLOWEEN,
        WORLDID_SUPERRETRO => track::SUPERRETRO,
        WORLDID_FRONTEND => track::FRONTEND,
        AUDIOTRK_GAMEOVER => track::GAMEOVER,
        AUDIOTRK_LEVELCOMPLETE => track::LEVELCOMPLETE,
        AUDIOTRK_LEVELCOMPLETELOOP => track::LEVELCOMPLETELOOP,
        _ => return None,
    })
}

/// Sound bank file for a world (`LoadSfx`, pcaudio.c:232).
pub fn world_bank(world: i16) -> Option<&'static str> {
    use song::*;
    Some(match world {
        WORLDID_GARDEN => "garden.wus",
        WORLDID_ANCIENT => "ancients.wus",
        WORLDID_SPACE => "space.wus",
        WORLDID_CITY => "city.wus",
        WORLDID_SUBTERRANEAN => "sub.wus",
        WORLDID_LABORATORY => "lab.wus",
        WORLDID_HALLOWEEN => "halloween.wus",
        WORLDID_SUPERRETRO => "superretro.wus",
        WORLDID_FRONTEND => "frontend.wus",
        WORLDID_TEST => "test.wus",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn util_sqrt_matches_bit_hack() {
        assert_eq!(util_sqrt(1), 65536);
        assert_eq!(util_sqrt(4), 2 * 65536);
        assert_eq!(util_sqrt(16), 4 * 65536);
        // Approximation: sqrt(2) ~ 1.5 with this hack.
        assert_eq!(util_sqrt(2), 98304);
        assert_eq!(fsqrt(4), 2 * 4096);
    }

    #[test]
    fn pan_law_and_bug_entry() {
        assert_eq!(ds_pan(0), 0);
        assert_eq!(ds_pan(255), -10000);
        assert_eq!(ds_pan(-255), 10000);
        // BUG(original) entry: pan 17 -> bytetoDB[238] = +100.
        assert_eq!(ds_pan(17), 100);
        assert_eq!(pan_gains(0), (1.0, 1.0));
        let (l, r) = pan_gains(ds_pan(-255));
        assert_eq!((l, r), (0.0, 1.0));
        assert!((db_to_gain(-600) - 0.501).abs() < 0.01);
    }

    #[test]
    fn pitch() {
        assert_eq!(pitch_to_freq(-1, 22050, 22050), 22050);
        assert_eq!(pitch_to_freq(0, 22050, 22050), 22050);
        assert_eq!(pitch_to_freq(128, 22050, 22050), 128 * 390);
        assert_eq!(pitch_to_freq(300, 22050, 11025), 11025);
    }
}
