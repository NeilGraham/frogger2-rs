//! Port of the `TIMER` helpers in `Common/general.h` (`GTInit`, `GTUpdate`).

use serde::{Deserialize, Serialize};

/// `TIMER` (`general.h`): a countdown/up in whole units advanced by `actFrameCount`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timer {
    pub time: u16,
    pub last_count: u32,
}

impl Timer {
    /// Port of `GTInit(t, s)`.
    #[inline]
    pub fn init(&mut self, s: i32) {
        self.time = s as u16;
        self.last_count = 0;
    }

    /// Port of `GTUpdate(t, s)`: once every `60*|s|` frames, `time += s`.
    #[inline]
    pub fn update(&mut self, s: i32, act_frame_count: u32) {
        if act_frame_count > self.last_count.wrapping_add(60 * s.unsigned_abs()) {
            self.last_count = act_frame_count;
            self.time = (self.time as i32).wrapping_add(s) as u16;
        }
    }

    /// Port of `GTUpdate2(t, s)`: once every `30*|s|` frames.
    #[inline]
    pub fn update2(&mut self, s: i32, act_frame_count: u32) {
        if act_frame_count > self.last_count.wrapping_add(30 * s.unsigned_abs()) {
            self.last_count = act_frame_count;
            self.time = (self.time as i32).wrapping_add(s) as u16;
        }
    }
}
