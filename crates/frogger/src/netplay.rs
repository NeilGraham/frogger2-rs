//! Online multiplayer (`docs/NETPLAY.md`): the [`f2_net::Simulation`] wrapper around
//! [`Game`], the match set-up exchanged in the lobby, and the per-frame driver shared by the
//! window host ([`crate::app::App`]), the headless runner (`--headless --host/--join`) and the
//! loopback tests.
//!
//! Peers exchange pads only. Every peer builds the identical start state
//! ([`setup_multi`] on a fresh [`Game`]); the host sends the level and a fingerprint of its
//! start state in the lobby's `Start` message, and a joiner whose state differs (other data
//! or version) refuses to play.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;

use f2_game::define::{mode, MULTILOCAL, SINGLEPLAYER};
use f2_game::{Game, TickInput};
use f2_net::{HostLobby, JoinLobby, JoinStatus, LobbyEvent, NetError, NetEvent, PeerKind, RollbackSession, SessionConfig, Simulation, Transport, UdpTransport};

use crate::data::Data;

/// Default UDP port for `--host`.
pub const DEFAULT_PORT: u16 = 7777;

// ------------------------------------------------------------------ match set-up

/// What every peer needs to build the match's start state (sent as the lobby `app_data`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchSetup {
    pub world: u32,
    pub level: u32,
    /// Number of frogs = number of players.
    pub frogs: u8,
    /// Fingerprint of the host's start state ([`fingerprint`]).
    pub fingerprint: u64,
}

const SETUP_MAGIC: &[u8; 4] = b"F2MP";
const SETUP_VERSION: u8 = 1;

impl MatchSetup {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = SETUP_MAGIC.to_vec();
        v.push(SETUP_VERSION);
        v.push(self.world as u8);
        v.push(self.level as u8);
        v.push(self.frogs);
        v.extend_from_slice(&self.fingerprint.to_le_bytes());
        v
    }

    pub fn decode(b: &[u8]) -> Option<MatchSetup> {
        if b.len() != 16 || &b[..4] != SETUP_MAGIC || b[4] != SETUP_VERSION {
            return None;
        }
        Some(MatchSetup { world: b[5] as u32, level: b[6] as u32, frogs: b[7], fingerprint: u64::from_le_bytes(b[8..16].try_into().ok()?) })
    }
}

/// Local multiplayer on `world`/`level` with `n` frogs (what `GoMulti`, options.c:966, sets
/// up before `InitLevel`). Used for local and online play alike.
pub fn setup_multi(g: &mut Game, data: &Data, world: u32, level: u32, n: i32) -> Result<f2_game::layout::LevelAssets, String> {
    g.session.data = Some(Arc::new(data.clone()));
    g.game_state.mode = mode::INGAME_MODE;
    g.game_state.multi = MULTILOCAL;
    g.frogger.num_frogs = n.clamp(2, 4);
    g.multi.multiplayer_mode = f2_game::multi::MULTI_GAME_TYPES.get(world as usize).copied().filter(|&m| m != 0).unwrap_or(f2_game::multi::MULTIMODE_RACE);
    for i in 0..4 {
        g.frogger.player[i].character = i as _;
    }
    g.load_level(world, level, data)
}

/// `CalcStringWidth` metrics from `bigfont.bmp` / `smallfont.bmp` (what the window host
/// installs; text layout is game state, so every peer must use the same metrics).
pub fn font_metrics(data: &Data) -> Option<f2_game::textover::FontMetrics> {
    let load = |p: &str| data.read(p).ok().and_then(|b| f2_engine::Font::from_bmp(&b));
    let (b, s) = (load("textures/font/bigfont.bmp")?, load("textures/font/smallfont.bmp")?);
    let w = |f: &f2_engine::Font| (0..=255u8).map(|c| f.char_width(c).min(255) as u8).collect::<Vec<u8>>();
    Some(f2_game::textover::FontMetrics { normal: w(&b), small: w(&s) })
}

/// Fingerprint of a start state: the game checksum plus the host-installed font metrics.
pub fn fingerprint(g: &Game) -> u64 {
    let mut h = f2_game::netsync::StateHasher::new();
    h.write_u64(g.checksum());
    if let Some(m) = &g.textover.metrics {
        h.write_bytes(&m.normal);
        h.write_bytes(&m.small);
    }
    h.finish()
}

