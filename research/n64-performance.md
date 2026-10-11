# N64 performance probes (phase 1B)

Measured in **ares v148** (Expansion Pak on, paraLLEl-RDP) with the bring-up ROM
(`tools/n64/build.sh`, release + LTO, Rust n32 `-C opt-level=3`, C `-O2`), probes in
`crates/frogger-n64/src/probes.rs` (+ `csrc/main.c` `n64_probe_c`). Timer: C0 COUNT,
46.875 MHz; CPU cycles = 2 × ticks (VR4300 @ 93.75 MHz); RCP cycles = 4/3 × ticks
(62.5 MHz). One 60 Hz frame = 1.5625 M CPU cycles. Date: 2026-10-10.

Correctness cross-check: the ROM's f2-math self-test (`088a1a37`), the scene.rs-style f32
transform (1759 tris, `b689c4ca`) and the fixed-point tick (`f78637a0`) checksums equal the
x86-64 host's (`cargo test --manifest-path crates/frogger-n64/Cargo.toml` asserts them).

## Workload: triangles per frame in the PC game
3D triangles queued per frame (`Renderer::queued_tris`, all `Frame`s, no backdrop/HUD),
headless `frogger --level W L`, frog idle, sampled every 30 ticks over 900 ticks
(temporary instrumentation, not committed):

| level (W L) | median | max | | level | median | max |
|---|---|---|---|---|---|---|
| 0 0 (Garden 1) | 612 | 648 | | 5 0 | 1177 | 1245 |
| 1 0 | 643 | 654 | | 5 1 | 958 | 1026 |
| 1 1 | 467 | 495 | | 5 2 / 5 3 | 497 / 208 | 504 / 215 |
| 1 2 / 1 3 | 655 / 485 | 686 / 500 | | 6 0 | 990 | 1028 |
| 2 0 | 1133 | 1219 | | 6 1 / 6 2 | 666 / 440 | 671 / 447 |
| 2 1 / 2 2 | 670 / 858 | 679 / 885 | | 7 0..3 | 353–768 | 780 |
| 3 0 | 473 | 477 | | **8 0** | **1471** | **1513** |
| 4 0 | 1044 | 1118 | | 8 3 | 715 | 732 |
| 4 1 / 4 2 | 741 / 821 | 794 / 931 | | 9 0..3 | 677–1084 | 1088 |

So: typically **450–1200**, worst **~1500** triangles per frame (more with effects,
multiplayer split-screen renders the scene per view).

## (a) CPU transform, scene.rs code shape (f32)
Faithful copies of `prepare_object` (mtx_cat + `mtx_xfm` + `View::project` with the
integer-truncated `oozd`) and `render_mesh` (screen reject, cull, `TlVertex` emission) on a
32×32 grid (1024 vertices, 1922 faces → 1759 visible triangles):

| stage | cost |
|---|---|
| `prepare_object` | **332 cycles / vertex** (3.6 ms for 1024) |
| raw `mtx_xfm` only | 148 cycles / vertex |
| `render_mesh` | **1736 cycles / face** (1897 per emitted triangle) |
| `TlVertex` → rdpq vertex (10 f32) | 1208 cycles / triangle (pure memory traffic) |

The f32 arithmetic is not the problem; **memory traffic is**: 40-byte f32 `TlVertex`es,
per-face-vertex `[f32; 4]` colours, `Option<[f32;3]>` caches and `Vec<Tri>` pushes miss
the 8 KiB D-cache constantly (RDRAM latency), e.g. the plain struct conversion costs
~1200 cycles per triangle.

## (b) rdpq triangle submission (320×240, 16-bit, z-buffer)
libdragon preview's `rdpq_triangle` does triangle setup on the **RSP**; the CPU converts
the float vertex to fixed point and writes 3 × `TRIANGLE_DATA` + 1 `TRIANGLE` command.
"CPU-only" = the batch recorded into an rspq block (nothing executes); "RSP+RDP-only" =
replaying that block until idle.

