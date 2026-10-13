//! `--record FILE` (with `--practice W L` or `--story N`): records a play session for the N64
//! replay boot (`F2N64_BOOT=replay`, tools/n64/README.md "Replays"). A story session runs on
//! through the next levels and their videos (which take no ticks on either build).
//!
//! The file is a pad script (player 1's pads as the game saw them, one line per change), so it
//! also replays on the PC with `--pad-script`. Comment lines carry the checks:
//!
//! ```text
//! # f2rec 1 practice 0 0
//! # h 0 0123456789abcdef     # Game::checksum before the first tick
//! 0 0x0000                   # pads from tick 0 on
//! 57 0x0010
//! # h 60 fedcba9876543210    # checksum after 60 ticks
//! # p 60 core=... map=...     # its parts (Game::checksum_parts)
//! # mode 812 6               # game mode after tick 812
//! # n 900 0x0020             # best-time name entry's pad edges at tick 900 (not a pad line:
//!                            # the PC feeds it the debounced pad, `App::tick`)
//! # video 1000 fmv07         # a video started during tick 1000 (information)
//! ```
//!
//! The header is `# f2rec 1 practice W L` or `# f2rec 1 story N`.
//!
//! While recording, the session matches the N64's: authentic options with the console and
//! settings-menu flags, no save, no mods, the default language (`App::session_config`).

use f2_game::Game;
use std::io::Write;

/// Ticks between two checksum lines.
pub const HASH_EVERY: u64 = 60;

/// The simulation options of the N64 build (`frogger-n64` `App::new`).
pub fn n64_options() -> f2_game::enhance::GameOptions {
    f2_game::enhance::GameOptions { console: true, settings_menu: true, ..f2_game::enhance::GameOptions::AUTHENTIC }
}

pub struct Recorder {
    out: std::io::BufWriter<std::fs::File>,
    path: std::path::PathBuf,
    /// Ticks recorded since [`Recorder::start`]; `None` before it.
    tick: Option<u64>,
    last_pad: Option<u16>,
    last_mode: u32,
}

impl Recorder {
    pub fn create(path: &std::path::Path) -> Result<Recorder, String> {
        let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Recorder { out: std::io::BufWriter::new(f), path: path.to_path_buf(), tick: None, last_pad: None, last_mode: u32::MAX })
    }

    /// The level just started (`App::start_choice`; `start`: `practice W L` / `story N`):
    /// header and the starting checksum.
    pub fn start(&mut self, start: &str, game: &Game) {
        if self.tick.is_some() {
            return;
        }
        self.tick = Some(0);
        self.last_mode = game.game_state.mode;
        self.line(format_args!("# f2rec 1 {start}"));
        self.hash(0, game);
        self.flush();
        eprintln!("frogger: recording to {}", self.path.display());
    }

    /// Before a tick: the best-time name entry got `edges` (`psx_name_entry_frame`).
    pub fn name_entry(&mut self, edges: u16) {
        if let (Some(t), true) = (self.tick, edges != 0) {
            self.line(format_args!("# n {t} 0x{edges:04x}"));
        }
    }

    /// A video started during the tick in progress.
    pub fn video(&mut self, name: &str) {
        if let Some(t) = self.tick {
            self.line(format_args!("# video {t} {name}"));
        }
    }

    /// After one tick with player 1's `pad`.
    pub fn tick(&mut self, pad: u16, game: &Game) {
        let Some(t) = self.tick else { return };
        if self.last_pad != Some(pad) {
            self.line(format_args!("{t} 0x{pad:04x}"));
            self.last_pad = Some(pad);
        }
        let t = t + 1;
        self.tick = Some(t);
        let mode = game.game_state.mode;
        if mode != self.last_mode {
            self.line(format_args!("# mode {t} {mode}"));
            self.last_mode = mode;
            self.flush();
        }
        if t % HASH_EVERY == 0 {
            self.hash(t, game);
            self.flush();
        }
    }

    /// The checksum line and the per-part checksums (which part diverged, on a desync).
    fn hash(&mut self, t: u64, game: &Game) {
        self.line(format_args!("# h {t} {:016x}", game.checksum()));
        let parts: Vec<String> = game.checksum_parts().iter().map(|(n, h)| format!("{n}={h:016x}")).collect();
        self.line(format_args!("# p {t} {}", parts.join(" ")));
    }

    fn line(&mut self, a: std::fmt::Arguments) {
        let _ = writeln!(self.out, "{a}");
    }

    fn flush(&mut self) {
        let _ = self.out.flush();
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(t) = self.tick {
            let _ = writeln!(self.out, "# end {t}");
        }
        self.flush();
    }
}
