//! Multi-binding input layer (`docs/enhancements.md`, "Input"): several keyboard keys, pad
//! buttons and stick directions per action, a rebinding state machine for a controls UI,
//! per-family button glyph names, controller auto-assignment, rumble parameters and the
//! enhanced-profile hop input buffer. Everything here is pure (no devices, no gilrs), so it
//! is unit-tested; `gamepad.rs` feeds it real hardware.
//!
//! The authentic default is exactly the original: [`Bindings::default`] reproduces
//! `DEFAULT_KEYMAP` for the keyboard and the retail `joymap` + left stick for pads.

use crate::dik;
use crate::input::*;
use serde::{Deserialize, Serialize};

/// The 14 rebindable actions per player (pad bits, in original keymap order).
pub const ACTIONS: [u16; KEYS_PER_PLAYER] =
    [PAD_UP, PAD_DOWN, PAD_LEFT, PAD_RIGHT, PAD_CROSS, PAD_SQUARE, PAD_START, PAD_CIRCLE, PAD_TRIANGLE, PAD_L2, PAD_R2, PAD_SELECT, PAD_L1, PAD_R1];

fn action_index(action: u16) -> Option<usize> {
    ACTIONS.iter().position(|&a| a == action)
}

/// Gamepad buttons, gilrs-independent (positional: South = bottom face button).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PadButton {
    South,
    East,
    West,
    North,
    L1,
    R1,
    L2,
    R2,
    Select,
    Start,
    Mode,
    L3,
    R3,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

impl PadButton {
    pub const ALL: [PadButton; 17] = [
        PadButton::South,
        PadButton::East,
        PadButton::West,
        PadButton::North,
        PadButton::L1,
        PadButton::R1,
        PadButton::L2,
        PadButton::R2,
        PadButton::Select,
        PadButton::Start,
        PadButton::Mode,
        PadButton::L3,
        PadButton::R3,
        PadButton::DPadUp,
        PadButton::DPadDown,
        PadButton::DPadLeft,
        PadButton::DPadRight,
    ];
    const NAMES: [&'static str; 17] =
        ["SOUTH", "EAST", "WEST", "NORTH", "L1", "R1", "L2", "R2", "SELECT", "START", "MODE", "L3", "R3", "DPADUP", "DPADDOWN", "DPADLEFT", "DPADRIGHT"];

    fn index(self) -> usize {
        Self::ALL.iter().position(|&b| b == self).unwrap()
    }
    pub fn name(self) -> &'static str {
        Self::NAMES[self.index()]
    }
    pub fn parse(s: &str) -> Option<PadButton> {
        let u = s.trim().to_ascii_uppercase();
        Self::NAMES.iter().position(|n| *n == u).map(|i| Self::ALL[i])
    }

    /// The label shown on `family`'s controllers.
    pub fn label(self, family: ControllerFamily) -> &'static str {
        use ControllerFamily::*;
        use PadButton::*;
        match (self, family) {
            (South, Xbox) => "A",
            (East, Xbox) => "B",
            (West, Xbox) => "X",
            (North, Xbox) => "Y",
            (L1, Xbox) => "LB",
            (R1, Xbox) => "RB",
            (L2, Xbox) => "LT",
            (R2, Xbox) => "RT",
            (Select, Xbox) => "View",
            (Start, Xbox) => "Menu",
            (Mode, Xbox) => "Guide",
            (South, PlayStation) => "Cross",
            (East, PlayStation) => "Circle",
            (West, PlayStation) => "Square",
            (North, PlayStation) => "Triangle",
            (L1, PlayStation) => "L1",
            (R1, PlayStation) => "R1",
            (L2, PlayStation) => "L2",
            (R2, PlayStation) => "R2",
            (Select, PlayStation) => "Create",
            (Start, PlayStation) => "Options",
            (Mode, PlayStation) => "PS",
            // Nintendo swaps A/B and X/Y relative to Xbox positions.
            (South, Nintendo) => "B",
            (East, Nintendo) => "A",
            (West, Nintendo) => "Y",
            (North, Nintendo) => "X",
            (L1, Nintendo) => "L",
            (R1, Nintendo) => "R",
            (L2, Nintendo) => "ZL",
            (R2, Nintendo) => "ZR",
            (Select, Nintendo) => "-",
            (Start, Nintendo) => "+",
            (Mode, Nintendo) => "Home",
            (South, Generic) => "Button South",
            (East, Generic) => "Button East",
            (West, Generic) => "Button West",
            (North, Generic) => "Button North",
            (L1, Generic) => "Left Bumper",
            (R1, Generic) => "Right Bumper",
            (L2, Generic) => "Left Trigger",
            (R2, Generic) => "Right Trigger",
            (Select, Generic) => "Select",
            (Start, Generic) => "Start",
            (Mode, Generic) => "Mode",
            (L3, Xbox | PlayStation | Nintendo) => "L3",
            (R3, Xbox | PlayStation | Nintendo) => "R3",
            (L3, Generic) => "Left Stick Click",
            (R3, Generic) => "Right Stick Click",
            (DPadUp, _) => "D-pad Up",
            (DPadDown, _) => "D-pad Down",
            (DPadLeft, _) => "D-pad Left",
            (DPadRight, _) => "D-pad Right",
        }
    }
}

