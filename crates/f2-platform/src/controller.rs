//! Native controller support matching the PlayStation version (`docs/CONTROLLERS.md`).
//!
//! Pure logic (no gilrs), unit-tested here; [`crate::gamepad::Gamepads`] applies it to real
//! hardware:
//! - [`StickDpad`]: left stick -> 4-way d-pad with dead zone and hysteresis, so a stick resting
//!   near a diagonal does not flip between two hop directions.
//! - [`FaceLayout`]: positional PS1 layout (South = Cross) or the Nintendo label layout.
//! - [`PsxVibration`] / [`rumble_motors`]: DualShock motor values -> force-feedback magnitudes.
//! - [`reassign`]: per-player device assignment by device identity across hot-plug.

use crate::bindings::{PadAxis, PadButton, RawPad};
use crate::input::Controller;
use serde::{Deserialize, Serialize};

// ---- left stick -> d-pad -----------------------------------------------------------------

/// How the left stick drives the d-pad directions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StickMode {
    /// 4-way with hysteresis ([`StickDpad`]): one direction at a time (default).
    #[default]
    FourWay,
    /// Each axis independently past the dead zone (the PC retail `OldProcessJoypad` behaviour;
    /// diagonals press two directions).
    Raw,
}

/// A d-pad direction produced by the stick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StickDir {
    Up,
    Down,
    Left,
    Right,
}

impl StickDir {
    /// Unit vector in gilrs convention (+Y up).
    fn vector(self) -> (f32, f32) {
        match self {
            StickDir::Up => (0.0, 1.0),
            StickDir::Down => (0.0, -1.0),
            StickDir::Left => (-1.0, 0.0),
            StickDir::Right => (1.0, 0.0),
        }
    }
    fn nearest(x: f32, y: f32) -> StickDir {
        // Ties (exact diagonals) go to the vertical axis: forward/back hops are the common case.
        if y.abs() >= x.abs() {
            if y >= 0.0 {
                StickDir::Up
            } else {
                StickDir::Down
            }
        } else if x >= 0.0 {
            StickDir::Right
        } else {
            StickDir::Left
        }
    }
    /// (component along this direction, perpendicular magnitude).
    fn components(self, x: f32, y: f32) -> (f32, f32) {
        match self {
            StickDir::Up => (y, x.abs()),
            StickDir::Down => (-y, x.abs()),
            StickDir::Left => (-x, y.abs()),
            StickDir::Right => (x, y.abs()),
        }
    }
}

/// Stick-to-d-pad converter for one stick. State: the direction currently held.
///
/// - Engage: radial deflection >= `deadzone`; the nearest axis wins.
/// - Hold: the direction stays while deflection >= `deadzone - release_margin` and the stick
///   stays within `45° + angle_hysteresis` of its axis, so wobble around a diagonal or around
///   the dead zone edge does not release / re-press (each re-press is a new hop).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickDpad {
    pub deadzone: f32,
    /// Radial hysteresis (release threshold = `deadzone - release_margin`).
    pub release_margin: f32,
    /// Extra degrees past the 45° diagonal before the held direction gives way.
    pub angle_hysteresis_deg: f32,
    held: Option<StickDir>,
}

/// Default radial hysteresis.
pub const STICK_RELEASE_MARGIN: f32 = 0.15;
/// Default angular hysteresis in degrees.
pub const STICK_ANGLE_HYSTERESIS: f32 = 15.0;

impl StickDpad {
    pub fn new(deadzone: f32) -> StickDpad {
        StickDpad { deadzone, release_margin: STICK_RELEASE_MARGIN, angle_hysteresis_deg: STICK_ANGLE_HYSTERESIS, held: None }
    }

    pub fn held(&self) -> Option<StickDir> {
        self.held
    }

    pub fn reset(&mut self) {
        self.held = None;
    }

