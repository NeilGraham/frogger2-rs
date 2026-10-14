//! Android input -> the game's input types: gamepad keys and joystick axes become player 1's
//! [`RawPad`], keyboard keys DirectInput codes. Uses the numeric `KEYCODE_*` values
//! (android.view.KeyEvent) so it is unit-tested on the host.
//!
//! Controllers differ in how they report the d-pad: most built-in and Xbox-style pads send
//! it as the HAT_X/HAT_Y axes of joystick motion events, some as `KEYCODE_DPAD_*` keys. Both
//! are read; the left stick (AXIS_X/Y) and right stick (AXIS_Z/RZ) become the pad's axes and
//! analogue triggers (LTRIGGER/RTRIGGER, BRAKE/GAS) press L2/R2.

use f2_platform::{dik, PadAxis, PadButton, RawPad};

/// `KEYCODE_*` values used here.
pub mod key {
    pub const BACK: u32 = 4;
    pub const DPAD_UP: u32 = 19;
    pub const DPAD_DOWN: u32 = 20;
    pub const DPAD_LEFT: u32 = 21;
    pub const DPAD_RIGHT: u32 = 22;
    pub const DPAD_CENTER: u32 = 23;
    pub const VOLUME_UP: u32 = 24;
    pub const VOLUME_DOWN: u32 = 25;
    pub const BUTTON_A: u32 = 96;
    pub const BUTTON_MODE: u32 = 110;
    pub const ESCAPE: u32 = 111;
    pub const VOLUME_MUTE: u32 = 164;
}

/// Gamepad button keys (and the d-pad keys) -> pad button.
pub fn pad_button(code: u32) -> Option<PadButton> {
    Some(match code {
        key::DPAD_UP => PadButton::DPadUp,
        key::DPAD_DOWN => PadButton::DPadDown,
        key::DPAD_LEFT => PadButton::DPadLeft,
        key::DPAD_RIGHT => PadButton::DPadRight,
        key::DPAD_CENTER => PadButton::South,
        96 => PadButton::South,  // BUTTON_A
        97 => PadButton::East,   // BUTTON_B
        99 => PadButton::West,   // BUTTON_X
        100 => PadButton::North, // BUTTON_Y
        102 => PadButton::L1,
        103 => PadButton::R1,
        104 => PadButton::L2,
        105 => PadButton::R2,
        106 => PadButton::L3, // BUTTON_THUMBL
        107 => PadButton::R3,
        108 => PadButton::Start,
        109 => PadButton::Select,
        key::BUTTON_MODE => PadButton::Mode,
        _ => return None,
    })
}

/// Keys the system should keep (volume): never consumed.
pub fn system_key(code: u32) -> bool {
    matches!(code, key::VOLUME_UP | key::VOLUME_DOWN | key::VOLUME_MUTE)
}

/// Keyboard keys -> DirectInput code. Back and Escape are Escape.
pub fn dik(code: u32) -> Option<u8> {
    const LETTERS: [u8; 26] = [
        dik::A,
        dik::B,
        dik::C,
        dik::D,
        dik::E,
        dik::F,
        dik::G,
        dik::H,
        dik::I,
        dik::J,
        dik::K,
        dik::L,
        dik::M,
        dik::N,
        dik::O,
        dik::P,
        dik::Q,
        dik::R,
        dik::S,
        dik::T,
        dik::U,
        dik::V,
        dik::W,
        dik::X,
        dik::Y,
        dik::Z,
    ];
    const FKEYS: [u8; 12] = [dik::F1, dik::F2, dik::F3, dik::F4, dik::F5, dik::F6, dik::F7, dik::F8, dik::F9, dik::F10, dik::F11, dik::F12];
    const NUMPAD: [u8; 10] =
        [dik::NUMPAD0, dik::NUMPAD1, dik::NUMPAD2, dik::NUMPAD3, dik::NUMPAD4, dik::NUMPAD5, dik::NUMPAD6, dik::NUMPAD7, dik::NUMPAD8, dik::NUMPAD9];
    Some(match code {
        key::BACK | key::ESCAPE => dik::ESCAPE,
        7 => dik::KEY_0,
        8..=16 => dik::KEY_1 + (code - 8) as u8,
        29..=54 => LETTERS[(code - 29) as usize],
        131..=142 => FKEYS[(code - 131) as usize],
        144..=153 => NUMPAD[(code - 144) as usize],
        55 => dik::COMMA,
        56 => dik::PERIOD,
        57 => dik::LMENU,
        58 => dik::RMENU,
        59 => dik::LSHIFT,
        60 => dik::RSHIFT,
        61 => dik::TAB,
        62 => dik::SPACE,
        66 => dik::RETURN,
        67 => dik::BACK, // DEL = backspace
        68 => dik::GRAVE,
        69 => dik::MINUS,
        70 => dik::EQUALS,
        71 => dik::LBRACKET,
        72 => dik::RBRACKET,
        73 => dik::BACKSLASH,
        74 => dik::SEMICOLON,
        75 => dik::APOSTROPHE,
        76 => dik::SLASH,
        92 => dik::PRIOR,
        93 => dik::NEXT,
        112 => dik::DELETE, // FORWARD_DEL
        113 => dik::LCONTROL,
        114 => dik::RCONTROL,
        115 => dik::CAPITAL,
        116 => dik::SCROLL,
        122 => dik::HOME,
        123 => dik::END,
        124 => dik::INSERT,
        143 => dik::NUMLOCK,
        154 => dik::DIVIDE,
        155 => dik::MULTIPLY,
        156 => dik::SUBTRACT,
        157 => dik::ADD,
        158 => dik::DECIMAL,
        160 => dik::NUMPADENTER,
        _ => return None,
    })
}

