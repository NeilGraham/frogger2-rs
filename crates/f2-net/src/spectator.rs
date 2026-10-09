//! Spectator: replays the confirmed inputs streamed by the host. Never predicts.

use std::collections::BTreeMap;

use crate::input::{FrameRing, INPUT_HISTORY};
use crate::peer::Peer;
use crate::transport::Transport;
use crate::wire::Msg;
use crate::{Frame, NetError, NetEvent, NetStats, PadInputs, PeerKind, SessionConfig, Simulation, NULL_FRAME};

const ACK_INTERVAL_MS: u64 = 16;
const QUALITY_INTERVAL_MS: u64 = 200;

pub struct SpectatorSession<T: Transport> {
    transport: T,
    config: SessionConfig,
    nonce: u16,
    host: Peer<T::Addr>,
    frame: Frame,
    inputs: FrameRing<PadInputs>,
    last_received: Frame,
    local_cs: BTreeMap<Frame, u64>,
    remote_cs: BTreeMap<Frame, u64>,
    events: Vec<NetEvent>,
    stats: NetStats,
    now_ms: u64,
    last_ack_ms: Option<u64>,
}

impl<T: Transport> SpectatorSession<T> {
    pub fn new(transport: T, config: SessionConfig, nonce: u16, host: T::Addr, now_ms: u64) -> Self {
        SpectatorSession {
            transport,
            config,
            nonce,
            host: Peer::new(host, PeerKind::Player(0), NULL_FRAME, now_ms),
            frame: 0,
            inputs: FrameRing::new(INPUT_HISTORY),
            last_received: NULL_FRAME,
            local_cs: BTreeMap::new(),
            remote_cs: BTreeMap::new(),
            events: Vec::new(),
            stats: NetStats::default(),
            now_ms,
            last_ack_ms: None,
        }
    }

    pub fn frame(&self) -> Frame {
        self.frame
    }
    /// Frames received but not yet simulated (advance several times per tick to catch up).
    pub fn frames_behind(&self) -> i32 {
        self.last_received + 1 - self.frame
    }
    pub fn stats(&self) -> NetStats {
        let mut s = self.stats.clone();
        s.rtt_ms[0] = self.host.rtt_ms.map(|r| r.round() as u32);
        s
    }
    pub fn events(&mut self) -> Vec<NetEvent> {
        std::mem::take(&mut self.events)
    }
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    pub fn poll(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        let mut got_new = false;
        while let Some((data, from)) = self.transport.recv_from() {
            self.stats.packets_received += 1;
            if from != self.host.addr {
                self.stats.packets_dropped += 1;
                continue;
            }
            let Some((nonce, seq, msg)) = crate::wire::decode::<T::Addr>(&data) else {
                self.stats.packets_dropped += 1;
                continue;
            };
            if nonce != self.nonce || !self.host.accept(seq, now_ms) {
                self.stats.packets_dropped += 1;
                continue;
            }
            match msg {
                Msg::SpectatorInputs { start_frame, inputs } => {
                    for (i, f_inputs) in inputs.into_iter().enumerate() {
                        let f = start_frame.wrapping_add(i as Frame);
                        if f == self.last_received + 1 && f - self.frame < INPUT_HISTORY as Frame {
                            self.inputs.set(f, f_inputs);
                            self.last_received = f;
                            got_new = true;
                        }
                    }
                }
                Msg::Quality { ping, cs_frame, cs, .. } => {
                    let reply = Msg::QualityReply { pong: ping };
                    self.host.send(&mut self.transport, self.nonce, &reply, &mut self.stats);
                    if cs_frame > 0 {
                        self.record_remote(cs_frame, cs);
                    }
                }
                Msg::QualityReply { pong } => self.host.on_pong(now_ms, pong),
                Msg::Checksum { frame, checksum } => self.record_remote(frame, checksum),
                _ => {}
            }
        }
        if !self.host.disconnected && now_ms.saturating_sub(self.host.last_recv_ms) > self.config.disconnect_timeout_ms as u64 {
            self.host.disconnected = true;
            self.events.push(NetEvent::Disconnected { peer: PeerKind::Player(0) });
        }
        let due = |t: Option<u64>, iv: u64| t.is_none_or(|t| now_ms.saturating_sub(t) >= iv);
        if got_new || due(self.last_ack_ms, ACK_INTERVAL_MS) {
            self.last_ack_ms = Some(now_ms);
            let msg = Msg::InputAck { ack_frame: self.last_received };
            self.host.send(&mut self.transport, self.nonce, &msg, &mut self.stats);
        }
        if due(self.host.last_quality_ms, QUALITY_INTERVAL_MS) {
            self.host.last_quality_ms = Some(now_ms);
            let msg = Msg::Quality { frame_advantage: 0, ping: now_ms as u32, cs_frame: NULL_FRAME, cs: 0 };
            self.host.send(&mut self.transport, self.nonce, &msg, &mut self.stats);
        }
    }

    /// Simulate the next frame if its inputs have arrived. `Ok(None)` = nothing to do yet.
    pub fn advance<S: Simulation>(&mut self, sim: &mut S) -> Result<Option<Frame>, NetError> {
        if self.host.disconnected && self.frame > self.last_received {
            return Err(NetError::PeerDisconnected(0));
        }
        if self.frame > self.last_received {
            return Ok(None);
        }
        let f = self.frame;
        let iv = self.config.checksum_interval as Frame;
        if iv > 0 && f > 0 && f % iv == 0 {
            let cs = sim.checksum();
            self.local_cs.insert(f, cs);
            if let Some(&r) = self.remote_cs.get(&f) {
                self.compare(f, cs, r);
            }
        }
        let inputs = self.inputs.get(f).expect("received input missing from history");
        sim.tick(inputs);
        self.frame += 1;
        let horizon = self.frame - 4096;
        self.local_cs = self.local_cs.split_off(&horizon);
        Ok(Some(f))
    }

    fn record_remote(&mut self, frame: Frame, cs: u64) {
        if self.remote_cs.insert(frame, cs).is_some() {
            return;
        }
        if let Some(&l) = self.local_cs.get(&frame) {
            self.compare(frame, l, cs);
        }
        let horizon = self.frame - 4096;
        self.remote_cs = self.remote_cs.split_off(&horizon);
    }

    fn compare(&mut self, frame: Frame, local: u64, remote: u64) {
        self.stats.checksums_compared += 1;
        if local != remote {
            self.stats.desyncs += 1;
            self.events.push(NetEvent::Desync { frame, local, remote, peer: PeerKind::Player(0) });
        }
    }
}