    /// Feeds one sample (gilrs convention, +Y up) and returns the direction to press.
    pub fn update(&mut self, x: f32, y: f32) -> Option<StickDir> {
        let press = self.deadzone.clamp(0.05, 0.95);
        let release = (press - self.release_margin).max(0.02);
        let mag = (x * x + y * y).sqrt();
        if let Some(d) = self.held {
            let (along, perp) = d.components(x, y);
            let limit = (45.0 + self.angle_hysteresis_deg).min(89.0).to_radians().tan();
            if mag >= release && along > 0.0 && perp <= along * limit {
                return self.held;
            }
            self.held = None;
        }
        if mag >= press {
            self.held = Some(StickDir::nearest(x, y));
        }
        self.held
    }

    /// Replaces the stick in `axes` (`(x, y)` indices into [`RawPad::axes`]) by the filtered
    /// direction as a full deflection on one axis (or centred).
    pub fn apply(&mut self, raw: &mut RawPad, x_axis: PadAxis, y_axis: PadAxis) {
        let d = self.update(raw.axis(x_axis), raw.axis(y_axis));
        let (fx, fy) = d.map_or((0.0, 0.0), StickDir::vector);
        raw.set_axis(x_axis, fx);
        raw.set_axis(y_axis, fy);
    }
}

// ---- face buttons -------------------------------------------------------------------------

/// Which face button is "Cross" (confirm / hop-action) on a pad.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FaceLayout {
    /// By physical position like a PS1 pad: bottom = Cross, right = Circle, left = Square,
    /// top = Triangle. Xbox A = Cross, B = Circle, X = Square, Y = Triangle; Nintendo B = Cross,
    /// A = Circle (default for every family).
    #[default]
    Positional,
    /// Nintendo only: by label, so A (right) confirms and B (bottom) backs out. Swaps
    /// South<->East and West<->North on Nintendo pads; other families stay positional.
    NintendoLabels,
}

impl FaceLayout {
    /// Applies the layout to a raw pad of `family`.
    pub fn apply(self, family: crate::bindings::ControllerFamily, raw: &mut RawPad) {
        if self == FaceLayout::NintendoLabels && family == crate::bindings::ControllerFamily::Nintendo {
            swap_buttons(raw, PadButton::South, PadButton::East);
            swap_buttons(raw, PadButton::West, PadButton::North);
        }
    }

    /// The physical button that acts as `b` (the inverse of [`FaceLayout::apply`]; the swap is
    /// its own inverse). Use it for prompts: "press Cross" -> the button to draw.
    pub fn physical(self, family: crate::bindings::ControllerFamily, b: PadButton) -> PadButton {
        if self == FaceLayout::NintendoLabels && family == crate::bindings::ControllerFamily::Nintendo {
            match b {
                PadButton::South => PadButton::East,
                PadButton::East => PadButton::South,
                PadButton::West => PadButton::North,
                PadButton::North => PadButton::West,
                o => o,
            }
        } else {
            b
        }
    }
}

/// Prompt glyph for `b` on a `family` pad: the PlayStation shape symbols (✕ ○ □ △) and
/// d-pad arrows, otherwise the [`PadButton::label`] text. Hosts whose font lacks the symbols
/// should use `label` ("Cross", ...).
pub fn glyph(b: PadButton, family: crate::bindings::ControllerFamily) -> &'static str {
    use crate::bindings::ControllerFamily::PlayStation;
    match (b, family) {
        (PadButton::South, PlayStation) => "\u{2715}",
        (PadButton::East, PlayStation) => "\u{25CB}",
        (PadButton::West, PlayStation) => "\u{25A1}",
        (PadButton::North, PlayStation) => "\u{25B3}",
        (PadButton::DPadUp, _) => "\u{2191}",
        (PadButton::DPadDown, _) => "\u{2193}",
        (PadButton::DPadLeft, _) => "\u{2190}",
        (PadButton::DPadRight, _) => "\u{2192}",
        _ => b.label(family),
    }
}

/// The physical buttons that confirm and go back in the game's menus (PS1: Cross = `C_SELECT`,
/// Triangle = `C_BACK`, `OptionsProcessController`, Common/options.c:1640-1647) on a `family`
/// pad under `layout`: `(confirm, back)`.
pub fn menu_buttons(layout: FaceLayout, family: crate::bindings::ControllerFamily) -> (PadButton, PadButton) {
    (layout.physical(family, PadButton::South), layout.physical(family, PadButton::North))
}

