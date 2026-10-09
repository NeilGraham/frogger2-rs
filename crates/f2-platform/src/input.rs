//! The original input abstraction: per-player PSX pad bits in `padData` (`PC/islpad.h`), fed
//! from the keyboard through the 56-entry keymap and from joypads (`PC/controll.cpp`).

use crate::dik;
use serde::{Deserialize, Serialize};

// PSX pad bits (islpad.h:8-23).
pub const PAD_SELECT: u16 = 1 << 0;
pub const PAD_L3: u16 = 1 << 1;
pub const PAD_R3: u16 = 1 << 2;
pub const PAD_START: u16 = 1 << 3;
pub const PAD_UP: u16 = 1 << 4;
pub const PAD_RIGHT: u16 = 1 << 5;
pub const PAD_DOWN: u16 = 1 << 6;
pub const PAD_LEFT: u16 = 1 << 7;
pub const PAD_L2: u16 = 1 << 8;
pub const PAD_R2: u16 = 1 << 9;
pub const PAD_L1: u16 = 1 << 10;
pub const PAD_R1: u16 = 1 << 11;
pub const PAD_TRIANGLE: u16 = 1 << 12;
pub const PAD_CIRCLE: u16 = 1 << 13;
pub const PAD_CROSS: u16 = 1 << 14;
pub const PAD_SQUARE: u16 = 1 << 15;

/// Names of the 16 pad bits, indexed by bit number.
pub const PAD_BIT_NAMES: [&str; 16] =
    ["SELECT", "L3", "R3", "START", "UP", "RIGHT", "DOWN", "LEFT", "L2", "R2", "L1", "R1", "TRIANGLE", "CIRCLE", "CROSS", "SQUARE"];

pub fn pad_bit_name(bit: u16) -> Option<&'static str> {
    (bit.count_ones() == 1).then(|| PAD_BIT_NAMES[bit.trailing_zeros() as usize])
}

pub fn parse_pad_bit(s: &str) -> Option<u16> {
    let u = s.trim().to_ascii_uppercase();
    let u = u.strip_prefix("PAD_").unwrap_or(&u);
    PAD_BIT_NAMES.iter().position(|n| *n == u).map(|i| 1 << i)
}

/// One player's pad (a column of `PadDataType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadState {
    /// Buttons held (`padData.digital[i]`).
    pub buttons: u16,
    /// Buttons newly pressed this frame (`padData.debounce[i]`).
    pub debounce: u16,
}

/// The digital part of `PadDataType` (islpad.h:26) the game reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PadData {
    pub digital: [u16; 8],
    pub debounce: [u16; 8],
}

impl PadData {
    pub fn pad(&self, i: usize) -> PadState {
        PadState { buttons: self.digital[i], debounce: self.debounce[i] }
    }
    pub fn set_pad(&mut self, i: usize, p: PadState) {
        self.digital[i] = p.buttons;
        self.debounce[i] = p.debounce;
    }
}

// ---- keymap -------------------------------------------------------------------------------

/// `KEYENTRY` (controll.h:27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEntry {
    pub player: u8,
    /// One pad bit.
    pub button: u16,
    /// DirectInput scan code, 0 = unbound.
    pub key: u8,
}

pub const KEYMAP_LEN: usize = 56;
/// Entries per player (`4*14` loops in `ProcessKeyboardInput`).
pub const KEYS_PER_PLAYER: usize = 14;

const fn ke(player: u8, button: u16, key: u8) -> KeyEntry {
    KeyEntry { player, button, key }
}

