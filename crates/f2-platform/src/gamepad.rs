//! Gamepads through gilrs.
//!
//! Two views of the same hardware:
//! - [`Gamepads::poll`]: the legacy `DIJOYSTATE`-like [`JoypadState`] (retail mapping with the
//!   original dead zone; d-pad, shoulders etc. in `extra`).
//! - [`Gamepads::poll_raw`]: [`RawPad`] states for the multi-binding layer
//!   ([`crate::bindings::Bindings::joypad_states`]), plus [`Gamepads::take_events`] for hot-plug
//!   notifications and raw button/stick events for the rebinding UI.
//!
//! Rumble goes through gilrs force feedback and is a silent no-op where unsupported.
//!
//! PS1-style handling (`crate::controller`, docs/CONTROLLERS.md): `poll_raw` applies the face
//! layout (Nintendo label swap) and the left-stick 4-way filter per pad; [`Gamepads::reassign`]
//! keeps players on their own pads across hot-plug and remembers devices by id.

use crate::bindings::*;
use crate::controller::{FaceLayout, StickDpad, StickMode};
use crate::input::*;
use gilrs::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder, Replay, Ticks};
use gilrs::{Axis, Button, GamepadId, Gilrs};
use std::time::{Duration, Instant};

pub struct Gamepads {
    gilrs: Gilrs,
    /// Joypad slots in connection order (`EnumDevices` order in the original).
    slots: Vec<GamepadId>,
    events: Vec<PadEvent>,
    /// Player -> device assignment, used by [`Gamepads::rumble`].
    controllers: [Controller; 4],
    /// Rumble intensity 0..=1 (0 = off).
    pub rumble_scale: f32,
    /// Running rumble effects per slot (dropping an effect stops it).
    active: Vec<(usize, Effect, Instant)>,
    /// Left stick -> d-pad filter per slot (parallel to `slots`).
    sticks: Vec<StickDpad>,
    /// Left stick handling (`[input] stick_mode`).
    pub stick_mode: StickMode,
    /// Stick dead zone used by the 4-way filter (`[input] deadzone`).
    pub deadzone: f32,
    /// Face button layout (`[input] face_layout`).
    pub face_layout: FaceLayout,
    /// Device ids of the slots when `controllers` was last assigned (`Joypad(n)` indexes it).
    assigned_ids: Vec<String>,
    /// Remembered device per player (`[input] pad_devices`).
    preferred: [Option<String>; 4],
}

/// At most 4 joypads (`MAXJOYPADS`).
pub const MAXJOYPADS: usize = 4;

/// Something the host should react to.
#[derive(Clone, Debug, PartialEq)]
pub enum PadEvent {
    /// A pad took joypad slot `slot`.
    Connected { slot: usize, name: String, family: ControllerFamily },
    /// The pad in `slot` left; later slots shift down by one.
    Disconnected { slot: usize },
    /// A raw press / strong stick movement on joypad `slot` (for [`Rebind::feed`]).
    Raw { slot: usize, event: RawEvent },
}

fn map_button(b: Button) -> Option<PadButton> {
    Some(match b {
        Button::South => PadButton::South,
        Button::East => PadButton::East,
        Button::West => PadButton::West,
        Button::North => PadButton::North,
        Button::LeftTrigger => PadButton::L1,
        Button::RightTrigger => PadButton::R1,
        Button::LeftTrigger2 => PadButton::L2,
        Button::RightTrigger2 => PadButton::R2,
        Button::Select => PadButton::Select,
        Button::Start => PadButton::Start,
        Button::Mode => PadButton::Mode,
        Button::LeftThumb => PadButton::L3,
        Button::RightThumb => PadButton::R3,
        Button::DPadUp => PadButton::DPadUp,
        Button::DPadDown => PadButton::DPadDown,
        Button::DPadLeft => PadButton::DPadLeft,
        Button::DPadRight => PadButton::DPadRight,
        _ => return None,
    })
}