fn swap_buttons(raw: &mut RawPad, a: PadButton, b: PadButton) {
    let (va, vb) = (raw.button(a), raw.button(b));
    raw.set_button(a, vb);
    raw.set_button(b, va);
}

// ---- vibration ----------------------------------------------------------------------------
//
// The PS1 game never vibrates: the DualShock macros (`SHOCK_*`, PSX/islpad.h:61-80) and the
// per-frame motor manager `padHandleShock` (PSX/islpad.c:411) exist but have no call sites,
// and `padHandleInput` locks every pad in digital mode (`PadSetMainMode(port,0,3)`,
// islpad.c:258). [`ShockMotors`] ports that unused manager so PC-side rumble (an optional
// enhancement) uses the same motor model, and [`rumble_motors`] maps it to force feedback.

/// A DualShock actuator setting (`padData.motor[p]`): the small (right, high-frequency)
/// motor is on/off (`motor[0]`), the large (left, low-frequency) motor is 0..=255 (`motor[1]`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PsxVibration {
    pub small: bool,
    pub large: u8,
}

/// One pad's shock state (`shock`, `currShock`, `shockDiv`, `buzzTime` of `PadDataType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShockMotors {
    shock: i32,
    curr_shock: i32,
    shock_div: i32,
    buzz_time: i32,
    motor: PsxVibration,
}

impl ShockMotors {
    /// `SHOCK_STOP(p)`.
    pub fn stop(&mut self) {
        self.motor = PsxVibration::default();
        self.shock = 0;
        self.buzz_time = 0;
    }
    /// `SHOCK_BUZZ(p)`: small motor on until [`Self::no_buzz`].
    pub fn buzz(&mut self) {
        self.buzz_time = i32::MAX;
    }
    /// `SHOCK_NOBUZZ(p)`.
    pub fn no_buzz(&mut self) {
        self.buzz_time = 0;
    }
    /// `SHOCK_BUZZTIME(p,t)`: small motor on for `t` frames.
    pub fn buzz_time(&mut self, frames: i32) {
        self.buzz_time = frames;
    }
    /// `SHOCK_SET(p,v)`: large motor to `v` now.
    pub fn set(&mut self, v: u8) {
        self.motor.large = v;
        self.shock = (v as i32) << 16;
        self.curr_shock = self.shock;
    }
    /// `SHOCK_SLIDE(p,v,d)`: large motor slides to `v`, closing 1/`d` of the gap per frame.
    pub fn slide(&mut self, v: u8, div: i32) {
        self.shock = (v as i32) << 16;
        self.shock_div = div;
    }
    /// One frame of `padHandleShock` (islpad.c:438-453) for a connected DualShock; returns the
    /// motor values to send (`PadSetAct`).
    pub fn frame(&mut self) -> PsxVibration {
        if (self.shock >> 16) != self.motor.large as i32 {
            if self.shock_div == 0 {
                self.shock_div = 1;
            }
            self.curr_shock += (self.shock - self.curr_shock) / self.shock_div;
            self.motor.large = (self.curr_shock >> 16).clamp(0, 255) as u8;
        }
        if self.buzz_time > 0 {
            self.buzz_time -= 1;
            self.motor.small = true;
        } else {
            self.motor.small = false;
        }
        self.motor
    }
}

/// Force-feedback magnitudes `(strong, weak)` (gilrs `Strong` = low-frequency motor = the
/// DualShock large motor, `Weak` = high-frequency = small motor), scaled by the user's
/// intensity `scale` (0..=1). The small motor is a fixed-speed buzz on a DualShock: full weak
/// magnitude.
pub fn rumble_motors(v: PsxVibration, scale: f32) -> (u16, u16) {
    let s = scale.clamp(0.0, 1.0);
    let strong = (v.large as f32 / 255.0 * s * u16::MAX as f32) as u16;
    let weak = if v.small { (s * u16::MAX as f32) as u16 } else { 0 };
    (strong, weak)
}

