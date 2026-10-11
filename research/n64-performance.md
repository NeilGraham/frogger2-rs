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
(Measured in (e) on real pack data: F2Z decodes at 0.7–1.8 MB/s, well below that target.)

## (e) F2Z decode on the VR4300 vs mkasset on the same bytes (real pack data)
`crates/frogger-n64/src/f2z_probe.rs`, ares v148, release + LTO. Input staged by
`tools/n64/stage-f2z-probe.py <pack dir>` (game-derived, only under `target/`): a Garden 1
level set from the F2Z pack (`tools/f2-n64pack`, 128 KiB model/texture dictionaries), each file
decoded on the host (`f2z d <in> <out> --dict <class.f2zd>`) and the decoded bytes
re-compressed with `mkasset -c 0..3 -w 256`, so every row compares identical output bytes.
Per file the ROM: reads the F2Z stream (`asset_load` of an uncompressed DFS file = PI DMA),
runs `Decoder::decode` 4 times (best reported; the first run is within 1 % except for the
796-byte file), times the inverse
structure transform alone, `asset_load`s each mkasset variant (read + decompress), and checks
FNV-1a 32 of every output against the host value. Dictionaries are decoded once without a
dictionary, parsed, their default tables built once (`load_dict`) and kept resident.
**Correctness: all 11 F2Z outputs and all 44 mkasset outputs match the host checksums.**
MB/s = 10⁶ bytes of *decoded output* per second; cyc/B = CPU cycles per output byte.

### Per class: F2Z (optimised decoder) vs libdragon codecs
| class (files, decoded bytes) | F2Z stored | F2Z ROM read | F2Z decode | F2Z read+decode | c0 raw read | c1 LZ4 | c2 aPLib | c3 Shrinkler |
|---|---|---|---|---|---|---|---|---|
| models (3, 338044) | 72678 (21.5 %) | 15.3 ms | 217.4 ms, **1.55 MB/s** | 1.45 MB/s | 5.07 MB/s | 112688 (33.3 %), 8.53 MB/s | 83385 (24.7 %), 5.60 MB/s | 69044 (20.4 %), 1.15 MB/s |
| textures (2, 243672) | 132451 (54.4 %) | 26.4 ms | 272.9 ms, **0.89 MB/s** | 0.81 MB/s | 5.08 MB/s | 168110 (69.0 %), 6.91 MB/s | 145321 (59.6 %), 3.45 MB/s | 130917 (53.7 %), 0.50 MB/s |
| collision (1, 49596) | 4223 (8.5 %) | 1.3 ms | 26.9 ms, **1.84 MB/s** | 1.75 MB/s | 4.95 MB/s | 11815 (23.8 %), 8.44 MB/s | 8453 (17.0 %), 6.44 MB/s | 6057 (12.2 %), 1.66 MB/s |
| maps (1, 20539) | 2996 (14.6 %) | 1.0 ms | 11.6 ms, **1.77 MB/s** | 1.64 MB/s | 4.65 MB/s | 5130 (25.0 %), 7.89 MB/s | 3433 (16.7 %), 6.32 MB/s | 2857 (13.9 %), 1.65 MB/s |
| scenics (1, 796) | 287 (36.1 %) | 0.46 ms | 0.73 ms, 1.09 MB/s | 0.67 MB/s | 1.38 MB/s | 391, 1.65 MB/s | 299, 1.39 MB/s | 273, 0.52 MB/s |
| **all assets (8, 652647)** | **212635 (32.6 %)** | 44.5 ms (4.78 MB/s in) | **529.5 ms, 1.23 MB/s** | **1.13 MB/s** | 5.04 MB/s | 298134 (45.7 %), **7.78 MB/s** | 240891 (36.9 %), **4.58 MB/s** | 209148 (32.0 %), **0.79 MB/s** |
| dictionaries (3, 332434; once at boot) | 143469 (43.2 %) | 29.1 ms | 310.4 ms, 1.07 MB/s | 0.97 MB/s | 5.08 MB/s | 182758, 6.89 MB/s | 149257, 3.88 MB/s | 137719, 0.66 MB/s |

