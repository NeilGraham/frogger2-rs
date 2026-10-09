//! Per-remote-endpoint connection state: sequence numbers, RTT, acks, time-sync samples.

use crate::transport::Transport;
use crate::wire::{self, Msg};
use crate::{Frame, NetStats, PeerKind};

/// Fixed-size window of samples for averaging.
#[derive(Clone, Debug)]
pub(crate) struct Window {
    samples: Vec<i32>,
    next: usize,
    filled: usize,
}

impl Window {
    pub fn new(n: usize) -> Self {
        Window { samples: vec![0; n], next: 0, filled: 0 }
    }
    pub fn push(&mut self, v: i32) {
        self.samples[self.next] = v;
        self.next = (self.next + 1) % self.samples.len();
        self.filled = (self.filled + 1).min(self.samples.len());
    }
    pub fn average(&self) -> Option<f32> {
        if self.filled == 0 {
            return None;
        }
        let s: i64 = self.samples[..self.filled].iter().map(|&v| v as i64).sum();
        Some(s as f32 / self.filled as f32)
    }
    pub fn is_full(&self) -> bool {
        self.filled == self.samples.len()
    }
}

pub(crate) struct Peer<A> {
    pub addr: A,
    pub kind: PeerKind,
    send_seq: u16,
    recv_seq: Option<u16>,
    /// Bit `i` set = packet `recv_seq - 1 - i` was received (duplicate filter).
    recv_window: u32,
    pub last_recv_ms: u64,
    pub last_input_send_ms: Option<u64>,
    pub last_quality_ms: Option<u64>,
    /// Highest frame of the data we stream to this peer that it has acknowledged.
    pub acked_frame: Frame,
    pub rtt_ms: Option<f32>,
    /// Our frame advantage over this peer (positive = we are ahead), sampled every tick.
    pub local_adv: Window,
    /// The peer's reported advantage over us.
    pub remote_adv: Window,
    pub disconnected: bool,
    /// Host only: the `Start` message to (re)send when this peer still asks to join.
    pub start_msg: Option<Msg<A>>,
}

impl<A: crate::NetAddr> Peer<A> {
    pub fn new(addr: A, kind: PeerKind, acked_frame: Frame, now_ms: u64) -> Self {
        Peer {
            addr,
            kind,
            send_seq: 0,
            recv_seq: None,
            recv_window: 0,
            last_recv_ms: now_ms,
            last_input_send_ms: None,
            last_quality_ms: None,
            acked_frame,
            rtt_ms: None,
            local_adv: Window::new(40),
            remote_adv: Window::new(10),
            disconnected: false,
            start_msg: None,
        }
    }

    pub fn send<T: Transport<Addr = A>>(&mut self, transport: &mut T, nonce: u16, msg: &Msg<A>, stats: &mut NetStats) {
        self.send_seq = self.send_seq.wrapping_add(1);
        let bytes = wire::encode(nonce, self.send_seq, msg);
        stats.packets_sent += 1;
        stats.bytes_sent += bytes.len() as u64;
        transport.send_to(&bytes, self.addr);
    }

    /// Record receipt of a packet; `false` for duplicates and packets more than 32 behind the
    /// newest (out-of-order packets inside the window are accepted: every message is idempotent).
    pub fn accept(&mut self, seq: u16, now_ms: u64) -> bool {
        match self.recv_seq {
            None => {
                self.recv_seq = Some(seq);
                self.recv_window = 0;
            }
            Some(last) if wire::seq_newer(seq, last) => {
                let shift = seq.wrapping_sub(last) as u32;
                // The previous newest packet is now `shift` behind: bit `shift - 1`.
                self.recv_window = match shift {
                    1..=31 => (self.recv_window << shift) | (1 << (shift - 1)),
                    32 => 1 << 31,
                    _ => 0,
                };
                self.recv_seq = Some(seq);
            }
            Some(last) => {
                let back = last.wrapping_sub(seq) as u32; // 0 = same as newest
                if back == 0 || back > 32 || self.recv_window & (1 << (back - 1)) != 0 {
                    return false;
                }
                self.recv_window |= 1 << (back - 1);
            }
        }
        self.last_recv_ms = now_ms;
        true
    }

    pub fn on_pong(&mut self, now_ms: u64, pong: u32) {
        let sample = (now_ms as u32).wrapping_sub(pong);
        if sample > 60_000 {
            return;
        }
        let s = sample as f32;
        self.rtt_ms = Some(match self.rtt_ms {
            None => s,
            Some(r) => r * 0.875 + s * 0.125,
        });
    }

    /// Smoothed RTT in 60 Hz frames.
    pub fn rtt_frames(&self) -> f32 {
        self.rtt_ms.unwrap_or(0.0) * 60.0 / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LoopbackAddr;

    #[test]
    fn duplicate_window() {
        let mut p = Peer::new(LoopbackAddr(1), PeerKind::Player(1), 0, 0);
        assert!(p.accept(10, 0));
        assert!(!p.accept(10, 0)); // duplicate
        assert!(p.accept(12, 0));
        assert!(p.accept(11, 0)); // reordered, accepted once
        assert!(!p.accept(11, 0));
        assert!(!p.accept(12, 0));
        assert!(p.accept(44, 0)); // 12 is now exactly 32 behind
        assert!(!p.accept(12, 0));
        assert!(p.accept(13, 0));
        assert!(!p.accept(11, 0)); // too old
        assert!(p.accept(65535u16.wrapping_add(50), 0)); // wrap-around is "newer"
    }
}
