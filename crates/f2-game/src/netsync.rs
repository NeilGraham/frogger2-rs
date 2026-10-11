//! Rollback / netplay support (`docs/NETPLAY.md` §6): snapshots, a deterministic state
//! checksum and effect-free resimulation.
//!
//! * **Snapshot** = [`Game::clone`]. Immutable data (models, parsed level files, the data
//!   source, the script buffer) sits behind `Arc`s, so a clone copies only mutable state.
//! * **Checksum** ([`Game::checksum`]): a fixed-key 64-bit hash over a canonical walk of all
//!   gameplay state (the serde field order of every sub-state; floats by bit pattern;
//!   no `HashMap` is walked). Identical on every peer and every run.
//! * **Resimulation** ([`Game::tick_resimulate`]): a rollback replays frames whose effects
//!   (sounds, music, rumble, FMV, saves) were already presented. Those effects are discarded
//!   so the platform layer only ever sees the effects of each frame's first simulation.
//!
//! No gameplay state lives outside [`Game`]: the only process-wide data in the crate are
//! constant tables (`ORIG_WORLD_VISUAL_DATA`, story tables) and the `view.rs` acos/sin
//! table cache (`OnceBox`, a pure function of nothing).

use crate::prelude::*;

use serde::ser::{self, Serialize};

use crate::{Game, TickInput};

/// Visitor of [`Game::walk_state`]: `(sub-state name, feeds the sub-state into a hasher)`.
type StateVisitor<'a> = dyn FnMut(&'static str, &mut dyn FnMut(&mut StateHasher)) + 'a;

impl Game {
    /// A rollback snapshot (a clone; effects not yet drained are left out).
    pub fn snapshot(&self) -> Game {
        let mut g = self.clone();
        g.effects.clear();
        g
    }

    /// Restores a snapshot taken by [`Game::snapshot`]. Pending effects of the current state
    /// are kept (they belong to frames that were already simulated and are still to be
    /// presented).
    pub fn restore(&mut self, s: &Game) {
        let effects = core::mem::take(&mut self.effects);
        self.clone_from(s);
        self.effects = effects;
    }

    /// One tick of a rollback resimulation: like [`Game::tick`], but the effects this tick
    /// produces are discarded (they were presented when the frame was first simulated).
    /// Level loads (`session.level_loads`) are state and are kept.
    pub fn tick_resimulate(&mut self, input: TickInput) {
        let n = self.effects.len();
        self.tick(input);
        self.effects.truncate(n);
    }

    /// Deterministic 64-bit checksum of all gameplay state (see the module docs). Excludes
    /// the effect queue, static data (models, level files, game text, data source) and host
    /// font metrics.
    pub fn checksum(&self) -> u64 {
        let mut h = StateHasher::new();
        self.hash_state(&mut h);
        h.finish()
    }

    /// Per-sub-state checksums `(name, hash)` for desync dumps (which part diverged).
    pub fn checksum_parts(&self) -> Vec<(&'static str, u64)> {
        let mut out = Vec::new();
        self.walk_state(&mut |name, f| {
            let mut h = StateHasher::new();
            f(&mut h);
            out.push((name, h.finish()));
        });
        out
    }

    fn hash_state(&self, h: &mut StateHasher) {
        self.walk_state(&mut |name, f| {
            h.write_bytes(name.as_bytes());
            f(h);
        });
    }

    /// Visits every gameplay sub-state in a fixed order.
    fn walk_state(&self, visit: &mut StateVisitor<'_>) {
        macro_rules! ser {
            ($($name:literal => $e:expr),* $(,)?) => {$(
                visit($name, &mut |h: &mut StateHasher| { let _ = $e.serialize(&mut *h); });
            )*};
        }
        ser! {
            "core" => (self.frame_count, self.act_frame_count, self.last_act_frame_count, self.game_speed, &self.rng),
            "game_state" => &self.game_state,
            "map" => &self.map,
            "actor2" => &self.actor2,
            "general" => &self.general,
            "path" => &self.path,
            "layout" => &self.layout,
            "frogger" => &self.frogger,
            "frogmove" => &self.frogmove,
            "platform" => &self.platform,
            "enemies" => &self.enemies,
            "babyfrog" => &self.babyfrog,
            "collect" => &self.collect,
            "cam" => &self.cam,
            "game" => &self.game,
            "multi" => &self.multi,
            "levplay" => &self.levplay,
            "tongue" => &self.tongue,
            "pcaudio" => &self.pcaudio,
            "frontend" => &self.frontend,
            "training" => &self.training,
            "savegame" => &self.savegame,
            "fadeout" => &self.fadeout,
            "backdrop" => &self.backdrop,
            "sprite" => &self.sprite,
            "particle" => &self.particle,
            "specfx" => &self.specfx,
            "view" => &self.view,
            "overlays" => &self.overlays,
            "hud" => &self.hud,
            "ptexture" => &self.ptexture,
            "menus" => &self.menus,
            "options" => &self.options,
            "charsel" => &self.charsel,
            "enhance" => &self.enhance, // P6-1 options + enemy-speed accumulator
            "modscript" => &self.modscript, // P7b script globals, claims, queued events
        }
        // Sub-states without serde derives: their `Debug` output is a canonical field walk
        // (no maps; floats print their exact shortest round-trip form).
        // Streamed into the hasher (`write_fmt`): no `String` of the whole text (the event
        // state of a big level prints to megabytes, more than an N64 heap holds).
        let s = &self.script;
        visit("memload", &mut |h: &mut StateHasher| h.write_fmt(format_args!("{:?}", self.memload)));
        visit("event", &mut |h: &mut StateHasher| h.write_fmt(format_args!("{:?}", self.event)));
        visit("script", &mut |h: &mut StateHasher| {
            h.write_fmt(format_args!(
                "{:?}{:?}{}{}{}{}",
                s.script_flags,
                s.script_counters,
                s.line_number,
                s.vol_test,
                s.show_end_level_screen,
                s.script_buffer.is_some()
            ))
        });
        visit("textover", &mut |h: &mut StateHasher| {
            h.write_fmt(format_args!("{} {} {:?}", self.textover.num_entries, self.textover.alloc, self.textover.block))
        });
        visit("session", &mut |h: &mut StateHasher| h.write_fmt(format_args!("{}", self.session.level_loads.len())));
    }
}