mkasset numbers are `asset_load` totals (PI read + decompress); F2Z's ROM read and decode are
timed separately (sum in "read+decode"). The F2Z ratios exclude the resident dictionaries
(amortised over all ~390 pack files; mkasset has no dictionary). Boot cost of the dictionaries:
decode 310 ms + `Dict::parse` ~3 ms each + `load_dict` 2.4 ms each (was 9.0 ms), resident
332 KB content + one 31 KB `Decoder`.

**Verdict:** F2Z matches Shrinkler's ratio (32.6 % vs 32.0 %) at 1.5× its speed, but decodes
3.7× slower than aPLib and 6.3× slower than LZ4 on the same bytes; a 3.5 MB level working set
would take ~2.8 s (LZ4 ~0.45 s, aPLib ~0.75 s). Below the ~2 MB/s bar on every class; models
and level data come closest (1.5–1.8 MB/s), literal-heavy textures are worst (0.74–0.89 MB/s)
and gain little ratio over aPLib there (54 % vs 60 %).

### Per file
| file | F2Z → decoded | nseg | ROM read | decode before | decode after | after MB/s (cyc/B) | of which inverse transform (before → after) |
|---|---|---|---|---|---|---|---|
| models/garden1.mdl | 46276 → 223064 | 16 | 9.4 ms | 224.7 ms | 139.3 ms | 1.60 (58) | 27.7 → 8.1 ms |
| models/gardenmaster.mdl | 12131 → 66032 | 16 | 2.8 ms | 62.3 ms | 43.1 ms | 1.53 (61) | 8.3 → 2.6 ms |
| models/gamegeneric.mdl | 14271 → 48948 | 16 | 3.2 ms | 59.1 ms | 35.0 ms | 1.39 (67) | 6.3 → 2.0 ms |
| textures/garden.ntx | 95459 → 146296 | 8 | 18.9 ms | 283.1 ms | 195.8 ms | 0.74 (125) | 20.4 → 10.0 ms |
| textures/backdrop/artwork00.ntx | 36992 → 97376 | 0 | 7.6 ms | 110.5 ms | 77.1 ms | 1.26 (74) | — |
| collision/garden1.col | 4223 → 49596 | 6 | 1.3 ms | 68.3 ms | 26.9 ms | 1.84 (50) | 44.0 → 11.1 ms |
| scenics/garden1.sce | 287 → 796 | 0 | 0.46 ms | 0.89 ms | 0.73 ms | 1.09 (85) | — |
| maps/entity-0-0.dat | 2996 → 20539 | 0 | 0.96 ms | 14.7 ms | 11.6 ms | 1.77 (52) | — |
| dict/models.f2zd | 54460 → 131885 | 0 | 11.0 ms | 218.6 ms | 116.5 ms | 1.13 (82) | — |
| dict/textures.f2zd | 70594 → 132393 | 0 | 14.0 ms | 215.4 ms | 149.8 ms | 0.88 (106) | — |
| dict/levels.f2zd | 18415 → 68156 | 0 | 4.1 ms | 82.9 ms | 44.1 ms | 1.54 (60) | — |
| **assets total** | 212635 → 652647 | | 44.5 ms | **823.4 ms (0.79 MB/s)** | **529.5 ms (1.23 MB/s)** | | 106.7 → 33.8 ms |

"Before" = the decoder as committed in `8ad6275`, in the same probe build. Caveat: ares'
VR4300 timing depends on code placement (16 KiB direct-mapped I-cache): unrelated code changes
moved the *same* decoder by up to ±10 % between builds (baseline assets total measured at 730,
797 and 823 ms in three builds); each ROM's numbers are deterministic, so before/after compare
builds that differ only in the decoder.

### Where the time goes
ares cost calibration (same ROM): simple ALU ops and cached loads ≈ 1 cycle per instruction,
a **D-cache miss ≈ 45 cycles** (8 KiB direct-mapped D-cache, 16-byte lines), a multiply ≈ +4.
So the decoder is bound by table lookups that miss, not by arithmetic.

