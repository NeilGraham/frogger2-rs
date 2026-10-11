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
title → (logo FMVs skipped) → frontend → Right, Up, A → Garden 1 (story).

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
