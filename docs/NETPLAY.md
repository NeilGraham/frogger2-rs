# Netplay (`crates/f2-net`)

Status: foundation (P6-2). The crate is self-contained (no dependencies, no `f2-game` dependency)
and tested with a toy deterministic simulation. Wiring into `f2-game`/`frogger` is a follow-up
task; the requirements it places on the simulation are listed in §6.

## 1. Overview

Online play reuses the property the whole port is built on: `f2-game` is a deterministic fixed
60 Hz state machine whose only input is `TickInput { pads: [u16; 4] }`. Peers therefore exchange
**pads only** — never game state — and every peer simulates the full match.

```
            ┌──────────── RollbackSession ────────────┐
pad (local) │ input delay → InputQueue[4] → predict    │ tick(inputs) ┌──────────────┐
──────────► │ snapshots ring, rollback/resim           ├─────────────►│ impl         │
            │ checksums, time sync                     │ snapshot/    │ Simulation   │
            └──────┬──────────────────────────────▲────┘ restore     └──────────────┘
                   │ wire (8-byte header + msg)    │
                   ▼                               │
              Transport (UdpTransport | LoopbackTransport)
```

| Module | Purpose |
|---|---|
| `lib.rs` | `Simulation` trait, `SessionConfig`, `SyncMode`, `NetEvent`, `NetStats`, `NetError` |
| `session.rs` | `RollbackSession`: rollback and lockstep netcode for one local player |
| `spectator.rs` | `SpectatorSession`: replays confirmed inputs streamed by the host |
| `lobby.rs` | `HostLobby` / `JoinLobby`: slot assignment and session start |
| `joincode.rs` | address + port + nonce ⇄ `XXXXX-XXXXX-XXXXX` |
| `wire.rs` | binary protocol (hand-rolled, bounds-checked, fuzz-safe decoding) |
| `transport/` | `Transport` trait, non-blocking `UdpTransport`, in-memory `LoopbackNetwork` |
| `input.rs`, `peer.rs` | input history/prediction; per-peer seq, acks, RTT, time-sync windows |

```rust
pub trait Simulation {
    type Snapshot: Clone;
    fn tick(&mut self, inputs: [u16; 4]);
    fn snapshot(&self) -> Self::Snapshot;
    fn restore(&mut self, s: &Self::Snapshot);
    fn checksum(&self) -> u64;
}
```

All session APIs take `now_ms` from the caller (no internal clock), so tests run on a virtual
clock and are fully reproducible.

## 2. Rollback netcode (GGPO-style)

* **Frames.** `frame` = next frame to simulate. Frames `0..input_delay` use pad 0 for everyone.
* **Input delay.** `advance(sim, pad)` schedules `pad` for `frame + input_delay` and immediately
  sends it to every player peer. Delay hides latency up to `delay × 16.7 ms` without rollbacks.
* **Prediction.** A missing remote pad is predicted as that player's last confirmed pad
  ("repeat last"); pads change rarely relative to 60 Hz, so most predictions are right.
* **Confirmed-input tracking.** Each player has an in-order queue (`PlayerInputs`,
  1024-frame history); `last_confirmed` per player, `confirmed_frame()` = min over players.
  The pads actually used for every simulated frame are recorded.
* **Rollback.** When a remote pad arrives for an already-simulated frame and differs from what
  was used, the earliest such frame is remembered; the next `advance`/`settle` restores the
  snapshot taken at the start of that frame and resimulates to the present (recording the new
  pads used). Snapshots are only taken for frames that were not fully confirmed when simulated
  (only those can be rollback targets), in a ring of `max_prediction + 2`.
* **Prediction window.** `advance` refuses (`NetError::PredictionThreshold`, counted as a stall)
  when it would simulate more than `max_prediction` frames past the last frame confirmed by
  every remote player. The caller simply skips that tick and keeps polling.
* **Desync detection.** Every `checksum_interval` frames the checksum of the state at the start
  of that frame is recorded; once all inputs before it are confirmed it is final and is sent to
  every peer (`Checksum` message, and the latest one is repeated in every `Quality` message for
  loss tolerance). Mismatches raise `NetEvent::Desync { frame, local, remote, peer }`.
* **Time sync.** Each tick a peer estimates its advantage over every remote player
  (`frame − (remote last input frame − delay + RTT/2)`), averages it over 40 frames and compares
  it with the advantage the peer reports in `Quality` packets. If it is ahead by ≥ 1 frame on
  average it schedules `(local − remote)/2` wait frames (≤ 9); `should_wait()` consumes them
  one per tick (`NetEvent::WaitRecommended` is also emitted). This keeps peers within a frame of
  each other so neither carries the whole rollback burden.

### Lockstep mode

`SyncMode::Lockstep` uses the same session with an effective prediction window of 0: a frame is
simulated only when every pad for it is confirmed, so it never snapshots or rolls back. It runs at
full speed when `input_delay ≥ one-way latency + jitter` (in frames); with less delay it stays
correct but slows down. Useful for slow-to-snapshot states, debugging and as a fallback.

### Spectators