- rANS symbol decode micro-benchmark (cycles/symbol, next table chosen by the previous symbol):
  with the original 4 KiB slot→symbol table + 1 KiB freq/cum per table: 62 / 72 / 95 / 107
  for 1 / 2 / 4 / 8 live tables; with the new 1.3 KiB bucket table: 75 / 74 / 80 / 90. Real
  blocks keep up to 9 literal + 3 code tables live (≈ 60 KiB before, ≈ 16 KiB now).
- Least-squares fit of the LZ part (decode − inverse transform) over the 10 large files
  against host-counted literals, sequences and match bytes (literals 1.5 k–73 k, sequences
  1.2 k–18 k per file): **before** ≈ 179 cycles per literal, 683 per sequence, 14 per match
  byte (39 % / 52 % / 10 % of LZ time); **after** ≈ 125–155 per literal, 300–430 per sequence,
  15–21 per match byte (≈ 40–48 % / 32–47 % / 14–20 %; the fit is collinear, max error 15 %).
  A literal costs its context selection (previous-byte class + position bits), a bucket +
  freq/cum lookup and a multiply, plus about one miss; a sequence costs three rANS symbols
  (LL, ML, OF), up to three extra-bit reads, repeat-offset update, the match copy setup, and
  register spills (the block loop keeps five streams live on a 32-register MIPS).
- Inverse transforms were 13 % of asset decode time before (planes gathered with one pass over
  the whole output per plane = a miss per byte; raw segments copied through `tmp`), 6 % after.
- Literal-heavy files are slowest: garden.ntx (73 k literals, 15.6 k sequences for 146 KB)
  0.74 MB/s; match-heavy collision/maps (≈ 1.3 k literals) 1.8 MB/s.

