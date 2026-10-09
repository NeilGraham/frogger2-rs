//! Per-player input history: confirmed inputs and "repeat last" prediction.

use crate::{Frame, NULL_FRAME};

/// How many frames of history are kept (spectators lagging further are dropped).
pub const INPUT_HISTORY: usize = 1024;

/// A frame-tagged ring buffer.
#[derive(Clone, Debug)]
pub struct FrameRing<T: Copy> {
    buf: Vec<(Frame, T)>,
}

impl<T: Copy + Default> FrameRing<T> {
    pub fn new(cap: usize) -> Self {
        FrameRing { buf: vec![(NULL_FRAME, T::default()); cap.max(1)] }
    }

    fn idx(&self, frame: Frame) -> usize {
        frame.rem_euclid(self.buf.len() as i32) as usize
    }

    pub fn set(&mut self, frame: Frame, v: T) {
        let i = self.idx(frame);
        self.buf[i] = (frame, v);
    }

    pub fn get(&self, frame: Frame) -> Option<T> {
        if frame < 0 {
            return None;
        }
        let (f, v) = self.buf[self.idx(frame)];
        (f == frame).then_some(v)
    }
}

/// Confirmed inputs of one player.
#[derive(Clone, Debug)]
pub struct PlayerInputs {
    ring: FrameRing<u16>,
    /// Last frame whose input is known (inputs arrive strictly in order).
    pub last_confirmed: Frame,
    /// Input at `last_confirmed` (the prediction for any later frame).
    pub last_value: u16,
}

impl PlayerInputs {
    /// Inputs for frames `0..initial_frames` are implicitly 0 (the input-delay prefix).
    pub fn new(initial_frames: Frame) -> Self {
        let mut ring = FrameRing::new(INPUT_HISTORY);
        for f in 0..initial_frames {
            ring.set(f, 0);
        }
        PlayerInputs { ring, last_confirmed: initial_frames - 1, last_value: 0 }
    }

    /// Append the input for `last_confirmed + 1`. Returns `false` (ignored) otherwise.
    pub fn confirm(&mut self, frame: Frame, v: u16) -> bool {
        if frame != self.last_confirmed + 1 {
            return false;
        }
        self.ring.set(frame, v);
        self.last_confirmed = frame;
        self.last_value = v;
        true
    }

    /// Confirmed input for `frame`, if known and still in history.
    pub fn confirmed(&self, frame: Frame) -> Option<u16> {
        if frame > self.last_confirmed {
            None
        } else {
            self.ring.get(frame)
        }
    }

    /// Confirmed input, or the prediction (repeat last confirmed input).
    pub fn input_or_prediction(&self, frame: Frame) -> u16 {
        if frame <= self.last_confirmed {
            self.ring.get(frame).unwrap_or(self.last_value)
        } else {
            self.last_value
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prediction_repeats_last() {
        let mut p = PlayerInputs::new(2);
        assert_eq!(p.last_confirmed, 1);
        assert_eq!(p.input_or_prediction(10), 0);
        assert!(!p.confirm(3, 5));
        assert!(p.confirm(2, 5));
        assert_eq!(p.input_or_prediction(1), 0);
        assert_eq!(p.input_or_prediction(2), 5);
        assert_eq!(p.input_or_prediction(9), 5);
        assert_eq!(p.confirmed(3), None);
        for f in 3..3000 {
            assert!(p.confirm(f, f as u16));
        }
        assert_eq!(p.confirmed(2), None); // overwritten in the ring
        assert_eq!(p.confirmed(2999), Some(2999));
    }
}
