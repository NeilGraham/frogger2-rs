# Netplay (`crates/f2-net`)

Status: integrated (P6-2). The crate is self-contained (no dependencies, no `f2-game` dependency)
and tested with a toy deterministic simulation; `frogger` drives it with the real game
(`crates/frogger/src/netplay.rs`, §8), `f2-game` provides snapshots/checksums/resimulation
(`crates/f2-game/src/netsync.rs`, §6). Floating-point policy for cross-platform play: §9.

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

Header (8 bytes, LE): `u16 magic 0x4EF2 | u8 version 2 | u16 session nonce | u16 seq | u8 kind`.

| Kind | Message | Notes |
|---|---|---|
| 1 | `Join { spectator }` | joiner → host, resent every 100 ms until `Start`; nonce 0 = "any" |
| 2 | `Welcome { slot }` | 0xFF = spectator |
| 3 | `Reject { reason }` | 1 = full, 2 = already started |
| 4 | `Start { config, your_slot, players[(slot, Sender \| addr)], app_data[≤255] }` | host's config wins; `app_data` = `HostLobby::set_app_data` (frogger: match set-up) |
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
   transcendental functions (`acos`, `sin`, …) from the platform libm differ between OSes, so
   the simulation calls them only through `f2_math::detfloat` (pure-Rust `libm`; §9). Never
   enable fast-math style flags.
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

### Implementation status of §6 (P6-2)

1. `Game: Clone` (one derive; `SessionState` got `Clone`). Models are `Arc<ModelInfo>`, the data
   source and script buffer were already `Arc`s; measured on a 4-frog race level after 300 ticks
   (`cargo test -p f2-game --release --test netsync -- --nocapture`): **snapshot 0.05 ms,
   restore 0.05–0.07 ms, checksum 0.11–0.13 ms, tick 0.007 ms** (debug build: 0.16 / 0.18 /
   1.3 / 0.1 ms). No further `Arc`-ing was needed.
2. Audit: the only process-wide data in `f2-game` are constant tables and the `view.rs` acos/sin
   table cache (`OnceLock`, pure). Host-side inputs to the simulation are `specfx.view` (the
   `IsPointVisible` transform, recomputed from game state after every tick by the netplay
   wrapper exactly as the window host does) and `textover.metrics` (font widths from the data;
   part of the start-state fingerprint). The test `snapshot_restore_resimulate_is_exact` also
   ticks a clone in lockstep with its original and compares effects + checksums.
5. `Game::tick_resimulate` = `tick` that drops the effects it produced. The wrapper calls it for
   every frame below the highest frame simulated so far, so each frame's effects reach the
   platform exactly once (on its first simulation; mispredicted sounds are not retracted, and
   sounds that only a corrected simulation would play are skipped — the usual GGPO trade-off).
   Saves are naturally absent (no save points in multiplayer levels).
6. Same start: the host sends `MatchSetup { world, level, frogs, fingerprint }` in the lobby's
   `Start.app_data` (wire version 2); the fingerprint hashes the start state checksum and the font
   metrics, and a joiner whose own start state differs refuses to play ("game data differs").
7. `Game::checksum()`: a fixed-key 64-bit streaming hash implemented as a minimal
   `serde::Serializer` (`StateHasher`) over every serde-derived sub-state in a fixed order (floats
   by bit pattern), plus the `Debug` text of the few sub-states without serde derives (`memload`,
   `event`, `script` minus the buffer, `textover` minus metrics). Excluded: effects, models,
   level files, game text, data source. `Game::checksum_parts()` gives per-sub-state hashes for
   desync dumps. The P6-1 gameplay options (`Game::enhance`: `GameOptions` + enemy-speed
   accumulator) are part of the checksum (sub-state `enhance`).
   Gameplay options: every peer must simulate with identical `GameOptions`, so online matches
   always run `GameOptions::AUTHENTIC` (`App::fresh_game` on host and joiner; the window host
   never re-applies its local `[gameplay]` options while `App.net` is set; headless peers use
   `Game::new()`). A peer with different options would fail the start fingerprint. Host-chosen
   options sent in `MatchSetup` are possible future work.
8. Pause menu / win screens / "quit" are pad-driven inside `tick`; quitting to the frontend ends
   the online match ("match over") and the game continues offline.

## 8. Integration in `frogger`

```text
frogger --host [--port N] [--host-addr IP] --multi WORLD LEVEL PLAYERS   # prints a join code
frogger --join CODE                                                      # or host:port
frogger --headless --frames F [--pad-seed S] (--host ... | --join CODE) # GPU-less scripted run
```

* `NetGame` implements `f2_net::Simulation` (orphan rule: the wrapper lives in `frogger`, `f2-game`
  stays independent of `f2-net`). `tick` = `Game::tick`/`tick_resimulate` + view update;
  `Snapshot = (Game, frame)`; checksum frames also keep the state (last 8) for desync dumps.
* `NetPlay<T: Transport>` drives lobby → session for the window host, the headless runner and the
  tests. The host auto-starts when PLAYERS have joined (host = slot 0 = player 1; joiners get
  slots 1.. in join order = their frog/pad slot). The local device's player-1 pad is sent as the
  local slot's pad (the enhanced hop buffer uses the local slot's frog state).
