//! Session setup: the host listens, joiners dial its address (or a join code); when the host
//! starts, every participant receives the slot table and builds its session.
//!
//! No external server is involved: peers talk directly (full mesh between players; spectators
//! only talk to the host). NAT traversal is out of scope (port forwarding / LAN / VPN).

use crate::peer::Peer;
use crate::session::RollbackSession;
use crate::spectator::SpectatorSession;
use crate::transport::Transport;
use crate::wire::{self, Msg, PeerLoc, StartMsg, REJECT_FULL, SPECTATOR_SLOT};
use crate::{NetError, NetStats, PeerKind, SessionConfig, Simulation, MAX_PLAYERS};

const JOIN_RESEND_MS: u64 = 100;

/// Lobby notifications on the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LobbyEvent<A> {
    PlayerJoined { slot: u8, addr: A },
    SpectatorJoined { addr: A },
    Rejected { addr: A },
}

/// Everything a participant needs to build its session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartInfo<A> {
    pub config: SessionConfig,
    pub nonce: u16,
    /// `None` for a spectator.
    pub local_slot: Option<u8>,
    pub host: A,
    /// Other players (for a spectator: empty — it only talks to the host).
    pub peers: Vec<(PeerKind, A)>,
}

/// The hosting side of a lobby. The host is always player slot 0.
pub struct HostLobby<T: Transport> {
    transport: T,
    config: SessionConfig,
    nonce: u16,
    max_players: u8,
    allow_spectators: bool,
    players: Vec<(u8, Peer<T::Addr>)>,
    spectators: Vec<Peer<T::Addr>>,
    events: Vec<LobbyEvent<T::Addr>>,
    stats: NetStats,
}

impl<T: Transport> HostLobby<T> {
    /// `config.num_players` is the maximum number of players (the session is started with the
    /// number that actually joined). `nonce` should be random and non-zero (see
    /// [`crate::joincode::random_nonce`]).
    pub fn new(transport: T, config: SessionConfig, nonce: u16, allow_spectators: bool) -> Result<Self, NetError> {
        config.validate()?;
        if nonce == 0 {
            return Err(NetError::InvalidConfig("nonce must be non-zero"));
        }
        Ok(HostLobby {
            transport,
            max_players: config.num_players,
            config,
            nonce,
            allow_spectators,
            players: Vec::new(),
            spectators: Vec::new(),
            events: Vec::new(),
            stats: NetStats::default(),
        })
    }

    pub fn nonce(&self) -> u16 {
        self.nonce
    }
    /// Players including the host.
    pub fn num_players(&self) -> u8 {
        1 + self.players.len() as u8
    }
    pub fn num_spectators(&self) -> usize {
        self.spectators.len()
    }
    pub fn events(&mut self) -> Vec<LobbyEvent<T::Addr>> {
        std::mem::take(&mut self.events)
    }
    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn poll(&mut self, now_ms: u64) {
        while let Some((data, from)) = self.transport.recv_from() {
            let Some((nonce, _seq, msg)) = wire::decode::<T::Addr>(&data) else { continue };
            let Msg::Join { spectator } = msg else { continue };
            if nonce != 0 && nonce != self.nonce {
                continue;
            }
            // Already joined: repeat the welcome.
            let known =
                self.players.iter_mut().map(|(s, p)| (*s, p)).chain(self.spectators.iter_mut().map(|p| (SPECTATOR_SLOT, p))).find(|(_, p)| p.addr == from);
            if let Some((slot, p)) = known {
                p.last_recv_ms = now_ms;
                p.send(&mut self.transport, self.nonce, &Msg::Welcome { slot }, &mut self.stats);
                continue;
            }
            let free_slot = self.num_players() < self.max_players;
            let reply = if !spectator && free_slot {
                let slot = self.num_players();
                let mut p = Peer::new(from, PeerKind::Player(slot), 0, now_ms);
                p.send(&mut self.transport, self.nonce, &Msg::Welcome { slot }, &mut self.stats);
                self.players.push((slot, p));
                self.events.push(LobbyEvent::PlayerJoined { slot, addr: from });
                None
            } else if self.allow_spectators {
                let mut p = Peer::new(from, PeerKind::Spectator, 0, now_ms);
                let msg = Msg::Welcome { slot: SPECTATOR_SLOT };
                p.send(&mut self.transport, self.nonce, &msg, &mut self.stats);
                self.spectators.push(p);
                self.events.push(LobbyEvent::SpectatorJoined { addr: from });
                None
            } else {
                Some(Msg::<T::Addr>::Reject { reason: REJECT_FULL })
            };
            if let Some(r) = reply {
                self.transport.send_to(&wire::encode(self.nonce, 0, &r), from);
                self.events.push(LobbyEvent::Rejected { addr: from });
            }
        }
    }

