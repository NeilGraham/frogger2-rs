//! N64 settings page (a host menu, not in the original): opened from the pause menus'
//! "Settings" item (`GameOptions::settings_menu` emits `Effect::OpenSettings`), kept in
//! cartridge SRAM after the save game.
//!
//! SRAM record at [`OFFSET`] (the save game's record is ~2.1 KiB from offset 0):
//! `"F2ST"`, version 1, FPS counter, frame limit, controls, then FNV-1a of those 8 bytes.

use alloc::string::String;
use f2_game::levplay::{PAD_CIRCLE, PAD_CROSS, PAD_DOWN, PAD_LEFT, PAD_RIGHT, PAD_START, PAD_TRIANGLE, PAD_UP};

/// SRAM offset of the settings record.
pub const OFFSET: usize = 0x7F00;
const MAGIC: [u8; 4] = *b"F2ST";
const LEN: usize = 12;
/// Frame limit choices (0 = unlimited).
pub const LIMITS: [u8; 3] = [0, 30, 20];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// Frames per second shown in the top-right corner.
    pub fps_counter: bool,
    /// Display frame cap: one of [`LIMITS`].
    pub frame_limit: u8,
    /// Controls preset: A and B swapped (B hops, A uses the tongue).
    pub swap_ab: bool,
}

fn fnv1a(d: &[u8]) -> u32 {
    d.iter().fold(0x811c_9dc5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

impl Settings {
    pub fn encode(&self) -> [u8; LEN] {
        let mut o = [0u8; LEN];
        o[..4].copy_from_slice(&MAGIC);
        o[4] = 1;
        o[5] = self.fps_counter as u8;
        o[6] = self.frame_limit;
        o[7] = self.swap_ab as u8;
        let h = fnv1a(&o[..8]);
        o[8..].copy_from_slice(&h.to_be_bytes());
        o
    }

    /// The settings of a record; `None` if missing (blank SRAM), corrupt or unknown.
    pub fn decode(r: &[u8]) -> Option<Settings> {
        let r = r.get(..LEN)?;
        if r[..4] != MAGIC || r[4] != 1 || u32::from_be_bytes([r[8], r[9], r[10], r[11]]) != fnv1a(&r[..8]) {
            return None;
        }
        Some(Settings { fps_counter: r[5] != 0, frame_limit: if LIMITS.contains(&r[6]) { r[6] } else { 0 }, swap_ab: r[7] != 0 })
    }

    #[cfg(target_vendor = "nintendo64")]
    pub fn load() -> Settings {
        crate::rt::sram_read(OFFSET, LEN).and_then(|r| Settings::decode(&r)).unwrap_or_default()
    }

    #[cfg(target_vendor = "nintendo64")]
    pub fn store(&self) -> bool {
        crate::rt::sram_write(OFFSET, &self.encode())
    }
}

/// Menu rows.
pub const ROWS: usize = 4;

/// The open settings page: selection and edge detection of the pad.
#[derive(Default)]
pub struct Menu {
    pub open: bool,
    pub sel: usize,
    prev: u16,
}

impl Menu {
    /// Opens the page. Buttons still held (the press that chose "Settings") count only
    /// once released.
    pub fn show(&mut self) {
        self.open = true;
        self.sel = 0;
        self.prev = u16::MAX;
    }

    /// One frame of input (`pad`: PSX bits with the standard mapping). Changes `s`; returns
    /// true when the page closes (B, Start, C-Up or A on "Back").
    pub fn input(&mut self, s: &mut Settings, pad: u16) -> bool {
        let new = pad & !self.prev;
        self.prev = pad;
        if !self.open {
            return false;
        }
        if new & (PAD_CIRCLE | PAD_START | PAD_TRIANGLE) != 0 || (new & PAD_CROSS != 0 && self.sel == ROWS - 1) {
            self.open = false;
            return true;
        }
        if new & PAD_UP != 0 {
            self.sel = (self.sel + ROWS - 1) % ROWS;
        }
        if new & PAD_DOWN != 0 {
            self.sel = (self.sel + 1) % ROWS;
        }
        let dir = if new & (PAD_RIGHT | PAD_CROSS) != 0 {
            1
        } else if new & PAD_LEFT != 0 {
            -1
        } else {
            0
        };
        if dir != 0 {
            match self.sel {
                0 => s.fps_counter = !s.fps_counter,
                1 => {
                    let i = LIMITS.iter().position(|&l| l == s.frame_limit).unwrap_or(0) as i32;
                    s.frame_limit = LIMITS[(i + dir).rem_euclid(LIMITS.len() as i32) as usize];
                }
                2 => s.swap_ab = !s.swap_ab,
                _ => {}
            }
        }
        false
    }

    /// Row `i`'s text.
    pub fn row(s: &Settings, i: usize) -> String {
        match i {
            0 => alloc::format!("FPS counter: {}", if s.fps_counter { "On" } else { "Off" }),
            1 => match s.frame_limit {
                0 => "Frame limit: Off".into(),
                n => alloc::format!("Frame limit: {n} fps"),
            },
            2 => alloc::format!("Controls: {}", if s.swap_ab { "B hop, A tongue" } else { "A hop, B tongue" }),
            _ => "Back".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trip() {
        let s = Settings { fps_counter: true, frame_limit: 20, swap_ab: true };
        let r = s.encode();
        assert_eq!(Settings::decode(&r), Some(s));
        let mut bad = r;
        bad[6] ^= 1;
        assert_eq!(Settings::decode(&bad), None);
        assert_eq!(Settings::decode(&[0xFF; LEN]), None, "blank SRAM");
    }

    #[test]
    fn menu_input() {
        let mut s = Settings::default();
        let mut m = Menu::default();
        m.show();
        // The A that opened the page is still held: ignored until released.
        assert!(!m.input(&mut s, PAD_CROSS));
        assert!(!s.fps_counter);
        m.input(&mut s, 0);
        m.input(&mut s, PAD_CROSS);
        assert!(s.fps_counter);
        m.input(&mut s, 0);
        m.input(&mut s, PAD_DOWN);
        m.input(&mut s, PAD_DOWN | PAD_RIGHT);
        assert_eq!(s.frame_limit, 30);
        m.input(&mut s, 0);
        m.input(&mut s, PAD_LEFT);
        m.input(&mut s, 0);
        m.input(&mut s, PAD_LEFT);
        assert_eq!(s.frame_limit, 20, "wraps");
        m.input(&mut s, 0);
        assert!(m.input(&mut s, PAD_CIRCLE), "B closes");
        assert!(!m.open);
    }
}
