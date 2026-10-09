//! Typed indices replacing the original's raw pointers.
//!
//! The C code links structures with pointers (`GAMETILE*`, `ENEMY*`, …). In
//! the port every such pointer becomes an index into the owning array/arena.
//! `Option<Id>` replaces nullable pointers.

macro_rules! id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub u32);
        impl $name {
            #[inline]
            pub fn idx(self) -> usize {
                self.0 as usize
            }
        }
        impl From<usize> for $name {
            fn from(v: usize) -> Self {
                $name(v as u32)
            }
        }
        impl From<$name> for usize {
            fn from(v: $name) -> usize {
                v.0 as usize
            }
        }
    };
}

id!(/// Index into `Map::tiles` (`GAMETILE*`, `&firstTile[i]`).
    TileId);
id!(/// Index into `Actors` (`ACTOR2*` / its `ACTOR`).
    ActorId);
id!(/// Index into `enemyList.array` (`ENEMY*`).
    EnemyId);
id!(/// Index into `platformList.array` (`PLATFORM*`).
    PlatformId);
id!(/// Index into `garibList.array` (`GARIB*`).
    GaribId);
id!(/// Index into the path store (`PATH*`).
    PathId);
id!(/// Index into the trigger pool (`TRIGGER*`).
    TriggerId);
id!(/// Index into the special-effect pool (`SPECFX*`).
    SpecFxId);
id!(/// Index into the sprite pool (`SPRITE*`).
    SpriteId);
id!(/// Index into the overlay pool (`SPRITEOVERLAY*`).
    OverlayId);
id!(/// Index into the text overlay pool (`TEXTOVERLAY*`).
    TextOverlayId);
id!(/// Index into the transition camera pool (`TRANSCAMERA*`).
    TransCameraId);
id!(/// Index into a sample table (`SAMPLE*`); the value is the sample uid (CRC).
    SampleId);