/// A fresh match start state (headless peers and tests; the window host does the same
/// through `App::start_multi` + its font hooks).
pub fn new_match_game(data: &Data, world: u32, level: u32, frogs: u8) -> Result<Game, String> {
    let mut g = Game::new();
    g.textover.metrics = font_metrics(data);
    setup_multi(&mut g, data, world, level, frogs as i32)?;
    g.take_effects();
    g.take_level_loads();
    Ok(g)
}

// ------------------------------------------------------------------ simulation wrapper

/// [`Game`] as an [`f2_net::Simulation`].
///
/// * `tick` is `Game::tick` plus the host's per-tick view update (`IsPointVisible` reads it).
/// * Frames simulated for the second time (rollback resimulation) run
///   [`Game::tick_resimulate`]: their effects were already presented and are discarded, so
///   the effect queue only ever holds each frame's first simulation.
/// * Every checksummed state is kept (last 8) for desync dumps.
pub struct NetGame {
    pub game: Game,
    /// Next frame to simulate.
    pub frame: i32,
    /// First frame never simulated before (effects of frames below it were presented).
    presented: i32,
    /// The level's far plane when no fog is set (`LevelScene::far_clip`).
    far_clip_fallback: f32,
    /// States whose checksum was taken `(frame, state)` (last 8; replaced on resimulation).
    history: std::cell::RefCell<VecDeque<(i32, Game)>>,
}

impl NetGame {
    pub fn new(game: Game) -> NetGame {
        let mut g = NetGame { game, frame: 0, presented: 0, far_clip_fallback: 1200.0, history: Default::default() };
        g.far_clip_fallback = level_far_clip(&g.game);
        g.update_view();
        g
    }

    /// `App::update_game_view` (the camera just computed, 640×480).
    fn update_view(&mut self) {
        let (_, f) = self.game.render_fog();
        let far = if f > 0.0 { f } else { self.far_clip_fallback };
        self.game.specfx.view = Some(crate::render::spec_fx_view(&self.game, far));
    }

    /// The stored state of checksum frame `frame`.
    pub fn state_at(&self, frame: i32) -> Option<Game> {
        self.history.borrow().iter().find(|(f, _)| *f == frame).map(|(_, g)| g.clone())
    }
}

/// `LevelScene::new`'s fallback far plane for the current level.
pub fn level_far_clip(g: &Game) -> f32 {
    let (w, l) = (g.frogger.player[0].world_num as usize, g.frogger.player[0].level_num as usize);
    match g.layout.world_visual_data.get(w).and_then(|wv| wv.level_visual_data.get(l)) {
        Some(lvd) if lvd.far_clip > 0 => lvd.far_clip as f32 * 0.1,
        _ => 1200.0,
    }
}

impl Simulation for NetGame {
    type Snapshot = (Game, i32);

    fn tick(&mut self, pads: [u16; 4]) {
        if self.frame < self.presented {
            self.game.tick_resimulate(TickInput { pads });
        } else {
            self.game.tick(TickInput { pads });
            self.presented = self.frame + 1;
        }
        self.frame += 1;
        self.update_view();
    }

    fn snapshot(&self) -> (Game, i32) {
        (self.game.snapshot(), self.frame)
    }

    fn restore(&mut self, s: &(Game, i32)) {
        self.game.restore(&s.0);
        self.frame = s.1;
    }

    fn checksum(&self) -> u64 {
        // Only called for checksum frames (every `checksum_interval`): keep the state for
        // desync dumps.
        let mut h = self.history.borrow_mut();
        h.retain(|(f, _)| *f != self.frame);
        h.push_back((self.frame, self.game.snapshot()));
        if h.len() > 8 {
            h.pop_front();
        }
        self.game.checksum()
    }
}

// ------------------------------------------------------------------ driver

/// Session parameters for online matches.
pub fn session_config(players: u8) -> SessionConfig {
    SessionConfig { num_players: players, input_delay: 2, max_prediction: 8, checksum_interval: 60, disconnect_timeout_ms: 5000, ..Default::default() }
}