/// One joystick motion sample: the axes this module reads.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Axes {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub rz: f32,
    pub hat_x: f32,
    pub hat_y: f32,
    /// max(LTRIGGER, BRAKE)
    pub lt: f32,
    /// max(RTRIGGER, GAS)
    pub rt: f32,
}

/// Player 1's physical controller state.
#[derive(Clone, Debug, Default)]
pub struct Gamepad {
    /// Buttons held through key events.
    keys: RawPad,
    axes: Axes,
}

impl Gamepad {
    /// A gamepad key event; false when the key is not a gamepad key.
    pub fn key(&mut self, code: u32, down: bool) -> bool {
        match pad_button(code) {
            Some(b) => {
                self.keys.set_button(b, down);
                true
            }
            None => false,
        }
    }

    pub fn motion(&mut self, a: Axes) {
        self.axes = a;
    }

    pub fn clear(&mut self) {
        *self = Gamepad::default();
    }

    /// The pad: key buttons, hat as d-pad, triggers as L2/R2, sticks (y up positive).
    pub fn pad(&self) -> RawPad {
        let mut p = self.keys;
        let a = &self.axes;
        let mut press = |b: PadButton, c: bool| {
            if c {
                p.set_button(b, true)
            }
        };
        press(PadButton::DPadLeft, a.hat_x < -0.5);
        press(PadButton::DPadRight, a.hat_x > 0.5);
        press(PadButton::DPadUp, a.hat_y < -0.5);
        press(PadButton::DPadDown, a.hat_y > 0.5);
        press(PadButton::L2, a.lt > 0.5);
        press(PadButton::R2, a.rt > 0.5);
        p.set_axis(PadAxis::LeftX, a.x.clamp(-1.0, 1.0));
        p.set_axis(PadAxis::LeftY, (-a.y).clamp(-1.0, 1.0));
        p.set_axis(PadAxis::RightX, a.z.clamp(-1.0, 1.0));
        p.set_axis(PadAxis::RightY, (-a.rz).clamp(-1.0, 1.0));
        p
    }

    /// Any input held (hides the touch controls).
    pub fn active(&self) -> bool {
        let p = self.pad();
        p.buttons != 0 || p.axes.iter().any(|v| v.abs() > 0.5)
    }
}

/// Merges two pads: buttons ORed, the larger deflection per axis.
pub fn merge(a: RawPad, b: RawPad) -> RawPad {
    let mut p = RawPad { buttons: a.buttons | b.buttons, axes: a.axes };
    for (o, v) in p.axes.iter_mut().zip(b.axes) {
        if v.abs() > o.abs() {
            *o = v;
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hat_and_sticks() {
        let mut g = Gamepad::default();
        g.motion(Axes { hat_x: 1.0, hat_y: -1.0, x: 0.8, y: -0.9, rt: 1.0, ..Axes::default() });
        let p = g.pad();
        assert!(p.button(PadButton::DPadRight) && p.button(PadButton::DPadUp) && p.button(PadButton::R2));
        assert!(!p.button(PadButton::DPadLeft) && !p.button(PadButton::DPadDown));
        assert_eq!(p.axis(PadAxis::LeftX), 0.8);
        assert_eq!(p.axis(PadAxis::LeftY), 0.9, "Android y is down; pads are y up");
        assert!(g.active());
        g.motion(Axes::default());
        assert_eq!(g.pad().buttons, 0);
        assert!(!g.active());
    }

    #[test]
    fn keys() {
        let mut g = Gamepad::default();
        assert!(g.key(96, true));
        assert!(g.key(key::DPAD_LEFT, true));
        assert!(!g.key(29, true), "letter A is a keyboard key");
        let p = g.pad();
        assert!(p.button(PadButton::South) && p.button(PadButton::DPadLeft));
        g.key(96, false);
        assert!(!g.pad().button(PadButton::South));
        assert_eq!(dik(29), Some(dik::A));
        assert_eq!(dik(54), Some(dik::Z));
        assert_eq!(dik(7), Some(dik::KEY_0));
        assert_eq!(dik(16), Some(dik::KEY_9));
        assert_eq!(dik(key::BACK), Some(dik::ESCAPE));
        assert_eq!(dik(142), Some(dik::F12));
        assert!(system_key(key::VOLUME_UP) && !system_key(96));
    }

    #[test]
    fn merging() {
        let mut a = RawPad::default();
        a.set_button(PadButton::South, true);
        a.set_axis(PadAxis::LeftX, 0.3);
        let mut b = RawPad::default();
        b.set_button(PadButton::DPadUp, true);
        b.set_axis(PadAxis::LeftX, -0.9);
        let m = merge(a, b);
        assert!(m.button(PadButton::South) && m.button(PadButton::DPadUp));
        assert_eq!(m.axis(PadAxis::LeftX), -0.9);
    }
}
