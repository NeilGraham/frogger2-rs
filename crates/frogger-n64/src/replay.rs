//! `F2N64_BOOT=replay`: plays back a PC recording (`frogger --practice W L --record FILE`,
//! `crates/frogger/src/record.rs`) from `rom:/replay.txt` and checks the simulation against
//! the recorded `Game::checksum`s (tools/n64/README.md "Replays").
//!
//! The file is a pad script: `TICK 0xMASK` lines hold player 1's pads from that tick on;
//! `# f2rec 1 practice W L` / `# f2rec 1 story N` says how the session starts, `# h TICK HEX`
//! is the checksum after TICK ticks, `# p TICK name=HEX ...` its parts, `# n TICK 0xMASK` the
//! best-time name entry's input at TICK, `# end TICK` the last tick recorded.
//!
//! Chain mode (`rom:/replay/list.txt`, [`Chain`]): several one-level recordings back to back
//! in one boot, each in a fresh session as the PC recorded it, while the renderer, heaps and
//! audio carry over from level to level as in a real session.

use alloc::string::String;
use alloc::vec::Vec;

/// How the recorded session starts (after the frontend session, as the PC's `start_choice`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Start {
    #[default]
    None,
    /// `--practice W L`: an arcade level.
    Practice(u32, u32),
    /// `--story N`: story mode at `storySequence[N]`, on through the next levels.
    Story(u32),
}

#[derive(Debug, Default)]
pub struct Replay {
    pub start: Start,
    /// `(tick, pad)` changes in tick order.
    pads: Vec<(u64, u16)>,
    /// `(ticks done, checksum)` in tick order.
    hashes: Vec<(u64, u64)>,
    /// `(ticks done, "name=hex ...")`.
    parts: Vec<(u64, String)>,
    /// `(tick, edges)`: the best-time name entry's input (`psx_name_entry_frame`).
    names: Vec<(u64, u16)>,
    /// The last tick to play (`# end`, else the last pad or checksum line).
    pub end: u64,
    /// Ticks played.
    pub tick: u64,
    next_pad: usize,
    next_hash: usize,
    pad: u16,
    pub checks_ok: u32,
    /// The first checksum that differed: `(tick, expected, got)`.
    pub desync: Option<(u64, u64, u64)>,
}

/// What [`Replay::after_tick`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    None,
    Ok(u64),
    /// `(tick, expected, got)`; only the first desync is reported.
    Desync(u64, u64, u64),
}

impl Replay {
    pub fn parse(text: &str) -> Result<Replay, String> {
        Replay::parse_with(text, true)
    }

    /// `keep_parts: false` leaves the `# p` lines out (most of a recording's size; chain mode
    /// finds them again with [`parts_in`] after a desync).
    pub fn parse_with(text: &str, keep_parts: bool) -> Result<Replay, String> {
        let mut r = Replay::default();
        let mut end = None;
        for (n, line) in text.lines().enumerate() {
            let err = |m: &str| alloc::format!("replay line {}: {m}", n + 1);
            let line = line.trim();
            if let Some(c) = line.strip_prefix('#') {
                let mut it = c.split_whitespace();
                match it.next() {
                    Some("f2rec") => {
                        let (_ver, kind) = (it.next(), it.next());
                        let mut num = || it.next().and_then(|s| s.parse().ok()).ok_or_else(|| err("bad number"));
                        r.start = match kind {
                            Some("practice") => Start::Practice(num()?, num()?),
                            Some("story") => Start::Story(num()?),
                            _ => return Err(err("unknown start (practice W L / story N)")),
                        };
                    }
                    Some("n") => {
                        let t = it.next().and_then(|s| s.parse().ok()).ok_or_else(|| err("bad tick"))?;
                        let m = it.next().and_then(|s| s.strip_prefix("0x")).and_then(|s| u16::from_str_radix(s, 16).ok()).ok_or_else(|| err("bad mask"))?;
                        r.names.push((t, m));
                    }
                    Some("h") => {
                        let t = it.next().and_then(|s| s.parse().ok()).ok_or_else(|| err("bad tick"))?;
                        let h = it.next().and_then(|s| u64::from_str_radix(s, 16).ok()).ok_or_else(|| err("bad checksum"))?;
                        r.hashes.push((t, h));
                    }
                    Some("p") if keep_parts => {
                        let t = it.next().and_then(|s| s.parse().ok()).ok_or_else(|| err("bad tick"))?;
                        let rest: Vec<&str> = it.collect();
                        r.parts.push((t, rest.join(" ")));
                    }
                    Some("end") => end = it.next().and_then(|s| s.parse().ok()),
                    _ => {}
                }
                continue;
            }
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut it = line.split_whitespace();
            let t: u64 = it.next().and_then(|s| s.parse().ok()).ok_or_else(|| err("bad tick"))?;
            let m = it.next().and_then(|s| s.strip_prefix("0x")).and_then(|s| u16::from_str_radix(s, 16).ok()).ok_or_else(|| err("bad pad mask"))?;
            r.pads.push((t, m));
        }
        if r.start == Start::None {
            return Err(String::from("replay: no `# f2rec` header"));
        }
        r.pads.sort_by_key(|p| p.0);
        r.hashes.sort_by_key(|h| h.0);
        r.pads.shrink_to_fit();
        r.hashes.shrink_to_fit();
        let last = r.pads.last().map_or(0, |p| p.0 + 1).max(r.hashes.last().map_or(0, |h| h.0));
        r.end = end.unwrap_or(last);
        Ok(r)
    }