enum Phase<T: Transport> {
    Hosting(HostLobby<T>),
    Joining(JoinLobby<T>),
    Running(RollbackSession<NetGame, T>),
    Ended,
}

/// What [`NetPlay::poll`] reports.
#[derive(Clone, Debug, PartialEq)]
pub enum NetNote {
    /// The match starts. A joiner must now build the start state from `setup`
    /// ([`NetPlay::accept_game`]); the host already has it.
    Started { setup: MatchSetup, slot: u8, players: u8 },
    /// The match ended (`reason` is shown to the player).
    Ended { reason: String },
}

/// Result of one [`NetPlay::step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Not running (lobby / ended).
    Idle,
    /// Time sync: this peer is ahead and skipped the tick.
    Waited,
    /// Too far ahead of the remote inputs: tick skipped.
    Stalled,
    /// One frame simulated (`rollback` frames resimulated first).
    Advanced { frame: i32, rollback: u32 },
}

/// One peer of an online match: lobby, then the rollback session.
pub struct NetPlay<T: Transport> {
    phase: Phase<T>,
    /// The simulation (owns the game while the match runs; the window host swaps its
    /// `App::game` in and out around [`NetPlay::step`]).
    pub sim: NetGame,
    pub setup: Option<MatchSetup>,
    pub join_code: Option<String>,
    /// Players the host waits for (including itself).
    want_players: u8,
    /// Lobby: joined players (`slot`, address text).
    lobby: Vec<(u8, String)>,
    pub local_slot: u8,
    pub players: u8,
    /// Shown on screen: why the match ended / the desync report.
    pub message: Option<String>,
    /// Final checksums `(frame, checksum)` as finalised by the session.
    pub checksums: Vec<(i32, u64)>,
    /// Write desync dumps (state parts + trace) into this directory.
    pub dump_dir: Option<std::path::PathBuf>,
    pub desyncs: u32,
}

