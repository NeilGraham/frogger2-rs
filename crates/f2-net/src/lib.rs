//! Online play for the deterministic Frogger 2 simulation.
//!
//! This crate knows nothing about `f2-game`: it drives any type implementing
//! [`Simulation`] (a deterministic fixed-tick state machine fed with four 16-bit pads per tick).
//!
//! * [`RollbackSession`] — GGPO-style rollback netcode (input delay, prediction by repeating the
//!   last confirmed input, rollback + resimulation on misprediction, periodic checksum exchange
//!   for desync detection, frame-advantage time sync). With [`SyncMode::Lockstep`] the same
//!   session never predicts: it only simulates frames whose inputs are confirmed.
//! * [`SpectatorSession`] — receives confirmed inputs from the host and replays them.
//! * [`HostLobby`] / [`JoinLobby`] — session setup by address or by [`joincode`].
//! * [`Transport`] — datagram abstraction; [`UdpTransport`] (non-blocking `std::net`) and
//!   [`LoopbackNetwork`] (in-memory, virtual clock, latency/jitter/loss) for tests.
//!
//! All session APIs take the current time in milliseconds (`now_ms`) from the caller, so tests
//! can run on a virtual clock and the game can use `Instant`.
//!
//! See `docs/NETPLAY.md` for the design and how `f2-game` plugs in.

pub mod addr;
pub mod input;
pub mod joincode;
pub mod lobby;
pub mod session;
pub mod spectator;
pub mod transport;
pub mod wire;

mod peer;

pub use addr::NetAddr;
pub use joincode::{decode_join_code, encode_join_code, JoinCodeError};
pub use lobby::{HostLobby, JoinLobby, JoinStatus, LobbyEvent, StartInfo};
pub use session::{AdvanceInfo, RollbackSession};
pub use spectator::SpectatorSession;
pub use transport::loopback::{LoopbackAddr, LoopbackConfig, LoopbackNetwork, LoopbackTransport};
pub use transport::udp::UdpTransport;
pub use transport::Transport;

/// A simulation frame number. Frame `n` is the `n`-th call to [`Simulation::tick`].
pub type Frame = i32;

/// "No frame yet".
pub const NULL_FRAME: Frame = -1;

/// Maximum number of players (pad slots).
pub const MAX_PLAYERS: usize = 4;

/// Inputs for one tick: one pad bitmask per player slot.
pub type PadInputs = [u16; MAX_PLAYERS];

/// A deterministic, fixed-tick simulation that can be saved and restored.
///
/// Requirements (see `docs/NETPLAY.md`): `tick` must be a pure function of the current state and
/// `inputs` (no clocks, no host RNG, no iteration over hash maps with random seeds, no floating
/// point that differs across builds), `restore(snapshot())` must give back a state that ticks
/// identically, and `checksum` must cover everything that influences future ticks.
pub trait Simulation {
    /// A full copy of the simulation state.
    type Snapshot: Clone;
    /// Advance one fixed 60 Hz tick.
    fn tick(&mut self, inputs: PadInputs);
    /// Capture the complete state.
    fn snapshot(&self) -> Self::Snapshot;
    /// Return to a previously captured state.
    fn restore(&mut self, s: &Self::Snapshot);
    /// A hash of the complete state (used for desync detection).
    fn checksum(&self) -> u64;
}

/// How a session keeps peers in sync.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncMode {
    /// Predict missing remote inputs, roll back and resimulate when a prediction was wrong.
    Rollback,
    /// Never predict: wait until every player's input for a frame has arrived.
    Lockstep,
}

/// Session parameters. The host's values are sent to every peer at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionConfig {
    /// Number of active player slots (1..=4). Slots `>= num_players` always read pad 0.
    pub num_players: u8,
    pub mode: SyncMode,
    /// Local inputs are scheduled this many frames in the future (hides latency).
    pub input_delay: u8,
    /// Maximum number of frames simulated ahead of the last fully confirmed frame (rollback
    /// mode). Beyond this [`RollbackSession::advance`] returns [`NetError::PredictionThreshold`].
    pub max_prediction: u8,
    /// Exchange a state checksum every this many frames (0 disables desync detection).
    pub checksum_interval: u16,
    /// A peer that sent nothing for this long is considered disconnected.
    pub disconnect_timeout_ms: u32,
    /// Recommend waiting frames when ahead of the other peers.
    pub time_sync: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            num_players: 2,
            mode: SyncMode::Rollback,
            input_delay: 2,
            max_prediction: 8,
            checksum_interval: 60,
            disconnect_timeout_ms: 5000,
            time_sync: true,
        }
    }
}

impl SessionConfig {
    /// Prediction window actually used (0 in lockstep).
    pub fn effective_max_prediction(&self) -> u8 {
        match self.mode {
            SyncMode::Rollback => self.max_prediction,
            SyncMode::Lockstep => 0,
        }
    }

    pub fn validate(&self) -> Result<(), NetError> {
        if self.num_players == 0 || self.num_players as usize > MAX_PLAYERS {
            return Err(NetError::InvalidConfig("num_players must be 1..=4"));
        }
        if self.input_delay as usize + self.max_prediction as usize > 120 {
            return Err(NetError::InvalidConfig("input_delay + max_prediction must be <= 120"));
        }
        Ok(())
    }
}

/// Who a remote endpoint is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PeerKind {
    /// A player owning this pad slot.
    Player(u8),
    /// A spectator (only connected to the host).
    Spectator,
}

/// Errors returned by session operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    /// Too far ahead of confirmed remote input; skip this tick and try again next frame.
    PredictionThreshold,
    /// A player peer timed out. The match cannot continue deterministically.
    PeerDisconnected(u8),
    /// Configuration rejected.
    InvalidConfig(&'static str),
    /// The host refused the join request.
    Rejected(u8),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetError::PredictionThreshold => write!(f, "prediction threshold reached"),
            NetError::PeerDisconnected(s) => write!(f, "player {s} disconnected"),
            NetError::InvalidConfig(m) => write!(f, "invalid config: {m}"),
            NetError::Rejected(r) => write!(f, "join rejected (reason {r})"),
        }
    }
}

impl std::error::Error for NetError {}

/// Asynchronous notifications from a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetEvent {
    /// Checksums for a confirmed frame differ: the simulations diverged.
    Desync { frame: Frame, local: u64, remote: u64, peer: PeerKind },
    /// A peer stopped responding.
    Disconnected { peer: PeerKind },
    /// Time sync: this peer is ahead; skipping `frames` ticks is recommended
    /// ([`RollbackSession::should_wait`] does it for you).
    WaitRecommended { frames: u32 },
}

/// Counters for diagnostics and tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetStats {
    /// Number of rollbacks performed.
    pub rollbacks: u64,
    /// Total frames resimulated by rollbacks.
    pub rollback_frames: u64,
    /// Deepest single rollback.
    pub max_rollback: u32,
    /// `advance` calls refused because of the prediction window.
    pub stalls: u64,
    /// Ticks skipped on time-sync recommendation.
    pub wait_frames: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    /// Packets dropped as duplicates/out of order/invalid.
    pub packets_dropped: u64,
    /// Checksums compared with a remote peer.
    pub checksums_compared: u64,
    pub desyncs: u64,
    /// Smoothed round-trip time per player slot (ms).
    pub rtt_ms: [Option<u32>; MAX_PLAYERS],
}