/// `defaultKeymap` (controll.cpp:119).
#[rustfmt::skip]
pub const DEFAULT_KEYMAP: [KeyEntry; KEYMAP_LEN] = {
    use dik::*;
    [
        ke(0, PAD_UP, UP), ke(0, PAD_DOWN, DOWN), ke(0, PAD_LEFT, LEFT), ke(0, PAD_RIGHT, RIGHT),
        ke(0, PAD_CROSS, RETURN), ke(0, PAD_SQUARE, RCONTROL), ke(0, PAD_START, ESCAPE),
        ke(0, PAD_CIRCLE, RSHIFT), ke(0, PAD_TRIANGLE, ESCAPE), ke(0, PAD_L2, NUMPAD4),
        ke(0, PAD_R2, NUMPAD6), ke(0, PAD_SELECT, NUMPAD0), ke(0, PAD_L1, NUMPAD1), ke(0, PAD_R1, R),

        ke(1, PAD_UP, Q), ke(1, PAD_DOWN, A), ke(1, PAD_LEFT, S), ke(1, PAD_RIGHT, D),
        ke(1, PAD_CROSS, TAB), ke(1, PAD_SQUARE, LSHIFT), ke(1, PAD_START, 0),
        ke(1, PAD_CIRCLE, Q), ke(1, PAD_TRIANGLE, ESCAPE), ke(1, PAD_L2, 0),
        ke(1, PAD_R2, 0), ke(1, PAD_SELECT, 0), ke(1, PAD_L1, 0), ke(1, PAD_R1, 0),

        ke(2, PAD_UP, KEY_8), ke(2, PAD_DOWN, I), ke(2, PAD_LEFT, O), ke(2, PAD_RIGHT, P),
        ke(2, PAD_CROSS, KEY_7), ke(2, PAD_SQUARE, U), ke(2, PAD_START, 0),
        ke(2, PAD_CIRCLE, 0), ke(2, PAD_TRIANGLE, ESCAPE), ke(2, PAD_L2, 0),
        ke(2, PAD_R2, 0), ke(2, PAD_SELECT, 0), ke(2, PAD_L1, 0), ke(2, PAD_R1, 0),

        ke(3, PAD_UP, NUMPAD5), ke(3, PAD_DOWN, NUMPAD2), ke(3, PAD_LEFT, NUMPAD1), ke(3, PAD_RIGHT, NUMPAD3),
        ke(3, PAD_CROSS, NUMPADENTER), ke(3, PAD_SQUARE, 0), ke(3, PAD_START, 0),
        ke(3, PAD_CIRCLE, 0), ke(3, PAD_TRIANGLE, ESCAPE), ke(3, PAD_L2, 0),
        ke(3, PAD_R2, 0), ke(3, PAD_SELECT, 0), ke(3, PAD_L1, 0), ke(3, PAD_R1, 0),
    ]
};

/// `menuKey` (controll.cpp:796): extra keys applied to player 0's first 7 keymap buttons
/// while `checkMenuKeys` is set.
pub const MENU_KEYS: [u8; 7] = [dik::UP, dik::DOWN, dik::LEFT, dik::RIGHT, dik::RETURN, 0, dik::ESCAPE];

/// The 56-entry keymap (`keymap`, controll.cpp:119).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keymap(pub [KeyEntry; KEYMAP_LEN]);

impl Default for Keymap {
    fn default() -> Self {
        Keymap(DEFAULT_KEYMAP)
    }
}

impl Keymap {
    /// The registry `Keymap` value written by `KeyboardDump` (controll.cpp:291): 56 × (player,
    /// bit index, key). A button with no bit set is written as index 16, as the original does.
    pub fn to_registry_bytes(&self) -> [u8; KEYMAP_LEN * 3] {
        let mut out = [0u8; KEYMAP_LEN * 3];
        for (i, e) in self.0.iter().enumerate() {
            let b = (0..16).find(|&b| (1u32 << b) == e.button as u32).unwrap_or(16);
            out[3 * i] = e.player;
            out[3 * i + 1] = b as u8;
            out[3 * i + 2] = e.key;
        }
        out
    }

    /// Reads a `KeyboardDump` buffer; `None` (= use defaults) if it is short, like the original.
    pub fn from_registry_bytes(b: &[u8]) -> Option<Keymap> {
        if b.len() < KEYMAP_LEN * 3 {
            return None;
        }
        let mut m = DEFAULT_KEYMAP;
        for (i, e) in m.iter_mut().enumerate() {
            e.player = b[3 * i];
            e.button = 1u32.checked_shl(b[3 * i + 1] as u32).unwrap_or(0) as u16;
            e.key = b[3 * i + 2];
        }
        Some(Keymap(m))
    }