impl<T: Transport> NetPlay<T>
where
    T::Addr: std::fmt::Debug,
{
    /// Host a match: `game` is the start state, `players` the number of players to wait for.
    pub fn host(transport: T, game: Game, world: u32, level: u32, players: u8, nonce: u16, join_code: Option<String>) -> Result<Self, String> {
        let setup = MatchSetup { world, level, frogs: players, fingerprint: fingerprint(&game) };
        let mut lobby = HostLobby::new(transport, session_config(players), nonce, false).map_err(|e| e.to_string())?;
        lobby.set_app_data(setup.encode());
        Ok(NetPlay {
            phase: Phase::Hosting(lobby),
            sim: NetGame::new(game),
            setup: Some(setup),
            join_code,
            want_players: players,
            lobby: Vec::new(),
            local_slot: 0,
            players,
            message: None,
            checksums: Vec::new(),
            dump_dir: None,
            desyncs: 0,
        })
    }

    /// Join the match hosted at `host` (`nonce` from the join code, 0 = any).
    pub fn join(transport: T, host: T::Addr, nonce: u16) -> Self {
        NetPlay {
            phase: Phase::Joining(JoinLobby::new(transport, host, nonce, false)),
            sim: NetGame::new(Game::default()),
            setup: None,
            join_code: None,
            want_players: 0,
            lobby: Vec::new(),
            local_slot: 0,
            players: 0,
            message: None,
            checksums: Vec::new(),
            dump_dir: None,
            desyncs: 0,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running(_))
    }
    pub fn in_lobby(&self) -> bool {
        matches!(self.phase, Phase::Hosting(_) | Phase::Joining(_))
    }
    pub fn session(&self) -> Option<&RollbackSession<NetGame, T>> {
        match &self.phase {
            Phase::Running(s) => Some(s),
            _ => None,
        }
    }

    /// Ends the match (the window host returns to the frontend).
    pub fn end(&mut self, reason: impl Into<String>) -> NetNote {
        let reason = reason.into();
        eprintln!("frogger: netplay: {reason}");
        self.message = Some(reason.clone());
        self.phase = Phase::Ended;
        NetNote::Ended { reason }
    }

    /// Network housekeeping; call once per rendered frame (lobby and match).
    pub fn poll(&mut self, now_ms: u64) -> Vec<NetNote> {
        let mut notes = Vec::new();
        match &mut self.phase {
            Phase::Hosting(lobby) => {
                lobby.poll(now_ms);
                for ev in lobby.events() {
                    if let LobbyEvent::PlayerJoined { slot, addr } = ev {
                        eprintln!("frogger: netplay: player {} joined from {addr:?}", slot + 1);
                        self.lobby.push((slot, format!("{addr:?}")));
                    }
                }
                if lobby.num_players() >= self.want_players {
                    let Phase::Hosting(lobby) = std::mem::replace(&mut self.phase, Phase::Ended) else { unreachable!() };
                    match lobby.start::<NetGame>(now_ms) {
                        Ok(s) => {
                            self.players = s.config().num_players;
                            self.phase = Phase::Running(s);
                            notes.push(NetNote::Started { setup: self.setup.unwrap(), slot: 0, players: self.players });
                        }
                        Err(e) => notes.push(self.end(format!("could not start: {e}"))),
                    }
                }
            }
            Phase::Joining(join) => match join.poll(now_ms) {
                JoinStatus::Started(info) => {
                    let Phase::Joining(join) = std::mem::replace(&mut self.phase, Phase::Ended) else { unreachable!() };
                    let Some(setup) = MatchSetup::decode(&info.app_data) else {
                        notes.push(self.end("the host runs an incompatible version"));
                        return notes;
                    };
                    match join.into_session::<NetGame>(&info, now_ms) {
                        Ok(s) => {
                            self.local_slot = s.local_slot();
                            self.players = s.config().num_players;
                            self.setup = Some(setup);
                            self.phase = Phase::Running(s);
                            notes.push(NetNote::Started { setup, slot: self.local_slot, players: self.players });
                        }
                        Err(e) => notes.push(self.end(format!("could not join: {e}"))),
                    }
                }
                JoinStatus::Rejected(r) => notes.push(self.end(format!("the host refused to let us join (reason {r}: {})", if r == 1 { "full" } else { "already started" }))),
                JoinStatus::Welcomed { slot } => {
                    if let Some(s) = slot {
                        if self.local_slot != s {
                            eprintln!("frogger: netplay: welcomed as player {}", s + 1);
                        }
                        self.local_slot = s;
                    }
                }
                JoinStatus::Waiting => {}
            },
            Phase::Running(s) => {
                s.poll(now_ms);
                let evs = s.events();
                for ev in evs {
                    if let Some(n) = self.on_event(ev) {
                        notes.push(n);
                    }
                }
            }
            Phase::Ended => {}
        }
        notes
    }

    /// Joiner: install the start state built from the `Started` set-up; refuses (ends the
    /// match) when it differs from the host's.
    pub fn accept_game(&mut self, game: Game) -> Result<(), String> {
        let setup = self.setup.ok_or("no match set-up")?;
        let fp = fingerprint(&game);
        self.sim = NetGame::new(game);
        if fp != setup.fingerprint {
            let msg = format!("game data differs from the host's (start state {fp:016x}, host {:016x}): check data/version", setup.fingerprint);
            self.end(msg.clone());
            return Err(msg);
        }
        Ok(())
    }

    fn on_event(&mut self, ev: NetEvent) -> Option<NetNote> {
        match ev {
            NetEvent::Desync { frame, local, remote, peer } => {
                self.desyncs += 1;
                let who = match peer {
                    PeerKind::Player(s) => format!("player {}", s + 1),
                    PeerKind::Spectator => "a spectator".into(),
                };
                let dump = self.dump_desync(frame, local, remote);
                Some(self.end(format!("desync with {who} at frame {frame} (local {local:016x}, remote {remote:016x}){dump}")))
            }
            NetEvent::Disconnected { peer: PeerKind::Player(s) } => Some(self.end(format!("player {} disconnected: match ended", s + 1))),
            NetEvent::Disconnected { .. } | NetEvent::WaitRecommended { .. } => None,
        }
    }

    /// Writes `netplay-desync-f<frame>-p<slot>.txt` (per-sub-state checksums and the trace of
    /// the stored state) so the peers' dumps can be diffed.
    fn dump_desync(&self, frame: i32, local: u64, remote: u64) -> String {
        let dir = self.dump_dir.clone().unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("netplay-desync-f{frame}-p{}.txt", self.local_slot + 1));
        let mut text = format!("frame {frame}\nlocal {local:016x}\nremote {remote:016x}\nslot {}\n", self.local_slot);
        if let Some(g) = &self.sim.state_at(frame) {
            for (name, h) in g.checksum_parts() {
                text += &format!("{name} {h:016x}\n");
            }
            text += &g.trace_frame().to_json_line();
            text += "\n";
        } else {
            text += "(state not kept)\n";
        }
        match std::fs::write(&path, text) {
            Ok(()) => format!("; dump: {}", path.display()),
            Err(e) => format!("; dump failed: {e}"),
        }
    }

    /// One 60 Hz tick of the match with this peer's pad. The caller dispatches the effects
    /// left in `sim.game` afterwards (they belong to the newly presented frame only).
    pub fn step(&mut self, local_pad: u16) -> (Step, Option<NetNote>) {
        let Phase::Running(s) = &mut self.phase else { return (Step::Idle, None) };
        if s.should_wait() {
            return (Step::Waited, None);
        }
        let step = match s.advance(&mut self.sim, local_pad) {
            Ok(info) => Step::Advanced { frame: info.frame, rollback: info.rollback_frames },
            Err(NetError::PredictionThreshold) => Step::Stalled,
            Err(NetError::PeerDisconnected(p)) => return (Step::Idle, Some(self.end(format!("player {} disconnected: match ended", p + 1)))),
            Err(e) => return (Step::Idle, Some(self.end(format!("netplay error: {e}")))),
        };
        let Phase::Running(s) = &mut self.phase else { unreachable!() };
        if let Some(c) = s.latest_checksum() {
            if self.checksums.last() != Some(&c) {
                self.checksums.push(c);
            }
        }
        let evs = s.events();
        for ev in evs {
            if let Some(n) = self.on_event(ev) {
                return (step, Some(n));
            }
        }
        // Back to the frontend (pause menu "quit"): the online match is over.
        if self.sim.game.game_state.multi == SINGLEPLAYER || self.sim.game.game_state.mode == mode::FRONTEND_MODE {
            return (step, Some(self.end("match over")));
        }
        (step, None)
    }

    /// End of a scripted run: apply pending rollbacks (all inputs confirmed).
    pub fn settle(&mut self) -> u32 {
        match &mut self.phase {
            Phase::Running(s) => s.settle(&mut self.sim),
            _ => 0,
        }
    }

    /// Last frame for which every player's input is known.
    pub fn confirmed_frame(&self) -> i32 {
        self.session().map_or(-1, |s| s.confirmed_frame())
    }

    /// Text overlay: lobby state, then ping / rollback stats while playing.
    pub fn overlay_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        match &self.phase {
            Phase::Hosting(l) => {
                if let Some(s) = &self.setup {
                    out.push(format!("ONLINE: hosting level {}-{} for {} players", s.world, s.level, self.want_players));
                }
                if let Some(c) = &self.join_code {
                    out.push(format!("join code: {c}   (frogger --join {c})"));
                }
                out.push(format!("players {}/{}:", l.num_players(), self.want_players));
                out.push("  P1 (you, host)".into());
                for (s, a) in &self.lobby {
                    out.push(format!("  P{} {a}", s + 1));
                }
                out.push("waiting for players...  (Esc quits)".into());
            }
            Phase::Joining(_) => {
                out.push("ONLINE: joining...".into());
                if self.local_slot > 0 {
                    out.push(format!("accepted as player {}; waiting for the host to start", self.local_slot + 1));
                } else {
                    out.push("contacting host (Esc quits)".into());
                }
            }
            Phase::Running(s) => {
                let st = s.stats();
                let mut line = format!("ONLINE P{}", self.local_slot + 1);
                for p in 0..self.players as usize {
                    if p == self.local_slot as usize {
                        continue;
                    }
                    match st.rtt_ms[p] {
                        Some(r) => line += &format!("  P{} {r} ms", p + 1),
                        None => line += &format!("  P{} --", p + 1),
                    }
                }
                line += &format!("  rb {} stall {}", st.rollbacks, st.stalls);
                out.push(line);
            }
            Phase::Ended => {}
        }
        if let Some(m) = &self.message {
            out.push(format!("ONLINE: {m}"));
        }
        out
    }
}