* Window host: `App` swaps its `game` into the wrapper around each `advance`, dispatches only the
  new frame's effects, and draws a text overlay: join code and joined players in the lobby,
  per-player ping, rollback and stall counts in the match. Time-sync waits and prediction stalls
  skip the tick (rendering continues).
* **Desync** → the match ends with a message naming the frame and both checksums, and
  `netplay-desync-f<frame>-p<slot>.txt` (per-sub-state checksums + the trace line of that frame's
  state) is written to the temp directory on each peer; diff the two files.
* **Disconnect** (no packets for 5 s) → "player N disconnected: match ended"; after 5 s the window
  host returns to the frontend. Headless runs exit with an error.
* The join code holds the LAN address of the default route (`--host-addr` overrides it, e.g. the
  public or VPN address).

Tests (game data required, skipped otherwise):

* `cargo test -p f2-game --test netsync` — snapshot at tick 300 of a 600-tick 4-frog race, restore
  and resimulate: identical checksum and oracle trace on every tick; clone-in-lockstep; costs.
* `cargo test -p frogger netplay` — `loopback_two_peers_100ms_5pct_loss`: two threads, lobby +
  3600 ticks of scripted pads on a race level over the loopback network (100 ms one-way, 10 ms
  jitter, 5 % loss, 1 % duplicates, virtual clock): ~1000 rollbacks per peer (max depth 8),
  59 checksums compared per peer, all equal, identical final state (≈ 4 s, debug).
* `cargo test -p frogger --test netplay_udp` — two real `frogger --headless` processes on
  127.0.0.1 (join code from the host's stdout), 600 ticks, identical `NETPLAY_FINAL` checksums.

## 9. Floating point across OS/CPU

`f2-game` is integer/fixed-point except for these float paths, all of which are now
platform-independent (bit-identical on Linux/Windows/macOS, x86-64/aarch64):

| Path | Operations | How it is made deterministic |
|---|---|---|
| `f2_math::util::{rsin, rcos, util_calc_angle}` | x87 PC24 emulation: argument / quotient rounded once to 24 bits (`mul_pc24`/`div_pc24`, Dekker two-product), `sin`/`cos`/`atan2`, `*4096` rounded to `f32`, `ftol32` | `f2_math::detfloat` (pure-Rust `libm` crate), IEEE `+ − × ÷` only |
| `f2_math::arccos` (24-bit x87 emulation) | `acos` → `f32` → `× 4096/PI2` → `f32` → `ftol32` | `detfloat::acos` |
| `f2-game/src/view.rs` | f32 matrices/quaternions, `f32::sqrt`; `InitMaths` acos/sin tables (`f64` → `f32`) | tables via `detfloat::{acos, sin}`; the rest is IEEE-exact |
| `tongue.rs`, `specfx.rs` view checks, host `spec_fx_view` (`f2_engine::gu_look_at`) | f32 `+ − × ÷ sqrt` | none needed on x86-64 / aarch64 (would differ on x87-only targets, i586) |
| `actor2.rs` | one `sqrt` | none needed |

Evidence (2026-10-09, see docs/DECISIONS.md):

* `cargo run --release -p f2-math --example float_audit [-- full]`: the integer results with
  `detfloat` equal those with the platform libm (glibc, Windows UCRT) for all 2^32
  `rsin`/`rcos` inputs, the whole `arccos` domain, `util_calc_angle` on all pairs in ±2048 plus
  50M sampled pairs, and the `view.rs` tables (f32 bits). Raw `f64` results of glibc and
  the UCRT do differ in the last ulp; `detfloat`'s do not.
* `-- digest` prints result digests; Linux x86-64 and Windows MSVC x86-64 print identical
  `detfloat` digests (the "system" raw-`f64` digest differs between the two OSes).
* `cargo run --release -p f2-math --example x87_check [-- full]` (x86-64): the port equals
  the retail MSVC6 instruction sequences executed on the host's x87 under control word
  0x007F for all 2^32 `rsin`/`rcos` inputs; the f2-math difftests do the same through the C.
* Oracle: the 0-0/1-0/6-1 attract demos and the 26-level parity corpus are identical.

Policy:

* Simulation code must not call `f32`/`f64` transcendental methods (`sin`, `cos`, `atan2`,
  `acos`, `exp`, `powf`, …) directly: use `f2_math::detfloat` (add wrappers there as needed).
  Rendering/audio code in `f2-engine`/`frogger` may use std (it never feeds the simulation).
* Never enable fast-math, FMA contraction (`-C target-feature=+fma` is fine only because Rust
  never contracts `a*b+c` implicitly; do not introduce `mul_add`) or x87 targets.
* Cross-OS / cross-CPU pairing is therefore allowed; a future `Start` extension may still
  carry an OS/arch tag next to the data fingerprint for diagnostics. The per-60-frame
  checksums remain the safety net.

## 10. Limitations / future work

* No NAT traversal or relay server; no IPv6-to-IPv4 bridging.
* A disconnect ends the match (no agreed "disconnect frame" continuation).
* The frogger host starts only when the lobby is full (no early start with fewer frogs) and has no
  in-game lobby menu yet (command line only); spectators are not wired into `frogger`.
* RTT is quantised by the 60 Hz poll rate (+ up to one frame).
* No encryption/authentication beyond the 16-bit session nonce (LAN/friends trust model).
* Only one local player per peer (couch + online mixing would need several local slots).