Optional (`HostLobby::new(.., allow_spectators)`). A spectator only talks to the host, which
streams fully confirmed frames (`SpectatorInputs`, redundant until acked by `InputAck`). The
spectator never predicts; `frames_behind()` lets it catch up by ticking several frames per
rendered frame. It also verifies the host's checksums. A spectator further behind than the
1024-frame history is dropped.

## 3. Protocol

Header (8 bytes, LE): `u16 magic 0x4EF2 | u8 version 1 | u16 session nonce | u16 seq | u8 kind`.

| Kind | Message | Notes |
|---|---|---|
| 1 | `Join { spectator }` | joiner → host, resent every 100 ms until `Start`; nonce 0 = "any" |
| 2 | `Welcome { slot }` | 0xFF = spectator |
| 3 | `Reject { reason }` | 1 = full, 2 = already started |
| 4 | `Start { config, your_slot, players[(slot, Sender \| addr)] }` | host's config wins |
| 5 | `Input { slot, ack_frame, start_frame, pads[≤120] }` | all unacked local pads (redundant) |
| 6 | `InputAck { ack_frame }` | spectator → host |
| 7 | `Quality { frame_advantage, ping, cs_frame, cs }` | every 200 ms; ping + time sync + latest checksum |
| 8 | `QualityReply { pong }` | RTT = now − pong (EWMA, α = 1/8) |
| 9 | `Checksum { frame, checksum }` | on finalization |
| 10 | `SpectatorInputs { start_frame, [pads; 4][≤120] }` | host → spectator |

* **Redundancy and acks.** Every `Input` packet carries *all* of the sender's pads the receiver
  has not acknowledged, and the sender's `ack_frame` for the receiver's pads. One delivered packet
  repairs any amount of earlier loss. Input packets go out on every `advance` and, while not
  advancing (stalled/waiting/finished), every 30 ms from `poll` (keeps acks and liveness flowing).
* **Sequence numbers.** Per-peer `u16` with a 32-packet duplicate window: duplicates and very old
  packets are dropped; reordered packets inside the window are accepted (every message is
  idempotent). Packets with the wrong nonce or from unknown addresses are ignored.
* **Disconnect.** No packet for `disconnect_timeout_ms` (default 5 s) ⇒ `NetEvent::Disconnected`;
  `advance` then returns `PeerDisconnected(slot)`. Continuing without the player (agreeing on a
  disconnect frame, as GGPO does) is future work.
* **Topology.** Full mesh between players (each sends its pads to every other player), star for
  spectators. 4 players at 60 Hz ≈ 3 × 60 small packets/s per peer.

## 4. Session setup

```rust
// Host
let t = UdpTransport::bind("0.0.0.0:7777")?;
let nonce = f2_net::joincode::random_nonce();
let code = f2_net::encode_join_code(public_addr, nonce);   // e.g. "C1Q4S-8XN0G-BW3MR"
let mut lobby = HostLobby::new(t, SessionConfig { num_players: 4, ..Default::default() }, nonce, true)?;
loop { lobby.poll(now_ms); /* show lobby.events(); break on "start" */ }
let mut session = lobby.start::<MyGame>(now_ms)?;            // host = slot 0

// Joiner
let (addr, nonce) = f2_net::decode_join_code(&code)?;        // or a typed address with nonce 0
let mut join = JoinLobby::new(UdpTransport::bind("0.0.0.0:0")?, addr, nonce, false);
let info = loop { if let JoinStatus::Started(i) = join.poll(now_ms) { break i } };
let mut session = join.into_session::<MyGame>(&info, now_ms)?; // or into_spectator(..)
```

* Up to 4 players; slots are assigned in join order (host 0, then 1, 2, 3) and map 1:1 to pad
  slots of `TickInput`. `num_players` in the started config is the number that joined.
* **Join code**: Crockford base32 of `ip | port | nonce | crc8` — 15 characters for IPv4
  (`XXXXX-XXXXX-XXXXX`), 34 for IPv6; case-insensitive, `I/L→1`, `O→0`, typo-detecting. There is
  no server: the code *is* the address. The host must be reachable (LAN, port forward, VPN such
  as Tailscale/ZeroTier); NAT hole punching / relays are out of scope. The host has to know its
  public address to make a code for internet play (show the LAN address by default and let the
  user override it).
* The host's lobby tells each joiner every other player's address as the host sees it; joiners
  then talk directly. If a joiner misses `Start`, its continued `Join`s make the host resend it.

## 5. Per-frame driver loop (how `frogger` will call it)

```rust
session.poll(now_ms);
for ev in session.events() { /* Desync → show warning / save both states; Disconnected → end */ }
if !session.should_wait() {
    match session.advance(&mut game, local_pad) {
        Ok(info) => { /* info.rollback_frames > 0: state was corrected */ }
        Err(NetError::PredictionThreshold) => { /* skip tick, keep rendering */ }
        Err(e) => { /* disconnected */ }
    }
}
render(&game);
```

