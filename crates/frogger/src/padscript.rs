//! `--pad-script FILE`: scripted input for player 1 (headless screenshots, repro runs).
//!
//! ```text
//! # comment
//! 120 start          # hold START from tick 120
//! 122 none           # release
//! 200 cross+right    # several buttons
//! 210 0x4000         # raw PAD_* mask
//! 300 shot book.png  # headless: screenshot before tick 300
//! ```

use f2_game::levplay::*;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    /// Hold these `PAD_*` buttons from the tick on.
    Pad(u16),
    /// Write a screenshot (headless only).
    Shot(PathBuf),
}

#[derive(Clone, Debug, Default)]
pub struct PadScript {
    /// `(tick, command)`, sorted by tick (stable: file order within a tick).
    pub events: Vec<(u64, Cmd)>,
}

/// `PAD_*` bit of a button name.
fn button(name: &str) -> Option<u16> {
    Some(match name.to_ascii_lowercase().as_str() {
        "none" | "-" | "0" => 0,
        "select" => PAD_SELECT,
        "start" => PAD_START,
        "up" => PAD_UP,
        "down" => PAD_DOWN,
        "left" => PAD_LEFT,
        "right" => PAD_RIGHT,
        "l1" => PAD_L1,
        "l2" => PAD_L2,
        "r1" => PAD_R1,
        "r2" => PAD_R2,
        "l3" => PAD_L3,
        "r3" => PAD_R3,
        "triangle" | "tri" => PAD_TRIANGLE,
        "circle" => PAD_CIRCLE,
        "cross" | "x" => PAD_CROSS,
        "square" => PAD_SQUARE,
        h if h.starts_with("0x") => u16::from_str_radix(&h[2..], 16).ok()?,
        _ => return None,
    })
}

impl PadScript {
    pub fn parse(text: &str) -> Result<PadScript, String> {
        let mut events = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let err = |m: &str| format!("pad script line {}: {m}: {line:?}", n + 1);
            let mut it = line.split_whitespace();
            let tick: u64 = it.next().and_then(|t| t.parse().ok()).ok_or_else(|| err("bad tick"))?;
            let what = it.next().ok_or_else(|| err("missing buttons"))?;
            let cmd = if what == "shot" {
                Cmd::Shot(PathBuf::from(it.next().ok_or_else(|| err("missing file"))?))
            } else {
                let mut mask = 0;
                for b in what.split(['+', ',']) {
                    mask |= button(b).ok_or_else(|| err("unknown button"))?;
                }
                Cmd::Pad(mask)
            };
            events.push((tick, cmd));
        }
        events.sort_by_key(|e| e.0);
        Ok(PadScript { events })
    }

    pub fn load(path: &std::path::Path) -> Result<PadScript, String> {
        let t = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&t)
    }

    /// The pad state to hold at `tick`, if a line changes it there.
    pub fn pad_at(&self, tick: u64) -> Option<u16> {
        self.events.iter().filter(|(t, c)| *t == tick && matches!(c, Cmd::Pad(_))).last().map(|(_, c)| match c {
            Cmd::Pad(m) => *m,
            Cmd::Shot(_) => 0,
        })
    }

    /// Screenshots due at `tick`.
    pub fn shots_at(&self, tick: u64) -> impl Iterator<Item = &PathBuf> {
        self.events.iter().filter(move |(t, _)| *t == tick).filter_map(|(_, c)| match c {
            Cmd::Shot(p) => Some(p),
            Cmd::Pad(_) => None,
        })
    }

    /// The last tick the script mentions.
    pub fn last_tick(&self) -> u64 {
        self.events.last().map_or(0, |e| e.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_buttons_and_shots() {
        let s = PadScript::parse("# x\n10 cross+up\n12 none\n5 0x0008 # start\n20 shot a.png\n").unwrap();
        assert_eq!(s.events[0], (5, Cmd::Pad(PAD_START)));
        assert_eq!(s.pad_at(10), Some(PAD_CROSS | PAD_UP));
        assert_eq!(s.pad_at(12), Some(0));
        assert_eq!(s.pad_at(11), None);
        assert_eq!(s.shots_at(20).count(), 1);
        assert_eq!(s.last_tick(), 20);
        assert!(PadScript::parse("1 jump").is_err());
    }
}
