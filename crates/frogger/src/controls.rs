//! Minimal in-game rebinding screen (F10): lists player 1's actions with their bindings,
//! captures a new key / pad button / stick direction, shows conflicts. Pure state machine
//! plus drawing; the host (`app.rs`) pauses the simulation while it is open and saves the
//! config when [`Act::Changed`] is returned.

use f2_engine::{Blend, FontId, Quad2D, Renderer};
use f2_platform::bindings::{ConflictPolicy, RebindMode, ACTIONS};
use f2_platform::input::*;
use f2_platform::{dik, Bindings, ControllerFamily, RawEvent, Rebind, RebindStatus};

/// The player whose bindings the screen edits.
pub const PLAYER: u8 = 0;

/// What the host should do after an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    None,
    /// Bindings changed: re-apply to the mapper and save the config.
    Changed,
    Close,
}

pub fn action_name(action: u16) -> &'static str {
    match action {
        PAD_UP => "Up",
        PAD_DOWN => "Down",
        PAD_LEFT => "Left",
        PAD_RIGHT => "Right",
        PAD_CROSS => "Super hop / Cross",
        PAD_SQUARE => "Tongue / Square",
        PAD_START => "Start / Pause",
        PAD_CIRCLE => "Croak / Circle",
        PAD_TRIANGLE => "Triangle",
        PAD_L2 => "L2",
        PAD_R2 => "R2",
        PAD_SELECT => "Select",
        PAD_L1 => "L1",
        PAD_R1 => "R1",
        _ => "?",
    }
}

#[derive(Default)]
pub struct ControlsUi {
    pub sel: usize,
    pub rebind: Option<Rebind>,
    /// Result of the last action (conflicts, resets).
    pub msg: String,
}

fn who(pl: u8, a: u16) -> String {
    if pl == PLAYER {
        action_name(a).to_string()
    } else {
        format!("P{} {}", pl + 1, action_name(a))
    }
}

impl ControlsUi {
    /// A key went down.
    pub fn key(&mut self, b: &mut Bindings, k: u8) -> Act {
        if self.rebind.is_some() {
            return self.feed(b, RawEvent::Key(k));
        }
        match k {
            dik::ESCAPE | dik::F10 => return Act::Close,
            dik::UP => self.sel = (self.sel + ACTIONS.len() - 1) % ACTIONS.len(),
            dik::DOWN => self.sel = (self.sel + 1) % ACTIONS.len(),
            dik::RETURN | dik::NUMPADENTER => self.rebind = b.begin_rebind_mode(PLAYER, ACTIONS[self.sel], RebindMode::Replace),
            dik::A | dik::INSERT => self.rebind = b.begin_rebind_mode(PLAYER, ACTIONS[self.sel], RebindMode::Add),
            dik::BACK | dik::DELETE => {
                b.reset_action(PLAYER, ACTIONS[self.sel]);
                self.msg = format!("{} reset to default", action_name(ACTIONS[self.sel]));
                return Act::Changed;
            }
            dik::R => {
                b.reset_all();
                self.msg = "all bindings reset to default".into();
                return Act::Changed;
            }
            _ => {}
        }
        Act::None
    }

    /// A pad button / stick push (only meaningful while capturing).
    pub fn raw(&mut self, b: &mut Bindings, ev: RawEvent) -> Act {
        if self.rebind.is_some() {
            self.feed(b, ev)
        } else {
            Act::None
        }
    }

    fn feed(&mut self, b: &mut Bindings, ev: RawEvent) -> Act {
        let Some(rb) = &mut self.rebind else { return Act::None };
        match rb.feed(b, ev) {
            RebindStatus::Waiting | RebindStatus::Finished => Act::None,
            RebindStatus::Cancelled => {
                self.rebind = None;
                Act::None
            }
            RebindStatus::Captured(c) => {
                let rb = self.rebind.take().unwrap();
                b.commit_rebind(&rb, c.binding, ConflictPolicy::Unbind);
                self.msg = if c.conflicts.is_empty() {
                    format!("{} -> {}", action_name(rb.action), c.binding.label(ControllerFamily::Generic))
                } else {
                    let list: Vec<String> = c.conflicts.iter().map(|&(p, a)| who(p, a)).collect();
                    format!("{} was bound to {}: moved to {}", c.binding.label(ControllerFamily::Generic), list.join(", "), action_name(rb.action))
                };
                Act::Changed
            }
        }
    }