    /// The checksum expected before the first tick.
    pub fn start_hash(&self) -> Option<u64> {
        self.hashes.first().filter(|h| h.0 == 0).map(|h| h.1)
    }

    pub fn done(&self) -> bool {
        self.tick >= self.end
    }

    /// Player 1's pad for the next tick.
    pub fn pad(&mut self) -> u16 {
        while let Some(&(t, m)) = self.pads.get(self.next_pad) {
            if t > self.tick {
                break;
            }
            self.pad = m;
            self.next_pad += 1;
        }
        if self.done() {
            0
        } else {
            self.pad
        }
    }

    /// The best-time name entry's input for the next tick (0: none recorded).
    pub fn name_entry(&self) -> u16 {
        self.names.iter().find(|n| n.0 == self.tick).map_or(0, |n| n.1)
    }

    /// After a tick: counts it, and compares `checksum()` when the recording has one here.
    pub fn after_tick(&mut self, checksum: impl FnOnce() -> u64) -> Check {
        self.tick += 1;
        while self.hashes.get(self.next_hash).is_some_and(|h| h.0 < self.tick) {
            self.next_hash += 1;
        }
        let Some(&(t, want)) = self.hashes.get(self.next_hash).filter(|h| h.0 == self.tick) else { return Check::None };
        self.next_hash += 1;
        self.check(t, want, checksum())
    }

    /// Compares one checksum (the start one: tick 0).
    pub fn check(&mut self, t: u64, want: u64, got: u64) -> Check {
        if want == got {
            self.checks_ok += 1;
            return Check::Ok(t);
        }
        if self.desync.is_some() {
            return Check::None;
        }
        self.desync = Some((t, want, got));
        Check::Desync(t, want, got)
    }

    /// The recorded parts at tick `t` (`name=hex ...`).
    pub fn parts_at(&self, t: u64) -> Option<&str> {
        self.parts.iter().find(|p| p.0 == t).map(|p| p.1.as_str())
    }
}

/// The `# p` parts at tick `t` in a recording's text (`name=hex ...`).
pub fn parts_in(text: &str, t: u64) -> Option<String> {
    text.lines().find_map(|l| {
        let mut it = l.trim().strip_prefix("# p ")?.split_whitespace();
        (it.next()?.parse::<u64>().ok()? == t).then(|| it.collect::<Vec<_>>().join(" "))
    })
}

/// Chain mode: the recordings `rom:/replay/list.txt` names, played one after another.
///
/// The list has one segment per line, `FILE NAME`: a file next to the list and the name to
/// log (the rest of the line, spaces and `#` included); blank lines are skipped.
#[derive(Debug, Default)]
pub struct Chain {
    /// `(path, name)` per segment.
    pub segments: Vec<(String, String)>,
    /// The segment playing (index into `segments`).
    pub cur: usize,
    /// Segments finished, their ticks and checksums matched.
    pub done: usize,
    pub ticks: u64,
    pub checks_ok: u32,
    /// The first desync: (segment index, tick).
    pub desync: Option<(usize, u64)>,
}