    /// Close the lobby and start the match as player slot 0.
    pub fn start<S: Simulation>(mut self, now_ms: u64) -> Result<RollbackSession<S, T>, NetError> {
        let mut config = self.config.clone();
        config.num_players = self.num_players();
        let table: Vec<(u8, T::Addr)> = self.players.iter().map(|(s, p)| (*s, p.addr)).collect();
        let start_for = |your_slot: u8, to: Option<T::Addr>| -> Msg<T::Addr> {
            let mut players = vec![(0u8, PeerLoc::Sender)];
            players.extend(table.iter().filter(|(_, a)| Some(*a) != to).map(|(s, a)| (*s, PeerLoc::Addr(*a))));
            Msg::Start(StartMsg { config: config.clone(), your_slot, players })
        };
        let mut starts = Vec::new();
        for (slot, p) in &mut self.players {
            let m = start_for(*slot, Some(p.addr));
            p.send(&mut self.transport, self.nonce, &m, &mut self.stats);
            starts.push((p.addr, m));
        }
        for p in &mut self.spectators {
            let m = start_for(SPECTATOR_SLOT, Some(p.addr));
            p.send(&mut self.transport, self.nonce, &m, &mut self.stats);
            starts.push((p.addr, m));
        }
        let peers = table.iter().map(|(s, a)| (PeerKind::Player(*s), *a)).chain(self.spectators.iter().map(|p| (PeerKind::Spectator, p.addr))).collect();
        let mut session = RollbackSession::new(self.transport, config, 0, self.nonce, peers, now_ms)?;
        session.set_start_messages(starts);
        Ok(session)
    }
}

/// Progress of a join attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinStatus<A> {
    Waiting,
    /// Accepted; `slot` is `None` for a spectator.
    Welcomed {
        slot: Option<u8>,
    },
    Started(StartInfo<A>),
    Rejected(u8),
}

/// The joining side of a lobby.
pub struct JoinLobby<T: Transport> {
    transport: T,
    host: T::Addr,
    nonce: u16,
    spectator: bool,
    last_send_ms: Option<u64>,
    seq: u16,
}

impl<T: Transport> JoinLobby<T> {
    /// Join `host`. `nonce` from a join code, or 0 to accept whatever session is hosted there.
    pub fn new(transport: T, host: T::Addr, nonce: u16, spectator: bool) -> Self {
        JoinLobby { transport, host, nonce, spectator, last_send_ms: None, seq: 0 }
    }

    /// Keep calling until `Started` or `Rejected`.
    pub fn poll(&mut self, now_ms: u64) -> JoinStatus<T::Addr> {
        let mut status = JoinStatus::Waiting;
        while let Some((data, from)) = self.transport.recv_from() {
            if from != self.host {
                continue; // early traffic from other players; they will resend
            }
            let Some((nonce, _seq, msg)) = wire::decode::<T::Addr>(&data) else { continue };
            if self.nonce != 0 && nonce != self.nonce {
                continue;
            }
            match msg {
                Msg::Welcome { slot } => {
                    self.nonce = nonce;
                    status = JoinStatus::Welcomed { slot: (slot != SPECTATOR_SLOT).then_some(slot) };
                }
                Msg::Reject { reason } => return JoinStatus::Rejected(reason),
                Msg::Start(s) => {
                    self.nonce = nonce;
                    return JoinStatus::Started(self.start_info(s));
                }
                _ => {}
            }
        }
        if self.last_send_ms.is_none_or(|t| now_ms.saturating_sub(t) >= JOIN_RESEND_MS) {
            self.last_send_ms = Some(now_ms);
            self.seq = self.seq.wrapping_add(1);
            let msg = Msg::<T::Addr>::Join { spectator: self.spectator };
            self.transport.send_to(&wire::encode(self.nonce, self.seq, &msg), self.host);
        }
        status
    }

    fn start_info(&self, s: StartMsg<T::Addr>) -> StartInfo<T::Addr> {
        let local_slot = (s.your_slot != SPECTATOR_SLOT && (s.your_slot as usize) < MAX_PLAYERS).then_some(s.your_slot);
        let peers = if local_slot.is_some() {
            s.players
                .into_iter()
                .filter(|(slot, _)| Some(*slot) != local_slot)
                .map(|(slot, loc)| {
                    let addr = match loc {
                        PeerLoc::Sender => self.host,
                        PeerLoc::Addr(a) => a,
                    };
                    (PeerKind::Player(slot), addr)
                })
                .collect()
        } else {
            Vec::new()
        };
        StartInfo { config: s.config, nonce: self.nonce, local_slot, host: self.host, peers }
    }

    /// Build the player session from a `Started` status.
    pub fn into_session<S: Simulation>(self, info: &StartInfo<T::Addr>, now_ms: u64) -> Result<RollbackSession<S, T>, NetError> {
        let slot = info.local_slot.ok_or(NetError::InvalidConfig("joined as spectator"))?;
        RollbackSession::new(self.transport, info.config.clone(), slot, info.nonce, info.peers.clone(), now_ms)
    }

    /// Build the spectator session from a `Started` status.
    pub fn into_spectator(self, info: &StartInfo<T::Addr>, now_ms: u64) -> SpectatorSession<T> {
        SpectatorSession::new(self.transport, info.config.clone(), info.nonce, info.host, now_ms)
    }
}