| batch | via Rust FFI: CPU cycles/tri | C loop: CPU cycles/tri | RSP+RDP per tri |
|---|---|---|---|
| shade + z, ~27 px | 2417 | 1866 | 385 RCP cycles |
| shade + z, ~247 px | 2439 | – | 386 |
| tex + shade + z, ~27 px | 2735 | 2125 | 391 |
| tex + shade + z, ~247 px | 2757 | – | 391 |
| tex + shade + z, ~1761 px | 2844 | – | 395 |
| tex + shade + z, ~11000 px | 2620 | – | 422 |

- CPU-bound limit through the current FFI path: **~550–650 tris/frame at 60 Hz** with the
  whole CPU spent on submission (the C loop with hot vertices: ~730–840).
- The FFI itself is cheap (stub + 4 register args); the extra ~500 cycles vs the C loop is
  reading the vertex arrays from RAM (the C loop reuses three cached vertices).
- RSP setup: ~390 RCP cycles/tri → **~2650 tris/frame @60, ~5300 @30** if the CPU were free.

### Full pipeline (transform + emit + convert + submit), 1759 triangles
prepare 3.8 ms + render_mesh 37 ms + convert 23 ms + submit 51 ms = **114 ms CPU
(6100 cycles per triangle)**, RSP/RDP finish 0.1 ms later. I.e. the faithful CPU path
manages only **~256 tris/frame @60, ~512 @30** with zero game logic. (The first version of
this probe measured 253 ms because its `Vec`s grew from empty: on the N64 every realloc
copy is expensive — presize everything.)

## (c) Fixed-point game work (f2-math)
| operation | cycles/call |
|---|---|
| `fmul` | 43 |
| `fdiv` | 99 |
| `fsqrt` (float bit trick + `ftol`) | 86 |
| `Random` | 59 |
| `make_unit` | 356 |
| **`rsin`/`rcos`** (libm f64 `sin` with PC24 rounding) | **461** |
| **`iquat_slerp`** (`arccos` + 3 `rsin` + `fdiv2`) | **~4200–4700** |

A synthetic per-actor update (steer + normalise + integrate + sqrt + slerp + sine bob +
RNG) costs ~8800 cycles: 64 actors = 6.0 ms per tick (35 % of a 60 Hz frame), 256 = 24.8 ms.
The real `f2-game` tick cost is measured once phase 1A makes it `no_std`; the hot spots will
be the float-backed PC helpers (`rsin`/`rcos`, `arccos`, slerp), not the integer maths. A
bit-exact precomputed table for `rsin`/`rcos` over the angle range the game uses (results fit
`i16`; e.g. ±8192 → 32 KiB) would cut 461 → ~20 cycles while keeping parity.

## (d) DFS read + decompression (`asset_load`, 1 MiB synthetic level-like file)
`tools/n64/gen-probe-data.py` (vertex tables, indices, keyframe deltas, names, CI8 texels,
10 % noise), `mkasset -w 256`:

| level | codec | stored | ratio | load time | output throughput |
|---|---|---|---|---|---|
| 0 | none (PI DMA) | 1048576 | 100 % | 202 ms | **5.06 MB/s** |
| 1 | LZ4 | 470663 | 44.9 % | 166 ms | **6.17 MB/s** |
| 2 | aPLib | 397780 | 37.9 % | 259 ms | 3.95 MB/s |
| 3 | Shrinkler | 331599 | 31.6 % | 1355 ms | 0.76 MB/s |

LZ4 beats raw reads (less ROM traffic); aPLib is the size/speed middle; Shrinkler is only for
data loaded once or rarely (a 3.5 MB level working set would take ~4.6 s). F2Z's target of
≥ 4 MB/s (N64-PLAN §2) sits at aPLib speed.

## What this means for 60 vs 30 fps
- With the phase-1 design (faithful f32 CPU transform → `TlVertex` → `rdpq_triangle`),
  measured ~6100 CPU cycles per triangle: Garden 1 (~610 tris) ≈ 2.4 frames of CPU time →
  **~25 fps before any game logic**; the heaviest levels (~1500) ≈ 10 fps. Not shippable.
- **30 fps is reachable on the CPU** with a fused, N64-specific path: transform per vertex
  straight to fixed-point screen coordinates, compact vertex data (s16 positions, packed
  RGBA8 colours, s16 UVs), no intermediate `TlVertex`/`Vec<Tri>`, and writing the rdpq
  `TRIANGLE_DATA` commands directly (skipping float → fixed reconversion). Target
  ≤ 1200 cycles/triangle → ~1300 tris in a 33 ms frame with room for two simulation ticks.