// ------------------------------------------------------------------ UDP helpers

/// The address to put in the join code: `--host-addr`, else the LAN address of the default
/// route (no packet is sent), else loopback.
pub fn public_ip(override_ip: Option<std::net::IpAddr>) -> std::net::IpAddr {
    if let Some(ip) = override_ip {
        return ip;
    }
    let probe = || -> Option<std::net::IpAddr> {
        let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        s.connect("192.0.2.1:9").ok()?; // TEST-NET-1: never routed further than the table lookup
        let ip = s.local_addr().ok()?.ip();
        (!ip.is_unspecified()).then_some(ip)
    };
    probe().unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
}

/// A UDP host: binds `port`, prints the join code.
pub fn udp_host(game: Game, world: u32, level: u32, players: u8, port: u16, ip: Option<std::net::IpAddr>) -> Result<NetPlay<UdpTransport>, String> {
    let t = UdpTransport::bind(("0.0.0.0", port)).map_err(|e| format!("cannot listen on UDP port {port}: {e}"))?;
    let port = t.local_addr().map_err(|e| e.to_string())?.port();
    let nonce = f2_net::joincode::random_nonce();
    let addr = SocketAddr::new(public_ip(ip), port);
    let code = f2_net::encode_join_code(addr, nonce);
    println!("frogger: hosting on {addr}; join code {code}");
    println!("JOIN_CODE {code}");
    NetPlay::host(t, game, world, level, players, nonce, Some(code))
}

