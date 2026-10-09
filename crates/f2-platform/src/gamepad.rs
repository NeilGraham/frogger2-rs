//! Gamepads through gilrs, read into the original `DIJOYSTATE`-like [`JoypadState`].
//!
//! Face buttons are mapped by position (PlayStation layout; Xbox A/X/B/Y = Cross/Square/
//! Circle/Triangle) into the original DirectInput button slots of `joymap`
//! (0 Cross, 1 Square, 2 Circle, 3 Triangle, 4 Start). The left stick is the `lX`/`lY` axis pair
//! with the original dead zone. D-pad, Select, shoulders/triggers and stick clicks — ignored
//! by the original — go to [`JoypadState::extra`].

use crate::input::*;
use gilrs::{Axis, Button, GamepadId, Gilrs};

pub struct Gamepads {
    gilrs: Gilrs,
    /// Joypad slots in connection order (`EnumDevices` order in the original).
    slots: Vec<GamepadId>,
}

/// At most 4 joypads (`MAXJOYPADS`).
pub const MAXJOYPADS: usize = 4;

impl Gamepads {
    pub fn new() -> Result<Gamepads, String> {
        let gilrs = Gilrs::new().map_err(|e| e.to_string())?;
        let slots = gilrs.gamepads().filter(|(_, g)| g.is_connected()).map(|(id, _)| id).take(MAXJOYPADS).collect();
        Ok(Gamepads { gilrs, slots })
    }

    pub fn count(&self) -> usize {
        self.slots.len()
    }

    /// Drains events (hot-plug) and returns the state of joypads 0..4.
    pub fn poll(&mut self) -> [Option<JoypadState>; MAXJOYPADS] {
        while let Some(ev) = self.gilrs.next_event() {
            match ev.event {
                gilrs::EventType::Connected => {
                    if !self.slots.contains(&ev.id) && self.slots.len() < MAXJOYPADS {
                        self.slots.push(ev.id);
                    }
                }
                gilrs::EventType::Disconnected => self.slots.retain(|&id| id != ev.id),
                _ => {}
            }
        }
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
}