/// Fixed-key 64-bit streaming hash (no per-process random seed) that is also a minimal
/// `serde::Serializer`: every primitive is fed as a little-endian 64-bit word, strings and
/// byte slices as length + bytes, sequences/maps with their length, enum variants by index.
#[derive(Clone, Debug)]
pub struct StateHasher {
    h: u64,
    n: u64,
}

impl Default for StateHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StateHasher {
    pub fn new() -> Self {
        StateHasher { h: 0x243F_6A88_85A3_08D3, n: 0 }
    }

    #[inline]
    pub fn write_u64(&mut self, v: u64) {
        self.h = (self.h.rotate_left(23) ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.n = self.n.wrapping_add(1);
    }

    pub fn write_bytes(&mut self, b: &[u8]) {
        self.write_u64(b.len() as u64);
        let (chunks, r) = b.as_chunks::<8>();
        for c in chunks {
            self.write_u64(u64::from_le_bytes(*c));
        }
        if !r.is_empty() {
            let mut w = [0u8; 8];
            w[..r.len()].copy_from_slice(r);
            self.write_u64(u64::from_le_bytes(w));
        }
    }

    /// `write_bytes` of the formatted text, without allocating it: one pass counts the
    /// bytes (the length prefix), a second feeds them in 8-byte words.
    pub fn write_fmt(&mut self, args: core::fmt::Arguments<'_>) {
        struct Count(usize);
        impl core::fmt::Write for Count {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                self.0 += s.len();
                Ok(())
            }
        }
        struct Feed<'a> {
            h: &'a mut StateHasher,
            w: [u8; 8],
            fill: usize,
        }
        impl core::fmt::Write for Feed<'_> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                for &b in s.as_bytes() {
                    self.w[self.fill] = b;
                    self.fill += 1;
                    if self.fill == 8 {
                        self.h.write_u64(u64::from_le_bytes(self.w));
                        self.fill = 0;
                    }
                }
                Ok(())
            }
        }
        let mut c = Count(0);
        let _ = core::fmt::write(&mut c, args);
        self.write_u64(c.0 as u64);
        let mut f = Feed { h: self, w: [0; 8], fill: 0 };
        let _ = core::fmt::write(&mut f, args);
        if f.fill != 0 {
            let mut w = [0u8; 8];
            w[..f.fill].copy_from_slice(&f.w[..f.fill]);
            f.h.write_u64(u64::from_le_bytes(w));
        }
    }

    /// splitmix64 finaliser over the state and the word count.
    pub fn finish(&self) -> u64 {
        let mut z = self.h ^ self.n.wrapping_mul(0xD6E8_FEB8_6659_FD93);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Never produced (hashing cannot fail).
#[derive(Debug)]
pub struct HashError;

impl core::fmt::Display for HashError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("hash error")
    }
}
impl core::error::Error for HashError {}
impl ser::Error for HashError {
    fn custom<T: core::fmt::Display>(_msg: T) -> Self {
        HashError
    }
}

type R = Result<(), HashError>;