/// Analog stick axes (gilrs sign convention: +Y is up, +X is right).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PadAxis {
    LeftX,
    LeftY,
    RightX,
    RightY,
}

impl PadAxis {
    pub const ALL: [PadAxis; 4] = [PadAxis::LeftX, PadAxis::LeftY, PadAxis::RightX, PadAxis::RightY];
    const NAMES: [&'static str; 4] = ["LEFTX", "LEFTY", "RIGHTX", "RIGHTY"];
    fn index(self) -> usize {
        Self::ALL.iter().position(|&a| a == self).unwrap()
    }
    pub fn name(self) -> &'static str {
        Self::NAMES[self.index()]
    }
}

/// One physical input bound to an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Binding {
    /// DirectInput scan code (never 0).
    Key(u8),
    Pad(PadButton),
    /// A stick pushed past the dead zone in one direction.
    Axis {
        axis: PadAxis,
        positive: bool,
    },
}

impl Binding {
    /// Config string: `key:UP`, `pad:SOUTH`, `axis:LEFTY+`.
    pub fn to_config(self) -> String {
        match self {
            Binding::Key(k) => format!("key:{}", dik::name(k)),
            Binding::Pad(b) => format!("pad:{}", b.name()),
            Binding::Axis { axis, positive } => format!("axis:{}{}", axis.name(), if positive { '+' } else { '-' }),
        }
    }

    pub fn from_config(s: &str) -> Option<Binding> {
        let (kind, rest) = s.split_once(':')?;
        match kind.trim().to_ascii_lowercase().as_str() {
            "key" => dik::parse(rest).filter(|&k| k != 0).map(Binding::Key),
            "pad" => PadButton::parse(rest).map(Binding::Pad),
            "axis" => {
                let r = rest.trim().to_ascii_uppercase();
                let positive = match r.chars().last()? {
                    '+' => true,
                    '-' => false,
                    _ => return None,
                };
                let name = &r[..r.len() - 1];
                PadAxis::NAMES.iter().position(|n| *n == name).map(|i| Binding::Axis { axis: PadAxis::ALL[i], positive })
            }
            _ => None,
        }
    }

    /// Text for a controls screen / button prompt on a `family` controller.
    pub fn label(self, family: ControllerFamily) -> String {
        match self {
            Binding::Key(k) => dik::name(k),
            Binding::Pad(b) => b.label(family).to_string(),
            Binding::Axis { axis, positive } => {
                let stick = if matches!(axis, PadAxis::LeftX | PadAxis::LeftY) { "Left Stick" } else { "Right Stick" };
                let dir = match (axis, positive) {
                    (PadAxis::LeftX | PadAxis::RightX, true) => "Right",
                    (PadAxis::LeftX | PadAxis::RightX, false) => "Left",
                    (_, true) => "Up",
                    (_, false) => "Down",
                };
                format!("{stick} {dir}")
            }
        }
    }

    fn is_keyboard(self) -> bool {
        matches!(self, Binding::Key(_))
    }
}

/// Controller family for button prompts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ControllerFamily {
    Xbox,
    PlayStation,
    Nintendo,
    #[default]
    Generic,
}

