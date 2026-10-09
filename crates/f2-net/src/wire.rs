//! Wire format: an 8-byte header followed by one message. All integers little-endian.
//!
//! ```text
//! u16 magic (0x4EF2) | u8 version | u16 session nonce | u16 sequence | u8 kind | payload
//! ```

use crate::addr::NetAddr;
use crate::{Frame, SessionConfig, SyncMode, MAX_PLAYERS};

pub const MAGIC: u16 = 0x4EF2;
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 8;
/// Maximum inputs per `Input` / `SpectatorInputs` message (keeps datagrams well under 1200 B).
pub const MAX_INPUTS_PER_PACKET: usize = 120;

/// Slot value meaning "spectator" in `Welcome`/`Start`.
pub const SPECTATOR_SLOT: u8 = 0xFF;

/// Byte writer.
#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// Bounds-checked byte reader; every getter returns `None` on truncation.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.bytes(2)?.try_into().ok()?))
    }
    pub fn i16(&mut self) -> Option<i16> {
        Some(i16::from_le_bytes(self.bytes(2)?.try_into().ok()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }
    pub fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.bytes(8)?.try_into().ok()?))
    }
    pub fn is_empty(&self) -> bool {
        self.pos == self.data.len()
    }
}

/// Where a player in the `Start` table can be reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerLoc<A> {
    /// The sender of the `Start` packet (the host; it does not know its own public address).
    Sender,
    Addr(A),
}

