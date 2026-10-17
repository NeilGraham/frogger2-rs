//! Minimal CUE sheet parser (FILE / TRACK / INDEX 01).

use crate::sector::{RawFormat, TrackMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueTrack {
    /// File name as written in the sheet (relative to the sheet).
    pub file: String,
    pub number: u32,
    /// Mode string, e.g. `MODE1/2352`, `MODE2/2352`, `MODE2/2336`, `AUDIO`.
    pub mode: String,
    /// INDEX 01 position in sectors from the start of `file`.
    pub index1: u32,
    /// INDEX 00 (pregap start) position, if present.
    pub index0: Option<u32>,
}

impl CueTrack {
    pub fn is_data(&self) -> bool {
        self.mode.to_ascii_uppercase().starts_with("MODE")
    }

    /// Stored sector format and default data mode for this track.
    pub fn layout(&self) -> Option<(RawFormat, TrackMode)> {
        let m = self.mode.to_ascii_uppercase();
        let mode = if m.starts_with("MODE2") { TrackMode::Mode2 } else { TrackMode::Mode1 };
        let fmt = match m.rsplit('/').next()? {
            "2352" => RawFormat::Raw2352,
            "2336" => RawFormat::Raw2336,
            "2048" => RawFormat::Cooked2048,
            _ => return None,
        };
        Some((fmt, mode))
    }
}

fn parse_msf(s: &str) -> Option<u32> {
    let mut it = s.split(':').map(|p| p.trim().parse::<u32>().ok());
    let (m, sec, f) = (it.next()??, it.next()??, it.next()??);
    Some((m * 60 + sec) * 75 + f)
}

fn unquote(rest: &str) -> String {
    let rest = rest.trim();
    if let Some(r) = rest.strip_prefix('"') {
        if let Some(end) = r.find('"') {
            return r[..end].to_string();
        }
    }
    // Unquoted: drop the trailing file-type word.
    match rest.rsplit_once(char::is_whitespace) {
        Some((name, _ty)) => name.trim().to_string(),
        None => rest.to_string(),
    }
}

/// Parses the tracks of a CUE sheet. Unknown commands are ignored.
pub fn parse(text: &str) -> Vec<CueTrack> {
    let mut tracks: Vec<CueTrack> = Vec::new();
    let mut file = String::new();
    for line in text.lines() {
        let line = line.trim();
        let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        match cmd.to_ascii_uppercase().as_str() {
            "FILE" => file = unquote(rest),
            "TRACK" => {
                let mut w = rest.split_whitespace();
                let number = w.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let mode = w.next().unwrap_or("").to_string();
                tracks.push(CueTrack { file: file.clone(), number, mode, index1: 0, index0: None });
            }
            "INDEX" => {
                let mut w = rest.split_whitespace();
                let idx = w.next().and_then(|n| n.parse::<u32>().ok());
                if let (Some(t), Some(pos)) = (tracks.last_mut(), w.next().and_then(parse_msf)) {
                    match idx {
                        Some(1) => t.index1 = pos,
                        Some(0) => t.index0 = Some(pos),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    tracks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redump_style_sheet() {
        let t = parse("FILE \"A (B).bin\" BINARY\r\n  TRACK 01 MODE2/2352\r\n    INDEX 01 00:00:00\r\n");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].file, "A (B).bin");
        assert_eq!(t[0].layout(), Some((RawFormat::Raw2352, TrackMode::Mode2)));
        assert!(t[0].is_data());
    }

    #[test]
    fn multi_track_and_index() {
        let t = parse("FILE game.bin BINARY\nTRACK 1 MODE1/2352\nINDEX 01 00:00:00\nTRACK 2 AUDIO\nINDEX 00 10:00:00\nINDEX 01 10:02:05\n");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].file, "game.bin");
        assert_eq!(t[1].index1, (10 * 60 + 2) * 75 + 5);
        assert!(!t[1].is_data());
        assert_eq!(t[0].layout(), Some((RawFormat::Raw2352, TrackMode::Mode1)));
    }
}
