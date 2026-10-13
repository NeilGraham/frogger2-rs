//! `F2N64_BOOT=replay`: plays back a PC recording (`frogger --practice W L --record FILE`,
//! `crates/frogger/src/record.rs`) from `rom:/replay.txt` and checks the simulation against
//! the recorded `Game::checksum`s (tools/n64/README.md "Replays").
//!
//! The file is a pad script: `TICK 0xMASK` lines hold player 1's pads from that tick on;
//! `# f2rec 1 practice W L` names the level, `# h TICK HEX` is the checksum after TICK ticks,
//! `# p TICK name=HEX ...` its parts, `# end TICK` the last tick recorded.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Default)]
pub struct Replay {
    pub world: u32,
    pub level: u32,
    /// `(tick, pad)` changes in tick order.
    pads: Vec<(u64, u16)>,
    /// `(ticks done, checksum)` in tick order.
    hashes: Vec<(u64, u64)>,
    /// `(ticks done, "name=hex ...")`.
    parts: Vec<(u64, String)>,
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
        let mut r = Replay::default();
        let mut have_level = false;
        let mut end = None;
        for (n, line) in text.lines().enumerate() {
            let err = |m: &str| alloc::format!("replay line {}: {m}", n + 1);
            let line = line.trim();
            if let Some(c) = line.strip_prefix('#') {
                let mut it = c.split_whitespace();
                match it.next() {
                    Some("f2rec") => {
                        let (_ver, kind, w, l) = (it.next(), it.next(), it.next(), it.next());
                        if kind != Some("practice") {
                            return Err(err("only `practice W L` recordings"));
                        }
                        r.world = w.and_then(|s| s.parse().ok()).ok_or_else(|| err("bad world"))?;
                        r.level = l.and_then(|s| s.parse().ok()).ok_or_else(|| err("bad level"))?;
                        have_level = true;
                    }
                    Some("h") => {
                        let t = it.next().and_then(|s| s.parse().ok()).ok_or_else(|| err("bad tick"))?;
                        let h = it.next().and_then(|s| u64::from_str_radix(s, 16).ok()).ok_or_else(|| err("bad checksum"))?;
                        r.hashes.push((t, h));
                    }
                    Some("p") => {
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
        if !have_level {
            return Err(String::from("replay: no `# f2rec` header"));
        }
        r.pads.sort_by_key(|p| p.0);
        r.hashes.sort_by_key(|h| h.0);
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

#[cfg(test)]
mod tests {
    use super::*;

    const REC: &str = "# f2rec 1 practice 0 0\n# h 0 00000000000000aa\n# p 0 core=01 map=02\n0 0x0000\n2 0x0010\n# h 2 00000000000000bb\n3 0x0000\n# h 4 00000000000000cc\n# end 5\n";

    #[test]
    fn plays_pads_and_checks() {
        let mut r = Replay::parse(REC).unwrap();
        assert_eq!((r.world, r.level, r.end), (0, 0, 5));
        assert_eq!(r.start_hash(), Some(0xaa));
        assert_eq!(r.parts_at(0), Some("core=01 map=02"));
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
        assert!(Replay::parse("# f2rec 1 story 3\n").is_err());
        assert!(Replay::parse("# f2rec 1 practice 0 0\n5 up\n").is_err());
    }
}