/// Payload of the `Start` message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartMsg<A> {
    pub config: SessionConfig,
    /// Receiver's slot, or [`SPECTATOR_SLOT`].
    pub your_slot: u8,
    /// Every player slot and where to reach it.
    pub players: Vec<(u8, PeerLoc<A>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg<A> {
    Join {
        spectator: bool,
    },
    Welcome {
        slot: u8,
    },
    Reject {
        reason: u8,
    },
    Start(StartMsg<A>),
    /// Redundant run of one player's inputs starting at `start_frame`; `ack_frame` is the last
    /// frame of the receiver's inputs the sender has.
    Input {
        slot: u8,
        ack_frame: Frame,
        start_frame: Frame,
        inputs: Vec<u16>,
    },
    /// Spectator → host acknowledgement of full-frame inputs.
    InputAck {
        ack_frame: Frame,
    },
    /// Periodic ping + time-sync report + latest confirmed checksum.
    Quality {
        frame_advantage: i16,
        ping: u32,
        cs_frame: Frame,
        cs: u64,
    },
    QualityReply {
        pong: u32,
    },
    Checksum {
        frame: Frame,
        checksum: u64,
    },
    /// Host → spectator: confirmed inputs of all players.
    SpectatorInputs {
        start_frame: Frame,
        inputs: Vec<[u16; MAX_PLAYERS]>,
    },
}

/// Reject reasons.
pub const REJECT_FULL: u8 = 1;
pub const REJECT_STARTED: u8 = 2;

pub fn encode<A: NetAddr>(nonce: u16, seq: u16, msg: &Msg<A>) -> Vec<u8> {
    let mut w = Writer(Vec::with_capacity(64));
    w.u16(MAGIC);
    w.u8(VERSION);
    w.u16(nonce);
    w.u16(seq);
    match msg {
        Msg::Join { spectator } => {
            w.u8(1);
            w.u8(*spectator as u8);
        }
        Msg::Welcome { slot } => {
            w.u8(2);
            w.u8(*slot);
        }
        Msg::Reject { reason } => {
            w.u8(3);
            w.u8(*reason);
        }
        Msg::Start(s) => {
            w.u8(4);
            let c = &s.config;
            w.u8(c.num_players);
            w.u8(match c.mode {
                SyncMode::Rollback => 0,
                SyncMode::Lockstep => 1,
            });
            w.u8(c.input_delay);
            w.u8(c.max_prediction);
            w.u16(c.checksum_interval);
            w.u32(c.disconnect_timeout_ms);
            w.u8(c.time_sync as u8);
            w.u8(s.your_slot);
            w.u8(s.players.len() as u8);
            for (slot, loc) in &s.players {
                w.u8(*slot);
                match loc {
                    PeerLoc::Sender => w.u8(0),
                    PeerLoc::Addr(a) => {
                        w.u8(1);
                        a.encode(&mut w);
                    }
                }
            }
        }
        Msg::Input { slot, ack_frame, start_frame, inputs } => {
            w.u8(5);
            w.u8(*slot);
            w.i32(*ack_frame);
            w.i32(*start_frame);
            let n = inputs.len().min(MAX_INPUTS_PER_PACKET);
            w.u8(n as u8);
            for &i in &inputs[..n] {
                w.u16(i);
            }
        }
        Msg::InputAck { ack_frame } => {
            w.u8(6);
            w.i32(*ack_frame);
        }
        Msg::Quality { frame_advantage, ping, cs_frame, cs } => {
            w.u8(7);
            w.i16(*frame_advantage);
            w.u32(*ping);
            w.i32(*cs_frame);
            w.u64(*cs);
        }
        Msg::QualityReply { pong } => {
            w.u8(8);
            w.u32(*pong);
        }
        Msg::Checksum { frame, checksum } => {
            w.u8(9);
            w.i32(*frame);
            w.u64(*checksum);
        }
        Msg::SpectatorInputs { start_frame, inputs } => {
            w.u8(10);
            w.i32(*start_frame);
            let n = inputs.len().min(MAX_INPUTS_PER_PACKET);
            w.u8(n as u8);
            for f in &inputs[..n] {
                for &p in f {
                    w.u16(p);
                }
            }
        }
    }
    w.0
}

/// Decode a datagram into `(nonce, seq, msg)`; `None` for anything malformed or foreign.
pub fn decode<A: NetAddr>(data: &[u8]) -> Option<(u16, u16, Msg<A>)> {
    let mut r = Reader::new(data);
    if r.u16()? != MAGIC || r.u8()? != VERSION {
        return None;
    }
    let nonce = r.u16()?;
    let seq = r.u16()?;
    let msg = match r.u8()? {
        1 => Msg::Join { spectator: r.u8()? != 0 },
        2 => Msg::Welcome { slot: r.u8()? },
        3 => Msg::Reject { reason: r.u8()? },
        4 => {
            let num_players = r.u8()?;
            let mode = match r.u8()? {
                0 => SyncMode::Rollback,
                1 => SyncMode::Lockstep,
                _ => return None,
            };
            let config = SessionConfig {
                num_players,
                mode,
                input_delay: r.u8()?,
                max_prediction: r.u8()?,
                checksum_interval: r.u16()?,
                disconnect_timeout_ms: r.u32()?,
                time_sync: r.u8()? != 0,
            };
            config.validate().ok()?;
            let your_slot = r.u8()?;
            let n = r.u8()? as usize;
            if n > MAX_PLAYERS {
                return None;
            }
            let mut players = Vec::with_capacity(n);
            for _ in 0..n {
                let slot = r.u8()?;
                if slot as usize >= MAX_PLAYERS {
                    return None;
                }
                let loc = match r.u8()? {
                    0 => PeerLoc::Sender,
                    1 => PeerLoc::Addr(A::decode(&mut r)?),
                    _ => return None,
                };
                players.push((slot, loc));
            }
            Msg::Start(StartMsg { config, your_slot, players })
        }
        5 => {
            let slot = r.u8()?;
            let ack_frame = r.i32()?;
            let start_frame = r.i32()?;
            let n = r.u8()? as usize;
            let mut inputs = Vec::with_capacity(n);
            for _ in 0..n {
                inputs.push(r.u16()?);
            }
            Msg::Input { slot, ack_frame, start_frame, inputs }
        }
        6 => Msg::InputAck { ack_frame: r.i32()? },
        7 => Msg::Quality { frame_advantage: r.i16()?, ping: r.u32()?, cs_frame: r.i32()?, cs: r.u64()? },
        8 => Msg::QualityReply { pong: r.u32()? },
        9 => Msg::Checksum { frame: r.i32()?, checksum: r.u64()? },
        10 => {
            let start_frame = r.i32()?;
            let n = r.u8()? as usize;
            let mut inputs = Vec::with_capacity(n);
            for _ in 0..n {
                let mut f = [0u16; MAX_PLAYERS];
                for p in &mut f {
                    *p = r.u16()?;
                }
                inputs.push(f);
            }
            Msg::SpectatorInputs { start_frame, inputs }
        }
        _ => return None,
    };
    if !r.is_empty() {
        return None;
    }
    Some((nonce, seq, msg))
}

/// `true` if sequence number `a` is newer than `b` (wrapping).
pub fn seq_newer(a: u16, b: u16) -> bool {
    (a.wrapping_sub(b) as i16) > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    #[test]
    fn roundtrip_all_messages() {
        let a: SocketAddr = "192.168.1.20:7000".parse().unwrap();
        let b: SocketAddr = "[::1]:9".parse().unwrap();
        let msgs: Vec<Msg<SocketAddr>> = vec![
            Msg::Join { spectator: true },
            Msg::Welcome { slot: 3 },
            Msg::Reject { reason: REJECT_FULL },
            Msg::Start(StartMsg {
                config: SessionConfig::default(),
                your_slot: 2,
                players: vec![(0, PeerLoc::Sender), (1, PeerLoc::Addr(a)), (2, PeerLoc::Addr(b))],
            }),
            Msg::Input { slot: 1, ack_frame: -1, start_frame: 77, inputs: vec![1, 2, 0xFFFF] },
            Msg::InputAck { ack_frame: 1234 },
            Msg::Quality { frame_advantage: -3, ping: 99, cs_frame: 600, cs: u64::MAX },
            Msg::QualityReply { pong: 4 },
            Msg::Checksum { frame: 60, checksum: 0xDEAD_BEEF },
            Msg::SpectatorInputs { start_frame: 5, inputs: vec![[1, 2, 3, 4], [5, 6, 7, 8]] },
        ];
        for m in msgs {
            let bytes = encode(0x1234, 65535, &m);
            let (nonce, seq, back) = decode::<SocketAddr>(&bytes).unwrap();
            assert_eq!((nonce, seq), (0x1234, 65535));
            assert_eq!(back, m);
            // Truncations never panic and never decode.
            for n in 0..bytes.len() {
                assert!(decode::<SocketAddr>(&bytes[..n]).is_none());
            }
        }
    }

    #[test]
    fn seq_wraps() {
        assert!(seq_newer(1, 0));
        assert!(seq_newer(0, 65535));
        assert!(!seq_newer(65535, 0));
        assert!(!seq_newer(5, 5));
    }
}
