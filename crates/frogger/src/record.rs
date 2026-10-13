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
//! One level per recording: [`AFTER_COMPLETE`] ticks after the level is complete the recording
//! ends and the game closes; a game over or a quit to the frontend deletes the recording and
//! closes the game (play it again).
//!
//! While recording, the session matches the N64's: authentic options with the console and
//! settings-menu flags, no save, no mods, the default language (`App::session_config`).

use f2_game::Game;
use std::io::Write;

/// Ticks between two checksum lines.
pub const HASH_EVERY: u64 = 60;

/// Ticks recorded after the level is complete (the results screen coming up).
pub const AFTER_COMPLETE: u64 = 180;

/// What [`Recorder::tick`] decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Recording,
    /// The level was completed: the recording is finished (close the game).
    Finished,
    /// Game over or quit: the recording was deleted (close the game).
    Scrapped,
}

/// The simulation options of the N64 build (`frogger-n64` `App::new`).
pub fn n64_options() -> f2_game::enhance::GameOptions {
    f2_game::enhance::GameOptions { console: true, settings_menu: true, ..f2_game::enhance::GameOptions::AUTHENTIC }
}

pub struct Recorder {
    /// `None` once a scrapped recording is closed and deleted.
    out: Option<std::io::BufWriter<std::fs::File>>,
    path: std::path::PathBuf,
    /// Ticks recorded since [`Recorder::start`]; `None` before it.
    tick: Option<u64>,
    last_pad: Option<u16>,
    last_mode: u32,
    /// The tick the recording ends at (level complete + [`AFTER_COMPLETE`]).
    finish_at: Option<u64>,
}

impl Recorder {
    pub fn create(path: &std::path::Path) -> Result<Recorder, String> {
        let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Recorder { out: Some(std::io::BufWriter::new(f)), path: path.to_path_buf(), tick: None, last_pad: None, last_mode: u32::MAX, finish_at: None })
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
    pub fn tick(&mut self, pad: u16, game: &Game) -> Outcome {
        use f2_game::define::mode::{FRONTEND_MODE, GAMECOMPLETE_MODE, GAMEOVER_MODE, LEVELCOMPLETE_MODE, WORLDCOMPLETE_MODE};
        let Some(t) = self.tick else { return Outcome::Recording };
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
            if self.finish_at.is_none() {
                match mode {
                    LEVELCOMPLETE_MODE | WORLDCOMPLETE_MODE | GAMECOMPLETE_MODE => self.finish_at = Some(t + AFTER_COMPLETE),
                    GAMEOVER_MODE | FRONTEND_MODE => return self.scrap(if mode == GAMEOVER_MODE { "game over" } else { "quit" }),
                    _ => {}
                }
            }
        }
        if t % HASH_EVERY == 0 || self.finish_at == Some(t) {
            self.hash(t, game);
            self.flush();
        }
        if self.finish_at == Some(t) {
            self.line(format_args!("# end {t}"));
            self.flush();
            self.tick = None;
            eprintln!("frogger: level complete, recording finished: {}", self.path.display());
            return Outcome::Finished;
        }
        Outcome::Recording
    }

    /// Deletes the recording (game over / quit).
    fn scrap(&mut self, why: &str) -> Outcome {
        self.tick = None;
        // Closed first: Windows does not delete open files.
        self.out = None;
        let _ = std::fs::remove_file(&self.path);
        eprintln!("frogger: {why}: recording deleted ({})", self.path.display());
        Outcome::Scrapped
    }

    /// The checksum line and the per-part checksums (which part diverged, on a desync).
    fn hash(&mut self, t: u64, game: &Game) {
        self.line(format_args!("# h {t} {:016x}", game.checksum()));
        let parts: Vec<String> = game.checksum_parts().iter().map(|(n, h)| format!("{n}={h:016x}")).collect();
        self.line(format_args!("# p {t} {}", parts.join(" ")));
    }

    fn line(&mut self, a: std::fmt::Arguments) {
        if let Some(o) = &mut self.out {
            let _ = writeln!(o, "{a}");
        }
    }

    fn flush(&mut self) {
        if let Some(o) = &mut self.out {
            let _ = o.flush();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(t) = self.tick {
            self.line(format_args!("# end {t}"));
        }
        self.flush();
    }
}
