//! Port of `Common/DefData.c` / `DefData.h`: default high-score and save-slot tables.
//!
//! Dead data in retail: nothing in the PC (or PSX) source references `scoreName`,
//! `hiScoreData` or `saveSlots` (the PC high scores and saves are `savegame.c`'s). Kept for
//! completeness of the port.

/// `MAX_SAVE_SLOTS`.
pub const MAX_SAVE_SLOTS: usize = 5;
/// `MAX_HISCORE_SLOTS`.
pub const MAX_HISCORE_SLOTS: usize = 5;

/// `SAVE_SLOT` (`DefData.h`): `char name[2]; long score;`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveSlot {
    pub name: [u8; 2],
    pub score: i32,
}

/// `SCORENAME` (`DefData.h`): `char name[4]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoreName {
    pub name: [u8; 4],
}

/// `HISCORE` (`DefData.h`): `char name[2]; long score;`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HiScore {
    pub name: [u8; 2],
    pub score: i32,
}

const JAM: ScoreName = ScoreName { name: *b"jam\0" };

/// `scoreName[MAX_HISCORE_SLOTS]` (DefData.c:16).
pub const SCORE_NAME: [ScoreName; MAX_HISCORE_SLOTS] = [JAM; MAX_HISCORE_SLOTS];

/// `hiScoreData[MAX_HISCORE_SLOTS]` (DefData.c:25).
pub const HI_SCORE_DATA: [HiScore; MAX_HISCORE_SLOTS] = [
    HiScore { name: [0; 2], score: 10000 },
    HiScore { name: [0; 2], score: 9000 },
    HiScore { name: [0; 2], score: 8000 },
    HiScore { name: [0; 2], score: 7000 },
    HiScore { name: [0; 2], score: 6000 },
];

/// `saveSlots[MAX_SAVE_SLOTS]` (DefData.c:34).
pub const SAVE_SLOTS: [SaveSlot; MAX_SAVE_SLOTS] = [SaveSlot { name: [0; 2], score: 0 }; MAX_SAVE_SLOTS];
