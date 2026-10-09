//! Rollback (GGPO-style) and lockstep session.
//!
//! ## Frame model
//! `frame` is the next frame to simulate. Calling [`RollbackSession::advance`] schedules the local
//! pad for `frame + input_delay`, sends it to every player peer, performs any pending rollback,
//! then ticks `frame` with confirmed inputs where known and predictions (each player's last
//! confirmed pad) elsewhere. Frames `0..input_delay` use pad 0 for everyone.
//!
//! When a remote input arrives for an already simulated frame and differs from what was used,
//! the next `advance`/`settle` restores the snapshot taken at the start of that frame and
//! resimulates up to the present.
//!
//! A snapshot is only taken for frames that are not fully confirmed (only those can be rollback
//! targets), so with enough input delay a session rarely snapshots at all; in lockstep mode it
//! never does.

use std::collections::BTreeMap;

use crate::input::{FrameRing, PlayerInputs, INPUT_HISTORY};
use crate::peer::Peer;
use crate::transport::Transport;
use crate::wire::{Msg, MAX_INPUTS_PER_PACKET, REJECT_STARTED};
use crate::{Frame, NetError, NetEvent, NetStats, PadInputs, PeerKind, SessionConfig, Simulation, MAX_PLAYERS, NULL_FRAME};

/// Resend interval for input packets while no new local input is produced (stalled, waiting,
/// finished). `advance` sends immediately, so this only matters when not advancing.
const INPUT_RESEND_MS: u64 = 30;
/// Ping / quality report interval.
const QUALITY_INTERVAL_MS: u64 = 200;
/// Cap on a single time-sync wait recommendation.
const MAX_WAIT_FRAMES: u32 = 9;
/// Checksums older than this many frames are forgotten.
const CHECKSUM_HISTORY: Frame = 4096;

/// Result of a successful [`RollbackSession::advance`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdvanceInfo {
    /// The frame that was just simulated.
    pub frame: Frame,
    /// Frames resimulated by a rollback before this tick (0 = none).
    pub rollback_frames: u32,
}

/// A rollback or lockstep netplay session for one local player.
pub struct RollbackSession<S: Simulation, T: Transport> {
    transport: T,
    config: SessionConfig,
    nonce: u16,
    local_slot: u8,
    is_host: bool,
    frame: Frame,
    players: Vec<PlayerInputs>,
    /// Inputs actually used for each simulated frame (to detect mispredictions).
    used: FrameRing<PadInputs>,
    snapshots: Vec<Option<(Frame, S::Snapshot)>>,
    first_incorrect: Option<Frame>,
    peers: Vec<Peer<T::Addr>>,
    pending_cs: BTreeMap<Frame, u64>,
    local_cs: BTreeMap<Frame, u64>,
    remote_cs: BTreeMap<Frame, Vec<(PeerKind, u64)>>,
    latest_cs: (Frame, u64),
    events: Vec<NetEvent>,
    stats: NetStats,
    now_ms: u64,
    wait_frames: u32,
    next_timesync_frame: Frame,
}

impl<S: Simulation, T: Transport> RollbackSession<S, T> {
    /// Create a session directly (without a lobby). All peers must use the same `config` and
    /// `nonce`; `peers` lists every other player (and, on the host, spectators).
    pub fn new(transport: T, config: SessionConfig, local_slot: u8, nonce: u16, peers: Vec<(PeerKind, T::Addr)>, now_ms: u64) -> Result<Self, NetError> {
        config.validate()?;
        if local_slot >= config.num_players {
            return Err(NetError::InvalidConfig("local slot out of range"));
        }
        let mut seen = [false; MAX_PLAYERS];
        seen[local_slot as usize] = true;
        for (kind, _) in &peers {
            if let PeerKind::Player(s) = kind {
                if *s >= config.num_players || seen[*s as usize] {
                    return Err(NetError::InvalidConfig("bad or duplicate peer slot"));
                }
                seen[*s as usize] = true;
            }
        }
        let delay = config.input_delay as Frame;
        let peers = peers
            .into_iter()
            .map(|(kind, addr)| {
                let acked = match kind {
                    PeerKind::Player(_) => delay - 1,
                    PeerKind::Spectator => NULL_FRAME,
                };
                Peer::new(addr, kind, acked, now_ms)
            })
            .collect();
        let snap_len = config.effective_max_prediction() as usize + 2;
        Ok(RollbackSession {
            transport,
            nonce,
            local_slot,
            is_host: false,
            frame: 0,
            players: (0..config.num_players).map(|_| PlayerInputs::new(delay)).collect(),
            used: FrameRing::new(INPUT_HISTORY),
            snapshots: (0..snap_len).map(|_| None).collect(),
            first_incorrect: None,
            peers,
            pending_cs: BTreeMap::new(),
            local_cs: BTreeMap::new(),
            remote_cs: BTreeMap::new(),
            latest_cs: (NULL_FRAME, 0),
            events: Vec::new(),
            stats: NetStats::default(),
            now_ms,
            wait_frames: 0,
            next_timesync_frame: 60,
            config,
        })
    }