/// A UDP joiner from a join code (or a plain `host:port`).
pub fn udp_join(code: &str) -> Result<NetPlay<UdpTransport>, String> {
    let (addr, nonce) = match f2_net::decode_join_code(code) {
        Ok(v) => v,
        Err(e) => match code.parse::<SocketAddr>() {
            Ok(a) => (a, 0),
            Err(_) => return Err(format!("bad join code {code:?}: {e}")),
        },
    };
    let bind = if addr.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
    let t = UdpTransport::bind(bind).map_err(|e| e.to_string())?;
    eprintln!("frogger: joining {addr}");
    Ok(NetPlay::join(t, addr, nonce))
}

/// Scripted pads for headless runs and tests (xorshift32; a press about every 6th frame,
/// never START).
pub struct PadGen(u32);

impl PadGen {
    pub fn new(seed: u32) -> PadGen {
        PadGen(seed | 1)
    }
    pub fn next_pad(&mut self) -> u16 {
        use f2_game::levplay::*;
        const BUTTONS: [u16; 8] = [PAD_UP, PAD_UP, PAD_RIGHT, PAD_LEFT, PAD_DOWN, PAD_CROSS, PAD_CIRCLE, PAD_SQUARE];
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        if self.0.is_multiple_of(6) {
            BUTTONS[(self.0 >> 8) as usize % BUTTONS.len()]
        } else {
            0
        }
    }
}