- **60 fps needs the RSP to do transform + setup** (libdragon's GL 1.1 RSP pipeline, or a
  tiny3d-style ucode, or a custom one reproducing MDX's projection/screen-reject maths):
  measured RSP setup alone allows ~2650 tris/frame @60, leaving the CPU for the 60 Hz
  simulation. The faithful projection quirks (`oozd` integer truncation, `BETWEEN`
  screen reject, `DIST`) must be reproduced in the ucode for visual parity.
- Recommendation: plan **30 fps as the baseline** (phase 2 fused CPU path), and make
  **RSP T&L the phase-3 route to 60 fps**; keep the simulation at its fixed 60 Hz tick
  either way (two ticks per 30 fps frame).

## ares accuracy caveats
- **RDP fill rate is not modelled**: the RSP+RDP time per triangle is flat from 27 px to
  11 000 px (385 → 422 RCP cycles). Real hardware fills at best ~1 px/clock in 1-cycle mode
  (62.5 Mpx/s) and noticeably less with z-buffer + 16-bit RMW traffic; the 50 × 11 000 px
  case (550 k px) would take several ms on hardware vs 0.34 ms here. RDP-bound numbers above
  are therefore **upper bounds**; fill rate (overdraw of backdrops, water, big floor polys)
  must be checked on real hardware or with an RDP timing model.
- CPU timing: ares emulates the VR4300 with caches and an approximate cycle model; absolute
  cycle counts can differ from hardware (likely by tens of percent, not factors). The
  relative costs (memory-bound emission, rsin/slerp hot spots) are robust.
- PI DMA speed depends on the cartridge's PI domain timing (real carts/flashcarts vary);
  5 MB/s is the usual order of magnitude.
- Audio: the mixer pulled the beep's samples (8820 for a 200 ms beep) — proven
  functionally; audio CPU/RSP cost under load is not yet measured.
- ares disables ISViewer (`debugf`) when the ROM exceeds 0x3FF0000 bytes (63.94 MiB): a
  full 64 MiB cartridge build needs another log channel (or a slightly smaller dev ROM).
- Not yet validated on hardware (no flashcart in this setup).

## 2G: the playable build (2026-10-10)

ROM: `tools/n64/build.sh` with the USA data (36 MiB; 1.45 MB code), ares v148, Expansion Pak.
Evidence (not in the repo, derived from game data): `C:\f2-n64test\evidence-2g\`
(`01-title.png`, `02-frontend.png`, `03-frontend-storybook.png` — the frontend's story book
reached with the pad, prompt "A - Select" from the N64 button names —, `04-garden1-start.png`,
`05-garden1-after-left.png` — the frog moved by the harness' Left key —, and the debugf log
`run-frontend-to-garden1.txt` with the `game:` mode/pad lines, `perf`/`heap` lines). Sequence:
title → (logo FMVs skipped) → frontend → Right, Up, A → Garden 1 (story). Garden 1 booted
directly (`F2N64_BOOT="level 0 0"`): `garden1-direct_*.png` + `run-garden1-direct.txt` (Up,
Up move the frog from tile 561 to 569 to 582; music track 2 playing, ambient + one-shot sfx
voices counted in the `perf` lines).

### Frame time (5 s windows of the `perf` log)

| scene | fps | sim ticks/s (of 60) | `Game::tick` | draw per frame (of which `Renderer::build` / RDP submit) | tris + rects per frame |
|---|---:|---:|---:|---|---|
| title (one 640x480 backdrop blit) | 21 | 27 (STARTUP mode is light) | 0.06 ms | 32 ms (0 / 19) | 0 + 1 |
| frontend 8-0, start view | 3.5 | 21 | 11.3–12.0 ms | 200 ms (25 / 75) | 1480 + 35 |
| frontend, story-book view | 6.3 | 38 | 9.7 ms | 88 ms (6 / 20) | 300 + 40 |
| training 8-3 | 5.1 | 30 | 9.3 ms | 130 ms (13 / 43) | 730 + 76 |
| **Garden 1 (0-0)** | **4.2** | **25** | **13.0–13.8 ms** | 145 ms (10 / 35) | 600 + 17 |

Per tick also: `audio_poll` (mixer) 0.35–0.45 ms. "Draw" = `f2_draw::render::draw_loop` (the
faithful CPU transform, `scene.rs`, ≈ draw − build − submit) + `Renderer::build` + the
submission (`n64_draw_tris`/`n64_draw_rects`, incl. the wait for a free framebuffer).
At most 6 ticks run per frame; the rest is dropped (logged as `dropped`), so the game runs at
35–45 % of real time. Level loads: frontend 6.8 s, Garden 1 4.9 s, training 6.7 s (each
object bank is decoded twice, once by the simulation's model table and once by the renderer;
no time-critical work yet).

**The simulation alone does not fit a 60 Hz tick on the VR4300 in ares**: 10–14 ms per tick
(≈ 1.0–1.3 M CPU cycles; 116 actors in Garden 1 → ≈ 10 k cycles per actor per tick), i.e.
60–85 % of the CPU before anything is drawn. Real time needs the simulation ≲ 5 ms/tick
*and* the RSP renderer (2R); keeping real time at a low frame rate needs at least the former
(60 ticks × 5 ms = 300 ms/s, leaving 700 ms/s for frames).

### Where the time goes (sampling profiler, `F2N64_FEATURES=profile[-sim]`, `tools/n64/prof.py`)

Simulation only (`profile-sim`, Garden 1, self time incl. inlined callees, 3555 samples):
`actor_list_draw` 14.3 % (per-tick `ActorListDraw` visibility: quaternion → matrix, `XfmPoint`
per actor, plus the `actor_draw_list` Vec), `update_path_platform` 7.8 % (whole `Platform`
clones, `PathNode` copies via `unwrap_or_default`), `copy_actor_list` 6.4 %,
`ViewXform::check_bounding_box` 5.3 %, `list::Head::ids` (a fresh `Vec` per walk) 4.4 %,
`xform_actor` 4.0 %, `update_path_nme` 3.4 %, `view::object_local` 3.4 %, `update_platforms`
3.3 %, `iquat_slerp` 3.0 %, `distance_between_points_ss` 2.8 %, `ptexture::water` 2.6 %,
`update_enemies` 2.4 %; `f2_math` 9.9 % in total, malloc/free ≈ 3 %. No single hot spot:
the cost is the volume of per-actor passes over large structs (`Option<Actor2>` slots,
16-byte D-cache lines, 8 KiB D-cache), many small allocations, and float helpers. Candidates
(behaviour-preserving, need the parity corpus): iterate lists without collecting ids,
borrow instead of cloning `Platform`/`PathNode`, cache per-actor `quaternion_to_matrix`,
a bit-exact `rsin`/`rcos`/`arccos` table (§c), keep hot actor fields in a compact array.

Whole frame (frontend start view): `scene::render_mesh` 22 %, `Renderer::build` 8 %,
`rdpq_triangle` CPU side 7 %, N64 submission 6 %, `prepare_object` 5.5 %; inside
`render_mesh` ≈ 5 % is the texture registry's `BTreeMap` lookup per face. 2R's RSP renderer
removes all of these. The VADPCM *Huffman* stage decodes on the CPU (≈ 10 % of a frontend
frame); the build uses `vadpcm,huffman=false` (RSP-only decode, +10 % sfx size).

### Memory (`heap` log lines)

libdragon heap: 6258 KiB total after code/data (1.7 MB); used at boot 1473 KiB (3 RGBA16
framebuffers + z-buffer, audio, fonts as N64 textures 405 KiB, `Game`), **peak used 4218 KiB**
(frontend running; Garden 1 3984 KiB, training 3620 KiB) → ≥ 2.0 MiB free in every scene
tried. Rust heap peak during a level load: 3245 KiB (frontend), 3341 KiB (Garden 1). N64
textures: 1.0–1.4 MiB per level (bank textures keep only a TMEM-sized RGBA16/RGBA32 copy;
fonts and backdrops keep full size for blits). What made it fit: per-image conversion while
loading texture banks (`TextureRegistry::load_bank_with`, RGBA8 dropped immediately); fonts
not kept twice (`Renderer::add_font` moves the pixels); the 640x480 backdrop converted in
16-row strips; the previous level's renderer side freed *before* the simulation's
`InitLevel` loads the next one (the data source's `before_access` hook) — the PC order
(load, then drop the old scene) runs out of RDRAM on the frontend → Garden 1 transition.

## 3S: simulation speed-up (zero behaviour change)

### Method
- **Benchmark** (`f2_game::simbench`): fixed scenarios run back to back without rendering —
  frontend (idle), arcade 0-0, 2-0, 2-2, 4-1, 6-0 (idle, 600 ticks; the heaviest levels by host
  cost of all 40 arcade levels: `sim_bench --all-levels`) and the attract demos 0-0, 1-0, 6-1
  (1500 ticks, gameplay). Host: `cargo run --release -p f2-game --example sim_bench`. N64:
  `F2N64_FEATURES=simbench tools/n64/build.sh` (add `profile` for one `PROF` window per
  scenario, window index = scenario index). Each scenario logs `Game::checksum` (netsync's
  full-state hash, floats by bit pattern) after its last tick: **the N64 checksums equal the
  host's**, so the MIPS build computes bit-identical state.
- **Parity** after every change: `f2-parity check --corpus all` (102 cases) + the three demo
  traces (`demo_trace`), all Rust traces sha1-hashed and compared with the pre-3S hashes
  (`b0818758…`, `148391ab…`, `592b9d2a…` for the demos), plus the simbench checksums.
- `Game::checksum` no longer builds the `Debug` text of the event state as a `String`
  (megabytes on 2-2 — the N64 ran out of heap); it streams it into the hasher (same value).

### Result (ares v148, simbench `Game::tick` mean / worst per tick, µs)

Same scenarios, same ticks, same end-of-run checksums (host = N64) before and after.
"After" = commit `28a02a7` (simbench ROM; a different build of the same code moves these
by ±5–10 %, see the cost-model note below).

| scenario | actors | before mean | before worst | after mean | after worst | speed-up |
|---|---:|---:|---:|---:|---:|---:|
| frontend | 67 | 10749 | 17241 | 8395 | 16246 | 1.28× |
| 0-0 Garden 1 | 116 | 12786 | 15180 | 9367 | 11162 | 1.37× |
| 2-0 | 131 | 27566 | 32358 | 15622 | 18069 | 1.76× |
| 2-2 | 197 | 26545 | 28382 | 15776 | 17484 | 1.68× |
| 4-1 | 148 | 20986 | 23721 | 12419 | 14033 | 1.69× |
| 6-0 | 121 | 17006 | 18953 | 11524 | 13277 | 1.48× |
| demo 0-0 | 116 | 13233 | 15641 | 9806 | 11852 | 1.35× |
| demo 1-0 | 117 | 11210 | 13220 | 8028 | 9642 | 1.40× |
| demo 6-1 | 141 | 19227 | 23129 | 11785 | 15027 | 1.63× |
| **sum** | | **159308** | | **102722** | | **1.55×** |

In the full game (`F2N64_BOOT="level 0 0"`, renderer + audio running, frog idle) the
`perf` log's `Game::tick` for Garden 1 went from 13.0–13.8 ms (§2G) to **8.4–8.5 ms**.

Per-phase split after (µs/tick, 0-0 / 2-0 / demo 6-1): actor pass (CopyActorList +
ActorListDraw) 4760 / 6990 / 5500, enemies 1600 / 2930 / 2500, platforms 890 / 1630 / 560,
procedural textures + overlays 590 / 1090 / 670, rest of GameLoop 600 / 690 / 590, special
effects 220 / 1620 / 530. Actor pass for Garden 1 (probe): CopyActorList ~0.9 ms (116
actors), sphere tests ~1.5 ms, XformActor of the ~20 visible actors ~1.7 ms (root object
~0.9 ms, frog head matrix ~0.25 ms).

The 2–3× target was not reached: what remains is the faithful float work (every FPU op
costs ~5 cycles in ares: the sphere test is ~950 cycles hot per actor, a root bounding box
~1700, `object_local` with a slerp ~2000–4500) and D-cache traffic over 100–200 actors of
~128 hot bytes each, all of it observable game state.

Baseline profile (Garden 1, self time): `actor_list_draw` 15 % (`xfm_point_f` 6.8 %),
`update_path_platform` 6.9 %, `copy_actor_list` 6.3 %, `list::Head::ids` 5.2 %,
`xform_actor` 4.3 %, `ftol` 4.9 % (inlined everywhere), `distance_between_points_ss` 3.4 %;
2-0: `iquat_slerp` 7.1 %, `make_unit` 6.8 %, `run_fx_update` (callers) 11 %, `orientate` 3.3 %.


### What changed (all exact: each commit parity-verified)
Every commit: 102 `f2-parity` cases + 3 demo traces byte-identical to the pre-3S hashes,
simbench checksums equal on host and N64; workspace tests, clippy `-D warnings`, fmt clean.
- f2-math: exact `rsin`/`rcos`/`arccos` lookup tables (float path kept for other inputs;
  lookups tested against it); `utilSqrt` without `f64` (equal over all 2^32 inputs); `FMul`
  as one 64-bit product (proof in the doc comment + 20M-pair test; 31 → 14 cycles);
  `leading_zeros` top-bit search in `MakeUnit`/`NormaliseQuaternion`.
- Actor pass: `CopyActorList` fused with `ActorListDraw(0)` in one `actList` walk (fields
  written only when they change); sphere test checks depth before projecting; root object
  matrix only when its bounding-box test counts; object sprites skip sprite-less subtrees;
  head matrix along the ancestor path only; keyframe search hinted for strictly increasing
  key times (unique answer; falls back to the original search); `sinFast`/`acosFast`
  tables as generated statics; `mtx_cat` without `memset`; hashed `ModelTable::get`.
- Data layout: `repr(C)` hot-first layouts of `Actor2`/`Actor`/`MdxActor`, `Platform`,
  `Enemy`, `GameTile` with `ser_order!` keeping the serialized field order (netsync checksum
  unchanged); D1 misses (i686 callgrind, 8 KiB direct-mapped) −15 % on 0-0.
- Algorithms: `EnemyAtFlag`/`PlatformAtFlag` from a per-`UpdateEvents` `(uid, fromNode)`
  snapshot (2-2 events 2.9 → 1.5 ms); `FindNearestTileS` skips square roots that cannot win
  (rotating enemies, 6-0 −1.4 ms); one sprite-overlay pass instead of three; no whole
  actor/platform/effect clones in hot updates (`SpecFx::head`); procedural textures reuse
  their surface buffer.
- Tooling: `f2_game::simbench` scenarios + `examples/sim_bench.rs` (host) +
  `F2N64_FEATURES=simbench` (N64: per-scenario mean/worst, checksums, per-phase times via
  `Game::phase_hook`, cost probes); `F2N64_TUNE` A/B masks run variants in one ROM.

### Cost model learned (ares v148)
Integer ALU op 1 cycle; every FPU op ~5 (`mul.s`/`add.s`, dependent or not); `div.s` ~30;
cold 16-byte D-cache line ~46; 16-bit atomics are emulated (slow). Code/heap layout alone
moves a build's numbers by ±5–12 % (16 KiB direct-mapped I-cache, 8 KiB D-cache), so only
same-ROM A/B (`F2N64_TUNE`) or deterministic callgrind cache simulation can judge small
changes.

### Tried and reverted
Memo tables of the per-actor sphere test / root matrix (memo memory traffic > float work
saved); bit-twiddled exact shortcuts of `mtx_cat` with identity/translate/scale matrices (no
faster than the FPU ops); hot-first `.text` ordering (per-tick hot code ~250 KiB);
`opt-level` "s"/2; `#[inline(never)]` on matrix helpers; precomputed first-key rotation;
passing the model through `XformActor` (A/B: +0.2 %); splitting the actor pass into
visibility and transform loops (A/B: 0 %).

### Next candidates
Exact speed-ups left are small each: lazily shared bounding-box corner products (corners
are axis-aligned combinations; ~10 % of `CheckBoundingBox`); a cheaper exact `rsin` for
large angles (frontend swarms call the float path ~30×/tick); `Enemy` clones in enemy
updates; text-overlay slot layout. Bigger wins need the work itself to go: e.g. running
the simulation at 30 Hz with the renderer (behaviour change, owner decision) or moving the
float transforms to the RSP like the renderer (2R).
