//! Small formats: demo input recordings and the save game.

use crate::read::{Reader, Result};

/// One record of `Demos/record-<w>-<l>.key` (`Common/levplay.c`):
/// from `frame` on, `player`'s pad reads `buttons`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyRecord {
    pub frame: u32,
    pub buttons: u32,
    pub player: u32,
}

pub fn parse_key_file(data: &[u8]) -> Result<Vec<KeyRecord>> {
    let mut r = Reader::new("key", data);
    if !data.len().is_multiple_of(12) {
        return Err(r.err(format!("size {} not a multiple of 12", data.len())));
    }
    let mut v = Vec::with_capacity(data.len() / 12);
    while !r.is_empty() {
        v.push(KeyRecord { frame: r.u32()?, buttons: r.u32()?, player: r.u32()? });
    }
    Ok(v)
}

pub fn write_key_file(recs: &[KeyRecord]) -> Vec<u8> {
    let mut v = Vec::with_capacity(recs.len() * 12);
    for k in recs {
        v.extend_from_slice(&k.frame.to_le_bytes());
        v.extend_from_slice(&k.buttons.to_le_bytes());
        v.extend_from_slice(&k.player.to_le_bytes());
    }
    v
}

pub const SAVE_SIZE: usize = 2112;
pub const SAVE_LEVELS: usize = 64;

/// `SAVEGAME_LEVELINFO` (32 bytes).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct LevelInfo {
    pub flags: u8,
    pub garibs: u8,
    pub res: [u8; 2],
    pub winner: [u8; 8],
    pub best_time: u32,
    pub res2: [u8; 16],
}

/// serde support for arrays longer than 32 (serialised as sequences).
#[cfg(feature = "serde")]
mod big_array {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: Serialize, const N: usize>(a: &[T; N], s: S) -> Result<S::Ok, S::Error> {
        a.as_slice().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>, const N: usize>(d: D) -> Result<[T; N], D::Error> {
        let v = Vec::<T>::deserialize(d)?;
        let n = v.len();
        v.try_into().map_err(|_| serde::de::Error::invalid_length(n, &"fixed-size array"))
    }
}

/// `savedata/savegame.dat` (`Common/savegame.h`). Kept byte-exact for
/// round-tripping; unknown/reserved bytes are preserved.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveGame {
    pub version: u8,
    pub version_check: u8,
    pub story_world: u8,
    pub story_level: u8,
    pub name: [u8; 8],
    pub language: u8,
    pub controller: u8,
    pub music_vol: u8,
    pub sfx_vol: u8,
    pub char_unlock: u8,
    pub lives: u8,
    pub res: [u8; 2],
    pub flags: [u8; 32],
    pub res2: [u8; 12],
    #[cfg_attr(feature = "serde", serde(with = "big_array"))]
    pub levels: [LevelInfo; SAVE_LEVELS],
}

impl SaveGame {
    pub fn parse(data: &[u8]) -> Result<SaveGame> {
        let mut r = Reader::new("savegame", data);
        if data.len() < SAVE_SIZE {
            return Err(r.err(format!("save is {} bytes, expected {SAVE_SIZE}", data.len())));
        }
        let arr = |n: usize, r: &mut Reader| -> Result<Vec<u8>> { Ok(r.bytes(n)?.to_vec()) };
        let version = r.u8()?;
        let version_check = r.u8()?;
        let story_world = r.u8()?;
        let story_level = r.u8()?;
        let name = arr(8, &mut r)?.try_into().unwrap();
        let language = r.u8()?;
        let controller = r.u8()?;
        let music_vol = r.u8()?;
        let sfx_vol = r.u8()?;
        let char_unlock = r.u8()?;
        let lives = r.u8()?;
        let res = arr(2, &mut r)?.try_into().unwrap();
        let flags = arr(32, &mut r)?.try_into().unwrap();
        let res2 = arr(12, &mut r)?.try_into().unwrap();
        let mut levels = [LevelInfo::default(); SAVE_LEVELS];
        for l in levels.iter_mut() {
            l.flags = r.u8()?;
            l.garibs = r.u8()?;
            l.res = arr(2, &mut r)?.try_into().unwrap();
            l.winner = arr(8, &mut r)?.try_into().unwrap();
            l.best_time = r.u32()?;
            l.res2 = arr(16, &mut r)?.try_into().unwrap();
        }
        Ok(SaveGame {
            version,
            version_check,
            story_world,
            story_level,
            name,
            language,
            controller,
            music_vol,
            sfx_vol,
            char_unlock,
            lives,
            res,
            flags,
            res2,
            levels,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(SAVE_SIZE);
        v.extend_from_slice(&[self.version, self.version_check, self.story_world, self.story_level]);
        v.extend_from_slice(&self.name);
        v.extend_from_slice(&[self.language, self.controller, self.music_vol, self.sfx_vol, self.char_unlock, self.lives]);
        v.extend_from_slice(&self.res);
        v.extend_from_slice(&self.flags);
        v.extend_from_slice(&self.res2);
        for l in &self.levels {
            v.extend_from_slice(&[l.flags, l.garibs]);
            v.extend_from_slice(&l.res);
            v.extend_from_slice(&l.winner);
            v.extend_from_slice(&l.best_time.to_le_bytes());
            v.extend_from_slice(&l.res2);
        }
        debug_assert_eq!(v.len(), SAVE_SIZE);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_round_trip() {
        let mut d = vec![0u8; SAVE_SIZE];
        for (i, b) in d.iter_mut().enumerate() {
            *b = (i * 31) as u8;
        }
        assert_eq!(SaveGame::parse(&d).unwrap().to_bytes(), d);
    }
    #[test]
    fn key_round_trip() {
        let k = vec![KeyRecord { frame: 5, buttons: 0x20, player: 0 }];
        assert_eq!(parse_key_file(&write_key_file(&k)).unwrap(), k);
    }
}