impl ControllerFamily {
    /// Guesses the family from the device name and USB vendor id (either may be missing).
    pub fn detect(name: &str, vendor_id: Option<u16>) -> ControllerFamily {
        let n = name.to_ascii_lowercase();
        let has = |ks: &[&str]| ks.iter().any(|k| n.contains(k));
        if has(&["xbox", "x-box", "xinput", "microsoft"]) || vendor_id == Some(0x045e) {
            ControllerFamily::Xbox
        } else if has(&["playstation", "dualshock", "dualsense", "sony", "ps3", "ps4", "ps5", "wireless controller"]) || vendor_id == Some(0x054c) {
            ControllerFamily::PlayStation
        } else if has(&["nintendo", "switch", "joy-con", "pro controller"]) || vendor_id == Some(0x057e) {
            ControllerFamily::Nintendo
        } else {
            ControllerFamily::Generic
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ControllerFamily::Xbox => "Xbox",
            ControllerFamily::PlayStation => "PlayStation",
            ControllerFamily::Nintendo => "Nintendo",
            ControllerFamily::Generic => "Generic",
        }
    }
}

/// A pad's raw state: pressed buttons (bit per [`PadButton`]) and stick axes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RawPad {
    pub buttons: u32,
    /// Indexed like [`PadAxis::ALL`], -1.0..=1.0.
    pub axes: [f32; 4],
}

impl RawPad {
    pub fn set_button(&mut self, b: PadButton, down: bool) {
        let m = 1u32 << b.index();
        if down {
            self.buttons |= m
        } else {
            self.buttons &= !m
        }
    }
    pub fn button(&self, b: PadButton) -> bool {
        self.buttons & (1 << b.index()) != 0
    }
    pub fn axis(&self, a: PadAxis) -> f32 {
        self.axes[a.index()]
    }
    pub fn set_axis(&mut self, a: PadAxis, v: f32) {
        self.axes[a.index()] = v;
    }
}

/// The original dead zone as a 0..1 fraction (`DEAD_ZONE` 500 of 1000).
pub const DEFAULT_DEADZONE: f32 = 0.5;

/// A raw hardware event fed to a [`Rebind`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RawEvent {
    /// Key went down (DirectInput code).
    Key(u8),
    PadButton(PadButton),
    PadAxis {
        axis: PadAxis,
        value: f32,
    },
}

/// How a rebind treats the action's existing bindings of the same device class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebindMode {
    /// Replace the keyboard (or pad) bindings of the action with the captured one.
    Replace,
    /// Add as an extra binding.
    Add,
}

/// What to do about actions that already use the captured input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Remove the input from the other actions (steal it).
    Unbind,
    /// Keep both (the input triggers several actions).
    Allow,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Captured {
    pub binding: Binding,
    /// `(player, action)` pairs that already use `binding` (excluding the target).
    pub conflicts: Vec<(u8, u16)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RebindStatus {
    Waiting,
    /// The user cancelled (Escape).
    Cancelled,
    Captured(Captured),
    /// Already captured or cancelled; ignore further events.
    Finished,
}

/// An in-progress "press the new input" prompt.
#[derive(Clone, Debug, PartialEq)]
pub struct Rebind {
    pub player: u8,
    pub action: u16,
    pub mode: RebindMode,
    finished: bool,
}

/// Stick deflection needed to capture an axis binding.
const REBIND_AXIS_THRESHOLD: f32 = 0.7;

impl Rebind {
    /// Feeds one raw event. Escape cancels; the first key/button/strong stick push is captured
    /// (apply it with [`Bindings::commit_rebind`] after resolving conflicts).
    pub fn feed(&mut self, bindings: &Bindings, ev: RawEvent) -> RebindStatus {
        if self.finished {
            return RebindStatus::Finished;
        }
        let binding = match ev {
            RawEvent::Key(dik::ESCAPE) => {
                self.finished = true;
                return RebindStatus::Cancelled;
            }
            RawEvent::Key(0) => return RebindStatus::Waiting,
            RawEvent::Key(k) => Binding::Key(k),
            RawEvent::PadButton(b) => Binding::Pad(b),
            RawEvent::PadAxis { axis, value } if value.abs() >= REBIND_AXIS_THRESHOLD => Binding::Axis { axis, positive: value > 0.0 },
            RawEvent::PadAxis { .. } => return RebindStatus::Waiting,
        };
        self.finished = true;
        RebindStatus::Captured(Captured { binding, conflicts: bindings.conflicts(self.player, self.action, binding) })
    }
}

type Slots = [Vec<Binding>; KEYS_PER_PLAYER];

/// All players' bindings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bindings {
    slots: [Slots; 4],
}

/// One action in the config file (`[[input.bindings]]`); `player` is 1-based.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingEntry {
    pub player: u8,
    pub button: String,
    pub inputs: Vec<String>,
}

