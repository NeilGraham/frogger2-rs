//! N64 controller → the game's PSX pad bits (`f2_game::levplay::PAD_*`, what
//! `TickInput::pads` carries).
//!
//! | N64 | PSX | in game |
//! |---|---|---|
//! | A | Cross | hop / confirm |
//! | B | Circle | tongue |
//! | Z | Square | croak |
//! | C-Up | Triangle | super hop / back in menus |
//! | C-Down | Select | |
//! | C-Left / C-Right | Square / Circle | |
//! | L / R | L1 / R1 | camera |
//! | Start | Start | pause |
//! | D-pad, stick | directions | stick past ±[`STICK_THRESHOLD`] |

use crate::ffi::pad;
use f2_game::levplay::*;

/// Stick deflection (of about ±85 on a real controller) that counts as a direction.
pub const STICK_THRESHOLD: i32 = 40;

/// `raw` = `n64_pad()`: buttons in the low 16 bits, stick x in bits 16..24, y in 24..32
/// (signed, up positive).
pub fn psx_pad(raw: u32) -> u16 {
    let map: [(u32, u16); 15] = [
        (pad::A, PAD_CROSS),
        (pad::B, PAD_CIRCLE),
        (pad::Z, PAD_SQUARE),
        (pad::START, PAD_START),
        (pad::UP, PAD_UP),
        (pad::DOWN, PAD_DOWN),
        (pad::LEFT, PAD_LEFT),
        (pad::RIGHT, PAD_RIGHT),
        (pad::L, PAD_L1),
        (pad::R, PAD_R1),
        (pad::C_UP, PAD_TRIANGLE),
        (pad::C_DOWN, PAD_SELECT),
        (pad::C_LEFT, PAD_SQUARE),
        (pad::C_RIGHT, PAD_CIRCLE),
        (0, 0),
    ];
    let mut out = 0u16;
    for (b, p) in map {
        if b != 0 && raw & b != 0 {
            out |= p;
        }
    }
    let sx = ((raw >> 16) & 0xff) as u8 as i8 as i32;
    let sy = (raw >> 24) as u8 as i8 as i32;
    if sx >= STICK_THRESHOLD {
        out |= PAD_RIGHT;
    } else if sx <= -STICK_THRESHOLD {
        out |= PAD_LEFT;
    }
    if sy >= STICK_THRESHOLD {
        out |= PAD_UP;
    } else if sy <= -STICK_THRESHOLD {
        out |= PAD_DOWN;
    }
    out
}

/// The "B hop, A tongue" controls preset (`settings::Settings::swap_ab`): Cross and Circle
/// exchanged (A, B and C-Right).
pub fn swap_ab(p: u16) -> u16 {
    let (c, o) = (p & PAD_CROSS != 0, p & PAD_CIRCLE != 0);
    let mut out = p & !(PAD_CROSS | PAD_CIRCLE);
    if c {
        out |= PAD_CIRCLE;
    }
    if o {
        out |= PAD_CROSS;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_and_stick() {
        assert_eq!(psx_pad(pad::A | pad::B), PAD_CROSS | PAD_CIRCLE);
        assert_eq!(psx_pad(pad::START | pad::UP), PAD_START | PAD_UP);
        // stick right (x = 70), down (y = -60)
        let raw = (70u32 << 16) | ((-60i8 as u8 as u32) << 24);
        assert_eq!(psx_pad(raw), PAD_RIGHT | PAD_DOWN);
        // small deflections are ignored
        assert_eq!(psx_pad((10u32 << 16) | ((-20i8 as u8 as u32) << 24)), 0);
    }

    #[test]
    fn swapped_preset() {
        assert_eq!(swap_ab(psx_pad(pad::A)), PAD_CIRCLE);
        assert_eq!(swap_ab(psx_pad(pad::B | pad::UP)), PAD_CROSS | PAD_UP);
        assert_eq!(swap_ab(PAD_CROSS | PAD_CIRCLE), PAD_CROSS | PAD_CIRCLE);
    }
}