    /// TOML-friendly form.
    pub fn to_bindings(&self) -> Vec<KeyBinding> {
        self.0.iter().map(|e| KeyBinding { player: e.player + 1, button: pad_bit_name(e.button).unwrap_or("NONE").into(), key: dik::name(e.key) }).collect()
    }

    pub fn from_bindings(b: &[KeyBinding]) -> Result<Keymap, String> {
        if b.len() != KEYMAP_LEN {
            return Err(format!("keymap must have {KEYMAP_LEN} entries, got {}", b.len()));
        }
        let mut m = DEFAULT_KEYMAP;
        for (e, kb) in m.iter_mut().zip(b) {
            if !(1..=4).contains(&kb.player) {
                return Err(format!("bad player {}", kb.player));
            }
            e.player = kb.player - 1;
            e.button =
                if kb.button.eq_ignore_ascii_case("NONE") { 0 } else { parse_pad_bit(&kb.button).ok_or_else(|| format!("bad button {:?}", kb.button))? };
            e.key = dik::parse(&kb.key).ok_or_else(|| format!("bad key {:?}", kb.key))?;
        }
        Ok(Keymap(m))
    }

    /// Rebinds the entry for `(player, button)` (what the controls menu does).
    pub fn bind(&mut self, player: u8, button: u16, key: u8) -> bool {
        match self.0.iter_mut().find(|e| e.player == player && e.button == button) {
            Some(e) => {
                e.key = key;
                true
            }
            None => false,
        }
    }
}

/// One keymap entry in the config file. `player` is 1-based.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    pub player: u8,
    pub button: String,
    pub key: String,
}

// ---- devices ------------------------------------------------------------------------------

/// `controllers[pl]` (controll.cpp:197): keyboard or joypad n.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Controller {
    #[default]
    Keyboard,
    Joypad(u8),
}

/// Keyboard state for one frame: held keys plus the buffered key-down events since the last
/// frame (DirectInput buffered data).
#[derive(Clone, Debug)]
pub struct KeyboardState {
    pub down: [bool; 256],
    pub pressed: Vec<u8>,
}

impl Default for KeyboardState {
    fn default() -> Self {
        KeyboardState { down: [false; 256], pressed: Vec::new() }
    }
}

impl KeyboardState {
    pub fn key_down(&mut self, dik: u8) {
        if dik != 0 {
            self.down[dik as usize] = true;
            self.pressed.push(dik);
        }
    }
    pub fn key_up(&mut self, dik: u8) {
        self.down[dik as usize] = false;
    }
    /// `KEYPRESS(keyDIK)`.
    pub fn is_down(&self, dik: u8) -> bool {
        self.down[dik as usize]
    }
    /// Clears the event buffer (call after each processed frame).
    pub fn end_frame(&mut self) {
        self.pressed.clear();
    }
    /// Releases everything (focus loss).
    pub fn clear(&mut self) {
        self.down = [false; 256];
        self.pressed.clear();
    }
}

/// `DEAD_ZONE` (controll.cpp:54) on the -1000..=1000 axis range set in `InitJoystickControl`.
pub const DEAD_ZONE: i32 = 500;
/// `MAXBUTTONS` (controll.cpp:56).
pub const MAXBUTTONS: usize = 6;
/// `joymap` (controll.cpp:182): DirectInput button n -> pad bit.
pub const JOYMAP: [u16; MAXBUTTONS] = [PAD_CROSS, PAD_SQUARE, PAD_CIRCLE, PAD_TRIANGLE, PAD_START, 0];

/// A joypad read the way the original reads `DIJOYSTATE`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JoypadState {
    /// `rgbButtons[0..6]`.
    pub buttons: [bool; MAXBUTTONS],
    /// `lX`, `lY` in -1000..=1000 (negative = left/up).
    pub x: i32,
    pub y: i32,
    /// Pad bits from inputs the original ignores (d-pad hat, shoulders, select, sticks
    /// clicks) — an addition so modern pads are fully usable.
    pub extra: u16,
}