/// Retail-equivalent pad bindings for an action, plus the d-pad etc.
fn default_pad(action: u16) -> Vec<Binding> {
    use PadButton::*;
    let ax = |axis, positive| Binding::Axis { axis, positive };
    match action {
        PAD_UP => vec![Binding::Pad(DPadUp), ax(PadAxis::LeftY, true)],
        PAD_DOWN => vec![Binding::Pad(DPadDown), ax(PadAxis::LeftY, false)],
        PAD_LEFT => vec![Binding::Pad(DPadLeft), ax(PadAxis::LeftX, false)],
        PAD_RIGHT => vec![Binding::Pad(DPadRight), ax(PadAxis::LeftX, true)],
        PAD_CROSS => vec![Binding::Pad(South)],
        PAD_SQUARE => vec![Binding::Pad(West)],
        PAD_CIRCLE => vec![Binding::Pad(East)],
        PAD_TRIANGLE => vec![Binding::Pad(North)],
        PAD_START => vec![Binding::Pad(Start)],
        PAD_SELECT => vec![Binding::Pad(Select)],
        PAD_L1 => vec![Binding::Pad(L1)],
        PAD_R1 => vec![Binding::Pad(R1)],
        PAD_L2 => vec![Binding::Pad(L2)],
        PAD_R2 => vec![Binding::Pad(R2)],
        _ => vec![],
    }
}

impl Default for Bindings {
    /// Authentic: the original keymap and retail pad mapping.
    fn default() -> Self {
        Bindings::from_keymap(&Keymap::default())
    }
}

impl Bindings {
    /// Backwards-compatible load of a single-key keymap; pads get the default mapping.
    pub fn from_keymap(k: &Keymap) -> Bindings {
        let mut b = Bindings { slots: std::array::from_fn(|_| std::array::from_fn(|_| Vec::new())) };
        for pl in 0..4 {
            for (i, &a) in ACTIONS.iter().enumerate() {
                b.slots[pl][i] = default_pad(a);
            }
        }
        for e in k.0.iter() {
            if e.key == 0 || e.player > 3 {
                continue;
            }
            if let Some(i) = action_index(e.button) {
                let key = Binding::Key(e.key);
                let slot = &mut b.slots[e.player as usize][i];
                if !slot.contains(&key) {
                    // Keys first, so the first key of an action is `to_keymap`'s entry.
                    let at = slot.iter().take_while(|x| x.is_keyboard()).count();
                    slot.insert(at, key);
                }
            }
        }
        b
    }

    pub fn get(&self, player: u8, action: u16) -> &[Binding] {
        match (self.slots.get(player as usize), action_index(action)) {
            (Some(s), Some(i)) => &s[i],
            _ => &[],
        }
    }

    fn slot_mut(&mut self, player: u8, action: u16) -> Option<&mut Vec<Binding>> {
        let i = action_index(action)?;
        self.slots.get_mut(player as usize).map(|s| &mut s[i])
    }

    /// Adds a binding (no-op if present). Returns false for an unknown player/action.
    pub fn add(&mut self, player: u8, action: u16, b: Binding) -> bool {
        match self.slot_mut(player, action) {
            Some(s) => {
                if !s.contains(&b) {
                    // Keys stay ahead of pad inputs (the first key is the keymap entry).
                    let at = if b.is_keyboard() { s.iter().take_while(|x| x.is_keyboard()).count() } else { s.len() };
                    s.insert(at, b);
                }
                true
            }
            None => false,
        }
    }

    pub fn remove(&mut self, player: u8, action: u16, b: Binding) {
        if let Some(s) = self.slot_mut(player, action) {
            s.retain(|x| *x != b);
        }
    }

    /// Starts a rebind prompt; `None` for an unknown player/action.
    pub fn begin_rebind(&self, player: u8, action: u16) -> Option<Rebind> {
        self.begin_rebind_mode(player, action, RebindMode::Replace)
    }

    pub fn begin_rebind_mode(&self, player: u8, action: u16, mode: RebindMode) -> Option<Rebind> {
        (player < 4 && action_index(action).is_some()).then_some(Rebind { player, action, mode, finished: false })
    }