fn map_axis(a: Axis) -> Option<PadAxis> {
    Some(match a {
        Axis::LeftStickX => PadAxis::LeftX,
        Axis::LeftStickY => PadAxis::LeftY,
        Axis::RightStickX => PadAxis::RightX,
        Axis::RightStickY => PadAxis::RightY,
        _ => return None,
    })
}

impl Gamepads {
    pub fn new() -> Result<Gamepads, String> {
        let gilrs = Gilrs::new().map_err(|e| e.to_string())?;
        let slots: Vec<_> = gilrs.gamepads().filter(|(_, g)| g.is_connected()).map(|(id, _)| id).take(MAXJOYPADS).collect();
        let sticks = vec![StickDpad::new(DEFAULT_DEADZONE); slots.len()];
        let mut pads = Gamepads {
            gilrs,
            slots,
            events: Vec::new(),
            controllers: [Controller::Keyboard; 4],
            rumble_scale: 1.0,
            active: Vec::new(),
            sticks,
            stick_mode: StickMode::default(),
            deadzone: DEFAULT_DEADZONE,
            face_layout: FaceLayout::default(),
            assigned_ids: Vec::new(),
            preferred: Default::default(),
        };
        pads.assigned_ids = pads.ids();
        for slot in 0..pads.slots.len() {
            pads.events.push(pads.connected_event(slot));
        }
        Ok(pads)
    }

    pub fn count(&self) -> usize {
        self.slots.len()
    }

    fn connected_event(&self, slot: usize) -> PadEvent {
        PadEvent::Connected { slot, name: self.name(slot).unwrap_or_default(), family: self.family(slot) }
    }

    /// Device name of joypad `slot`.
    pub fn name(&self, slot: usize) -> Option<String> {
        self.slots.get(slot).map(|&id| self.gilrs.gamepad(id).name().to_string())
    }