### Decoder optimisations made (bit-exact, stream format unchanged)
Verified by `cargo test -p f2-n64z --features encoder`, `cargo test -p f2-n64pack`, identical
SHA-1 of the decoded output for all 394 F2Z files of the pack before/after (host), and the
ROM's FNV-1a checks.
1. `rans::Table`: the 4 KiB slot→symbol array is replaced by a 256-entry bucket array (symbol
   owning each 16-slot bucket's first slot) + forward scan over `cum` with a sentinel — 1.3 KiB
   per table; `Decoder` 123 KB → 31 KB; `load_dict` 9.0 → 2.4 ms. Biggest single win.
2. `RansDec::get`, `BitReader::read`, `codes::decode` forced inline (they were out-of-line calls
   in the hot loops); the wide (> 24-bit) bit read stays out of line.
3. Literal loop rewritten without a capturing closure (context function and table array
   hoisted per block); single-symbol LL/ML/OF streams decode through a degenerate table with a
   constant state (`Table::build_single`, `RansDec::constant`), so the sequence loop has one
   branch-free path; an unused stream with sequences is rejected up front.
4. Match copy: 8-byte chunks when 8 bytes of slack follow the match (overlapping distances
   d < 8 are first extended to the smallest multiple of d ≥ 8); `d = 1` runs use `fill`.
5. Inverse transforms: planes gathered in tiles of 16 records (strides 2 and 4 specialised),
   delta + zigzag specialised per word width/endianness; when raw segments dominate, LZ decodes
   straight into `out` and only the transformed segments go through `tmp`
   (`transform::inverse_in_place`).
6. `Table::build` fills symbol ranges with slice fills.
Tried and reverted: decoding each 128-sequence chunk in separate passes (LL / ML / OF symbols,
then values, then execution) to cut register spills — 9 % slower in ares.

### What would reach ≥ 2 MB/s (format changes; not done, the format was frozen here)
- Fewer / smaller rANS tables on the N64: literal modes with ≤ 2 contexts, or 10–11-bit
  probabilities (the sym/bucket footprint halves per bit); per-literal cost is mostly the miss.
- Cheaper sequences: byte-aligned LZ4-style tokens for LL/ML (keep rANS only for literals and
  offsets) or longer minimum matches; per-sequence cost is ~3 symbol decodes + spills.
- Per class today: F2Z for models / collision / maps (best ratio, 1.5–1.8 MB/s, small files),
  LZ4 or aPLib for textures and backdrops (F2Z 0.7–1.3 MB/s for ≤ 6 points of ratio).

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

### 2G follow-up: visual fixes (2026-10-11) and status

Evidence: `C:\f2-n64test\evidence-2g\fixes\` (`*-before-n64.png`, `*-after-n64.png`, PC references
`*-pc-reference-320x240.png` from `frogger --headless --profile authentic --size 320x240`).
- **Water red/green splotches — fixed.** The garden/frontend water is a white caustics texture
  (`00wate04.bmp`) drawn *Additive* over the dark river bed. The RDP's additive blender
  (`IN*IN_ALPHA + MEM*1`) wraps past 1.0 instead of saturating. Additive batches now use the alpha
  blender with a per-texture variant (`csrc/main.c` `make_add_variant`: colour normalised to its
  brightest channel, alpha = alpha × brightness, RGBA32 ≤ 1024 texels, made on first additive use).
  `IN*a'` equals the D3D additive term exactly; the background is only attenuated by `1 - a'` where
  the effect is bright. The texel conversion was ruled out with `examples/host_shot.rs --n64`
  (RGBA16/RGBA32 round trip through the CPU rasteriser looks like the PC).
- **Story book showed no picture/text — fixed.** The page is a render-to-texture
  (`grabToTexture`, PC `DrawPageB`) the desktop composes on the CPU; the N64 front end never did it.
  `book.rs` moved to `f2-draw` (shared); the N64 keeps the RGBA8 pixels of `page32.bmp` and
  `chapterN.bmp` (everything else drops them after conversion), keeps a glyph subset of
  `monofont.bmp` built at boot (only the chapter strings' characters), and draws large textures on
  triangles in 32x32 TMEM tiles (`draw_tri_tiled`, textures flagged `N64_TEX_TILE3D`: images added
  after a bank load that do not fit TMEM) so the 256x256 page is shown at full resolution.

**Status / next steps** (resume from this branch): playable title → frontend → Garden 1 with pad,
sfx, music; slideshow (3.5–4 fps, sim 35–45 % of real time) until 2R's RSP renderer and a
simulation speed-up (§2G profile, `Game::tick` 10–14 ms) land. Left: arcade-machine screen copy
(`sync_arcade_screen` / `book::copy_texture`, needs source pixels or a handle alias in
`render_cpu.rs`), subtractive blend approximated as multiply, 1-bit alpha on large textures
(CI4/CI8 via the 1C pack), FMV, saves, load times (object banks decoded twice).
`examples/host_shot.rs` renders the N64 drawing path on the host for comparisons.
## Phase 2R: RSP renderer (r3d), 2026-10-11
Code: `crates/frogger-n64/csrc/rsp_f2mdx.S` (magma vertex shader), `rsp_f2clip.S` (clipping
overlay), `r3d.c` (C side), `src/r3d.rs` (Rust API), `src/r3dtest.rs` (test ROM app: cull check,
benches, scenes); format `crates/f2-n64fmt` + `docs/n64-model-format.md`; exporter
`tools/n64/export`; comparison `tools/n64/compare.sh`.

### Design
- **libdragon magma** (preview 3D API: vertex-shader overlays, 48-entry vertex cache, RSP
  triangle setup `RDPQ_Triangle_Send_Async`, guard-band clipping) with our own shader:
  `pos x M` (M = model x view x P folded on the CPU, s15.16), `q = floor(W)` (the
  integer-truncated `oneOver[]` depth), `sx = cx - FOV*s*X/q` in s13.2, linear depth
  `z = K*vz` (like `sz = vz*0.00025`, full 15-bit range up to far), colour = rgba*mul+add
  (alpha_mul, Gouraud override), UV scale per texture, fog into shade alpha.
- **MDX face rejection on the RSP, exactly:** per vertex, the trivial-reject byte holds
  "x strictly inside" / "y strictly inside" bits -> RDPQ rejects a triangle when no vertex is
  inside on x or on y = `BETWEEN` screen reject (including faces straddling both sides);
  near/far map to magma's z clip planes and our clip handler *rejects* (never clips) faces
  with such a vertex = MDX's `vz = 0` skip. Back faces: magma culling (`CULL_MODE = 3`,
  verified by framebuffer read-back: `R3D CULL ... PASS`). Guard band (6 x FOV*s px):
  only real clipping (out-of-range RDP coordinates), in a copy of magma's clipper that keeps
  linear depth and affine UVs.
- **Texture loads inside the magma overlay:** a 0-vertex load command makes the shader DMA a
  prebuilt RDP descriptor (TLUT + LOAD_BLOCK + tile setup + syncs, libultra order) into the
  RDP stream and update the UV-scale uniform. Going through rdpq instead costs two RSP
  overlay switches (~2500 RCP cycles) per texture change: measured 750 us for 19 changes;
  now 20 changes cost 57 us.
- **Bounding-sphere culling per batch** (CPU, conservative: only batches every face of which
  MDX would reject; host test `culled_batches_are_fully_rejected`). Garden 1 game camera:
  3717 landscape tris -> 1053 sent, RSP 13.5 -> 4.3 ms.
- CPU per draw: matrix fold + s15.16 uniform into a 3-frame arena (RSP loads it by DMA),
  sphere tests, then one rspq block call per visible batch.

### Results (ares v148; RDP fill rate not modelled, so RSP+RDP ~= RSP time)
Synthetic: textured, Gouraud, z-buffered grids, every draw switches texture, 320x240.
"RSP+RDP" = frame recorded (flat mode) and replayed until idle; "CPU" = frame recorded into
an rspq block (no execution); "live" = 60 vsynced frames, triple buffered.

| scene | draws | tris sent | CPU | RSP+RDP | RCP cyc/tri | live fps |
|---|---|---|---|---|---|---|
| 500 / 10 meshes | 10 | 500 | 0.67 ms | 2.1 ms | 261 | 60 |
| 1000 / 20 | 20 | 1000 | 1.27 ms | 4.1 ms | 253 | 60 |
| 1000 / 20, 1 texture | 20 | 1000 | 1.18 ms | 4.0 ms | 250 | 60 |
| 1500 / 30 | 30 | 1500 | 1.77 ms | 6.0 ms | 250 | 60 |
| 2000 / 40 | 40 | 2000 | 2.35 ms | 7.9 ms | 247 | 60 |
| 2000 / 10 | 10 | 1998 | 0.98 ms | 7.9 ms | 245 | 60 |
| 2940 / 30 | 30 | 2940 | 2.05 ms | 11.6 ms | 246 | 60 |
| Garden 1 game camera (landscape; PC draws 187) | 8 | 1053 of 3717 | 2.1 ms | 4.5 ms | 269 | 60 |
| Garden 1 wide + 5 actors (PC draws 2241) | 37 | 3814 of 4540 | 7.8 ms | 19.6 ms | 321 | 42 |
| same, far 1100, fog, water (PC draws 1188) | 32 | 2548 of 4489 | 5.5 ms | 12.8 ms | 313 | 60 |

- **RSP capacity: ~4100 sent triangles per frame at 60 fps (~8300 at 30)** for compact meshes
  (0.7 vertices/tri); real MDX meshes have ~2 vertices/tri (per-corner UVs/colours) ->
  ~3300/frame at 60. The game's 450-1500 drawn tris/frame (worst 1513) need at most ~2500
  sent with culling -> **60 fps on the RSP in typical scenes, 30 fps guaranteed** by a wide
  margin. Triangle setup (~210 cycles) dominates; vertices ~45 cycles each.
- vs. phase 1 faithful CPU path (6100 CPU cycles/tri, ~256 tris/frame @60): now the CPU
  costs ~5000-6000 cycles **per draw** (not per triangle): 60 draws ~ 3 ms.
- CPU per draw breakdown (`R3D CPU`): `R3d::draw` ~4200 cycles (draw_uniform ~950: 16
  f32->s15.16 conversions; the rest is the fold, sphere tests and D-cache misses), flush
  ~1300 cycles per visible batch (C + rspq writes). Next: fixed-point matrix fold, f64
  magic-number conversion, fewer rspq writes per batch.
- Real hardware caveat (as phase 1): RDP fill (z-buffered 16-bit, bilinear, ~1 px/clk at
  best) is not modelled by ares; heavy overdraw scenes must be checked on hardware.

### Visual comparison (PC reference vs N64)
`tools/n64/export` renders the PC reference with f2-engine's CPU renderer (authentic
options, same camera, same geometry) and writes the F2NM + scene files; the ROM shows
`rom:/scene*.f2nm` (`target/n64/filesystem/r3d_show.txt` = scene index to pin one). Evidence
(derived from game data: target/ only, never committed): `target/n64/compare/scene{1,2,3}_side.png`
(PC 2x | N64 ares capture), PSNR 25.5 / 21.9 / 21.6 dB (VI filter + 16-bit dither + the VI's
left border dominate). Geometry, rejection, culling, textures (CI8), alpha-test, skinned frogs,
multi-object actors, Xlu water and fog match.

### Deviations from the PC renderer
- Depth: 15-bit linear (RDP z-buffer ~14-bit float: ~0.3 units near the camera at far 1200)
  vs PC float depth; z compare is the RDP's (less vs LESSEQUAL ties).
- Bilinear is the RDP's 3-point filter (texel centres matched with a -0.5 texel offset).
- `Additive` (SRCALPHA, ONE) -> `src*a + dst*(1-a)`: the RDP blender wraps instead of
  clamping (`N64_TRUE_ADDITIVE` selects the real, wrapping one). `Subtractive`
  (ZERO, INVSRCCOLOR) -> `dst*(1-a)` (exact for grey sources with the grey in alpha).
- Opaque lists are not re-sorted, but a mesh's faces are grouped by texture (order matters
  only for semi-transparent texels > 100 alpha); `ORDERED` keeps face order for translucent
  meshes.
- Fog (enhancement) replaces vertex alpha (RDP fog uses shade alpha); per-object fog uses
  the object origin's depth instead of the first face's vertex.
- Textures resampled to fit TMEM (Garden: all 32x32 CI8); UVs of clamped faces beyond
  ~16 periods saturate.
- Clipped (guard-band) triangles: new vertices use the exact W, not floor(W).
- Affine texturing only (as retail); perspective-correct option not implemented.
- `queued_tris` semantics differ: r3d counts mesh tris and culled-batch survivors; the exact
  MDX survivor count is only known on the RSP.

### API for the game loop (`frogger_n64::r3d`)
```rust
let mut r = R3d::new();                          // once
let meshes = r.load_file(bytes)?;                // F2NM: textures by CRC + meshes
r.find_mesh(id) / r.set_texture(crc, tex)        // packer ids; animated textures
// per frame:
rt::begin_frame(clear);
r.begin_frame(&ViewParams { view: camera_view(src, tgt, up), far, fog, ..ViewParams::pc_default(320.0, 240.0) });
r.draw(List::Normal, mesh, &object_world_matrix, &DrawParams { object_fog: true, ..Default::default() });
r.draw_skinned(List::Normal, skin, &bone_matrices, &params);   // xform_actor matrices
r.flush(|list| { /* CPU batches after each list: sprites (Late), HUD (Overlay) via
                    n64_draw_tris; z: ViewParams::tl_z(sz) */ });
rt::present();
```
`List` = scene.rs frames in `DrawAllFrames` order; the game picks the list per object
(`object_frame`) exactly as `draw_actor`.

### Next steps
- Integrate with the phase-2 game loop (actors from `f2-game`, sprites/HUD through the CPU
  rdpq path, animated textures via `set_texture`).
- CPU per-draw cost (see above); optional cross-draw texture sort.
- Hardware check of RDP fill rate.
- Measuring RSP time needs flat mode: replaying magma blocks nested inside an
  rdpq-recorded block corrupted the RDP buffers in this libdragon preview (probe kind 6).