    /// Actions already using `b`. Keys are shared by every player; pad inputs only conflict
    /// within `player`.
    pub fn conflicts(&self, player: u8, action: u16, b: Binding) -> Vec<(u8, u16)> {
        let mut out = Vec::new();
        for pl in 0..4u8 {
            if !b.is_keyboard() && pl != player {
                continue;
            }
            for (i, &a) in ACTIONS.iter().enumerate() {
                if (pl, a) != (player, action) && self.slots[pl as usize][i].contains(&b) {
                    out.push((pl, a));
                }
            }
        }
        out
    }

    /// Applies a captured binding. `Replace` first clears the action's bindings of the same
    /// device class (keys vs pad/axis). Returns the conflicts that were resolved/left.
    pub fn commit_rebind(&mut self, rb: &Rebind, b: Binding, policy: ConflictPolicy) -> Vec<(u8, u16)> {
        let conflicts = self.conflicts(rb.player, rb.action, b);
        if policy == ConflictPolicy::Unbind {
            for &(pl, a) in &conflicts {
                self.remove(pl, a, b);
            }
        }
        if let Some(s) = self.slot_mut(rb.player, rb.action) {
            if rb.mode == RebindMode::Replace {
                s.retain(|x| x.is_keyboard() != b.is_keyboard());
            }
        }
        self.add(rb.player, rb.action, b);
        conflicts
    }

    /// Restores one action to the authentic defaults.
    pub fn reset_action(&mut self, player: u8, action: u16) {
        let v = Bindings::default().get(player, action).to_vec();
        if let Some(s) = self.slot_mut(player, action) {
            *s = v;
        }
    }

    pub fn reset_player(&mut self, player: u8) {
        if player < 4 {
            self.slots[player as usize] = Bindings::default().slots[player as usize].clone();
        }
    }

    pub fn reset_all(&mut self) {
        *self = Bindings::default();
    }

    /// The first key of each action as the original 56-entry keymap (menu keys index it).
    pub fn to_keymap(&self) -> Keymap {
        let mut m = DEFAULT_KEYMAP;
        for (n, e) in m.iter_mut().enumerate() {
            let (pl, i) = (n / KEYS_PER_PLAYER, n % KEYS_PER_PLAYER);
            *e = KeyEntry {
                player: pl as u8,
                button: ACTIONS[i],
                key: self.slots[pl][i].iter().find_map(|b| if let Binding::Key(k) = b { Some(*k) } else { None }).unwrap_or(0),
            };
        }
        Keymap(m)
    }

    /// Second and later keys of each action (processed in addition to [`Self::to_keymap`]).
    pub fn extra_keys(&self) -> Vec<KeyEntry> {
        let mut out = Vec::new();
        for pl in 0..4 {
            for (i, s) in self.slots[pl].iter().enumerate() {
                for b in s.iter().filter(|b| b.is_keyboard()).skip(1) {
                    if let Binding::Key(k) = b {
                        out.push(KeyEntry { player: pl as u8, button: ACTIONS[i], key: *k });
                    }
                }
            }
        }
        out
    }

    /// Pad bits produced by `raw` for `player` (buttons and sticks past `deadzone`).
    pub fn pad_bits(&self, player: u8, raw: &RawPad, deadzone: f32) -> u16 {
        let dz = deadzone.clamp(0.0, 0.95);
        let mut bits = 0;
        for (i, &a) in ACTIONS.iter().enumerate() {
            let hit = self.slots[(player & 3) as usize][i].iter().any(|b| match *b {
                Binding::Key(_) => false,
                Binding::Pad(p) => raw.button(p),
                Binding::Axis { axis, positive } => {
                    let v = raw.axis(axis);
                    if positive {
                        v > dz
                    } else {
                        v < -dz
                    }
                }
            });
            if hit {
                bits |= a;
            }
        }
        bits
    }

    /// Joypad states for `InputMapper::process`: joypad `n` gets the bindings of the first
    /// player assigned to it. Bits ride in [`JoypadState::extra`].
    pub fn joypad_states(&self, controllers: &[Controller; 4], raw: &[Option<RawPad>], deadzone: f32) -> [Option<JoypadState>; 4] {
        let mut out = [None; 4];
        for (pl, c) in controllers.iter().enumerate() {
            if let Controller::Joypad(n) = *c {
                let n = n as usize;
                if n < 4 && out[n].is_none() {
                    if let Some(Some(r)) = raw.get(n) {
                        out[n] = Some(JoypadState { extra: self.pad_bits(pl as u8, r, deadzone), ..Default::default() });
                    }
                }
            }
        }
        out
    }

