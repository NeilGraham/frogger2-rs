//! DirectInput keyboard scan codes (`DIK_*`, dinput.h). The original keymap stores these, so
//! they stay the canonical key id in our keymap and config (named without the `DIK_` prefix).

macro_rules! diks {
    ($($name:ident = $val:expr),* $(,)?) => {
        $(pub const $name: u8 = $val;)*
        /// All named keys, `(code, name)`.
        pub const NAMES: &[(u8, &str)] = &[$(($val, stringify!($name))),*];
    };
}

diks! {
    ESCAPE = 0x01, KEY_1 = 0x02, KEY_2 = 0x03, KEY_3 = 0x04, KEY_4 = 0x05, KEY_5 = 0x06,
    KEY_6 = 0x07, KEY_7 = 0x08, KEY_8 = 0x09, KEY_9 = 0x0A, KEY_0 = 0x0B, MINUS = 0x0C,
    EQUALS = 0x0D, BACK = 0x0E, TAB = 0x0F, Q = 0x10, W = 0x11, E = 0x12, R = 0x13, T = 0x14,
    Y = 0x15, U = 0x16, I = 0x17, O = 0x18, P = 0x19, LBRACKET = 0x1A, RBRACKET = 0x1B,
    RETURN = 0x1C, LCONTROL = 0x1D, A = 0x1E, S = 0x1F, D = 0x20, F = 0x21, G = 0x22, H = 0x23,
    J = 0x24, K = 0x25, L = 0x26, SEMICOLON = 0x27, APOSTROPHE = 0x28, GRAVE = 0x29,
    LSHIFT = 0x2A, BACKSLASH = 0x2B, Z = 0x2C, X = 0x2D, C = 0x2E, V = 0x2F, B = 0x30, N = 0x31,
    M = 0x32, COMMA = 0x33, PERIOD = 0x34, SLASH = 0x35, RSHIFT = 0x36, MULTIPLY = 0x37,
    LMENU = 0x38, SPACE = 0x39, CAPITAL = 0x3A, F1 = 0x3B, F2 = 0x3C, F3 = 0x3D, F4 = 0x3E,
    F5 = 0x3F, F6 = 0x40, F7 = 0x41, F8 = 0x42, F9 = 0x43, F10 = 0x44, NUMLOCK = 0x45,
    SCROLL = 0x46, NUMPAD7 = 0x47, NUMPAD8 = 0x48, NUMPAD9 = 0x49, SUBTRACT = 0x4A,
    NUMPAD4 = 0x4B, NUMPAD5 = 0x4C, NUMPAD6 = 0x4D, ADD = 0x4E, NUMPAD1 = 0x4F, NUMPAD2 = 0x50,
    NUMPAD3 = 0x51, NUMPAD0 = 0x52, DECIMAL = 0x53, OEM_102 = 0x56, F11 = 0x57, F12 = 0x58,
    F13 = 0x64, F14 = 0x65, F15 = 0x66, KANA = 0x70, CONVERT = 0x79, NOCONVERT = 0x7B,
    YEN = 0x7D, NUMPADEQUALS = 0x8D, NUMPADENTER = 0x9C, RCONTROL = 0x9D,
    NUMPADCOMMA = 0xB3, DIVIDE = 0xB5, SYSRQ = 0xB7, RMENU = 0xB8, PAUSE = 0xC5, HOME = 0xC7,
    UP = 0xC8, PRIOR = 0xC9, LEFT = 0xCB, RIGHT = 0xCD, END = 0xCF, DOWN = 0xD0, NEXT = 0xD1,
    INSERT = 0xD2, DELETE = 0xD3, LWIN = 0xDB, RWIN = 0xDC, APPS = 0xDD,
}

/// Name of a scan code (`"RETURN"`, `"NONE"` for 0, `"0xNN"` for unnamed codes).
pub fn name(code: u8) -> String {
    if code == 0 {
        return "NONE".into();
    }
    NAMES.iter().find(|(c, _)| *c == code).map(|(_, n)| n.to_string()).unwrap_or_else(|| format!("0x{code:02X}"))
}

/// Parses a name produced by [`name`] (case-insensitive; `DIK_` prefix optional).
pub fn parse(s: &str) -> Option<u8> {
    let u = s.trim().to_ascii_uppercase();
    let u = u.strip_prefix("DIK_").unwrap_or(&u);
    if u == "NONE" || u.is_empty() {
        return Some(0);
    }
    if let Some(h) = u.strip_prefix("0X") {
        return u8::from_str_radix(h, 16).ok();
    }
    if let Some(&(c, _)) = NAMES.iter().find(|(_, n)| *n == u) {
        return Some(c);
    }
    // Plain digits "0".."9" as an alias of KEY_n.
    if u.len() == 1 && u.as_bytes()[0].is_ascii_digit() {
        let d = u.as_bytes()[0] - b'0';
        return Some(if d == 0 { KEY_0 } else { KEY_1 + d - 1 });
    }
    None
}