/// `Effect::Rumble { strength, frames }` as DualShock calls: `SHOCK_SET(strength)` for the
/// duration, plus a `SHOCK_BUZZTIME(frames)` buzz when the strength is high (>= 192) so strong
/// hits use both motors like DualShock titles do.
pub fn rumble_effect_vibration(strength: u8) -> PsxVibration {
    PsxVibration { small: strength >= 192, large: strength }
}

// ---- device assignment --------------------------------------------------------------------

/// Assigns pads to players after a hot-plug. `prev` are the players' controllers whose joypad
/// numbers index `prev_ids`; `ids` are the currently connected devices in connection order
/// (`Joypad(n)` in the result indexes `ids`); `preferred` is each player's remembered device.
///
/// 1. A player keeps the device they had if it is still connected (slots may have shifted).
/// 2. A player without a device gets their remembered device when it is connected and free.
/// 3. Remaining free devices go, in connection order, to the lowest players on the keyboard
///    (so pads become players 1-4 in connection order).
pub fn reassign(prev: &[Controller; 4], prev_ids: &[String], ids: &[String], preferred: &[Option<String>; 4]) -> [Controller; 4] {
    let mut out = [Controller::Keyboard; 4];
    let mut taken = vec![false; ids.len()];
    let take = |id: &str, taken: &mut Vec<bool>| -> Option<usize> {
        let n = ids.iter().enumerate().position(|(i, x)| x == id && !taken[i])?;
        taken[n] = true;
        Some(n)
    };
    // 1. keep
    for (pl, c) in prev.iter().enumerate() {
        if let Controller::Joypad(n) = *c {
            if let Some(id) = prev_ids.get(n as usize) {
                if let Some(m) = take(id, &mut taken) {
                    out[pl] = Controller::Joypad(m as u8);
                }
            }
        }
    }
    // 2. remembered
    for pl in 0..4 {
        if out[pl] == Controller::Keyboard {
            if let Some(id) = &preferred[pl] {
                if let Some(m) = take(id, &mut taken) {
                    out[pl] = Controller::Joypad(m as u8);
                }
            }
        }
    }
    // 3. the rest, in connection order, to the lowest players still on the keyboard.
    for (n, t) in taken.iter_mut().enumerate().take(4) {
        if *t {
            continue;
        }
        if let Some(pl) = (0..4).find(|&pl| out[pl] == Controller::Keyboard) {
            out[pl] = Controller::Joypad(n as u8);
            *t = true;
        }
    }
    out
}