    pub fn to_entries(&self) -> Vec<BindingEntry> {
        let mut out = Vec::new();
        for pl in 0..4 {
            for (i, &a) in ACTIONS.iter().enumerate() {
                out.push(BindingEntry {
                    player: pl as u8 + 1,
                    button: pad_bit_name(a).unwrap().into(),
                    inputs: self.slots[pl][i].iter().map(|b| b.to_config()).collect(),
                });
            }
        }
        out
    }

    /// Entries not listed keep the authentic defaults (so a partial file works).
    pub fn from_entries(entries: &[BindingEntry]) -> Result<Bindings, String> {
        let mut b = Bindings::default();
        for e in entries {
            if !(1..=4).contains(&e.player) {
                return Err(format!("bad player {}", e.player));
            }
            let bit = parse_pad_bit(&e.button).ok_or_else(|| format!("bad button {:?}", e.button))?;
            let mut v = Vec::new();
            for s in &e.inputs {
                let bd = Binding::from_config(s).ok_or_else(|| format!("bad binding {s:?}"))?;
                if !v.contains(&bd) {
                    v.push(bd);
                }
            }
            *b.slot_mut(e.player - 1, bit).ok_or_else(|| format!("{:?} is not rebindable", e.button))? = v;
        }
        Ok(b)
    }
}

/// Keeps valid pad assignments, drops ones whose pad is gone (back to keyboard) and gives
/// unassigned pads to the lowest players still on the keyboard. Use after hot-plug.
pub fn auto_assign(current: &[Controller; 4], num_pads: usize) -> [Controller; 4] {
    let mut out = *current;
    let mut used = [false; 4];
    for c in out.iter_mut() {
        match *c {
            Controller::Joypad(n) if (n as usize) < num_pads.min(4) && !used[n as usize] => used[n as usize] = true,
            Controller::Joypad(_) => *c = Controller::Keyboard,
            Controller::Keyboard => {}
        }
    }
    for (n, u) in used.iter().enumerate().take(num_pads.min(4)) {
        if !u {
            if let Some(c) = out.iter_mut().find(|c| **c == Controller::Keyboard) {
                *c = Controller::Joypad(n as u8);
            }
        }
    }
    out
}

/// Rumble motor magnitudes for the game's `Effect::Rumble { strength, frames }` (60 Hz
/// frames): `(strong, weak, milliseconds)`. `scale` is the user's rumble intensity (0..=1).
pub fn rumble_params(strength: u8, frames: u16, scale: f32) -> (u16, u16, u32) {
    let s = (strength as f32 / 255.0 * scale.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    let strong = (s * u16::MAX as f32) as u16;
    let weak = (s * 0.5 * u16::MAX as f32) as u16;
    (strong, weak, frames as u32 * 1000 / 60)
}

const HOP_DIRS: u16 = PAD_UP | PAD_DOWN | PAD_LEFT | PAD_RIGHT;

/// Enhanced-profile hop buffer: remembers a direction press the frog could not act on and
/// re-presents it for up to `window` frames. One per player; `window == 0` is a pass-through
/// (the authentic default).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HopBuffer {
    pub window: u32,
    pending: Option<(u16, u32)>,
}

impl HopBuffer {
    pub fn new(window: u32) -> HopBuffer {
        HopBuffer { window, pending: None }
    }

    pub fn clear(&mut self) {
        self.pending = None;
    }