/// `--headless --host/--join`: plays `frames` ticks with scripted pads (`--pad-seed`) in real
/// time over UDP, then prints the final state checksum (all inputs confirmed).
pub fn run_headless(data: &Data, mut net: NetPlay<UdpTransport>, frames: u32, pad_seed: u32) -> Result<u64, String> {
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let now = || t0.elapsed().as_millis() as u64;
    let tick = Duration::from_nanos(1_000_000_000 / 60);
    let mut next = Instant::now();
    let mut pads: Option<PadGen> = None;
    let mut pending: Option<u16> = None;
    let lobby_deadline = Duration::from_secs(60);
    loop {
        for n in net.poll(now()) {
            match n {
                NetNote::Started { setup, slot, players } => {
                    eprintln!("frogger: netplay: match started: level {}-{}, {players} players, we are P{}", setup.world, setup.level, slot + 1);
                    if slot != 0 {
                        let g = new_match_game(data, setup.world, setup.level, setup.frogs)?;
                        net.accept_game(g)?;
                    }
                    pads = Some(PadGen::new(pad_seed ^ (slot as u32).wrapping_mul(0x9E37_79B9)));
                    next = Instant::now();
                }
                NetNote::Ended { reason } => return Err(reason),
            }
        }
        if net.in_lobby() && t0.elapsed() > lobby_deadline {
            return Err("timed out in the lobby".into());
        }
        if net.is_running() {
            let frame = net.sim.frame;
            if frame >= frames as i32 {
                break;
            }
            if Instant::now() >= next {
                next += tick;
                let pad = pending.take().unwrap_or_else(|| pads.as_mut().map_or(0, |p| p.next_pad()));
                let (st, note) = net.step(pad);
                if st == Step::Stalled || st == Step::Waited {
                    // The pad was not consumed: use it on the next tick.
                    pending = Some(pad);
                }
                if let Some(NetNote::Ended { reason }) = note {
                    return Err(reason);
                }
                net.sim.game.take_effects();
                net.sim.game.take_level_loads();
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    // Wait until every input of the played frames is confirmed, apply the last rollback,
    // and linger so the peers receive our last inputs and checksums.
    let deadline = Instant::now() + Duration::from_secs(10);
    while net.confirmed_frame() < frames as i32 - 1 {
        if Instant::now() > deadline {
            return Err(format!("remote inputs never confirmed (confirmed {})", net.confirmed_frame()));
        }
        for n in net.poll(now()) {
            if let NetNote::Ended { reason } = n {
                return Err(reason);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    net.settle();
    let cs = net.sim.game.checksum();
    let linger = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < linger {
        net.poll(now());
        std::thread::sleep(Duration::from_millis(2));
    }
    if net.desyncs > 0 {
        return Err(format!("{} desync(s)", net.desyncs));
    }
    let st = net.session().map(|s| s.stats()).unwrap_or_default();
    println!(
        "frogger: netplay: {frames} frames, rollbacks {} ({} frames resimulated), stalls {}, checksums compared {}",
        st.rollbacks, st.rollback_frames, st.stalls, st.checksums_compared
    );
    println!("NETPLAY_FINAL frame {frames} checksum {cs:016x}");
    Ok(cs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use f2_net::{LoopbackConfig, LoopbackNetwork, NetStats};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;

    fn data() -> Option<Data> {
        let base = std::env::var("F2_GAME_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../game-data/usa").into());
        let p = PathBuf::from(base);
        let root = if p.file_name().is_some_and(|n| n == "game") { p.parent()?.to_path_buf() } else { p };
        if !root.join("game").is_dir() {
            eprintln!("skipping: no game data (F2_GAME_DIR)");
            return None;
        }
        Data::locate(&f2_platform::Config::default(), Some(root)).ok()
    }

    /// The first race level (`multiGameTypes`), as `(world, level)`.
    fn race_level() -> (u32, u32) {
        use f2_game::bank_tables::collision_file_for_coll_bank;
        use f2_game::layout::ORIG_WORLD_VISUAL_DATA;
        for (w, wvd) in ORIG_WORLD_VISUAL_DATA.iter().enumerate() {
            if f2_game::multi::MULTI_GAME_TYPES.get(w) != Some(&f2_game::multi::MULTIMODE_RACE) {
                continue;
            }
            for l in 0..wvd.num_levels as usize {
                if collision_file_for_coll_bank(wvd.level_visual_data[l].coll_bank).is_some_and(|f| f.contains("MULTI")) {
                    return (w as u32, l as u32);
                }
            }
        }
        panic!("no race level")
    }

    #[test]
    fn match_setup_roundtrip() {
        let s = MatchSetup { world: 3, level: 4, frogs: 2, fingerprint: 0x0123_4567_89AB_CDEF };
        assert_eq!(MatchSetup::decode(&s.encode()), Some(s));
        assert_eq!(MatchSetup::decode(&s.encode()[..15]), None);
    }

    struct PeerResult {
        final_cs: u64,
        checksums: Vec<(i32, u64)>,
        stats: NetStats,
        desyncs: u32,
    }

    /// One peer's thread: lobby, `frames` scripted ticks, settle. All threads share a
    /// virtual clock advanced in lockstep (barriers) by the leader.
    fn peer(mut np: NetPlay<f2_net::LoopbackTransport>, data: Data, net: LoopbackNetwork, barrier: Arc<Barrier>, done: Arc<AtomicUsize>, leader: bool, frames: i32) -> PeerResult {
        let mut pads: Option<PadGen> = None;
        let mut pending = None;
        let mut finished = false;
        for k in 0u64.. {
            assert!(k < 40_000, "match did not finish (frame {})", np.sim.frame);
            barrier.wait();
            let now = k * 50 / 3;
            if leader {
                net.set_time(now);
            }
            barrier.wait();
            for n in np.poll(now) {
                match n {
                    NetNote::Started { setup, slot, .. } => {
                        if slot != 0 {
                            np.accept_game(new_match_game(&data, setup.world, setup.level, setup.frogs).unwrap()).unwrap();
                        }
                        pads = Some(PadGen::new(0xABCD ^ (slot as u32 * 7919)));
                    }
                    NetNote::Ended { reason } => panic!("match ended: {reason}"),
                }
            }
            if np.is_running() && !finished {
                if np.sim.frame < frames {
                    let pad = pending.take().unwrap_or_else(|| pads.as_mut().unwrap().next_pad());
                    let (st, note) = np.step(pad);
                    if matches!(st, Step::Stalled | Step::Waited) {
                        pending = Some(pad);
                    }
                    assert!(note.is_none(), "{note:?}");
                    np.sim.game.take_effects();
                } else if np.confirmed_frame() >= frames - 1 {
                    np.settle();
                    finished = true;
                    done.fetch_add(1, Ordering::SeqCst);
                }
            }
            barrier.wait();
            if done.load(Ordering::SeqCst) == 2 {
                break;
            }
        }
        let stats = np.session().unwrap().stats();
        PeerResult { final_cs: np.sim.game.checksum(), checksums: np.checksums.clone(), stats, desyncs: np.desyncs }
    }

    /// Two peers (threads) play 3600 ticks of scripted pads on a race level over the loopback
    /// network (100 ms one-way latency, 10 ms jitter, 5 % loss, 1 % duplicates). Every 60th
    /// frame's checksum must agree (compared by the session and here), and the final states
    /// must be identical.
    #[test]
    fn loopback_two_peers_100ms_5pct_loss() {
        let Some(data) = data() else { return };
        const FRAMES: i32 = 3600;
        let (w, l) = race_level();
        let net = LoopbackNetwork::new(LoopbackConfig { latency_ms: 100, jitter_ms: 10, loss: 0.05, duplicate: 0.01, seed: 42 });
        let host_t = net.endpoint();
        let host_addr = host_t.addr();
        let join_t = net.endpoint();
        let barrier = Arc::new(Barrier::new(2));
        let done = Arc::new(AtomicUsize::new(0));

        let host_game = new_match_game(&data, w, l, 2).expect("level");
        let host = NetPlay::host(host_t, host_game, w, l, 2, 0x5EED, None).unwrap();
        let joiner = NetPlay::join(join_t, host_addr, 0x5EED);
        let (d1, n1, b1, c1) = (data.clone(), net.clone(), barrier.clone(), done.clone());
        let h = std::thread::spawn(move || peer(host, d1, n1, b1, c1, true, FRAMES));
        let j = std::thread::spawn(move || peer(joiner, data, net, barrier, done, false, FRAMES));
        let (a, b) = (h.join().unwrap(), j.join().unwrap());

        for (name, r) in [("host", &a), ("joiner", &b)] {
            eprintln!(
                "{name}: rollbacks {} ({} frames, max {}), stalls {}, waits {}, checksums compared {}, rtt {:?}, final {:016x}",
                r.stats.rollbacks, r.stats.rollback_frames, r.stats.max_rollback, r.stats.stalls, r.stats.wait_frames, r.stats.checksums_compared, r.stats.rtt_ms, r.final_cs
            );
            assert_eq!(r.desyncs, 0, "{name} saw a desync");
        }
        assert!(a.stats.rollbacks + b.stats.rollbacks > 0, "no rollbacks at 100 ms: resimulation not exercised");
        // Identical checksums every 60 frames.
        let common: Vec<_> = a.checksums.iter().filter(|c| b.checksums.iter().any(|d| d.0 == c.0)).collect();
        assert!(common.len() >= (FRAMES / 60 - 5) as usize, "only {} common checksum frames", common.len());
        for c in &common {
            assert!(b.checksums.contains(c), "checksum mismatch at frame {}", c.0);
        }
        assert_eq!(a.final_cs, b.final_cs, "final states differ");
    }
}