The render/audio layer only reads `game` after `advance`, so a rollback is invisible except as a
small correction. `NetStats` exposes rollbacks, resimulated frames, stalls, waits, RTT per slot
and packet counters for an on-screen netgraph.

## 6. How `f2-game` plugs in — requirements

`impl f2_net::Simulation for f2_game::Game` (in `frogger` or `f2-game`, behind a feature) with
`tick = Game::tick(TickInput { pads })`. For that to be correct:

1. **Snapshot = clone of all mutable state.** `Game` must be `Clone` (today it is only
   `Default`); every sub-state (`actor2`, `frogger`, `frogmove`, `platform`, `enemies`, `collect`,
   `babyfrog`, `cam`, `game`, `multi`, `tongue`, `path`, `layout`, `pcaudio`, `levplay`, RNG,
   counters …) must derive `Clone`. Snapshots are taken up to once per predicted frame, so they
   must be cheap: immutable level data (`map`, `models`, `level_files`, `memload` file blobs,
   parsed banks) should sit behind `Arc` so a clone only bumps refcounts. Target: snapshot +
   restore well under 1 ms; resimulating 8 frames must fit in a 16.7 ms budget.
2. **No hidden state.** Nothing that affects future ticks may live outside `Game`: no statics,
   thread-locals, `OnceCell` caches with gameplay data, or state in the engine/audio layers that
   feeds back into gameplay. (Engine-side animation that gameplay reads must be part of `Game`.)
3. **Deterministic tick.** Pure function of state + pads: no clocks, no OS RNG
   (`Rng::seed_random` is a constant seed — keep it that way; netplay must start every peer from
   the same seed/level/options), no iteration over `HashMap`/`HashSet` with random hashers in
   gameplay paths (`assets.rs` uses `HashMap` for model lookup only — fine as long as nothing
   iterates it to drive gameplay; prefer `BTreeMap` or a fixed hasher), no pointer-address
   ordering.
4. **Floating point.** The few float paths (`actor2.rs` sqrt, `tongue.rs` matrices, `arccos`,
   MDX animation time) must give identical results on all peers: IEEE `+ - * / sqrt` are fine;
   transcendental functions (`acos`, `sin`, …) from the platform libm can differ between OSes
   and must be replaced by a pure-Rust implementation (e.g. a vendored `libm` port) or tables
   before cross-platform play. Never enable fast-math style flags.
5. **Side effects are not state.** `Game::effects` (sounds, music, rumble, FMV, saves) is emitted
   on every tick including resimulated ones. The platform layer must consume effects only from
   the final tick of an `advance` (or deduplicate by frame) and must never feed back into the
   simulation. Saves/high scores are suppressed online.
6. **Same start.** All peers must start the match from an identical `Game` (same data version —
   compare an asset hash in the lobby as a future `Start` extension, same level, same options,
   same "authentic/fixed" bug-fix profile from PLAN §6 Phase 6) at frame 0.
7. **`checksum()`** hashes all gameplay state with a fixed hash (FNV-1a/xxHash over a canonical
   field walk, *not* `std` `DefaultHasher` over a random seed). A `#[derive(Hash)]` on the state
   tree with a fixed-key hasher is the simplest route. Only every 60th frame is hashed by default.
8. **Menus/pauses.** Pausing, level transitions and the multiplayer menus must be driven by pads
   through `tick` as well (or happen outside a session), otherwise peers drift.

A `Desync` event in development should dump both peers' snapshots at that frame (the existing
trace tooling in `tools/f2-trace` can diff them).

## 7. Tests

`cargo test -p f2-net` (≈ 4 s):

* `rollback_{2,3,4}p_{0,50,150}ms` — 2–4 peers set up through the lobby on the loopback network
  with one-way latency 0/50/150 ms, jitter (latency/4, 5 ms at 0), **5 % loss**, 1 % duplicates;
  10 000 frames each; every peer must end with the same checksum as a single-process reference
  run, with no desync event. Rollback counts are printed (`--nocapture`): e.g. 4 players at 50 ms
  ≈ 1 500 rollbacks / 3 000 resimulated frames per peer, max depth 4–6; at 150 ms the default
  8-frame window is exceeded and peers stall ~15 % of ticks (raise `input_delay`/`max_prediction`
  for such links).
* `lockstep_*` — lockstep at 50/150 ms runs near real time with adequate delay; 0 rollbacks.
* `spectator_follows_host`, `desync_is_detected` (nondeterministic sim → first mismatch at the
  first checksum frame after divergence on both peers), `time_sync_slows_the_peer_that_is_ahead`,
  `lobby_rejects_when_full`, `udp_two_players_with_join_code` (real sockets on 127.0.0.1),
  plus wire/join-code/loopback/duplicate-window unit tests.

## 8. Limitations / future work

* No NAT traversal or relay server; no IPv6-to-IPv4 bridging.
* A disconnect ends the match (no agreed "disconnect frame" continuation).
* RTT is quantised by the 60 Hz poll rate (+ up to one frame).
* No encryption/authentication beyond the 16-bit session nonce (LAN/friends trust model).
* Only one local player per peer (couch + online mixing would need several local slots).