    /// Call once per game frame with the freshly mapped pad state. `can_accept` is true when
    /// the frog is able to start a hop this frame. Returns the state to give the game.
    pub fn apply(&mut self, mut pad: PadState, can_accept: bool) -> PadState {
        if self.window == 0 {
            self.pending = None;
            return pad;
        }
        let fresh = pad.debounce & HOP_DIRS;
        if fresh != 0 {
            if can_accept {
                self.pending = None; // the game sees the real press
            } else {
                // Lowest set direction wins if several arrive together.
                self.pending = Some((1 << fresh.trailing_zeros(), self.window));
            }
            return pad;
        }
        if let Some((dir, left)) = self.pending {
            if can_accept {
                pad.debounce |= dir;
                pad.buttons |= dir;
                self.pending = None;
            } else if left <= 1 {
                self.pending = None;
            } else {
                self.pending = Some((dir, left - 1));
            }
        }
        pad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrips_original_keymap() {
        let b = Bindings::default();
        assert_eq!(b.to_keymap(), Keymap::default());
        assert!(b.extra_keys().is_empty());
        assert_eq!(Bindings::from_entries(&b.to_entries()).unwrap(), b);
    }

    #[test]
    fn config_strings() {
        for b in [Binding::Key(dik::RETURN), Binding::Pad(PadButton::DPadUp), Binding::Axis { axis: PadAxis::RightY, positive: false }] {
            assert_eq!(Binding::from_config(&b.to_config()), Some(b));
        }
        assert_eq!(Binding::from_config("key:NONE"), None);
        assert_eq!(Binding::from_config("axis:LEFTX"), None);
        assert_eq!(Binding::from_config("bogus"), None);
    }

    #[test]
    fn retail_pad_mapping_and_deadzone() {
        let b = Bindings::default();
        let mut r = RawPad::default();
        r.set_button(PadButton::South, true);
        r.set_axis(PadAxis::LeftX, 0.5); // exactly the dead zone: not past it
        assert_eq!(b.pad_bits(0, &r, DEFAULT_DEADZONE), PAD_CROSS);
        r.set_axis(PadAxis::LeftX, 0.51);
        r.set_axis(PadAxis::LeftY, -0.9);
        assert_eq!(b.pad_bits(0, &r, DEFAULT_DEADZONE), PAD_CROSS | PAD_RIGHT | PAD_DOWN);
        assert_eq!(b.pad_bits(0, &r, 0.95) & PAD_RIGHT, 0); // 0.51 < 0.95
        let controllers = [Controller::Joypad(1), Controller::Keyboard, Controller::Keyboard, Controller::Keyboard];
        let js = b.joypad_states(&controllers, &[None, Some(r)], 0.2);
        assert!(js[0].is_none());
        assert_eq!(js[1].unwrap().extra & PAD_CROSS, PAD_CROSS);
    }

    #[test]
    fn multiple_keys_via_mapper() {
        let mut b = Bindings::default();
        b.add(0, PAD_UP, Binding::Key(dik::W));
        let m = InputMapper { keymap: b.to_keymap(), extra_keys: b.extra_keys(), ..Default::default() };
        let mut kb = KeyboardState::default();
        kb.key_down(dik::W);
        let mut pad = PadData::default();
        m.process(&mut pad, &kb, &[]);
        assert_eq!(pad.digital[0], PAD_UP);
        assert_eq!(pad.debounce[0], PAD_UP);
    }

    #[test]
    fn rebind_flow_and_conflicts() {
        let mut b = Bindings::default();
        let mut rb = b.begin_rebind(0, PAD_CROSS).unwrap();
        assert_eq!(rb.feed(&b, RawEvent::PadAxis { axis: PadAxis::LeftX, value: 0.3 }), RebindStatus::Waiting);
        // Capturing UP's key conflicts with player 0 UP.
        let RebindStatus::Captured(c) = rb.feed(&b, RawEvent::Key(dik::UP)) else { panic!() };
        assert_eq!(c.conflicts, vec![(0, PAD_UP)]);
        assert_eq!(rb.feed(&b, RawEvent::Key(dik::A)), RebindStatus::Finished);
        let left = b.commit_rebind(&rb, c.binding, ConflictPolicy::Unbind);
        assert_eq!(left, vec![(0, PAD_UP)]);
        assert_eq!(b.get(0, PAD_CROSS)[0], Binding::Key(dik::UP));
        assert!(!b.get(0, PAD_UP).contains(&Binding::Key(dik::UP)));
        // Replace kept the pad binding.
        assert!(b.get(0, PAD_CROSS).contains(&Binding::Pad(PadButton::South)));
        // Pad inputs don't conflict across players.
        assert!(b.conflicts(1, PAD_CROSS, Binding::Pad(PadButton::East)).iter().all(|c| c.0 == 1));
        // Cancel.
        let mut rb = b.begin_rebind(0, PAD_CROSS).unwrap();
        assert_eq!(rb.feed(&b, RawEvent::Key(dik::ESCAPE)), RebindStatus::Cancelled);
        assert!(b.begin_rebind(0, PAD_L3).is_none());
        // Reset.
        b.reset_action(0, PAD_CROSS);
        b.reset_action(0, PAD_UP);
        assert_eq!(b, Bindings::default());
    }

    #[test]
    fn add_mode_and_reset_all() {
        let mut b = Bindings::default();
        let rb = b.begin_rebind_mode(1, PAD_UP, RebindMode::Add).unwrap();
        b.commit_rebind(&rb, Binding::Key(dik::T), ConflictPolicy::Allow);
        assert_eq!(b.get(1, PAD_UP)[..2], [Binding::Key(dik::Q), Binding::Key(dik::T)]);
        assert_eq!(b.extra_keys(), vec![KeyEntry { player: 1, button: PAD_UP, key: dik::T }]);
        b.reset_all();
        assert_eq!(b, Bindings::default());
    }

    #[test]
    fn legacy_keymap_loads() {
        let mut k = Keymap::default();
        k.bind(0, PAD_CROSS, dik::SPACE);
        let b = Bindings::from_keymap(&k);
        assert_eq!(b.to_keymap(), k);
        assert_eq!(b.get(0, PAD_CROSS)[0], Binding::Key(dik::SPACE));
    }

    #[test]
    fn glyphs() {
        use ControllerFamily::*;
        assert_eq!(PadButton::South.label(Xbox), "A");
        assert_eq!(PadButton::South.label(PlayStation), "Cross");
        assert_eq!(PadButton::South.label(Nintendo), "B");
        assert_eq!(PadButton::Start.label(Nintendo), "+");
        assert_eq!(Binding::Axis { axis: PadAxis::LeftY, positive: true }.label(Generic), "Left Stick Up");
        assert_eq!(Binding::Key(dik::RETURN).label(Xbox), "RETURN");
        assert_eq!(ControllerFamily::detect("Xbox 360 Controller", None), Xbox);
        assert_eq!(ControllerFamily::detect("Wireless Controller", Some(0x054c)), PlayStation);
        assert_eq!(ControllerFamily::detect("Pro Controller", None), Nintendo);
        assert_eq!(ControllerFamily::detect("Logitech F310", Some(0x046d)), Generic);
        for f in [Xbox, PlayStation, Nintendo, Generic] {
            for b in PadButton::ALL {
                assert!(!b.label(f).is_empty());
            }
        }
    }

    #[test]
    fn assignment() {
        use Controller::*;
        let kb = [Keyboard; 4];
        assert_eq!(auto_assign(&kb, 2), [Joypad(0), Joypad(1), Keyboard, Keyboard]);
        assert_eq!(auto_assign(&[Joypad(1), Keyboard, Keyboard, Keyboard], 1), [Joypad(0), Keyboard, Keyboard, Keyboard]);
        assert_eq!(auto_assign(&[Joypad(0), Joypad(0), Keyboard, Keyboard], 1)[1], Keyboard);
        assert_eq!(auto_assign(&kb, 0), kb);
    }

    #[test]
    fn rumble_scaling() {
        assert_eq!(rumble_params(0, 10, 1.0), (0, 0, 166));
        let (s, w, ms) = rumble_params(255, 60, 1.0);
        assert_eq!((s, ms), (u16::MAX, 1000));
        assert!(w < s);
        assert_eq!(rumble_params(255, 6, 0.0).0, 0);
    }

    fn press(dir: u16) -> PadState {
        PadState { buttons: dir, debounce: dir }
    }

    #[test]
    fn hop_buffer() {
        let mut h = HopBuffer::new(3);
        // Press while the frog is busy: swallowed by the game, re-presented when ready.
        assert_eq!(h.apply(press(PAD_UP), false), press(PAD_UP));
        assert_eq!(h.apply(PadState::default(), false), PadState::default());
        assert_eq!(h.apply(PadState::default(), true), press(PAD_UP));
        assert_eq!(h.apply(PadState::default(), true), PadState::default()); // consumed
                                                                             // Expires after the window.
        h.apply(press(PAD_LEFT), false);
        for _ in 0..3 {
            h.apply(PadState::default(), false);
        }
        assert_eq!(h.apply(PadState::default(), true), PadState::default());
        // A newer press replaces the older one; a real press while ready clears it.
        h.apply(press(PAD_LEFT), false);
        h.apply(press(PAD_RIGHT), false);
        assert_eq!(h.apply(PadState::default(), true).debounce, PAD_RIGHT);
        h.apply(press(PAD_LEFT), false);
        h.apply(press(PAD_UP), true);
        assert_eq!(h.apply(PadState::default(), true), PadState::default());
        // Authentic default: off.
        let mut off = HopBuffer::default();
        off.apply(press(PAD_UP), false);
        assert_eq!(off.apply(PadState::default(), true), PadState::default());
    }
}
