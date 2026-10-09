//! `.key` input recordings (portable, host-tested).
//!
//! Format (PC/controll.cpp `RecordButtons`): little-endian `u32` triplets
//! `(actFrameCount, padData.digital[pnum], pnum)`, one written whenever a
//! pad's digital state changes. Common/levplay.c `RunDemoMode` replays them.
//!
//! Scenario mode replays a recording as *held* pad state: for each tick, a
//! player's `digital` is the buttons of their latest record with
//! `frame < actFrameCount` (the same `<` test the demo player uses), and
//! `debounce = ~previous & digital`, which is what `ProcessUserInput`
//! computes from live input. (The attract demo itself ORs only the records
//! of the current tick into a cleared pad, so buttons are seen for one tick;
//! that is the game's own behaviour and runs unmodified in demo mode.)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyRecord {
    pub frame: u32,
    pub buttons: u16,
    pub player: u8,
}

/// Parse a `.key` file. Trailing bytes and records for players >= 4 are rejected.
pub fn parse(bytes: &[u8]) -> Result<Vec<KeyRecord>, String> {
    if !bytes.len().is_multiple_of(12) {
        return Err(format!("length {} is not a multiple of 12", bytes.len()));
    }
    let mut out = Vec::with_capacity(bytes.len() / 12);
    for (i, c) in bytes.as_chunks::<12>().0.iter().enumerate() {
        let w = |k: usize| u32::from_le_bytes([c[k], c[k + 1], c[k + 2], c[k + 3]]);
        let (frame, buttons, player) = (w(0), w(4), w(8));
        if player >= 4 {
            return Err(format!("record {i}: player {player} >= 4"));
        }
        out.push(KeyRecord { frame, buttons: buttons as u16, player: player as u8 });
    }
    Ok(out)
}

/// Held-state replay of a recording.
#[derive(Clone, Debug, Default)]
pub struct Player {
    records: Vec<KeyRecord>,
    next: usize,
    held: [u16; 4],
}

impl Player {
    pub fn new(records: Vec<KeyRecord>) -> Player {
        Player { records, next: 0, held: [0; 4] }
    }

    /// Advance to `act_frame_count`; returns `(digital, debounce)` per pad.
    pub fn step(&mut self, act_frame_count: u32) -> ([u16; 4], [u16; 4]) {
        let prev = self.held;
        while let Some(r) = self.records.get(self.next) {
            if r.frame >= act_frame_count {
                break;
            }
            self.held[r.player as usize] = r.buttons;
            self.next += 1;
        }
        let mut deb = [0u16; 4];
        for i in 0..4 {
            deb[i] = !prev[i] & self.held[i];
        }
        (self.held, deb)
    }

    /// All records consumed.
    pub fn finished(&self) -> bool {
        self.next >= self.records.len()
    }

    /// Frame of the last record (0 when empty).
    pub fn last_frame(&self) -> u32 {
        self.records.last().map_or(0, |r| r.frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(f: u32, b: u32, p: u32) -> Vec<u8> {
        [f, b, p].iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn parse_and_hold() {
        let mut b = rec(5, 0x20, 0);
        b.extend(rec(8, 0, 0));
        b.extend(rec(8, 0x10, 1));
        let r = parse(&b).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[2], KeyRecord { frame: 8, buttons: 0x10, player: 1 });
        let mut p = Player::new(r);
        assert_eq!(p.step(5), ([0; 4], [0; 4]));
        assert_eq!(p.step(6), ([0x20, 0, 0, 0], [0x20, 0, 0, 0]));
        assert_eq!(p.step(7), ([0x20, 0, 0, 0], [0; 4]));
        assert_eq!(p.step(9), ([0, 0x10, 0, 0], [0, 0x10, 0, 0]));
        assert!(p.finished());
        assert_eq!(p.last_frame(), 8);
    }

    #[test]
    fn rejects_bad_files() {
        assert!(parse(&[0; 13]).is_err());
        assert!(parse(&rec(1, 1, 4)).is_err());
    }
}