/// Bits a joypad contributes (`OldProcessJoypad`, controll.cpp:664).
pub fn joypad_bits(j: &JoypadState) -> u16 {
    let mut b = 0u16;
    for (m, &pressed) in j.buttons.iter().enumerate() {
        if pressed {
            b |= JOYMAP[m];
        }
    }
    if j.x < -DEAD_ZONE {
        b |= PAD_LEFT;
    } else if j.x > DEAD_ZONE {
        b |= PAD_RIGHT;
    }
    if j.y < -DEAD_ZONE {
        b |= PAD_UP;
    } else if j.y > DEAD_ZONE {
        b |= PAD_DOWN;
    }
    b | j.extra
}

/// Keyboard/joypad -> `padData` mapping (`ProcessUserInput` & co.).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputMapper {
    pub keymap: Keymap,
    /// Additional keys beyond the one-per-action `keymap` (multiple bindings per action).
    pub extra_keys: Vec<KeyEntry>,
    pub controllers: [Controller; 4],
    /// `checkMenuKeys`: set by the game on menu screens.
    pub check_menu_keys: bool,
    /// `keysEnabled`.
    pub keys_enabled: bool,
    /// `windowActive`: keyboard is ignored while unfocused.
    pub window_active: bool,
}

impl Default for InputMapper {
    fn default() -> Self {
        InputMapper {
            keymap: Keymap::default(),
            extra_keys: Vec::new(),
            controllers: [Controller::Keyboard; 4],
            check_menu_keys: false,
            keys_enabled: true,
            window_active: true,
        }
    }
}

impl InputMapper {
    pub fn new(keymap: Keymap) -> InputMapper {
        InputMapper { keymap, ..Default::default() }
    }

    /// `ResetControllerSetup` (controll.cpp:343): joypads to the first players.
    pub fn reset_controllers(&mut self, num_joypads: usize) {
        for (pl, c) in self.controllers.iter_mut().enumerate() {
            *c = if pl < num_joypads { Controller::Joypad(pl as u8) } else { Controller::Keyboard };
        }
    }

    /// Port of `ProcessUserInput` (controll.cpp:935) + `ProcessKeyboardInput` (:798) +
    /// `OldProcessJoypad` (:664) for one frame. `joypads[n]` is joypad n (`None` = absent).
    pub fn process(&self, pad: &mut PadData, kb: &KeyboardState, joypads: &[Option<JoypadState>]) {
        let old = pad.digital;
        pad.debounce = [0; 8];
        pad.digital = [0; 8];
        let mut joypad_debounce = [0u16; 4];
        let joypad_buttons = [0u16; 4]; // only written by the (disabled) joypad thread

        if self.window_active {
            // Buffered key-down events -> joypadDebounce.
            for &key in &kb.pressed {
                for e in self.keymap.0.iter().take(4 * KEYS_PER_PLAYER).chain(&self.extra_keys) {
                    if e.key == key {
                        joypad_debounce[(e.player & 3) as usize] |= e.button;
                    }
                }
            }
            for e in self.keymap.0.iter().take(4 * KEYS_PER_PLAYER).chain(&self.extra_keys) {
                if e.key > 0 && kb.is_down(e.key) {
                    pad.digital[(e.player & 7) as usize] |= e.button;
                }
            }
            if self.check_menu_keys {
                for (i, &k) in MENU_KEYS.iter().enumerate() {
                    if k != 0 && kb.is_down(k) {
                        pad.digital[0] |= self.keymap.0[i].button;
                    }
                }
            }
        }

        for (i, c) in self.controllers.iter().enumerate() {
            if let Controller::Joypad(n) = *c {
                if let Some(Some(j)) = joypads.get(n as usize) {
                    pad.digital[i] |= joypad_bits(j);
                }
            }
        }

        for i in 0..4 {
            if self.keys_enabled {
                pad.digital[i] |= joypad_buttons[i];
                pad.debounce[i] |= (!old[i] & pad.digital[i]) | joypad_debounce[i];
            }
        }
    }
}
