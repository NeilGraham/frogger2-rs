//! Port of `Common/bbtimer.c` / `bbtimer.h`: profiling timers.
//!
//! On PC every `BBTIMER` operation is a no-op macro (`ZeroTime`, `TimerStart`, ...)
//! and the C file contains only commented-out code, so only the struct is kept.

use serde::{Deserialize, Serialize};

/// `BBTIMER` (`bbtimer.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BbTimer {
    pub start: i32,
    pub total: i32,
    pub on: i32,
}

impl BbTimer {
    /// `ZeroTime()`, `TimerStart(t)`, `TimerRestart(t)`, `TimerStop(t)`,
    /// `TimerStopAdd(t)`: all expand to nothing on PC.
    #[inline]
    pub fn noop(&mut self) {}
}