    /// Host only: remember the `Start` message for each peer so late/lost starts are resent.
    pub(crate) fn set_start_messages(&mut self, msgs: Vec<(T::Addr, Msg<T::Addr>)>) {
        self.is_host = true;
        for (addr, msg) in msgs {
            if let Some(p) = self.peers.iter_mut().find(|p| p.addr == addr) {
                p.start_msg = Some(msg);
            }
        }
    }

    // ---------------------------------------------------------------- accessors

    /// Next frame to simulate (= number of frames simulated).
    pub fn frame(&self) -> Frame {
        self.frame
    }
    /// Last frame for which every player's input is confirmed.
    pub fn confirmed_frame(&self) -> Frame {
        self.min_confirmed(false)
    }
    pub fn local_slot(&self) -> u8 {
        self.local_slot
    }
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }
    pub fn nonce(&self) -> u16 {
        self.nonce
    }
    pub fn stats(&self) -> NetStats {
        let mut s = self.stats.clone();
        for p in &self.peers {
            if let PeerKind::Player(slot) = p.kind {
                s.rtt_ms[slot as usize] = p.rtt_ms.map(|r| r.round() as u32);
            }
        }
        s
    }
    /// Drain pending events.
    pub fn events(&mut self) -> Vec<NetEvent> {
        std::mem::take(&mut self.events)
    }
    pub fn transport(&self) -> &T {
        &self.transport
    }
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }
    /// Confirmed inputs of all players for `frame` (if every one is known and in history).
    pub fn confirmed_inputs(&self, frame: Frame) -> Option<PadInputs> {
        let mut out = [0u16; MAX_PLAYERS];
        for (s, p) in self.players.iter().enumerate() {
            out[s] = p.confirmed(frame)?;
        }
        Some(out)
    }
    /// Most recent locally finalized checksum `(frame, checksum)`.
    pub fn latest_checksum(&self) -> Option<(Frame, u64)> {
        (self.latest_cs.0 != NULL_FRAME).then_some(self.latest_cs)
    }

    // ---------------------------------------------------------------- driving

    /// Process incoming packets and do periodic sends (resends, pings, spectator feed).
    /// Call once per rendered frame (also while waiting or stalled).
    pub fn poll(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        while let Some((data, from)) = self.transport.recv_from() {
            self.stats.packets_received += 1;
            let Some((nonce, seq, msg)) = crate::wire::decode::<T::Addr>(&data) else {
                self.stats.packets_dropped += 1;
                continue;
            };
            let Some(pi) = self.peers.iter().position(|p| p.addr == from) else {
                if self.is_host && matches!(msg, Msg::Join { .. }) {
                    let reject = crate::wire::encode(self.nonce, 0, &Msg::<T::Addr>::Reject { reason: REJECT_STARTED });
                    self.transport.send_to(&reject, from);
                }
                self.stats.packets_dropped += 1;
                continue;
            };
            if let Msg::Join { .. } = msg {
                // A joiner that missed our Start keeps asking: resend it.
                if let Some(start) = self.peers[pi].start_msg.clone() {
                    let p = &mut self.peers[pi];
                    p.send(&mut self.transport, self.nonce, &start, &mut self.stats);
                }
                continue;
            }
            if nonce != self.nonce || !self.peers[pi].accept(seq, now_ms) {
                self.stats.packets_dropped += 1;
                continue;
            }
            self.handle(pi, msg);
        }

        let timeout = self.config.disconnect_timeout_ms as u64;
        for pi in 0..self.peers.len() {
            if self.peers[pi].disconnected {
                continue;
            }
            if now_ms.saturating_sub(self.peers[pi].last_recv_ms) > timeout {
                self.peers[pi].disconnected = true;
                self.events.push(NetEvent::Disconnected { peer: self.peers[pi].kind });
                continue;
            }
            let due = |t: Option<u64>, iv: u64| t.is_none_or(|t| now_ms.saturating_sub(t) >= iv);
            if due(self.peers[pi].last_input_send_ms, INPUT_RESEND_MS) {
                match self.peers[pi].kind {
                    PeerKind::Player(_) => self.send_inputs(pi),
                    PeerKind::Spectator => self.send_spectator_inputs(pi),
                }
            }
            if due(self.peers[pi].last_quality_ms, QUALITY_INTERVAL_MS) {
                self.send_quality(pi);
            }
        }
    }

    /// Simulate one frame with `local_input` as this player's pad (scheduled `input_delay`
    /// frames ahead). On [`NetError::PredictionThreshold`] nothing happened: skip this tick
    /// (keep polling) and call again next frame.
    pub fn advance(&mut self, sim: &mut S, local_input: u16) -> Result<AdvanceInfo, NetError> {
        if let Some(p) = self.peers.iter().find(|p| p.disconnected) {
            if let PeerKind::Player(s) = p.kind {
                return Err(NetError::PeerDisconnected(s));
            }
        }
        let maxp = self.config.effective_max_prediction() as Frame;
        if self.frame - self.min_confirmed(true) > maxp {
            self.stats.stalls += 1;
            return Err(NetError::PredictionThreshold);
        }

        let lf = self.frame + self.config.input_delay as Frame;
        let ok = self.players[self.local_slot as usize].confirm(lf, local_input);
        debug_assert!(ok, "local input out of sequence");
        for pi in 0..self.peers.len() {
            if matches!(self.peers[pi].kind, PeerKind::Player(_)) && !self.peers[pi].disconnected {
                self.send_inputs(pi);
            }
        }

        let rollback_frames = self.rollback(sim);
        self.sim_frame(sim);
        self.finalize_checksums();
        self.time_sync();
        Ok(AdvanceInfo { frame: self.frame - 1, rollback_frames })
    }

    /// Apply any pending rollback and finalize checksums without advancing (e.g. at the end
    /// of a match, after the last inputs are confirmed). Returns frames resimulated.
    pub fn settle(&mut self, sim: &mut S) -> u32 {
        let n = self.rollback(sim);
        self.finalize_checksums();
        n
    }

    /// Time sync: `true` if this tick should be skipped because we are ahead of the peers.
    /// Keep calling `poll` while waiting.
    pub fn should_wait(&mut self) -> bool {
        if self.wait_frames > 0 {
            self.wait_frames -= 1;
            self.stats.wait_frames += 1;
            true
        } else {
            false
        }
    }

    // ---------------------------------------------------------------- internals

    fn min_confirmed(&self, remote_only: bool) -> Frame {
        self.players
            .iter()
            .enumerate()
            .filter(|(s, _)| !remote_only || *s != self.local_slot as usize)
            .map(|(_, p)| p.last_confirmed)
            .min()
            .unwrap_or(i32::MAX / 2)
    }

    fn inputs_for(&self, frame: Frame) -> PadInputs {
        let mut out = [0u16; MAX_PLAYERS];
        for (s, p) in self.players.iter().enumerate() {
            out[s] = p.input_or_prediction(frame);
        }
        out
    }

    fn sim_frame(&mut self, sim: &mut S) {
        let f = self.frame;
        if f > self.min_confirmed(false) {
            let i = f as usize % self.snapshots.len();
            self.snapshots[i] = Some((f, sim.snapshot()));
        }
        let iv = self.config.checksum_interval as Frame;
        if iv > 0 && f > 0 && f % iv == 0 {
            self.pending_cs.insert(f, sim.checksum());
        }
        let inputs = self.inputs_for(f);
        self.used.set(f, inputs);
        sim.tick(inputs);
        self.frame += 1;
    }

    fn rollback(&mut self, sim: &mut S) -> u32 {
        let Some(target) = self.first_incorrect.take() else { return 0 };
        if target >= self.frame {
            return 0;
        }
        let i = target as usize % self.snapshots.len();
        match &self.snapshots[i] {
            Some((f, snap)) if *f == target => sim.restore(snap),
            _ => panic!("f2-net: no snapshot for rollback target frame {target}"),
        }
        let end = self.frame;
        self.frame = target;
        while self.frame < end {
            self.sim_frame(sim);
        }
        let depth = (end - target) as u32;
        self.stats.rollbacks += 1;
        self.stats.rollback_frames += depth as u64;
        self.stats.max_rollback = self.stats.max_rollback.max(depth);
        depth
    }

    fn handle(&mut self, pi: usize, msg: Msg<T::Addr>) {
        let kind = self.peers[pi].kind;
        match msg {
            Msg::Input { slot, ack_frame, start_frame, inputs } => {
                if kind != PeerKind::Player(slot) {
                    self.stats.packets_dropped += 1;
                    return;
                }
                let p = &mut self.peers[pi];
                p.acked_frame = p.acked_frame.max(ack_frame);
                let player = &mut self.players[slot as usize];
                for (i, v) in inputs.into_iter().enumerate() {
                    let f = start_frame.wrapping_add(i as Frame);
                    if f <= player.last_confirmed {
                        continue;
                    }
                    if !player.confirm(f, v) {
                        break;
                    }
                    if f < self.frame {
                        let used = self.used.get(f).map(|u| u[slot as usize]);
                        if used != Some(v) {
                            self.first_incorrect = Some(self.first_incorrect.map_or(f, |x| x.min(f)));
                        }
                    }
                }
            }
            Msg::InputAck { ack_frame } => {
                if kind == PeerKind::Spectator {
                    let p = &mut self.peers[pi];
                    p.acked_frame = p.acked_frame.max(ack_frame);
                }
            }
            Msg::Quality { frame_advantage, ping, cs_frame, cs } => {
                let p = &mut self.peers[pi];
                p.remote_adv.push(frame_advantage as i32);
                let reply = Msg::QualityReply { pong: ping };
                p.send(&mut self.transport, self.nonce, &reply, &mut self.stats);
                if cs_frame > 0 {
                    self.record_remote_checksum(kind, cs_frame, cs);
                }
            }
            Msg::QualityReply { pong } => self.peers[pi].on_pong(self.now_ms, pong),
            Msg::Checksum { frame, checksum } => self.record_remote_checksum(kind, frame, checksum),
            _ => {}
        }
    }

    fn send_inputs(&mut self, pi: usize) {
        let PeerKind::Player(slot) = self.peers[pi].kind else { return };
        let local = &self.players[self.local_slot as usize];
        let start = self.peers[pi].acked_frame + 1;
        let end = local.last_confirmed.min(start + MAX_INPUTS_PER_PACKET as Frame - 1);
        let mut inputs = Vec::with_capacity((end - start + 1).max(0) as usize);
        for f in start..=end {
            match local.confirmed(f) {
                Some(v) => inputs.push(v),
                None => break,
            }
        }
        let msg = Msg::Input { slot: self.local_slot, ack_frame: self.players[slot as usize].last_confirmed, start_frame: start, inputs };
        let p = &mut self.peers[pi];
        p.last_input_send_ms = Some(self.now_ms);
        p.send(&mut self.transport, self.nonce, &msg, &mut self.stats);
    }

    fn send_spectator_inputs(&mut self, pi: usize) {
        self.peers[pi].last_input_send_ms = Some(self.now_ms);
        let start = self.peers[pi].acked_frame + 1;
        let end = self.min_confirmed(false).min(start + MAX_INPUTS_PER_PACKET as Frame - 1);
        if start > end {
            return;
        }
        let mut inputs = Vec::new();
        for f in start..=end {
            match self.confirmed_inputs(f) {
                Some(i) => inputs.push(i),
                None => {
                    // Spectator fell out of our input history: drop it.
                    self.peers[pi].disconnected = true;
                    self.events.push(NetEvent::Disconnected { peer: PeerKind::Spectator });
                    return;
                }
            }
        }
        let msg = Msg::SpectatorInputs { start_frame: start, inputs };
        let p = &mut self.peers[pi];
        p.send(&mut self.transport, self.nonce, &msg, &mut self.stats);
    }

    /// Our estimate of how many frames we are ahead of player peer `pi`.
    fn local_advantage(&self, pi: usize) -> i32 {
        let PeerKind::Player(slot) = self.peers[pi].kind else { return 0 };
        let remote_input_frame = self.players[slot as usize].last_confirmed;
        // The peer sent input for frame `remote_frame + delay` about RTT/2 ago.
        let est = remote_input_frame - self.config.input_delay as Frame + (self.peers[pi].rtt_frames() / 2.0).round() as Frame;
        self.frame - est
    }

    fn send_quality(&mut self, pi: usize) {
        let adv = self.local_advantage(pi).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let msg = Msg::Quality { frame_advantage: adv, ping: self.now_ms as u32, cs_frame: self.latest_cs.0, cs: self.latest_cs.1 };
        let p = &mut self.peers[pi];
        p.last_quality_ms = Some(self.now_ms);
        p.send(&mut self.transport, self.nonce, &msg, &mut self.stats);
    }

    fn time_sync(&mut self) {
        for pi in 0..self.peers.len() {
            if matches!(self.peers[pi].kind, PeerKind::Player(_)) {
                let adv = self.local_advantage(pi);
                self.peers[pi].local_adv.push(adv);
            }
        }
        if !self.config.time_sync || self.wait_frames > 0 || self.frame < self.next_timesync_frame {
            return;
        }
        let mut rec = 0u32;
        for p in &self.peers {
            if !matches!(p.kind, PeerKind::Player(_)) || !p.local_adv.is_full() {
                continue;
            }
            let (Some(local), Some(remote)) = (p.local_adv.average(), p.remote_adv.average()) else {
                continue;
            };
            let diff = (local - remote) / 2.0;
            if diff >= 1.0 {
                rec = rec.max((diff.round() as u32).min(MAX_WAIT_FRAMES));
            }
        }
        if rec > 0 {
            self.wait_frames = rec;
            self.events.push(NetEvent::WaitRecommended { frames: rec });
            self.next_timesync_frame = self.frame + 120;
        } else {
            self.next_timesync_frame = self.frame + 30;
        }
    }

    fn finalize_checksums(&mut self) {
        if self.pending_cs.is_empty() {
            return;
        }
        let limit = self.min_confirmed(false).saturating_add(1);
        let ready: Vec<Frame> = self.pending_cs.range(..=limit).map(|(f, _)| *f).collect();
        for f in ready {
            let cs = self.pending_cs.remove(&f).unwrap();
            self.local_cs.insert(f, cs);
            self.latest_cs = (f, cs);
            let msg = Msg::Checksum { frame: f, checksum: cs };
            for p in self.peers.iter_mut().filter(|p| !p.disconnected) {
                p.send(&mut self.transport, self.nonce, &msg, &mut self.stats);
            }
            if let Some(list) = self.remote_cs.get(&f).cloned() {
                for (kind, remote) in list {
                    self.compare(f, kind, cs, remote);
                }
            }
        }
        let horizon = self.frame - CHECKSUM_HISTORY;
        self.local_cs = self.local_cs.split_off(&horizon);
        self.remote_cs = self.remote_cs.split_off(&horizon);
    }

    fn record_remote_checksum(&mut self, kind: PeerKind, frame: Frame, cs: u64) {
        if frame < self.frame - CHECKSUM_HISTORY {
            return;
        }
        let list = self.remote_cs.entry(frame).or_default();
        if list.iter().any(|(k, _)| *k == kind) {
            return;
        }
        list.push((kind, cs));
        if let Some(&local) = self.local_cs.get(&frame) {
            self.compare(frame, kind, local, cs);
        }
    }

    fn compare(&mut self, frame: Frame, peer: PeerKind, local: u64, remote: u64) {
        self.stats.checksums_compared += 1;
        if local != remote {
            self.stats.desyncs += 1;
            self.events.push(NetEvent::Desync { frame, local, remote, peer });
        }
    }
}