    pub fn draw(&self, r: &mut Renderer, font: FontId, b: &Bindings, family: ControllerFamily) {
        let ow = draw_backdrop(r, font, "CONTROLS - PLAYER 1");
        let white = [255, 255, 255, 255];
        let grey = [170, 170, 170, 255];
        let yellow = [255, 230, 80, 255];
        r.draw_text(font, 0.0, 40.0, &format!("pad prompts: {}   (Tab: gameplay / level select)", family.name()), grey, 1.0, Some(ow / 2.0));
        let (x0, x1, mut y) = (24.0, 220.0, 64.0);
        for (i, &a) in ACTIONS.iter().enumerate() {
            let sel = i == self.sel;
            let capturing = sel && self.rebind.is_some();
            let col = if sel { yellow } else { white };
            r.draw_text(font, x0 - 12.0, y, if sel { ">" } else { " " }, col, 1.0, None);
            r.draw_text(font, x0, y, action_name(a), col, 1.0, None);
            let text = if capturing {
                match self.rebind.as_ref().map(|r| r.mode) {
                    Some(RebindMode::Add) => "press a key / button to ADD (Esc cancels)".to_string(),
                    _ => "press a key / button (Esc cancels)".to_string(),
                }
            } else {
                let v: Vec<String> = b.get(PLAYER, a).iter().map(|x| x.label(family)).collect();
                if v.is_empty() {
                    "-".into()
                } else {
                    v.join(", ")
                }
            };
            r.draw_text(font, x1, y, &text, if capturing { yellow } else { grey }, 1.0, None);
            y += 18.0;
        }
        if !self.msg.is_empty() {
            r.draw_text(font, 0.0, y + 8.0, &self.msg, [120, 255, 120, 255], 1.0, Some(ow / 2.0));
        }
        r.draw_text(font, 0.0, y + 30.0, "Up/Down select   Enter rebind   A add extra", grey, 1.0, Some(ow / 2.0));
        r.draw_text(font, 0.0, y + 46.0, "Backspace reset action   R reset all   F10/Esc close", grey, 1.0, Some(ow / 2.0));
    }
}

/// The settings screen's dimmed backdrop and page title; returns the output width.
pub fn draw_backdrop(r: &mut Renderer, font: FontId, title: &str) -> f32 {
    let (iw, ih) = (r.view.width, r.view.height);
    r.draw_quad(Quad2D::rect(0.0, 0.0, iw, ih, [0.0, 0.0, 1.0, 1.0], None, [0, 0, 0, 225], Blend::Semi));
    let ow = r.output_size().0 as f32;
    r.draw_text(font, 0.0, 14.0, title, [255, 255, 255, 255], 1.5, Some(ow / 2.0));
    ow
}

#[cfg(test)]
mod tests {
    use super::*;
    use f2_platform::Binding;

    #[test]
    fn rebind_flow_and_conflict() {
        let mut b = Bindings::default();
        let mut ui = ControlsUi::default();
        assert_eq!(ui.key(&mut b, dik::DOWN), Act::None); // select Down
        assert_eq!(ui.key(&mut b, dik::RETURN), Act::None);
        assert!(ui.rebind.is_some());
        let up_key = b.get(PLAYER, PAD_UP).iter().find_map(|x| if let Binding::Key(k) = x { Some(*k) } else { None }).unwrap();
        assert_eq!(ui.key(&mut b, up_key), Act::Changed);
        assert!(ui.msg.contains("Up"), "{}", ui.msg);
        assert!(b.get(PLAYER, PAD_DOWN).contains(&Binding::Key(up_key)));
        assert!(!b.get(PLAYER, PAD_UP).contains(&Binding::Key(up_key)));
        assert_eq!(ui.key(&mut b, dik::BACK), Act::Changed);
        assert!(!b.get(PLAYER, PAD_DOWN).contains(&Binding::Key(up_key)));
        assert_eq!(ui.key(&mut b, dik::RETURN), Act::None);
        assert_eq!(ui.key(&mut b, dik::ESCAPE), Act::None); // cancels the prompt only
        assert!(ui.rebind.is_none());
        assert_eq!(ui.key(&mut b, dik::ESCAPE), Act::Close);
    }
}