impl ser::Serializer for &mut StateHasher {
    type Ok = ();
    type Error = HashError;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, v: bool) -> R {
        self.write_u64(v as u64);
        Ok(())
    }
    fn serialize_i8(self, v: i8) -> R {
        self.write_u64(v as i64 as u64);
        Ok(())
    }
    fn serialize_i16(self, v: i16) -> R {
        self.write_u64(v as i64 as u64);
        Ok(())
    }
    fn serialize_i32(self, v: i32) -> R {
        self.write_u64(v as i64 as u64);
        Ok(())
    }
    fn serialize_i64(self, v: i64) -> R {
        self.write_u64(v as u64);
        Ok(())
    }
    fn serialize_i128(self, v: i128) -> R {
        self.write_u64(v as u64);
        self.write_u64((v >> 64) as u64);
        Ok(())
    }
    fn serialize_u8(self, v: u8) -> R {
        self.write_u64(v as u64);
        Ok(())
    }
    fn serialize_u16(self, v: u16) -> R {
        self.write_u64(v as u64);
        Ok(())
    }
    fn serialize_u32(self, v: u32) -> R {
        self.write_u64(v as u64);
        Ok(())
    }
    fn serialize_u64(self, v: u64) -> R {
        self.write_u64(v);
        Ok(())
    }
    fn serialize_u128(self, v: u128) -> R {
        self.write_u64(v as u64);
        self.write_u64((v >> 64) as u64);
        Ok(())
    }
    fn serialize_f32(self, v: f32) -> R {
        self.write_u64(v.to_bits() as u64);
        Ok(())
    }
    fn serialize_f64(self, v: f64) -> R {
        self.write_u64(v.to_bits());
        Ok(())
    }
    fn serialize_char(self, v: char) -> R {
        self.write_u64(v as u64);
        Ok(())
    }
    fn serialize_str(self, v: &str) -> R {
        self.write_bytes(v.as_bytes());
        Ok(())
    }
    fn serialize_bytes(self, v: &[u8]) -> R {
        self.write_bytes(v);
        Ok(())
    }
    fn serialize_none(self) -> R {
        self.write_u64(0);
        Ok(())
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> R {
        self.write_u64(1);
        v.serialize(self)
    }
    fn serialize_unit(self) -> R {
        Ok(())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> R {
        Ok(())
    }
    fn serialize_unit_variant(self, _name: &'static str, idx: u32, _variant: &'static str) -> R {
        self.write_u64(idx as u64);
        Ok(())
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _name: &'static str, v: &T) -> R {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(self, _name: &'static str, idx: u32, _variant: &'static str, v: &T) -> R {
        self.write_u64(idx as u64);
        v.serialize(self)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self, HashError> {
        self.write_u64(len.map_or(u64::MAX, |l| l as u64));
        Ok(self)
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self, HashError> {
        Ok(self)
    }
    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> Result<Self, HashError> {
        Ok(self)
    }
    fn serialize_tuple_variant(self, _name: &'static str, idx: u32, _variant: &'static str, _len: usize) -> Result<Self, HashError> {
        self.write_u64(idx as u64);
        Ok(self)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self, HashError> {
        self.write_u64(len.map_or(u64::MAX, |l| l as u64));
        Ok(self)
    }
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self, HashError> {
        Ok(self)
    }
    fn serialize_struct_variant(self, _name: &'static str, idx: u32, _variant: &'static str, _len: usize) -> Result<Self, HashError> {
        self.write_u64(idx as u64);
        Ok(self)
    }
}

macro_rules! compound {
    ($tr:ident, $m:ident $(, $key:ident)?) => {
        impl ser::$tr for &mut StateHasher {
            type Ok = ();
            type Error = HashError;
            fn $m<T: ?Sized + Serialize>(&mut self, $($key: &'static str,)? v: &T) -> R {
                v.serialize(&mut **self)
            }
            fn end(self) -> R {
                Ok(())
            }
        }
    };
}
compound!(SerializeSeq, serialize_element);
compound!(SerializeTuple, serialize_element);
compound!(SerializeTupleStruct, serialize_field);
compound!(SerializeTupleVariant, serialize_field);
compound!(SerializeStruct, serialize_field, _key);
compound!(SerializeStructVariant, serialize_field, _key);

impl ser::SerializeMap for &mut StateHasher {
    type Ok = ();
    type Error = HashError;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, k: &T) -> R {
        k.serialize(&mut **self)
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> R {
        v.serialize(&mut **self)
    }
    fn end(self) -> R {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_is_stable_and_sensitive() {
        let a = Game::new();
        let b = Game::new();
        assert_eq!(a.checksum(), b.checksum());
        let mut c = a.snapshot();
        assert_eq!(a.checksum(), c.checksum());
        c.frogger.player[1].lives += 1;
        assert_ne!(a.checksum(), c.checksum());
        let mut d = a.snapshot();
        d.rng.rseed ^= 1;
        assert_ne!(a.checksum(), d.checksum());
    }

    #[test]
    fn checksum_covers_enhance_and_croak_state() {
        let a = Game::new();
        let mut e = a.snapshot();
        e.set_game_options(crate::enhance::GameOptions::ENHANCED);
        assert_ne!(a.checksum(), e.checksum());
        let mut t = a.snapshot();
        t.tongue.croak_dir = 1;
        assert_ne!(a.checksum(), t.checksum());
        let mut h = a.snapshot();
        h.tongue.head_post = Some([1.0; 3]);
        assert_ne!(a.checksum(), h.checksum());
    }

    #[test]
    fn resimulate_discards_only_new_effects() {
        let mut g = Game::new();
        g.effects.push(crate::Effect::Quit);
        g.play_sample(1, None, 0, 100, -1);
        let n = g.effects.len();
        assert_eq!(n, 2);
        g.tick_resimulate(TickInput::default());
        assert_eq!(g.effects.len(), 2);
    }
}