    /// Stable-ish id of joypad `slot` for remembering assignments: the SDL-style GUID (bus,
    /// vendor, product, version) as hex. Identical pads share it; [`crate::controller::reassign`]
    /// matches them one each.
    pub fn device_id(&self, slot: usize) -> Option<String> {
        self.slots.get(slot).map(|&id| self.gilrs.gamepad(id).uuid().iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Device ids of all connected joypads in slot order.
    pub fn ids(&self) -> Vec<String> {
        (0..self.slots.len()).filter_map(|s| self.device_id(s)).collect()
    }

    /// Sets the remembered device per player (config `pad_devices`, empty string = none).
    pub fn set_preferred(&mut self, devices: &[String; 4]) {
        self.preferred = std::array::from_fn(|i| (!devices[i].is_empty()).then(|| devices[i].clone()));
    }

    /// The remembered device per player, for saving (empty string = none).
    pub fn preferred(&self) -> [String; 4] {
        std::array::from_fn(|i| self.preferred[i].clone().unwrap_or_default())
    }

    /// Re-assigns pads to players after hot-plug (or at startup with the configured
    /// `controllers`): players keep their own pads even when slots shift, remembered pads
    /// return to their players, new pads go to the lowest keyboard players. Updates the rumble
    /// routing and the remembered devices; returns the new `controllers` for the mapper.
    // controller
    pub fn reassign(&mut self, current: &[Controller; 4]) -> [Controller; 4] {
        let ids = self.ids();
        let c = crate::controller::reassign(current, &self.assigned_ids, &ids, &self.preferred);
        for (pl, id) in crate::controller::assigned_ids(&c, &ids).into_iter().enumerate() {
            if id.is_some() {
                self.preferred[pl] = id;
            }
        }
        self.assigned_ids = ids;
        self.controllers = c;
        c
    }

    /// Controller family of joypad `slot` ([`ControllerFamily::Generic`] if absent).
    pub fn family(&self, slot: usize) -> ControllerFamily {
        match self.slots.get(slot) {
            Some(&id) => {
                let g = self.gilrs.gamepad(id);
                ControllerFamily::detect(g.name(), g.vendor_id())
            }
            None => ControllerFamily::Generic,
        }
    }

    /// The family whose glyphs to show for `player`'s prompts (the keyboard counts as generic).
    pub fn family_for_player(&self, player: usize) -> ControllerFamily {
        match self.controllers.get(player) {
            Some(Controller::Joypad(n)) => self.family(*n as usize),
            _ => ControllerFamily::Generic,
        }
    }

    /// Tells the pads which device each player uses (for rumble routing).
    pub fn set_controllers(&mut self, c: [Controller; 4]) {
        self.controllers = c;
    }

    /// Drains gilrs events: hot-plug, queued host events and raw rebinding events.
    fn pump(&mut self) {
        self.active.retain(|(_, _, until)| *until > Instant::now());
        while let Some(ev) = self.gilrs.next_event() {
            match ev.event {
                gilrs::EventType::Connected => {
                    if !self.slots.contains(&ev.id) && self.slots.len() < MAXJOYPADS {
                        self.slots.push(ev.id);
                        self.sticks.push(StickDpad::new(self.deadzone));
                        let slot = self.slots.len() - 1;
                        let e = self.connected_event(slot);
                        self.events.push(e);
                    }
                }
                gilrs::EventType::Disconnected => {
                    if let Some(slot) = self.slots.iter().position(|&id| id == ev.id) {
                        self.slots.remove(slot);
                        self.sticks.remove(slot);
                        // Effects are keyed by slot; later slots shift down.
                        self.active.retain(|(s, _, _)| *s != slot);
                        for a in self.active.iter_mut().filter(|a| a.0 > slot) {
                            a.0 -= 1;
                        }
                        self.events.push(PadEvent::Disconnected { slot });
                    }
                }
                gilrs::EventType::ButtonPressed(b, _) => {
                    if let (Some(slot), Some(b)) = (self.slots.iter().position(|&id| id == ev.id), map_button(b)) {
                        self.events.push(PadEvent::Raw { slot, event: RawEvent::PadButton(b) });
                    }
                }
                gilrs::EventType::AxisChanged(a, v, _) => {
                    if let (Some(slot), Some(axis)) = (self.slots.iter().position(|&id| id == ev.id), map_axis(a)) {
                        if v.abs() >= 0.7 {
                            self.events.push(PadEvent::Raw { slot, event: RawEvent::PadAxis { axis, value: v } });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Events gathered by the last `poll`/`poll_raw` calls (empties the queue).
    pub fn take_events(&mut self) -> Vec<PadEvent> {
        std::mem::take(&mut self.events)
    }

    /// Raw state of joypads 0..4 for [`Bindings::joypad_states`].
    pub fn poll_raw(&mut self) -> [Option<RawPad>; MAXJOYPADS] {
        self.pump();
        let mut out = [None; MAXJOYPADS];
        for (i, &id) in self.slots.iter().enumerate() {
            let g = self.gilrs.gamepad(id);
            if !g.is_connected() {
                continue;
            }
            let mut r = RawPad::default();
            for b in PadButton::ALL {
                let gb = match b {
                    PadButton::South => Button::South,
                    PadButton::East => Button::East,
                    PadButton::West => Button::West,
                    PadButton::North => Button::North,
                    PadButton::L1 => Button::LeftTrigger,
                    PadButton::R1 => Button::RightTrigger,
                    PadButton::L2 => Button::LeftTrigger2,
                    PadButton::R2 => Button::RightTrigger2,
                    PadButton::Select => Button::Select,
                    PadButton::Start => Button::Start,
                    PadButton::Mode => Button::Mode,
                    PadButton::L3 => Button::LeftThumb,
                    PadButton::R3 => Button::RightThumb,
                    PadButton::DPadUp => Button::DPadUp,
                    PadButton::DPadDown => Button::DPadDown,
                    PadButton::DPadLeft => Button::DPadLeft,
                    PadButton::DPadRight => Button::DPadRight,
                };
                r.set_button(b, g.is_pressed(gb));
            }
            r.set_axis(PadAxis::LeftX, g.value(Axis::LeftStickX));
            r.set_axis(PadAxis::LeftY, g.value(Axis::LeftStickY));
            r.set_axis(PadAxis::RightX, g.value(Axis::RightStickX));
            r.set_axis(PadAxis::RightY, g.value(Axis::RightStickY));
            // controller: PS1 layout options and the left stick as a 4-way d-pad.
            self.face_layout.apply(ControllerFamily::detect(g.name(), g.vendor_id()), &mut r);
            if let (StickMode::FourWay, Some(st)) = (self.stick_mode, self.sticks.get_mut(i)) {
                st.deadzone = self.deadzone;
                st.apply(&mut r, PadAxis::LeftX, PadAxis::LeftY);
            }
            out[i] = Some(r);
        }
        out
    }

    /// Drains events (hot-plug) and returns the state of joypads 0..4 in the retail layout.
    pub fn poll(&mut self) -> [Option<JoypadState>; MAXJOYPADS] {
        self.pump();
        let mut out = [None; MAXJOYPADS];
        for (i, &id) in self.slots.iter().enumerate() {
            let g = self.gilrs.gamepad(id);
            if !g.is_connected() {
                continue;
            }
            let pressed = |b| g.is_pressed(b);
            let mut j = JoypadState {
                buttons: [pressed(Button::South), pressed(Button::West), pressed(Button::East), pressed(Button::North), pressed(Button::Start), false],
                // gilrs: +Y is up; DirectInput: -Y is up.
                x: (g.value(Axis::LeftStickX) * 1000.0) as i32,
                y: (-g.value(Axis::LeftStickY) * 1000.0) as i32,
                extra: 0,
            };
            for (b, bit) in [
                (Button::DPadUp, PAD_UP),
                (Button::DPadDown, PAD_DOWN),
                (Button::DPadLeft, PAD_LEFT),
                (Button::DPadRight, PAD_RIGHT),
                (Button::Select, PAD_SELECT),
                (Button::LeftTrigger, PAD_L1),
                (Button::RightTrigger, PAD_R1),
                (Button::LeftTrigger2, PAD_L2),
                (Button::RightTrigger2, PAD_R2),
                (Button::LeftThumb, PAD_L3),
                (Button::RightThumb, PAD_R3),
            ] {
                if pressed(b) {
                    j.extra |= bit;
                }
            }
            out[i] = Some(j);
        }
        out
    }

    /// Maps the game's `Effect::Rumble { player, strength, frames }` to the pad assigned to
    /// `player` (needs [`Gamepads::set_controllers`]). Silent no-op when the player is on the
    /// keyboard, the pad has no force feedback, or `rumble_scale` is 0. Returns whether a
    /// rumble started.
    pub fn rumble(&mut self, player: u8, strength: u8, frames: u16) -> bool {
        let Some(Controller::Joypad(n)) = self.controllers.get(player as usize).copied() else { return false };
        let slot = n as usize;
        let Some(&id) = self.slots.get(slot) else { return false };
        let (strong, weak, ms) = rumble_params(strength, frames, self.rumble_scale);
        if ms == 0 || (strong == 0 && weak == 0) || !self.gilrs.gamepad(id).is_ff_supported() {
            return false;
        }
        let replay = Replay { play_for: Ticks::from_ms(ms), ..Default::default() };
        let built = EffectBuilder::new()
            .add_effect(BaseEffect { kind: BaseEffectType::Strong { magnitude: strong }, scheduling: replay, ..Default::default() })
            .add_effect(BaseEffect { kind: BaseEffectType::Weak { magnitude: weak }, scheduling: replay, ..Default::default() })
            .gamepads(&[id])
            .finish(&mut self.gilrs);
        match built {
            Ok(effect) if effect.play().is_ok() => {
                self.active.retain(|(s, _, _)| *s != slot); // replaces (drops) the previous one
                self.active.push((slot, effect, Instant::now() + Duration::from_millis(ms as u64)));
                true
            }
            _ => false,
        }
    }
}