impl Chain {
    /// Parses the list; `dir` (e.g. `rom:/replay/`) is put before each file name.
    pub fn parse(text: &str, dir: &str) -> Result<Chain, String> {
        let mut c = Chain::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (file, name) = line.split_once(char::is_whitespace).unwrap_or((line, line));
            c.segments.push((alloc::format!("{dir}{file}"), String::from(name.trim())));
        }
        if c.segments.is_empty() {
            return Err(String::from("replay chain: empty list"));
        }
        Ok(c)
    }

    /// `i/N name` of the segment playing (1-based).
    pub fn label(&self) -> String {
        alloc::format!("{}/{} {}", self.cur + 1, self.segments.len(), self.segments.get(self.cur).map_or("?", |s| s.1.as_str()))
    }

    /// Counts the segment that just ended; true when another one follows (`cur` moves on).
    pub fn finish(&mut self, r: &Replay) -> bool {
        self.done += 1;
        self.ticks += r.tick;
        self.checks_ok += r.checks_ok;
        if let (None, Some((t, _, _))) = (self.desync, r.desync) {
            self.desync = Some((self.cur, t));
        }
        if self.cur + 1 < self.segments.len() {
            self.cur += 1;
            true
        } else {
            false
        }
    }

    /// The text after `replay chain done: `.
    pub fn summary(&self) -> String {
        let head = alloc::format!("{} segments of {}, {} ticks, {} checksums matched", self.done, self.segments.len(), self.ticks, self.checks_ok);
        match self.desync {
            None => alloc::format!("{head}, no desync"),
            Some((s, t)) => alloc::format!("{head}, first desync at segment {}/{} tick {t} ({})", s + 1, self.segments.len(), self.segments[s].1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REC: &str = "# f2rec 1 practice 0 0\n# h 0 00000000000000aa\n# p 0 core=01 map=02\n0 0x0000\n2 0x0010\n# h 2 00000000000000bb\n3 0x0000\n# h 4 00000000000000cc\n# end 5\n";

    #[test]
    fn plays_pads_and_checks() {
        let mut r = Replay::parse(REC).unwrap();
        assert_eq!((r.start, r.end), (Start::Practice(0, 0), 5));
        assert_eq!(r.start_hash(), Some(0xaa));
        assert_eq!(r.parts_at(0), Some("core=01 map=02"));
        assert_eq!(parts_in(REC, 0).as_deref(), Some("core=01 map=02"));
        assert_eq!(parts_in(REC, 2), None);
        let lean = Replay::parse_with(REC, false).unwrap();
        assert_eq!((lean.parts_at(0), lean.end, lean.start_hash()), (None, 5, Some(0xaa)));
        let mut pads = Vec::new();
        let mut checks = Vec::new();
        let sums = [0, 0xbb, 0, 0xcd, 0];
        while !r.done() {
            pads.push(r.pad());
            let s = sums[r.tick as usize];
            checks.push(r.after_tick(|| s));
        }
        assert_eq!(pads, [0, 0, 0x10, 0, 0]);
        assert_eq!(checks, [Check::None, Check::Ok(2), Check::None, Check::Desync(4, 0xcc, 0xcd), Check::None]);
        assert_eq!(r.pad(), 0);
        assert_eq!(r.desync, Some((4, 0xcc, 0xcd)));
    }

    #[test]
    fn rejects_bad_files() {
        assert!(Replay::parse("0 0x0000\n").is_err());
        assert!(Replay::parse("# f2rec 1 multi 3\n").is_err());
        let s = Replay::parse("# f2rec 1 story 3\n0 0x0000\n# n 0 0x0020\n# end 2\n").unwrap();
        assert_eq!((s.start, s.name_entry()), (Start::Story(3), 0x20));
        assert!(Replay::parse("# f2rec 1 practice 0 0\n5 up\n").is_err());
    }

    #[test]
    fn chains_segments() {
        assert!(Chain::parse("\n  \n", "rom:/replay/").is_err());
        let mut c = Chain::parse("000.txt story 02 Boulder Canyon #1\n\n001.txt 0-0 The Garden #1\r\n002.txt\n", "rom:/replay/").unwrap();
        assert_eq!(c.segments[0], (String::from("rom:/replay/000.txt"), String::from("story 02 Boulder Canyon #1")));
        assert_eq!(c.segments[1].1, "0-0 The Garden #1");
        assert_eq!(c.segments[2], (String::from("rom:/replay/002.txt"), String::from("002.txt")));
        assert_eq!(c.label(), "1/3 story 02 Boulder Canyon #1");
        let mut r = Replay::parse(REC).unwrap();
        while !r.done() {
            r.pad();
            r.after_tick(|| 0xbb);
        }
        assert!(c.finish(&r));
        assert_eq!((c.cur, c.ticks, c.checks_ok, c.desync), (1, 5, 1, Some((0, 4))));
        assert!(c.finish(&Replay::parse(REC).unwrap()));
        assert!(!c.finish(&Replay::parse(REC).unwrap()));
        assert_eq!(c.summary(), "3 segments of 3, 5 ticks, 1 checksums matched, first desync at segment 1/3 tick 4 (story 02 Boulder Canyon #1)");
        let c = Chain::parse("000.txt a", "").unwrap();
        assert_eq!(c.summary(), "0 segments of 1, 0 ticks, 0 checksums matched, no desync");
    }
}