/// The device each player uses now (`None` on the keyboard), for remembering in the config.
pub fn assigned_ids(c: &[Controller; 4], ids: &[String]) -> [Option<String>; 4] {
    std::array::from_fn(|pl| match c[pl] {
        Controller::Joypad(n) => ids.get(n as usize).cloned(),
        Controller::Keyboard => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::ControllerFamily;
    use Controller::*;

    fn deg(a: f32, m: f32) -> (f32, f32) {
        let r = a.to_radians();
        (m * r.cos(), m * r.sin())
    }

    #[test]
    fn stick_engages_past_deadzone_on_nearest_axis() {
        let mut s = StickDpad::new(0.5);
        assert_eq!(s.update(0.3, 0.3), None); // |v| = 0.42 < 0.5
        assert_eq!(s.update(0.0, 0.6), Some(StickDir::Up));
        s.reset();
        assert_eq!(s.update(-0.9, 0.2), Some(StickDir::Left));
        s.reset();
        assert_eq!(s.update(0.1, -0.7), Some(StickDir::Down));
        s.reset();
        assert_eq!(s.update(0.8, 0.8), Some(StickDir::Up)); // exact diagonal: vertical wins
    }

    #[test]
    fn stick_angle_hysteresis_holds_near_diagonal() {
        let mut s = StickDpad::new(0.5);
        let (x, y) = deg(80.0, 1.0);
        assert_eq!(s.update(x, y), Some(StickDir::Up));
        // Wobble across the diagonal: stays Up.
        for a in [50.0, 44.0, 40.0, 35.0, 46.0, 31.0] {
            let (x, y) = deg(a, 1.0);
            assert_eq!(s.update(x, y), Some(StickDir::Up), "{a} deg");
        }
        // More than 45 + 15 degrees away from Up: Right.
        let (x, y) = deg(25.0, 1.0);
        assert_eq!(s.update(x, y), Some(StickDir::Right));
        // And back: Right holds until the stick is within 30 degrees of Up.
        let (x, y) = deg(55.0, 1.0);
        assert_eq!(s.update(x, y), Some(StickDir::Right));
        let (x, y) = deg(65.0, 1.0);
        assert_eq!(s.update(x, y), Some(StickDir::Up));
    }

    #[test]
    fn stick_radial_hysteresis() {
        let mut s = StickDpad::new(0.5);
        assert_eq!(s.update(0.55, 0.0), Some(StickDir::Right));
        assert_eq!(s.update(0.45, 0.0), Some(StickDir::Right)); // above the 0.35 release
        assert_eq!(s.update(0.36, 0.0), Some(StickDir::Right));
        assert_eq!(s.update(0.30, 0.0), None);
        assert_eq!(s.update(0.45, 0.0), None); // must pass the dead zone again
        assert_eq!(s.update(0.51, 0.0), Some(StickDir::Right));
        // Flick through the centre to the other side: switches.
        assert_eq!(s.update(-0.8, 0.0), Some(StickDir::Left));
    }

    #[test]
    fn stick_apply_rewrites_axes() {
        let mut s = StickDpad::new(0.5);
        let mut r = RawPad::default();
        r.set_axis(PadAxis::LeftX, 0.7);
        r.set_axis(PadAxis::LeftY, 0.6);
        r.set_axis(PadAxis::RightX, 0.4);
        s.apply(&mut r, PadAxis::LeftX, PadAxis::LeftY);
        assert_eq!((r.axis(PadAxis::LeftX), r.axis(PadAxis::LeftY)), (1.0, 0.0));
        assert_eq!(r.axis(PadAxis::RightX), 0.4);
        // Through the default bindings this is exactly one hop direction.
        let bits = crate::bindings::Bindings::default().pad_bits(0, &r, 0.5);
        assert_eq!(bits, crate::input::PAD_RIGHT);
    }

    #[test]
    fn nintendo_label_swap() {
        let mut r = RawPad::default();
        r.set_button(PadButton::East, true); // Nintendo "A"
        let mut p = r;
        FaceLayout::Positional.apply(ControllerFamily::Nintendo, &mut p);
        assert_eq!(p, r);
        FaceLayout::NintendoLabels.apply(ControllerFamily::Xbox, &mut p);
        assert_eq!(p, r);
        FaceLayout::NintendoLabels.apply(ControllerFamily::Nintendo, &mut p);
        assert!(p.button(PadButton::South) && !p.button(PadButton::East)); // A acts as Cross
        assert_eq!(FaceLayout::NintendoLabels.physical(ControllerFamily::Nintendo, PadButton::South), PadButton::East);
        assert_eq!(FaceLayout::NintendoLabels.physical(ControllerFamily::Nintendo, PadButton::North), PadButton::West);
        assert_eq!(FaceLayout::NintendoLabels.physical(ControllerFamily::PlayStation, PadButton::South), PadButton::South);
    }

    #[test]
    fn glyphs_and_menu_buttons() {
        assert_eq!(glyph(PadButton::South, ControllerFamily::PlayStation), "\u{2715}");
        assert_eq!(glyph(PadButton::North, ControllerFamily::PlayStation), "\u{25B3}");
        assert_eq!(glyph(PadButton::South, ControllerFamily::Xbox), "A");
        assert_eq!(glyph(PadButton::L1, ControllerFamily::PlayStation), "L1");
        assert_eq!(menu_buttons(FaceLayout::Positional, ControllerFamily::Nintendo), (PadButton::South, PadButton::North));
        assert_eq!(menu_buttons(FaceLayout::NintendoLabels, ControllerFamily::Nintendo), (PadButton::East, PadButton::West));
    }

    #[test]
    fn shock_motors_port() {
        let mut m = ShockMotors::default();
        assert_eq!(m.frame(), PsxVibration::default());
        m.set(200);
        m.buzz_time(2);
        assert_eq!(m.frame(), PsxVibration { small: true, large: 200 });
        assert_eq!(m.frame(), PsxVibration { small: true, large: 200 });
        assert_eq!(m.frame(), PsxVibration { small: false, large: 200 });
        // Slide to 0 closing half the gap per frame.
        m.slide(0, 2);
        assert_eq!(m.frame().large, 100);
        assert_eq!(m.frame().large, 50);
        m.stop();
        assert_eq!(m.frame(), PsxVibration::default());
        m.buzz();
        assert!(m.frame().small);
        m.no_buzz();
        assert!(!m.frame().small);
    }

    #[test]
    fn rumble_translation() {
        assert_eq!(rumble_motors(PsxVibration { small: false, large: 255 }, 1.0), (u16::MAX, 0));
        assert_eq!(rumble_motors(PsxVibration { small: true, large: 0 }, 1.0), (0, u16::MAX));
        assert_eq!(rumble_motors(PsxVibration { small: true, large: 255 }, 0.0), (0, 0));
        let (s, w) = rumble_motors(PsxVibration { small: true, large: 255 }, 0.5);
        assert!((32766..=32768).contains(&s) && (32766..=32768).contains(&w));
        assert_eq!(rumble_effect_vibration(100), PsxVibration { small: false, large: 100 });
        assert_eq!(rumble_effect_vibration(200), PsxVibration { small: true, large: 200 });
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn assignment_connection_order() {
        let none = [None, None, None, None];
        let kb = [Keyboard; 4];
        assert_eq!(reassign(&kb, &[], &ids(&["a", "b"]), &none), [Joypad(0), Joypad(1), Keyboard, Keyboard]);
        assert_eq!(reassign(&kb, &[], &[], &none), kb);
        let five = ids(&["a", "b", "c", "d", "e"]);
        assert_eq!(reassign(&kb, &[], &five, &none), [Joypad(0), Joypad(1), Joypad(2), Joypad(3)]);
    }

    #[test]
    fn assignment_survives_slot_shift() {
        let none = [None, None, None, None];
        let prev = [Joypad(0), Joypad(1), Joypad(2), Keyboard];
        // Pad "a" (player 1) unplugged: b and c shift down but stay with players 2 and 3.
        let c = reassign(&prev, &ids(&["a", "b", "c"]), &ids(&["b", "c"]), &none);
        assert_eq!(c, [Keyboard, Joypad(0), Joypad(1), Keyboard]);
        // A new pad goes to the free player 1.
        let c2 = reassign(&c, &ids(&["b", "c"]), &ids(&["b", "c", "x"]), &none);
        assert_eq!(c2, [Joypad(2), Joypad(0), Joypad(1), Keyboard]);
    }

    #[test]
    fn assignment_remembers_devices() {
        let kb = [Keyboard; 4];
        // Player 2 remembered "pad-b"; it connects first, player 1 gets the other pad.
        let pref = [None, Some("pad-b".to_string()), None, None];
        assert_eq!(reassign(&kb, &[], &ids(&["pad-b", "pad-a"]), &pref), [Joypad(1), Joypad(0), Keyboard, Keyboard]);
        // Player 1 lost pad-a; on replug it returns to player 1.
        let pref = [Some("pad-a".to_string()), Some("pad-b".to_string()), None, None];
        let c = reassign(&[Keyboard, Joypad(0), Keyboard, Keyboard], &ids(&["pad-b"]), &ids(&["pad-b", "pad-a"]), &pref);
        assert_eq!(c, [Joypad(1), Joypad(0), Keyboard, Keyboard]);
        // Identical devices (same id twice) are matched one each.
        let pref = [Some("same".to_string()), Some("same".to_string()), None, None];
        assert_eq!(reassign(&kb, &[], &ids(&["same", "same"]), &pref), [Joypad(0), Joypad(1), Keyboard, Keyboard]);
        assert_eq!(assigned_ids(&[Joypad(1), Keyboard, Keyboard, Keyboard], &ids(&["x", "y"])), [Some("y".into()), None, None, None]);
    }
}
