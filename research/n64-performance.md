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

## 3I: the game on the RSP renderer from the 64 MB pack (2026-10-11)

Branch `worktree-agent-adf53493d6484db51` (from `feature/n64` + 2R merged). The default ROM is
the game drawn by r3d, with every asset from the 1C pack; `render-cpu` (phase-2G renderer),
`prep-data` (phase-2G loose files), `r3d-test` (2R test app) and `bringup` (1B probes) are
crate features.

### What changed
- **Rendering** (`src/render_r3d.rs`): `f2-engine`'s `Renderer` has an optional *mesh sink*;
  with it `draw_actor` / `draw_landscape` record `MeshDraw`s (model, pre-order object,
  `xform_actor` matrices, `object_frame` list, alpha/gouraud/overrideTex/noClip/fog) instead
  of transforming vertices, so `f2-draw`'s draw loop is unchanged. r3d draws them from F2NM
  meshes (`f2_n64fmt::bank`: one file per object bank, ids `model << 16 | object`);
  everything else the engine builds on the CPU (backdrops, shadows, sprites, effects,
  overlays, text) is drawn by `render_cpu.rs` between r3d's lists
  (`Renderer::build_segments`), in `DrawAllFrames` order; CPU triangles share r3d's linear
  depth (`ViewParams::tl_z`). The sky (`noClip`) is a depth-less first list: its depth
  beyond the far plane saturates into the coarse far end of the RDP z-buffer. Models showing
  the story book's render-to-texture pages (256x256) stay on the CPU path (TMEM tiles).
- **Textures**: NTX1 banks (1C) are loaded zero-copy (`src/ntxload.rs`, `src/ntx.rs`):
  each registry entry gets a pixel-less placeholder image and a C texture pointing into the
  file (CI4/CI8 + TLUT, IA, I, RGBA16/32, per-tile palettes for backdrops; `ai_` masks and
  opaque I images become IA, procedural textures show the PC's initial `ai_`-style upload);
  r3d aliases them (images larger than one TMEM load get a downscaled RGBA32 copy).
  Animated textures follow the registry's `current`; procedural textures are updated in
  place (aliases and load descriptors stay valid).
- **Additive**: r3d's additive lists use each texture's additive variant (colour / max,
  alpha * max: the alpha blender then adds the exact D3D `src * a` term without the RDP's
  wrap-around); vertex colours of additive meshes are split the same way at pack time
  (`additive_corner`, incl. the models the game makes additive by name: `ada*`, `wat_*`,
  `xxa_*`) and on the CPU path. Found on Space 1, where the glass (`ada_glss`) was opaque.
- **Data** (`src/data.rs`): `PackStore` dispatches on the magic (`F2Z1` → F2Z with the
  resident dictionaries, `DCA` → `asset_load`, raw); object banks as postcard with
  vertex-only geometry (all the simulation reads) + F2NM render meshes; `sfx/index.bin`
  (F2SX) for the simulation's sound banks and the voices (one open per deduplicated file);
  `textures/<bank>.prc` = the procedural-texture sources `pt_texture_load` needs.
- **Audio**: sound effects all VADPCM without the Huffman stage (RSP-only decode; Opus
  wav64 cannot be shared by voices). **Music is ULC stereo ABR 128 kbps** (Opus 99 kbps cost
  ~14 ms of CPU per 60 Hz tick in ares, a third of the frame; ULC ~2 ms at the same bitrate).
- **FMV**: `Effect::PlayVideo` → `fmv_play("rom:/video/<name>.h264")` over the game's
  display (no display re-init), skippable with A/B/Start; logos and story FMVs play.
- **Saves**: the 2112-byte PC block in 256 Kbit SRAM (`src/save.rs`: header + FNV-1a), read
  at boot, written on `Effect::SaveGame`; flashcarts (EverDrive, SummerCart64) and ares
  support SRAM (ares writes `<rom>.ram` on a clean exit; the harness now closes ares
  gracefully). The "training done" boot hack is gone (`F2N64_BOOT=frontend-trained` for tests).
- **RDRAM** (heap 5.6 MiB after the 2.5 MiB of code/BSS): BSS -290 KiB (one inline surface per
  texture slot), mixer limits (48 kHz music, 44.1 kHz sfx) and 300 ms latency, the book page
  composed in a frontend-only canvas straight into a half-size texture, game fonts kept
  across levels (reloading `bigfont.bmp` needed a 427 KiB RGBA8 block), NTX banks loaded
  first (largest blocks) — level loads failed on a fragmented heap before.

### ROM and budget
`F2_GAME_DIR=… tools/n64/build.sh` → `target/n64/frogger2.z64` = **64,323,584 bytes
(61.34 MiB)**, 2,785,280 bytes under the 64 MiB cartridge and 2,719,744 under ares' ISViewer
limit (0x3FF0000), so `debugf` logging works on the full ROM; `--small` (no music/FMV, 20.5 MB)
remains as the quick-iteration ROM. Pack (`f2-n64pack`, DFS 62,136,336 B):

| class | files | packed | MiB |
|---|---:|---:|---:|
| models (postcard vertex-only banks + F2NM meshes, aPLib) | 142 | 2,976,279 | 2.84 |
| textures + fonts + backdrops + procedural sources (NTX1, aPLib) | 87 | 2,798,430 | 2.67 |
| levels (F2Z + 64 KiB dictionary) | 245 | 386,176 | 0.37 |
| sound effects (VADPCM, no Huffman) | 418 | 12,307,394 | 11.74 |
| music (ULC stereo ABR 128 kbps) | 11 | 24,299,658 | 23.17 |
| FMV (H.264 320x240 + Opus 48/96 kbps) | 28 | 19,310,600 | 18.42 |
| code (compressed ELF: text 2.03 MB, data 0.12 MB, BSS 0.40 MB) | | ~2,187,000 | 2.09 |

### Per scene (ares v148, 5 s `perf` windows, median after the first)
Evidence (derived from game data; not in the repo): `C:\f2-n64test-3i\evidence\`:
`f2-boot*.png` + `f2-boot-run.txt` (logo FMVs, intro FMV, frontend), `f2-fe*.png` +
`f2-fe-run.txt` (FMVs skipped with A, frontend, story book, A → training level 8-3),
`f2-lv{W}-0*.png` + `lv{W}-0-run.txt` (each world's first level, booted with
`F2N64_BOOT="level W 0"`), `side-lv{W}-0.png` (PC `frogger --headless --profile authentic
--size 320x240 --frames 600` at 2x | N64 capture; `pc/pc-lv{W}-0.png`), `f2-savetest-boot{1,2}-run.txt`
(SRAM save written, then loaded on the next boot).

| scene | fps | sim ticks/s | `Game::tick` | audio poll / tick | sim / frame | draw / frame (of which draw loop) | RSP busy | RDP busy | r3d draws | tris queued / sent | batches | tex loads | load | heap used |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| frontend (8-0) | 7.8 | 46 | 9.8 ms | 1.8 ms | 71 ms | 51 ms (20) | 20 % | 14 % | 151 | 4859 / 2023 | 124 | 91 | 0.8 s | 5012 KiB |
| Garden 1 (0-0) | 6.6 | 39 | 12.9 ms | 1.8 ms | 90 ms | 54 ms (27) | 19 % | 10 % | 205 | 9228 / 2122 | 136 | 98 | 0.7 s | 4171 KiB |
| Ancients 1 (1-0) | 7.0 | 42 | 10.4 ms | 1.9 ms | 77 ms | 60 ms (33) | 18 % | 10 % | 248 | 9846 / 1586 | 93 | 80 | 0.8 s | 4527 KiB |
| Space 1 (2-0) | 3.6 | 21 | 25.1 ms | 4.5 ms | 180 ms | 85 ms (43) | 19 % | 10 % | 243 | 9522 / 3089 | 159 | 85 | 0.7 s | 4231 KiB |
| City (3-0) | 27.3 | 60 | 5.0 ms | 0.8 ms | 14 ms | 19 ms (7) | 29 % | 24 % | 27 | 2600 / 862 | 43 | 28 | 0.7 s | 3822 KiB |
| Subterranean 1 (4-0) | 4.6 | 28 | 15.1 ms | 3.5 ms | 118 ms | 78 ms (45) | 24 % | 12 % | 328 | 10525 / 1901 | 100 | 68 | 0.7 s | 4422 KiB |
| Lab 1 (5-0) | 4.1 | 24 | 16.7 ms | 4.8 ms | 133 ms | 87 ms (43) | 23 % | 12 % | 271 | 9326 / 2148 | 151 | 127 | 0.7 s | 4443 KiB |
| Halloween 1 (6-0) | 4.9 | 29 | 16.7 ms | 3.0 ms | 122 ms | 74 ms (37) | 20 % | 10 % | 293 | 9227 / 2962 | 202 | 155 | 0.8 s | 4368 KiB |
| Retro 1 (7-0) | 9.7 | 58 | 10.1 ms | 1.1 ms | 69 ms | 30 ms (15) | 16 % | 7 % | 112 | 2433 / 843 | 42 | 23 | 0.5 s | 3444 KiB |

Heap total 5642 KiB. "draw" = `App::draw` on the CPU: the `f2-draw` loop (matrices via
`xform_actor`, sprites, effects, HUD), texture uploads, CPU-batch build and submission, r3d's
queue and flush, and the time the CPU waits for room in the RSP command queue. RSP/RDP busy =
share of wall time the RSP is not halted / the RDP pipeline busy (2 kHz sampling of SP/DP
status; includes the audio mixer's RSP work). Phase 2G for comparison (CPU renderer, Garden 1):
4.2 fps, draw 145 ms, level loads 4.9–6.8 s.

**The frame is simulation-bound**: at most 6 ticks run per frame and each costs 10–25 ms
(`Game::tick`) + 1–5 ms of audio polling; the rest of real time is dropped. With phase 3S's
work-in-progress simulation (`worktree-agent-a90c2347f335f8b21` at `b3f1209`, f2-game/f2-math
only, measured in this ROM and not kept): Garden 1 `Game::tick` 13.1 → 9.7 ms, 6.5 → 7.7 fps.
The draw side costs 20–90 ms per frame today: per frame ~15–45 ms of `xform_actor` and the PC
draw loop for the recorded meshes, ~5–13 ms of CPU batches, and ~10–20 ms of r3d queueing,
part of it waiting for the RSP: libdragon's low-priority rspq ring is 2 x 2 KiB
(`RSPQ_DRAM_LOWPRI_BUFFER_SIZE` 0x200 words), so the CPU blocks in `rspq_next_buffer` once it
is ~4 KiB of commands ahead of the RSP instead of overlapping the next frame's simulation.

### Visual comparison (PC authentic 320x240 vs N64, 2x upscale PSNR)
Garden 1 17.5, Ancients 19.6, Space 17.9, City 20.5, Subterranean 18.3, Lab 16.1, Halloween
21.4, Retro 14.4 dB (moving actors and the clock differ between the captures: PC at tick 600,
N64 ~12 s of game time; the VI filter and 16-bit dither as in 2R). Geometry, textures, HUD,
text, shadows, sprites and effects match; the water and other additive surfaces are slightly
paler (the background is attenuated by the effect's brightness); the story book's pages are
softer (half-size render target, see RDRAM above).

### Status / next steps (resume from this branch)
- Runs: boot logos + intro FMV → frontend → story book → training (new player) / Garden 1
  and every world's first level; saves persist in SRAM; music, sfx, FMV audio.
- Speed: (1) the simulation (phase 3S); (2) the CPU's wait on the 4 KiB rspq ring: a larger
  `RSPQ_DRAM_LOWPRI_BUFFER_SIZE` (libdragon rebuild) or recording the frame's r3d commands into
  one block; (3) `xform_actor` per actor per frame (cache matrices of actors whose pose did not
  change); (4) procedural textures re-make their additive variant every update (~1 ms each).
- RDRAM: ~600 KiB free in the frontend; long sessions fragment the heap (big NTX banks): an
  arena for level texture files would make loads deterministic.
- Not done: multiplayer, arcade-machine screen copy, subtractive blend (approximated),
  real-hardware check (RDP fill rate, PI speed, SRAM on a flashcart).

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
## 3P: draw-side CPU time (2026-10-11)
Branch `worktree-agent-aaa520b4d4f6d5752` (from `feature/n64` at `673125c`). Goal: CPU draw
time per frame ≤ ~10 ms; result: **−35…−50 %** (Garden 1 54 → 33 ms, Lab 1 87 → 60 ms,
Halloween 74 → 42 ms, City 19 → 13 ms), not yet the target — see "What's left".
### Before / after (ares v148, 5 s `perf` windows, median after the first; `level W 0` boots)
"Before" = the 3I ROMs (`f2-lv{W}-0.z64`) re-run with the same harness, frontend from the §3I
table. "draw" = `App::draw` on the CPU; after 3P its parts are the draw loop **including**
r3d queueing (meshes now go to r3d while the loop records them), the r3d flush (C-side command
writing, without the CPU batches), and the CPU batches (sprites, shadows, HUD, text). City's
"after" includes 11–12 ms of waiting for the next 20 fps display slot (frame pacing): its CPU
work is 13 ms.
| scene | fps before → after | ticks/s | `Game::tick` | sim / frame | **draw / frame before → after** | after: loop (r3d queue) / flush / CPU batches | RSP busy | r3d draws (reused) |
|---|---|---|---|---|---|---|---|---|
| frontend (8-0) | 7.8 → 8.5 | 46 → 51 | 11.2 ms | 84 ms | **51 → 28 ms** | 14 (3.3) / 3.5 / 5 | 20 → 28 % | 100 (63) |
| Garden 1 | 6.6 → 7.4 | 40 → 44 | 13.6 ms | 95 ms | **54 → 33 ms** | 21 (5.3) / 2.9 / 6 | 20 → 19 % | 205 (144) |
| Ancients 1 | 7.0 → 8.6 | 42 → 52 | 10.8 ms | 79 ms | **60 → 31 ms** | 21 (4.6) / 2.4 / 5 | 18 → 22 % | 248 (214) |
| Space 1 | 3.6 → 3.8 | 21 → 22 | 26.4 ms | 188 ms | **85 → 62 ms** | 37 (7.9) / 2.8 / 14 | 19 → 18 % | 243 (106) |
| City | 27.4 → 20 (paced) | 60 | 5.1 ms | 20 ms | **19 → 13 ms** (+12 pacing wait) | 6 (0.9) / 1.3 / 3 | 30 → 25 % | 27 (21) |
| Subterranean 1 | 4.6 → 5.5 | 28 → 33 | 15.6 ms | 122 ms | **78 → 44 ms** | 29 (6.4) / 1.8 / 9 | 24 % | 328 (269) |
| Lab 1 | 4.0 → 4.6 | 24 → 27 | 16.8 ms | 136 ms | **87 → 60 ms** | 36 (7.5) / 2.9 / 15 | 22 → 24 % | 271 (150) |
| Halloween 1 | 4.8 → 5.6 | 29 → 33 | 17.6 ms | 126 ms | **74 → 42 ms** | 28 (8.7) / 3.5 / 7 | 20 → 21 % | 293 (146) |
| Retro 1 | 9.7 → 9.8 | 58 → 58 | 11.5 ms | 78 ms | **30 → 20 ms** | 13 (3.3) / 1.3 / 3 | 16 → 15 % | 112 (64) |
The frame rate barely moves because every scene except City is simulation-bound
(`Game::tick` 10–26 ms × up to 6 ticks per frame; phase 3S). "reused" = mesh draws whose
uniform and visible parts came unchanged from the previous frame (idle camera in these runs:
with a moving camera every draw is recomputed, ~40 µs each).
Visual parity: `C:\f2-n64test-3p\evidence\side-lv{0..7}.png` (3I build | 3P build, ares
captures ~50 s after boot; not in the repo, derived from game data): identical geometry,
textures, HUD, water, sky; differences are moving actors and the clock (PSNR 19–33 dB, as
between any two runs). The FMV boot path (logos, intro, frontend) plays.
### What changed
1. **libdragon rspq ring 2 × 2 KiB → 2 × 32 KiB** (`tools/n64/libdragon-3p.sh` +
   `libdragon-3p.patch`, a separate prefix `~/opt/n64root-3p`; `build.sh` uses it when
   installed). The CPU no longer waits in `rspq_next_buffer` while the RSP draws (flush
   ~9.7 → 2.9 ms in Garden). The H.264 decoder recycles its input assuming the stock ring
   (RSP assert `0xAF00`), so `n64_fmv_play` sets `rspq_set_lowpri_window(0x200)` (patch)
   while a video plays.
2. **Matrix fold on the RSP**: the `F2_DRAW` uniform carries the model matrix (pos_scale
   folded into the direction rows), `F2_FRAME` the view × projection; `rsp_f2mdx.S`
   (`F2FoldRow`) multiplies them (32×32-bit s15.16, per vertex-load command). The CPU fold
   was ~1500 cycles per draw / per bone. Host model `r3d::rsp_fold`; sky (`noClip`) draws get
   their own frame uniform (`R3dDraw::frame`). Depth error ≤ 3 of 32767 steps.
3. **Integer `to_fixed`** (exponent/mantissa shifts, bit-identical to the FPU path, host test)
   and no `memset` in `draw_uniform`; skinned bone uniforms by DMA from the frame arena
   instead of inline copies; the root-bone fog fold only with fog.
4. **Pose caches** (`f2_engine::anim::xform_actor_cached`, mesh-sink path only): unchanged
   actors reuse their matrices; actors that only moved reuse the hierarchy below an identity
   top (`L × top`, one matrix product per object; equal up to rounding); landscape matrices
   once per level. Garden: 38 → 5 full hierarchy evaluations per frame. `lean` feature
   (f2-draw/f2-engine, N64 only): no slerp for actors that did not move, no unit-scale
   matrix product.
5. **Stamped draw reuse** (`R3d::draw_slot`): `MeshDraw::stamp` versions the matrices; a
   draw slot with the same mesh, stamp, parameters and view as last frame reuses its uniform
   (kept in the slot, ring of 3 so in-flight frames are safe) and visible parts; ~10 µs per
   reused draw vs ~55 µs computed.
6. **Direct mesh backend** (`f2_engine::MeshBackend`, `render_r3d::R3dBackend`): recorded
   meshes go to r3d during the draw loop (no sink copy); mesh-level bounding sphere before
   the per-batch spheres; draw-slot mesh lookup memo.
7. **I-cache layout**: the VR4300's 16 KiB I-cache is direct-mapped; with link-order
   placement the same code ran 2.6 or 9.6 ms per frame depending on unrelated edits.
   `tools/n64/Makefile` generates `build/f2.ld` (libdragon's `n64.ld` with the hot code first):
   `.text.f2hot_d`/`.text.f2hot_q` (draw-loop actor path + r3d queueing, 16 KiB) and
   `.text.f2hot_f` + listed libdragon functions (flush and CPU batches); Rust functions are
   tagged with `#[cfg_attr(target_vendor = "nintendo64", link_section = ...)]`.
8. Newlib's `memcpy`/`memset` for Rust too (compiler-builtins' weak copies localised in
   `build.sh`); procedural textures refilled from their ARGB4444 surface without the RGBA8
   registry copy; additive variants by reciprocal table (exact) with word stores;
   minified HUD blits from the TMEM-sized copy (one load instead of one per 4 KiB piece).
9. **Frame pacing** (`App::pace`): running averages of a tick and of a frame pick the highest
   of 60/30/20/15 fps at which 60 ticks/s + frames fit into 90 % of a second (faster only
   below 80 %, 8 stable frames), applied with `display_set_fps_limit`; 0 (unlimited, ticks
   beyond 6 per frame dropped) when even 15 does not fit. Rendering is already decoupled from
   ticks with interpolation (`alpha`, PC-style); the cap keeps the rate steady and leaves the
   CPU to the simulation, so real time is kept whenever sim + draw allow it (City: 60
   ticks/s at a steady 20–30 fps). Logged as `pacing N fps` / `app: frame pacing`.
10. Measurement: `r3d queue`/`flush` timers and `reused` in the `perf` line; the profiler
    prints every bucket (`prof.py` sees 100 %); log lines up to 1 KiB.
### Profiling notes (for the next round)
- ares attributes timer samples to basic-block entries, so a straight-line function's cost
  shows at its caller's return point; use the in-ROM timers for precise splits.
- Costs are dominated by memory, not FLOPs: a D-cache miss ≈ 45 cycles, an I-cache miss
  similar; hot-loop micro-benchmarks of the same function varied 5600–9500 cycles with
  layout before the hot sections. Per draw: ~10 µs reused, ~55 µs computed.
### What's left (draw side) / status
- Draw loop proper (f2-draw `draw_actor_list`, `Renderer::draw_actor`, record, xform for
  animated actors) 9–29 ms: per actor ~40 µs of scattered reads of `Actor2`/models; next: an
  N64-side compact per-actor draw state (model slot, override texture, last inputs) so
  unchanged actors skip f2-draw entirely; animated actors (5–30 per frame) need the
  hierarchy maths (slerp ~700 cycles/object).
- CPU batches 3–15 ms (Space, Lab: many sprites/effects through `rdpq_triangle`, ~2500
  cycles per triangle incl. uncached rspq writes): batch them into a magma dynamic mesh or
  `rdpq_texture_rectangle` for screen-aligned sprites.
- Moving camera: every draw recomputed (~55 µs) → ~11 ms queue in Garden while hopping.
- The ≤ 10 ms target is met only by City/Retro-class scenes; the frame rate is bound by the
  simulation (3S) in all others.
## 3T: the retail variable timestep (N64-only deviation from 60 Hz parity)
After 3S the simulation still costs 8–19 ms per tick on the VR4300, so heavy levels cannot
run 60 ticks/s next to the renderer. The original PC game never needed that: it has a
**variable timestep** and simply took larger steps on slow PCs. Phase 3T ports that
mechanism exactly and lets the N64 use it when it falls behind.
### Retail semantics (source and exe agree)
- Main loop `RunWindowsLoop` (`MDX Library/mdxWindows.cpp:216`): `AppLoop()` (= `LoopFunc`)
  then `UpdateTiming()` per rendered frame, so frame N runs with the time measured at the end
  of frame N-1.
- `UpdateTiming` (`mdxTiming.cpp:50-69`, retail `0x44b430`): `mult = frameSpeed*0.001f`
  (PC24 product, stored as a float: 0.060000002); `currentTicks = timeGetTime()+tickModifier
  -firstTicks`, `interval = currentTicks - tickCount` (unsigned); if `interval >
  maxTickIncrement` (500 ms) the excess is dropped (`firstTicks += excess`);
  `speed = (float)(interval*mult)`, `frameCount = ftol(currentTicks*mult)` (chop),
  `tickCount = currentTicks`. `InitTiming(60)` (inside `InitLevel`, `layout.c:742`/`:942`)
  restarts the clock; `InitBackdrop` calls `UpdateTiming` after its load (`backdrop.c:55`).
- `LoopFunc` (`PC/Main.cpp:686-713`, retail `0x43cc90`): unless paused (`mode == PAUSE_MODE`
  and not `quittingLevel && !pauseConfirmMode`): `lastActFrameCount = actFrameCount;
  actFrameCount = ftol((frameCount - pFrameModifier) * (float)(turbo/4096))` (absolute,
  not incremental); `gameSpeed = ftol(turbo/4096 * speed * 4096.0f)`; `lastFrames/lastTicks`
  saved; on the first frame after a pause `pFrameModifier += actFrameCount -
  lastActFrameCount`, `actFrameCount` kept, `gameSpeed = pauseGameSpeed`; then
  `pauseGameSpeed = gameSpeed`. Paused: `beenPaused = 1`, `gameSpeed = 0`, `speed = 0`,
  `frameCount/tickCount` held. `pFrameModifier` is also zeroed in `InitLevel` (`layout.c:680`)
  and when the art viewer opens (`options.c:2311`). `RunPauseMenu`'s resume sets
  `gameSpeed = pauseGameSpeed` (`menus.c:671`; the global LoopFunc writes, retail
  `0x36bc608` — the port's old comment "never assigned" was wrong for PC).
- `timeInfo.speed` itself (not `gameSpeed`) drives MDX animation (`UpdateAnims`
  `animTime += animationSpeed*speed`) and the croak swell (`actor.cpp:181-219`, double
  constants 0.1/0.08 at PC24).
### API (`crates/f2-game/src/timing.rs`)
- `Game::tick` is unchanged (gameSpeed 4096, `actFrameCount += 1`) and remains the parity
  reference; it never touches the timing state (excluded from `Game::checksum` until the
  variable API is used, so fixed-step checksums are unchanged).
- `Game::tick_frames(input, n)`: one main-loop iteration that took `n` 60 Hz frames
  (`advance_timing_frames(n)` = the oracle's `h_tick_timing` generalised: `frameCount += n`,
  `tickCount = frameCount*1000/60`, `speed = n`, cap 30 = 500 ms) + `tick_timed(input)` =
  `LoopFunc` including the pause branch. `tick_timed_game_loop` is its trace point.
- `Game::update_timing(now_ms)`: the exact retail `UpdateTiming` from a millisecond clock
  (fractional speeds like a real PC: 17 ms → `speed` 1.0200001, `gameSpeed` 4177);
  `Game::timing_clock` (optional `fn() -> u32`) lets `InitTiming`/`InitBackdrop` read it.
- Port fixes made for it (no effect under `tick`): `InitTiming` and `pFrameModifier = 0` in
  `InitLevel`, `pFrameModifier = 0` at the art viewer, `gameSpeed = pauseGameSpeed` on
  resume (only with the variable timestep), `time_info_speed()` returns `timeInfo.speed`,
  croak factors as PC24 products of the double constants.
### Parity evidence (speed 1)
Before/after hashes of all Rust traces: the three demo traces (`b0818758…`, `148391ab…`,
`592b9d2a…`) and the 102 `f2-parity check --corpus all` cases (all `ok`, 0 regressed) are
byte-identical both with `Game::tick` and with `tick_frames(input, 1)` (`F2_TICK_FRAMES=1`
for `demo_trace`/`scenario_trace`). Known difference of `tick` vs the retail loop: in
`PAUSE_MODE` the retail game freezes `gameSpeed` (0) and `actFrameCount`; `tick` does not
(the corpus never pauses; `tick_frames` does it right).
### Behaviour at speed 2–4 (host, `examples/timestep_check`, `tests/timestep.rs`)
Render-free, all 40 arcade levels idle and with a scripted pad (presses timed in game
frames) for 60 s of game time, plus the three attract demos, at k = 1, 2, 3, 4 (and 5, 6, 8):
- **No panics; every tick advances `actFrameCount` by k with `gameSpeed` = 4096·k.**
- **Game time keeps real-time rate**: the arcade runs reach 3600 game frames in 3600/k ticks;
  the demos end after 792/537/409 ticks at k = 2/3/4 (1557 at k = 1): the `GTUpdate` timers
  (`general.h:34`, "second" = `actFrameCount > lastCount+60`) tick every 61 frames at k = 1
  but every 62/63/64 at k = 2/3/4, so every game timer (demo timeout, level clocks, safe
  time, mode timers) runs 1.6/3.3/4.9 % slow — the original's own quantisation.
- **Path entities stay on their paths**: path movement is a function of `actFrameCount`
  (`UpdatePathNME`/platforms: `progress = afc - startFrame`, catch-up loops for several
  nodes per tick), so clock-driven enemies/platforms are where k = 1 has them; the only
  offsets are enemies that stop to wait, which freeze where their last update left them:
  up to (k-1) frames short of the node (29/59/88 units in Garden 1 at k = 2/3/4; p99 over
  all levels 58/117/176). Entities driven by `Random()` (flappy things, random speeds,
  procedural textures) take one draw per tick instead of per frame and so diverge
  (statistically alike). Cyclic paths show a one-sample spike (~8100 units) when one run
  has wrapped to the start and the other not yet.
- **Frog**: idle frog identical to k = 1; with the pad pattern the frog never left the map,
  the at-rest distance from its tile centre stays ≤ 394 units (328 at k = 1; 0 in most
  levels: it sits on the centre), deaths over 40 levels 115/115/115 vs 117 at k = 1, no
  extra out-of-map entities (the same off-map entity as at k = 1). No tunnelling seen up
  to k = 8; frog hops and most collisions are tile-based.
- **Frame-rate artefacts reproduced (known-bugs #10)**: `actFrameCount & 1` effects never
  fire at even k (respawn safe-time and teleport rings `game.c:125/130`, the double preen
  idle `frogger.c`, `DrawFXRing` twinkle trails on odd frames) and every other tick at
  odd k; `frameCount & 1` smoke (enemies.c pilot lights/smoke bursts) is per tick, i.e.
  1/k as much smoke per second; per-tick increments that are not scaled by `gameSpeed`
  (e.g. enemy `currNormal += deltaNormal`) progress per tick; specfx fades scale with
  `gameSpeed`; input is sampled once per tick (a press shorter than a tick is lost — the
  scripted pattern's 3-frame presses at k ≥ 5 mostly vanish, which is why the "play" runs
  diverge there); `UpdateAnims` advances by `speed` per tick.
- Render-side, not ported here: MDX texture animation (`mdxTexture.cpp:785`) and the water
  wobble (`mdxRender.cpp:443`) use `timeInfo.frameCount`, which differs from
  `actFrameCount` by `pFrameModifier` after a pause (`f2-draw` passes `act_frame_count`;
  use `game.timing.time_info.frame_count` when `timing.active`).
### N64 wiring (`crates/frogger-n64/src/pacing.rs`)
On time: ordinary 60 Hz ticks (`tick_frames(input, 1)`, bit-identical to `tick` outside the
pause menu). Behind by k frames: **one** tick of k frames, k ≤ `F2N64_MAXSTEP` (default 4 =
15 fps game steps); real time beyond the cap is dropped. Render interpolation spans the
tick's k frames. `F2N64_TIMESTEP=fixed` restores the fixed loop (≤ 6 60 Hz ticks per frame,
`Game::tick`). The `pace` log line reports game frames per wall second.
ares v148, `F2N64_BOOT="level W 0"` (frontend: `frontend-trained`, FMVs skipped with A),
frog idle, median of 5 s windows after the first; evidence in `C:\f2-n64test-3t\` (not in
the repo). "real time" = game frames simulated per wall-clock second / 60.
| scene | fixed 60 Hz (before): fps / real time / `Game::tick` | variable, cap 4 (default): fps / real time / frames per tick / `Game::tick` | variable, cap 6: fps / real time / frames per tick |
| frontend (8-0) | 7.8 / 76 % / 8.3 ms | 12.6 / 84 % / 4.00 / 9.7 ms | 12.7 / 100 % / 4.70 |
| Garden 1 (0-0) | 7.4 / 73 % / 9.8 ms | 12.9 / 86 % / 4.00 / 10.8 ms | 12.8 / 100 % / 4.67 |
| Ancients 1 (1-0) | 7.6 / 75 % / 8.2 ms | 12.1 / 81 % / 4.00 / 9.0 ms | 12.1 / 100 % / 4.95 |
| Space 1 (2-0) | 4.5 / 45 % / 15.8 ms | 8.0 / 53 % / 4.00 / 18.5 ms | 8.0 / 80 % / 6.00 |
| City (3-0) | 28.0 / 100 % / 4.4 ms | 34.0 / 100 % / 1.76 / 4.7 ms | 34.1 / 100 % / 1.75 |
| Subterranean 1 (4-0) | 4.9 / 48 % / 12.7 ms | 8.5 / 56 % / 4.00 / 13.4 ms | 8.5 / 85 % / 6.00 |
| Lab 1 (5-0) | 4.5 / 44 % / 13.4 ms | 7.5 / 50 % / 4.00 / 14.8 ms | 7.5 / 75 % / 6.00 |
| Halloween 1 (6-0) | 5.9 / 59 % / 11.8 ms | 9.6 / 64 % / 4.00 / 13.3 ms | 9.6 / 96 % / 6.00 |
| Retro 1 (7-0) | 12.5 / 100 % / 8.0 ms | 22.0 / 100 % / 2.72 / 8.5 ms | 22.0 / 100 % / 2.72 |
- The frame rate goes up 1.6–1.8× (one simulation tick per drawn frame instead of up to 6);
  a k-frame tick costs ~1 ms more than a 1-frame tick (more animation/path catch-up).
- The game keeps real time whenever a frame takes ≤ cap/60 s: City and Retro always, the
  first worlds at cap 6. At cap 4 the heavy levels (Space, Subterranean, Lab) still run at
  50–56 % of real time: their frames take 115–135 ms (draw 80–90 ms + sim with audio polling 18–25 ms), i.e.
  7–8 frames. Remaining cost is now the draw side (phase 3P).
- Cap choice (owner decision, `F2N64_MAXSTEP`): 4 keeps the game steps close to 60 Hz
  behaviour (host checks above: no tunnelling, positions within (k-1) frames) and is the
  default; 6 reaches real time in the first worlds and 75–96 % elsewhere, with
  correspondingly coarser steps (host checks at k = 5, 6, 8: still no panics, no
  out-of-map entities, waiting enemies up to 4–7 frames short of their nodes). The PC
  original allowed up to 30.
### Status / next steps
- Done: `f2_game::timing` (exact `UpdateTiming`/`LoopFunc`), `tick_frames`/`tick_timed`,
  host check + test, N64 pacing (default variable, cap 4). Commits on branch
  `worktree-agent-adfb9e878e97eca51`.
- Next: the render-side `timeInfo.frameCount` users in `f2-draw` (texture animation) when
  `timing.active`; owner decision on the cap; real-hardware check.

## 3D: draw-side caches + FPU floor (2026-10-11)
ares v148, Garden 1 (`level 0 0`), frog hopping (camera moving), 5 s `perf` windows:

| | before | after |
|---|---|---|
| fps | 13.8–14.6 | 15.6–16.6 (~16.2) |
| draw / frame | 46–50 ms | 39–43 ms |
| CPU-side submit | 9–10 ms | 7–8 ms |
| `Game::tick` | 11.3–11.8 ms | 11.2–11.6 ms |

- Shared, exact (PC frames pixel-identical before/after, 6 levels x 2 frames): per-actor
  `models` lookup cached in `LevelScene::actors`; `record_actor` walks a cached per-model
  list of mesh objects (`Actor::mesh_objects`).
- r3d: draws with unchanged matrices/parameters but a new view (camera moving; ~140 of
  208 draws while hopping) re-cull from cached world-space spheres and keep their converted
  uniform (`view-only` in the perf log); culling always uses the world-sphere path.
- `floor`/`floorf` (weak in compiler_builtins, generic soft code) replaced in `main.c` by
  `floor.w.s`/`floor.w.d` with exact fallbacks; libdragon's CPU triangle setup calls them
  ~16x per triangle. Compiled without fast-math (libdragon's `-ffast-math` dropped signed
  zeros); boot self-test (`floor selftest: 0 mismatches` in the log) against the bit-level
  versions, which match glibc on 30M+ host values.
- Profile after (draw only): draw loop + per-actor engine work ~27 %, r3d per-draw ~15 %,
  CPU triangle path (sprites, shadows, effects, HUD) ~15 %, memcpy/memset ~7 %.
  Next: batch CPU sprites/effects; reduce per-actor work for animated actors; merge
  static scenery draws.

## 3D (cont.): 2D rect fast paths (2026-10-11)
`cputri:` log line (every 300 frames, main.c) splits the CPU-side 2D/3D submission; libdragon-3p
now counts RSP-wait ticks (`rspq_wait_ticks`): 0 here, so the cost is CPU work, not waiting.
Measured in ares: `rdpq_tex_blit` ~41 us per call vs ~19 us for upload + one
`rdpq_texture_rectangle_scaled` and ~3.8 us for the rectangle alone.
- Rects of a surface that fits TMEM: upload once per batch, then one texture rectangle each
  (`rect_fast`); TMEM-sized minified copies (HUD clock) likewise.
- Sub-rects of bigger surfaces (font atlas glyphs): `rdpq_tex_upload_sub` of the sub-rect
  (+1 texel border when filtering) + one rectangle (`rect_sub`); must set the TLUT mode
  (a preceding CI icon leaves palette lookup on: the digits drew as dark boxes until fixed).
- Garden 1 hopping: rects 3.7 -> 2.2 ms/frame, ~16.2 -> ~17.8 fps. Left: ~6 full RDP mode
  resets/frame after r3d lists (~100 us each; push/pop could cut them), ~40 CPU triangles
  (shadows/effects, ~2 ms; candidates for the RSP path).

## Polygon budget (2026-10-12): the triangles are not the cost

Question: would fewer polygons (the N64's 320x240 hides much of the detail) raise the frame
rate? Measured in ares, idle in each level, normal build against `F2N64_CFLAGS=-DF2N64_NO_TRIS`
(r3d does all of its CPU work but sends no triangles) and `F2N64_MEASURE_SIZES=1` (on-screen
radius of each draw's bounding sphere):

| level | fps | fps, no triangles | sim | draw loop | RSP busy | RDP busy |
|---|---|---|---|---|---|---|
| Garden 1 (0-0) | 15.7 | 15.8 | 14 ms | 33 ms | 31 % | 18 % |
| Space 1 (2-0) | 9.6 | 9.7 | 22 ms | 53 ms | 29 % | 17 % |
| Lab 1 (5-0) | 9.3 | 9.6 | 19 ms | 53 ms | 30 % | 19 % |

Without any triangles the frame rate moves 0-3 %: the RSP and RDP idle ~70 % of the time and
the CPU is the whole bottleneck (per-object draw work and the simulation). Mesh decimation
would cost model quality for no measurable gain; dropped.

Draws per frame by on-screen radius (draws / their triangles):

| level | all | < 1 px | < 1.5 px | < 3 px |
|---|---|---|---|---|
| Garden 1 | 204 / 8986 | 22 / 194 | 41 / 418 | 68 / 1016 |
| Space 1 | 241 / 9249 | 6 / 54 | 11 / 98 | 27 / 356 |
| Lab 1 | 269 / 9053 | 25 / 215 | 52 / 476 | 81 / 994 |

15-30 % of the draws in Garden and Lab are a few pixels big (~10 % of the triangles): skipping
them saves per-object CPU work, but only if the test comes before that work. Order chosen:
per-object CPU cost first (unchanged objects skip their draw work), then the small-object skip
on its cached transforms, then the simulation (19-22 ms in Space/Lab).

## 30 fps push (2026-10-12): benchmark, first fixes, audio

Benchmark boot `F2N64_BOOT="level-hop W L"` (arcade level, the frog hops up/right/down/left
every 32 game frames after 3 s: the camera moves every frame, so view-dependent caches are
exercised) and `tools/n64/bench.sh NAME "level-hop W L"` (build, ares run, per-window
summary, `sim phases` line: µs per tick per `f2_game::simbench::phase`, logged by the game
itself through `Game::phase_hook`). ares v148, Expansion Pak, stock clock.

Baselines at `ed3e8ce`/`34c6a58` (median 5 s window, hopping):

| level | fps | sim / frame | draw / frame | r3d queue | CPU 2D submit | Game::tick | actordraw phase |
|---|---|---|---|---|---|---|---|
| Garden 1 (0-0) | 15.4 (`cc1988b`) → 16.8 | 15 ms | 45 → 40 ms | 18.7 → 15 ms | 5 → 4 ms | 11.5 ms | 6.2 ms |
| Ancients 1 (1-0) | 19.0 | 12 | 36 | 11.9 | 3 | 9.5 | 5.0 |
| Space 1 (2-0) | 10.2 | 21 | 71 | 24.2 | 11 (155 CPU tris) | 17.8 | 7.9 |
| City (3-0) | 39.8 | 6 | 16 | 5.4 | 2 | 4.7 | 2.3 |
| Subterranean 1 (4-0) | 12.6 | 19 | 55 | 16.6 | 7 | 15.1 | 9.0 |
| Lab 1 (5-0) | 10.6 | 18 | 68 | 24.0 | 10 (160 CPU tris) | 13.8 | 7.6 |
| Halloween 1 (6-0) | 13.5 | 16 | 53 | 20.8 | 6 | 13.5 | 6.0 |
| Retro 1 (7-0) | 28.7 | 10 | 22 | 7.9 | 2 | 8.4 | 4.8 |
| frontend (8-0) | 16.5 | 12 | 44 | 13.7 | 7 | 8.8 | 5.0 |

A 30 fps frame is 33.3 ms for one variable-timestep tick (2 game frames) + audio + drawing.

### Fixes (commit `f45845f`)
- **Water (`R3d::animate_modge`) was ~7.4 ms/frame** in Garden 1 for 7 meshes / ~770
  vertices: f32 with Rust's saturating float→int casts (compare/branch sequences around every
  `trunc.w.s`) and ~2000 table sines. Now integer: phases as `u32` turns, a Q14 sine table,
  nested sines through phase lookups, Q10 sums; host test `modge_matches_pc_terms` against
  the f64 PC formula (s/t within 1/1024 texel, alpha ±2, y ≤ 0.6 world units). And only
  meshes whose draw was queued (after culling) are animated. Still ~3 ms (memory + per-vertex
  multiplies): next step handed to the r3d work (direct lookups of summed phases).
- Camera-only r3d redraws reuse the slot's ring uniform when the fog fields are unchanged
  (validity flag `u_ok`: never a uniform of an older matrix); the cached world-sphere `Vec`
  is swapped instead of copied (`memcpy` from `draw_slot` was ~1 ms/frame).
- HUD icons (64×64 / 128×128 RGBA32 drawn 32–35 px wide) missed the TMEM-copy fast path by
  a few pixels and took the multi-load `rdpq_tex_blit` (~0.45 ms each, 1.4 ms/frame): the
  copy may now be magnified up to 1.25×.

### Audio (commit `34c6a58`)
New `audio:` log line = CPU time inside mixer polls per second. Garden 1: **106 ms/s
(10.6 % of the CPU)**, of which the ULC music ~95 ms/s (`F2N64_CFLAGS=-DF2N64_NO_MUSIC`:
7–12 ms/s). Music was 44.1 kHz but the mixer outputs 32 kHz, so decoding + resampling the top
27 % was wasted; re-encoded at 32 kHz (same 128 kbps, same ROM size): **83 ms/s**. Remaining
split (instrumented libdragon copy, not committed): ULC coefficient decode ~18 ms/s, its
synchronous per-block ROM reads ~9 ms/s (`read()` of `max_block_size` per 1024-sample block),
effects fetch ~8 ms/s, per-round channel bookkeeping (frequency sync, volumes, RSP commands)
~25 ms/s. Mixer channel count (24 vs 12) made no difference. Tried: libdragon built with
`-DNDEBUG` (asserts are compiled in everywhere: no NDEBUG in n64.mk) — the ROM ran but an RSP
DMA landed on the stack and frames stalled (an assertion with a side effect somewhere);
dropped. `-ftrivial-auto-var-init=uninitialized` for libdragon: no measurable change.
MIDI instead of the recorded music was considered (owner question) and rejected: the PC
soundtrack is mixed audio, a transcription would be a different arrangement through a
generic soundfont, and the synth's voices would cost mixer time too.

## 3S2: simulation tick on the VR4300, second round (2026-10-12)

Branch `worktree-agent-aa400f32a70aac672` (from `ed3e8ce`). Goal: `Game::tick` in Garden 1
while hopping from ~11.5 ms toward 7 ms, bit-identical. Result: **Garden 1 11.7 -> 8.6 ms,
Lab 1 14.3 -> 9.9 ms, Space 1 18.0 -> 14.6 ms** (-27 / -31 / -19 %); not the 7 ms.

### Method
- `tools/n64/bench.sh NAME "level-hop W L" 40` (ares v148, 5 s windows; numbers = a steady
  window). `level-hop` now ignores the real pad: a stray host key (ares' keyboard pad)
  paused the game in the first runs (`mode 9`), so earlier "hopping" numbers may be mixed.
- **Same-ROM A/B** (`F2N64_TUNE_AB=mask`): the experiment is gated on
  `f2_game::simbench::tune(bit)` and the ROM alternates per window, so both sides have the
  same code layout. Between builds the same code moves by up to ±10 % per phase from I-cache
  placement alone (actordraw 3.5-4.0 ms with identical sim code), so single-build
  comparisons of small changes are meaningless.
- Probes: temporary extra `simbench::phase` marks inside the actor pass (copy / cull /
  xform parts; the hook call itself perturbs the I-cache, use for proportions only), the
  profiler (`profile-sim`) and i686 callgrind with the VR4300 cache geometry
  (`--D1=8192,1,16 --I1=16384,1,32`) for D-cache misses per function.
- Parity after every change: the three demo traces and all 102 `f2-parity check --corpus
  all` Rust traces sha1-identical to the `ed3e8ce` ones; f2-game tests in debug (the caches
  assert every reuse there); simbench end-of-run checksums equal before/after **and N64 =
  host** (`F2N64_DATA=prep F2N64_FEATURES=simbench`; the SWAR filters run big-endian there).

### Result (`level-hop`, µs per tick by phase; fps of the same run)

| level | | tick | actordraw | enemies | platforms | draw2d | draw3d | loop-rest | frogs | specfx | fps |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Garden 1 (0-0) | before | 11 700 | 6 070 | 1 980 | 820 | 990 | 290 | 665 | 545 | 235 | 16.7 |
| | after | 8 560 | 3 745 | 1 590 | 800 | 765 | 155 | 645 | 530 | 215 | 16.9 |
| Lab 1 (5-0) | before | 14 300 | 7 880 | 2 550 | 700 | 1 030 | 580 | 850 | 400 | 185 | 10.4 |
| | after | 9 900 | 4 760 | 2 210 | 690 | 610 | 235 | 825 | 330 | 190 | 11.3 |
| Space 1 (2-0) | before | 18 000 | 8 020 | 3 200 | 1 800 | 1 990 | 430 | 710 | 385 | 1 100-1 300 | 10.3 |
| | after | 14 600 | 5 850 | 2 850 | 1 800 | 1 130 | 245 | 755 | 420 | 1 200 | 10.0 |

simbench on the N64 (fixed 60 Hz step, no rendering, mean µs/tick, same checksums):

| | frontend | 0-0 | 2-0 | 2-2 | 4-1 | 6-0 | demo 0-0 | demo 1-0 | demo 6-1 | sum |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| before | 8 631 | 9 557 | 16 432 | 15 886 | 13 511 | 11 870 | 10 046 | 8 162 | 13 212 | 107 307 |
| after | 7 109 | 8 179 | 14 743 | 13 909 | 11 336 | 10 294 | 8 553 | 6 897 | 10 257 | 91 277 |

**The frame rate barely moved**: in the same runs the draw side's `r3d queue` went from
14.4 to 15-17 ms (Garden) and 21 to 24 ms (Space). That is code placement too: the draw
loop's non-region functions move whenever anything before them changes size, and across
the builds of this round the same draw code took 14.4-19.8 ms in Garden. The draw side
needs its own hot-function regions (or the per-frame draw code in fewer, contiguous
places) before sim savings show up as fps.

### What changed (each commit parity-verified as above)
1. **I-cache regions for the simulation loops** (`tools/n64/Makefile`, after the draw
   regions): `.text.f2hot_s` = the per-actor pass (`copy_and_draw_actors`, `copy_actor`,
   `actor_list_draw_one`, `xform_actor`, root transform, key search, slerp, `mtx_cat`,
   bounding box, `update_anims`; 15.9 KiB), `.text.f2hot_e` = the enemy loop,
   `.text.f2hot_p` = the platform loop + `iquat_slerp`/`make_unit`. Before, `copy_actor`
   and `sphere_visible` sat on the same I-cache lines and evicted each other for every
   actor: Garden actordraw 6.2 -> 3.9 ms, enemies -0.4 ms; Space platforms 2.4 -> 1.8 ms
   (when `iquat_slerp` happened to collide). The biggest single win of the round.
2. **Per-actor caches** (`view.rs`; fields of `MdxActor`/`Actor2` listed in `ser_order!`'s
   new `skip {}`: never serialized, `PartialEq` always true, `Debug` constant, so state,
   checksums and traces are untouched; a deserialized actor starts invalid):
   - `SphereCache`: `CopyActorList`'s integer sources (position/size/qRot): unchanged
     sources skip the ten int->float conversions and compares (A/B -200 µs); the culling
     sphere's world centre and radius (quaternion matrix + transform) while the placement
     and model are unchanged (-0.4 ms); a valid entry also proves the model exists (no
     `ModelTable` lookup). Placed before the placement, so an unchanged culled actor reads
     ~9 cache lines and never the placement floats.
   - `RootCache`: the root object's key indices and object matrix while model, `animTime`
     bits, start keys and placement are unchanged (A/B -120 µs).
   - `SpriteCache`: object-sprite placements under the same key, reapplied to the sprites.
   - One `SphereCache::invalidate()` clears all of them; **every write of
     `MdxPlacement::{pos, q_rot, scale}` must call it** (today: `copy_actor`,
     `xform_new_actor`; new actors start invalid). Debug builds recompute and assert on
     every reuse (the f2-game tests and debug demo runs exercise it).
3. **Procedural textures eight pixels at a time** (exact SWAR: 16-bit lanes in a `u64`,
   biased subtraction for the clamp, row ends kept, zero/circular padding): water ripples,
   fire/candle/steam smoothing, forcefield; references kept under `cfg(test)`, equal on
   4 000 random/extreme buffers. Blit in place without bounds checks. Garden draw2d 990 ->
   770 µs, Space 2.0 -> 1.1 ms, Lab 1.0 -> 0.6 ms.
4. Small: frog head matrix path in a fixed array and the head name borrowed (no
   `String` clone + `Vec` per tick); script `If` sub-paths on the stack; the sprite
   rotation walk without an id snapshot.

### Tried and reverted
- Integer-only `util_sqrt` (equal to the float one for all 2^32 inputs, test kept out):
  **+1.0 ms/tick** in A/B. The VR4300 (MIPS III) has no CLZ, so `leading_zeros` is a
  software sequence; the FPU path is cheaper. (3S's `leading_zeros` in `MakeUnit` may be
  worth re-checking the same way.)
- `opt-level = "s"` for f2-game: 8.5 -> 10.5 ms.
- Closed-form `translate·I`, `R·T`, `scale·M`, `I·M` (exact incl. signed zeros; 300 000
  random cases): A/B 0 (as 3S found).
- Reusing sphere / bounding-box results while the view is bit-identical (a view epoch): A/B
  0 while hopping (the camera moves every tick); it would only help an idle camera.

### Where the time is now / next candidates
- Actor pass (Garden 3.7 ms, 116 actors, ~20 drawn): culled actors cost ~9 D-cache lines
  each (copy + sphere + flags); drawn animated actors the keyframe/slerp/bounding-box maths
  (Lab: animated sprite objects ~0.9 ms). Further: a compact hot side array (would need
  every writer of position/size/qRot to update it), or fewer bytes per actor.
- Enemies/platforms (1.6 + 0.8 ms Garden; 2.9 + 1.8 ms Space): scattered memory (actor,
  path node, tile per entity: ~660 D-cache misses per tick for Garden's 45 platforms on
  callgrind) and region `e` is 25 KiB (> I-cache): splitting the rarely executed parts
  (`process_attached_effects`' effect creation) out would bring it under 16 KiB.
- Once per tick code (frogs, loop-rest, events ~1.3 ms) is mostly I-cache misses of
  straight-line code; only less code helps.

## 3E: draw-loop and 2D CPU cost (2026-10-12)

Scope: the CPU work of the draw loop outside r3d (`f2-draw`, `f2-engine`), the CPU batches
(`render_cpu.rs`, the 2D half of `csrc/main.c`), the procedural texture refill and
`render::snapshot`. Branch `worktree-agent-a1cb1a2f375241a0b` (from `ed3e8ce`).

### Before / after (ares v148, `tools/n64/bench.sh`, 5 s windows, frog hopping)
"mine" = (draw loop − r3d queue) + CPU-side submit + build, the target metric.

| scene | fps | draw / frame | draw loop (r3d queue) | CPU-side submit | build | CPU tris + rects | mine |
|---|---|---|---|---|---|---|---|
| Garden 1, before | 16.7 | 39 ms | 29 (15.0) ms | 4 ms | 1 ms | 37 + 22 | ~19 ms |
| Garden 1, after | **18.8** | 34 ms | 28 (14.9) ms | 1 ms | 0 ms | 14 + 31 | **~14 ms** |
| Space 1, before | 10.4 | 67 ms | 48 (22.4) ms | 10 ms | 3 ms | 156 + 24 | ~39 ms |
| Space 1, after | **11.85** | 57.5 ms | 46.5 (21.9) ms | 3 ms | 1 ms | 92 + 58 | **~29 ms** |
| Lab 1, before (1st window) | 10.6 | 65 ms | 43 (20.6) ms | 9 ms | 3 ms | 121 + 41 | ~34 ms |
| Lab 1, after (1st window) | **12.9** | 51 ms | 37 (18.2) ms | 2 ms | 0 ms | 111 + 42 | ~21 ms |

(Lab's hopping frog dies after ~15 s and the later windows are the game-over / frontend,
so only the first window compares.) Idle (`level W 0`): Garden 18.4 -> 20.2 fps, Space
10.3 -> 11.7, Lab 10.3 -> 11.6. Procedural refill 0.9 -> 0.48 ms (Garden), snapshot 0.75 ->
0.49 ms. Garden's per-phase split after (`draw phases` log line, µs/frame; the actor phases
include r3d queueing): setup 355, world 1660, actors0 20350, shadows 860, actors1 2610,
sprites 1060, specfx 150, overlays 1160, refill 480, snapshot 490. With
`F2N64_NO_BACKEND=1` (no r3d) Garden's actor phases are ~9 ms, Space's ~16 ms: the
per-actor work, not the 2D, is now the bulk of "mine".

The ≤ 8 ms target is not reached: what is left is ~90 µs per drawn actor in Garden
(87 actors: list walk, interpolation, the pose cache test, one `MeshDraw` per object), plus
~1 ms each of shadows, sprites and overlays. Calibration in ares (boot-time loop, removed
again): a D-cache line miss costs ~40 cycles over a hit, a dependent f32 multiply-add ~22
cycles, an f32 -> i32 cast ~41 cycles; a warm second call of the per-actor code still took
~19 µs + ~39 µs, so it is instruction-bound as much as miss-bound.

### What changed (commits 68e23fa .. 784c95b)
CPU batches (Space/Lab: 9-10 -> 2-3 ms submit):
- `RenderCpu::build` reads the new `Renderer::frame_view` (frames, backdrops, overlays,
  pass states; shares its pass table with `build_segments`) instead of `build_segments`:
  no intermediate `TlVertex` list. Scene triangles go to C by address
  (`n64_draw_tl_tris`, converted there); affine ones are written straight as the rdpq
  `TRIANGLE_DATA`/`TRIANGLE` commands (`fast_tri`: the fixed-point words
  `rdpq_triangle_rsp` makes, without its float vertex, 1/w and s16.16 conversions).
- Sprite quads (screen-aligned, one depth) of Z-tested, non-writing, clamped passes
  (coins, pickups, flashes) are texture rectangles with primitive depth
  (`rdpq_mode_zoverride` + `rdpq_set_prim_depth_raw` per rect), not two triangles.
  Garden: 37 -> 14 triangles a frame. 2D quads / sprite pairs are tested in place.
- Texture uploads (`rdpq_tex_upload` / `_sub`, ~50 µs of CPU each) are recorded once into
  rspq blocks (4-way cache keyed by surface, sub-rect, tile, repeat; evicted blocks freed
  4 frames later) and replayed. The direct-mapped first version thrashed (4 misses a
  frame from glyph sub-rects).
- CPU-batch render modes (`apply_state`) are recorded per distinct mode into rspq blocks
  and replayed (0.55 -> 0.35 ms for 7 resets in Space).
- Procedural textures: refilled only when the game blitted (`ProcTexture::timer`), converted
  straight into the texture's texels (`n64_tex_update_begin/_end`), nibble expansion by
  bit operations (host-tested against the RGBA8 path).
Draw loop (`f2-draw`/`f2-engine`):
- `render::Snap` keeps the simulation's integers (`snapshot_into` reuses its buffer; float
  conversion only for drawn actors; lean: an unmoved actor reuses its converted pose and
  scale; no slerp when only the position changed).
- Both draw lists come from one `actList` walk (list 1 kept while drawing list 0);
  per-frame invariants hoisted; `overrideTex` CRC kept per actor (was a lowercase `String`
  allocation per actor and frame).
- `xform_actor_cached`: matrices written in place; moving-not-turning actors add the
  translation to a cached `L x R` (exact: the same sums); turning / re-animated actors write
  `L x R` straight into the result; `next_stamp` without an atomic RMW on the N64;
  `mtx_cat` without its `memset`; `Renderer::draw_actor` borrows the sink in place and keeps
  the CPU fallback out of line.
- `update_animating_textures` walks only animated entries; shadows / decals / rings memoise
  `quaternion_to_matrix(rotation_to_normal(n))` per normal (acos / sin_cos / normalise).
- `rt::ticks` reads COP0 Count inline, `rt::audio_poll_due` tests in Rust (no C call per
  draw); `build/f2.ld` now regenerates when the Makefile changes; hot-region order
  q / `cull_world` closure / d.
- Measurement: `draw phases` log line (`f2_draw::render::set_phase_hook`), `cputri:` rect
  paths and upload-cache hits, `F2N64_NO_BACKEND=1`.

### Evidence
- PC pixel identity: `frogger --headless --frames {90,300} --level W 0 --profile
  {authentic,enhanced} --size 320x240` for W = 0,1,2,3,5,8 (24 frames): byte-identical PNGs
  before (`ed3e8ce`) and after every shared-crate commit. N64-only behaviour sits behind
  `lean` or in `frogger-n64`.
- N64 visuals: idle ares captures of Garden 1 / Space 1 / Lab 1 before and after (coins,
  shadows, water refill, HUD, lightning): same content; differences are animation phase
  and the clock. Sprite rectangles vs the triangle pairs: same texels (bilinear), the tint
  rounds instead of truncating (≤ 1/255).
- Tests: `cargo test -p f2-engine -p f2-draw`, `cargo +nightly test --manifest-path
  crates/frogger-n64/Cargo.toml` (new: ARGB4444 conversions vs the RGBA8 path).

### Tried and reverted
- Writing procedural texels / additive variants through the cached alias + writeback: 50 %
  slower than uncached stores (write-allocate misses).
- Hot-region orders d/q, q/d (+ `draw_listed_actor` tagged): the r3d queue timer moved
  14.1 - 17.3 ms between builds with identical r3d code. Unlisted functions
  (`cull_world::{closure#1}`, `memcpy`, ...) shift with every edit; their aliasing with
  `draw_slot` dominates. Tagging r3d's remaining per-draw helpers (and keeping
  `.text.f2hot_d` ≤ ~6 KiB) is the robust fix.

### Next (draw side)
- Per-actor cost (~90 µs Garden, more in Space): a compact per-actor draw record (model,
  mesh-object list, last stamp, last raw inputs) so unchanged actors replay their
  `MeshDraw`s without touching `Actor2`, the engine `Actor` and the model; actors whose
  bounding sphere is under a pixel skipped before any work.
- Shadows (Space 2.5 ms): the enemy / platform list walks touch every entry; sprites
  (1-2.5 ms): `Tri` (152 bytes) per corner pair; a compact sprite record for the console.
- `R3dLevel::sync_textures` walks every registry entry each frame (~0.6 ms).

## 3R: r3d CPU cost per frame (2026-10-12)
Branch `worktree-agent-aef7edadeadb4ae33` (from `ed3e8ce`). Goal: the CPU cost of the RSP
renderer (`src/r3d.rs`, `src/render_r3d.rs`, `csrc/r3d.c`, the shaders) in Garden 1 hopping from
~17.5 ms (r3d queue + flush) to ≤ 7 ms, images unchanged.

### Before / after (ares v148, `tools/n64/bench.sh NAME "level-hop W 0" 40`, last 5 s windows, clean builds)
"r3d queue" in the perf line: before = recording + queueing inside the draw loop (and any
audio poll that landed in it); after = the batch after the draw loop (`resolve + run_queue`;
the recording that stays in the draw loop is part of "draw loop"). "draw" = all of `App::draw`.

| level (hopping) | fps | draw | draw loop | r3d queue | flush |
|---|---|---|---|---|---|
| Garden 1 (0-0) | 17.0 → **21.3** | 38–39 → **28 ms** | 29 → 15 ms | 14.7 → **4.9 ms** | 2.56 → **1.42 ms** |
| Space 1 (2-0) | 10.4 → **12.2** | 67–68 → **50 ms** | 49 → 27 ms | 23.1 → **5.8 ms** | 2.40 → **1.51 ms** |
| Lab 1 (5-0) | 10.6 → **12.2** | 62–66 → **44–49 ms** | 39–43 → 24–26 ms | 17.7–20.7 → **5.7 ms** | 2.0 → **1.2–1.5 ms** |

Recording inside the draw loop now costs ~1.5–2 ms (a build where recording returns at once:
Garden draw loop 15 → 12 ms); all r3d CPU work together ≈ 8–9 ms in Garden (was ~20).
Idle boots (`level W 0`): Garden 1 18.2 → 20.5 fps, Space 1 10.3 → 11.7, Lab 1 10.1 → 11.1.

Per phase (Garden 1 hopping, `F2N64_R3D_PROF=1` build; timer calls inflate small phases):

| phase per frame | before | after |
|---|---|---|
| 60 full draws (actors whose pose changed) | 5.19 ms | 1.20 ms |
| 144 camera-only redraws | 4.45 ms | 1.91 ms |
| of which sphere culling (204 calls, 450 tests) | 3.68 ms | 1.63 ms |
| skinned frog (17 bones) | 0.30 ms | 0.23 ms |
| MODGE/WAVE water (4 meshes) | 3.05 ms | 0.07 ms |
| recording (214 draws, in the draw loop) | – | 1.90 ms |
| flush: Rust / C list states / C draws | – / – / – (2.5–3.3 in all) | 0.31 / 0.12 / 0.97 ms |

### What changed (commits on the branch, oldest first)
1. **I-cache budget**: the `.text.f2hot_d`+`f2hot_q` region had grown to 26 KiB (> the 16 KiB
   direct-mapped I-cache) and the cull test closure was linked outside it. Per-draw code kept
   small and in the region; MODGE, skinned and cold paths moved out.
2. **Water on the RSP** (`csrc/rsp_f2modge.S`, its own overlay since IMEM was full): the CPU
   writes a 56-byte header per mesh and frame (six 16-bit time phases); the RSP computes the
   per-x/z phase summands, per (x, z, alpha) position the y/s/t offsets and alpha (257-entry Q14
   sine table in DMEM, `VMUDM`/`VMULF`, 8 lanes) and applies them to the vertices. Bit-exact
   Rust model (`modge_x16`/`modge_z16`/`modge_pair16`), host test against the f64 PC formula
   (s/t ≤ 1/1024 texel, y ≤ 0.13 world units, alpha ≤ 1), in-ROM `F2N64_MODGE_CHECK=1` (Garden
   and Lab meshes: 0 mismatches). The 8-position input chunk sits in the clip cache and the
   position table is rounded to 8, so Halloween's 424-vertex / 134-position mesh fits too
   (Halloween 1 queue 8.7 → 7.4 ms, see the sweep below); bigger meshes keep the CPU path (logged
   "animated on the CPU").
3. **Batch after the draw loop**: the game's draw loop only records each draw (`queue_raw`:
   model id/object, parameters, and the matrix only when its (stamp, object) changed); the
   culling and uniforms run in one batch (`run_queue_with`) where r3d's data and code stay in
   the caches. Meshes resolve from a per-slot (model, object) memo, the level's lookup only on
   a miss (~10 per frame). The immediate API (`draw`, `draw_slot`, `draw_skinned`) queues too,
   so the per-draw path is inlined into the loop (one stack frame).
4. **Compact per-draw data**: 32-byte draw slots in a dense array (frame, stamp, mesh sphere
   centre and scale, vis range, interned parameter id, uniform ring state; uniforms in stable
   chunks, written only when they change); 32-byte `MeshHot` per mesh; dense sphere and part
   arrays; 16-byte `DrawRec`; parameters interned through a small hash (no 24-byte compare per
   draw); `R3d` `repr(C)` with the per-draw fields first.
5. **Culling**: world-space frustum planes; the mesh sphere test returns the planes it straddles
   and the parts are tested only against those, moved into model space (no per-part centre
   transforms, no per-slot centre arrays); single-part meshes use the part sphere; meshes with
   some unbatched groups still get a sphere over the others.
6. **Flush**: group textures through a dense CRC-id table that remaps in place (animated
   textures changed the registry every frame, so every drawn mesh re-resolved its groups through
   BTreeMap lookups into a new Vec: Rust flush 1.34 → 0.31 ms).
7. **Texture sync** (`R3dLevel::sync_textures`): per frame only the animated entries and the
   generations of the images the static ones show (~0.7 → ~0.3 ms).
8. **Matrix conversion on the RSP**: the draw uniform carries floats; the shader converts them
   once per uniform load (`F2_Convert`, exactly `to_fixed`; `F2N64_CVT_CHECK=1`: 1920 values incl.
   ±32768, inf, NaN, denormals: 0 mismatches). Marginal on the CPU (full draws were not
   conversion-bound; skinned −0.05 ms); kept since it is exact and only costs RSP time.
9. No audio polls inside the batch (a 40 ms poll, ~3.5 ms in Lab, landed in it and counted as
   r3d time); in the draw loop every 16th recorded draw. Per-level r3d storage freed on unload
   (it kept its capacity: Lab game over → frontend ran out of the large-block pool). No
   separate fog-less base uniform per slot.

### Fix: sky spikes and missing far background (pre-existing, also on `ed3e8ce`)
Reported as "vertex explosions on far geometry / the Space background" (Space 2 start, top
left): long wedges from the lamp post towards the top-left corner in ~10 % of frames, varying
with the camera. Bisected with `F2N64_SKIP_MODEL` to the background model `7f3a4793` (scale 5,
coordinates up to 6610: view-space x/y beyond ±33000, W up to the sky far plane 30000). Two
s15.16 overflows in sky (`noClip`) draws: X/Y saturated beyond ±32768, and magma's clip codes
(clip factors (1, 1, G, G): X vs W·G, Z·G vs W·G) saturated beyond W = 32768/G = 5461 at guard
6, so far faces were far-flagged and dropped, and guard-flagged vertices reached the clipper
with saturated coordinates. Normal draws end at W = 1200 and never get there. The sky's frame
(`frame_uniform_vp(_, true)`) now carries X/Y divided by G, the screen scale multiplied by G,
and guard factor 1 (`r3d.c` sets the clip factor per loaded frame): same projection and
on-screen guard band, W / Z / near-far unchanged. Host test `sky_vertices_project_like_mdx`
(`emulate_vertex` now models the saturated clip products; fails without the change). 20
captures of the Space 2 intro: spikes in 3 of 19 frames before, 0 of 19 after, and the far
background (a nebula band and a planet ring) now drawn: `C:\f2-n64test-r3d2\evidence\sky-*`.
A first attempt (X/Y ÷ 4, guard unchanged; then ÷ 2 with guard 3) still showed beams, since
W·G overflowed: the guard product was the main source. Residual: the clipper's plane dot
products (W ∓ X) can still saturate for sky triangles spanning the whole band at W > 16384,
and the sky's far test is approximate within ~1000 units of 30000 (s15.16 `a·W`).

### Visual checks
- r3d-test ROM with the animation frozen (`F2N64_R3DT_FRAME=777`, synthetic scene, same patch
  in an `ed3e8ce` worktree): **0 of 307200 pixels differ**, both shots.
- Idle and hopping captures, `ed3e8ce` vs this branch, Garden 1 / Space 1 / Lab 1:
  `C:\f2-n64test-r3d2\evidence\{idle,hop}-*-{base,r3d2}.png` (derived from game data, not in
  the repo): same scenery, water/lava, sky, HUD; differences are actor animation phases, timers.
- Culling stays conservative (host test `culled_batches_are_fully_rejected`: every face of a
  culled batch is one MDX rejects), so it never changes the image.

### Measurement notes (for the next round)
- **Data and code layout move the batch by tens of percent.** With the D-cache 8 KiB direct-
  mapped, 1 KB of padding before `R3d` took the batch from 7.0 to 5.2 ms in one build;
  `F2N64_PADX=N` (bytes of padding) samples data layouts: before step 3's inlining 4.8 / 5.1 /
  8.2 / 7.6 ms at 0 / 256 / 512 / 768, after 4.6 / 4.9 / 4.7 / 4.5. Judge changes over several
  layouts, and prefer fewer distinct lines per draw (one stack frame, contiguous hot fields).
- **In ares a 32-bit integer `mult` costs ~25 cycles** (VR4300 manual: 5): an integer (Q14)
  sphere test took 518 cycles vs 258 for the float one (`classify`, 6 planes), so integer
  rewrites of float code lose here.
- `level-hop` runs can end in the pause menu (mode 9) or game over (Lab ~40 s): check the
  `perf [W-L] mode M` tags of the windows compared.
- `F2N64_R3D_PROF=1`: per-phase timers (`r3dprof` line every 150 frames).

### Tried and dropped
- Integer whole-mesh sphere test (above: 2× slower in ares).
- Per-vertex water arithmetic dedupe on the CPU (per position instead of per vertex): the cost
  was cache misses on the vertex lines and tables, not the arithmetic (→ RSP).

### All arcade levels (idle boot `level W L`, 14 s, last level window; branch head)
No assertions, panics or out-of-memory in any level (0-1, 0-2, 3-1, 3-2 boot without a level
in this harness: no tiles, nothing drawn). fps / r3d queue / flush (ms):

| W \ L | 0 | 1 | 2 |
|---|---|---|---|
| 0 Garden | 21.3 / 3.3 / 1.5 | – | – |
| 1 Ancients | 22.4 / 3.4 / 1.6 | 23.0 / 2.9 / 1.3 | 16.6 / 3.3 / 1.5 |
| 2 Space | 11.4 / 4.8 / 1.7 | 32.2 / 1.2 / 1.2 | 12.0 / 4.7 / 1.4 |
| 3 City | 42.2 / 3.4 / 0.8 | – | – |
| 4 Subterranean | 14.6 / 5.6 / 1.3 | 12.9 / 7.4 / 1.3 | 17.6 / 5.1 / 1.2 |
| 5 Lab | 11.1 / 5.0 / 1.8 | 14.9 / 8.0 / 2.0 | 17.5 / 3.3 / 1.5 |
| 6 Halloween | 15.9 / 7.4 / 2.0 | 17.8 / 4.5 / 1.5 | 25.2 / 1.5 / 0.9 |
| 7 Retro | 33.7 / 2.0 / 0.9 | 39.7 / 1.8 / 0.9 | 32.8 / 2.5 / 1.3 |

Water meshes still animated on the CPU (too big for the DMEM scratch): City 1 (2: 194 and
144 positions), Subterranean 3 (167 positions, 1272 bytes of terms), Lab 2 (184). Lab 1
hopping on the head: 11.1–13.1 fps, queue 5.2–5.6 ms, flush 1.3–1.8 ms, no crash
(`evidence\hop-lv50-r3d2.png`).

### Risks
- RSP code grew: `F2_Convert` (per uniform load, ~150 scalar instructions: RSP time, not CPU),
  the water overlay (`rsp_f2modge.S`, overlay swap per water mesh per frame) and a 0-vertex
  load used as a custom command. Checked by bit-exact models and in-ROM self-checks
  (`F2N64_MODGE_CHECK`, `F2N64_CVT_CHECK`), not by the r3d-test frame alone.
- Timing is layout-sensitive (above): a later unrelated change can move the batch by ±20 %.
- Matrices are copied at record time only when a slot's (stamp, object) changes: a caller
  that changes a matrix without a new stamp would draw the old one (the game's stamps cover
  every pose change; host test `queued_draws_match_immediate_ones`).
- The sky change widens nothing on screen but sends far sky faces that used to be dropped:
  more sky triangles in Space (fps unchanged within noise: Space 1 idle 11.4 vs 11.7).

### Left / next candidates
- The four water meshes still on the CPU path (sweep above): process the term list or the
  position table in chunks too.
- Camera-only redraws (144 × ~13 µs): reuse the visibility while the camera moved less than
  the sphere's distance to the planes (temporal coherence) would skip most sphere tests.
- Recording in the draw loop (~1.5–2 ms): bound by cache misses in the game's draw loop.
- C flush (~1 ms): per draw a uniform writeback and load, per group a block call; RSP-side TLUT
  switching (rdpq's mode state is in shared DMEM) would avoid the rdpq mode changes between
  palette and RGBA textures.
- Merging the objects of a multi-object actor into one multi-matrix draw (70 of 205 draws in
  Garden are one 14-object model × 5) needs packer + f2-engine changes.

## After the 3S2 + 3E + 3R merges (2026-10-12, `e087180`)
Same benchmark (`tools/n64/bench.sh m3-wW "level-hop W 0" 40`, clean build, last 4 windows of
5 s, frog hopping). fps before (first table of "30 fps push") → now; per frame now:

| level | fps | sim | draw (loop / r3d queue / flush / 2D submit) | Game::tick (of which actordraw) | audio poll / tick | worst frame |
|---|---|---|---|---|---|---|
| Garden 1 (0-0) | 15.4 → **28.4** | 10 ms | 21–22 (13 / 4.1 / 1.4 / 1) | 8.9 (4.0) | 1.0 | 36 ms |
| Ancients 1 (1-0) | 19.0 → **29.4** | 9 | 22 (13 / 4.1 / 1.4 / 1) | 7.1 (3.0) | 0.9 | 34 |
| Space 1 (2-0) | 10.2 → **16.6** | 18 | 37 (24 / 4.7 / 1.5 / 3–4) | 14.9 (6.4) | 2.1 | 58 |
| City (3-0) | 39.8 → **59.0** | 5 | 10 (4 / 3.1 / 0.8 / 0) | 3.6 (1.5) | 0.6 | 19 |
| Subterranean 1 (4-0) | 12.6 → **19.3** | 15 | 30 (20 / 4.9 / 1.3 / 1) | 10.9 (4.8) | 2.5 | 49 |
| Lab 1 (5-0) | 10.6 → **17.5** | 15 | 34 (21 / 5.0 / 1.4 / 3) | 11.1 | 2.8 | 60 |
| Halloween 1 (6-0) | 13.5 → **22.2** | 13 | 27 (16 / 5.5 / 1.2 / 1) | 11.0 (4.1) | 1.6 | 45 |
| Retro 1 (7-0) | 28.7 → **45.6** | 8 | 12 (7 / 2.1 / 0.8 / 0) | 6.9 (3.4) | 0.7 | 25 |
| frontend (8-0) | 16.5 → **25.5** | 8 | 26 (13 / 5.2 / 1.7 / 3) | 6.8 (3.4) | 1.1 | 41 |

Lab 1's last window is a game over (mode 0), so its row uses the earlier windows. Garden draw
phases (µs/frame): actors0 7.8 ms, actors1 1.6, shadows 0.9, sprites 1.0, overlays 1.2,
refill 0.4, snapshot 0.5. Host tests pass. Screenshots: `target/n64/bench-m3/` (not in the repo).

## 3A: audio, 2D submit and frame stability (2026-10-12)

Branch `worktree-agent-a34484d17ec254d7b` (from `350099b`). Scope: the per-frame CPU outside
the simulation and the 3D draw loop — the mixer, the CPU 2D batches (`render_cpu.rs`, the 2D
half of `csrc/main.c`), frame-time spikes and the frame limit.

### Before / after (ares v148, `level-hop W 0`, 40 s, last 3 windows; mean of 3 code layouts)
Every row is the mean over three builds that differ only by `-DF2N64_CODEPAD=0/1024/3072`
(padding at the end of the hot flush region): a single build's fps moves by up to 10 % with
code layout alone (below). Before = `350099b` with libdragon-3p, after = this branch with
libdragon-3a at `1007c9d` (`6545f29`'s perspective-triangle change measured separately).

| level | fps | mixer CPU | mixer / frame | CPU-side submit (C tris + rects) | worst frame |
|---|---|---|---|---|---|
| Garden 1 | 28.0 → **28.7** | 101 → **47 ms/s** | 3.6 → **1.6 ms** | 1 ms (1.5 → 1.3 ms) | 36 → 36 ms |
| Space 1 | 16.3 → **17.0** | 122 → **56 ms/s** | 7.5 → **3.3 ms** | 3–4 → 3 ms (4.3 → 3.4 ms) | 62 → 61 ms |
| Lab 1 | 17.3 → **19.0** | 131 → **65 ms/s** | 7.6 → **3.4 ms** | 3 → 2 ms (3.5 → 2.8 ms) | 124 → 75 ms (first gameplay window) |
| frontend (8-0) | 23.8 → **25.5** | 128 → **58 ms/s** | 5.4 → **2.3 ms** | 3 → 2 ms (3.4 → 2.7 ms with `6545f29`) | 44 → 43 ms |

Lab's last windows include the game over and the frontend load; its gameplay windows after:
18.2–19.5 fps; the game-over → frontend load frame 670 → 575 ms. The perf line's `audio poll`
(only the poll after each tick: the 1.0 / 2.1 / 2.8 ms quoted for this task) is now 0.5 / 1.3 /
1.5 ms per tick in Garden / Space / Lab; the whole mixer cost is the new `audio N us/frame`
field and the `audio:` line.

### Audio (libdragon-3a, `tools/n64/libdragon-3a.sh`)
`-DF2N64_AUDIO_PROF` (libdragon's own mixer profile slots, enabled with `profile_init` +
`__mixer_profile_init`) showed per Lab frame (55 ms, 8.3 ms in the mixer) 18 mixer rounds,
where a round may be 512 samples: ~110 samples per round. Cause: a streamed channel's round is
capped by the contiguous window its sample ring can hand the RSP, computed from the fixed
128-unit margin — for the 16-bit stereo ULC music (one unit = one sample) 111 samples at
32 kHz, although ULC rings already carry a 1024-sample mirrored tail. The per-channel
bookkeeping of every round (frequency sync, window fetch, volume ramps, MIX_CHANNEL commands;
10–12 channels in Lab) ran ~290 times a second instead of ~63.
- **Round cap from the ring's real mirrored tail** (`mixer_refresh_max_ns`, and the matching
  `samplebuffer_get` assert): Lab 157 → 85 ms/s, Garden 110 → 56 ms/s. The mixed stream is the
  same (the ucode already mixes 512-sample rounds for every other channel; the window mirror
  copies up to the tail size, which ULC rings allocate anyway).
- **ULC read-ahead**: the decoder did `lseek` + `read(max_block_size)` per 1024-sample block
  (~150 µs of synchronous PI each, ~3x the average block). Now a 4 KiB window of ROM addresses
  in a cached buffer, refilled by `dma_read_async` once less than half is left ahead; blocks
  are decoded in place. `ulc_io` 144 → 73 µs per Garden frame, PI wait 179 → 6 µs. Checked with
  a self-test build comparing every block with the old read (1536 blocks in Garden and in the
  frontend, 0 mismatches; `F2_ULC_RA_CHECK` in the patch).
- Left (Lab, 3.9 ms per 52 ms frame): ULC coefficient unpacking ~0.9 ms (≈530 µs per stereo
  block; the RSP does the IMDCT), the VADPCM effects' sample buffers ~0.7 ms and prefetch
  0.45 ms, MIX_CHANNEL / volumes 0.75 ms, round preparation 0.25 ms. The effect voices are
  what separates Lab (10 voices, all audible: the new `(N silent)` count is 0) from Garden (3).

### 2D submit
Space's 90–100 CPU triangles: 44 are the ground shadows (`ai_circle`, 22 projected quads), 56
the additive lightning (`electric.bmp`), 4 the HUD clock; its ~57 rects are coins, the
copper/chrome sprites and the HUD. A CPU triangle cost ~27 µs (3 x TRIANGLE_DATA + TRIANGLE,
nine float → int conversions per vertex, cold `Tri` loads).
- **Triangle-data slot reuse** (`fast_tri_shared`): the rdpq overlay keeps its three
  TRIANGLE_DATA slots in its saved DMEM state and the triangle command only sorts pointers to
  them, so a vertex already in a slot is not sent again (a quad's second triangle sends one).
  Space 294 → 190 vertices converted per frame; with a per-vertex colour cache (four
  float → int conversions and the additive split's divisions) C-side triangles 2.7 → 2.1 ms.
- **Perspective CPU triangles** (`6545f29`): `rdpq_triangle_rsp`'s w / 1-w conversion copied
  (`f2_s16_16`), so non-tiled triangles with rhw ≠ 1 skip the float vertex + libdragon set-up
  as well: frontend 2.7 → 2.0 ms.
- Rects: primitive colour / depth only when they change within a batch.
- `update_argb4444` (procedural refill) wrote each texel as an swl/swr pair (RGBA32) or two
  byte stores (RGBA16) through `[u8; N]` chunks; now one aligned word store (two RGBA16 texels
  per store).
- `draw_segment` polled the mixer unconditionally every 16 batches, so a whole mixer refill
  (1–3 ms) could be booked as "CPU-side submit"; now `audio_poll_due` (the 40 ms rule).

The ≤ 1 ms target is not reached in Space / Lab / frontend. Left: ~20 µs per CPU triangle and
~15–25 µs per rect — libdragon's command building, not memory (`rdpq_set_prim_color` alone
costs 256 CPU cycles, `-DF2N64_MEMBENCH`) — plus `RenderCpu::build` (~1 ms in Space, cold
`Tri` reads). Next step: the shadows and the lightning as RSP geometry (they are world-space
quads the CPU projects today; f2-draw / r3d, outside this slice).

### Frame stability: spike sources
New diagnostics: `spike` lines (a main-loop period > 50 ms and > 1.5x the running average, or
the first frame after a load, with its breakdown), `slow cpu batch` (> 2 ms), `cpu texture`
(conversions), `display` (vblanks each image stayed on screen). Found (Lab hopping, frontend):
- **First use of a texture in a CPU batch converts it on the CPU**: the HUD clock's 128x128 CI8
  hand / head → their downscaled copies (35 ms each: with a 3.8 ms glyph batch the 124 ms Lab
  frame), an effect's IA8 64x64 additive variant (8 ms), r3d's TMEM copies of ~40 64x64 CI8
  textures at level start (~2.4 ms each). `tex_fetch` per texel was ~2 µs. Now per-format decode
  tables (16 / 256 values), word loads/stores, SWAR box sums and exact reciprocal divisions
  (same texels; the division identities checked exhaustively on the host), and the small copy
  shares r3d's identical TMEM copy: clock 35 → 6 ms, additive variant 8 → 5 ms, the Lab spike
  124 → 73–77 ms. Still ~0.4 µs per CI8 texel; doing these conversions at level load (or in the
  pack) would remove the rest.
- **Text appearing**: glyph sub-rect uploads of the 214-texel-wide font atlases miss the
  upload-block cache (2.5–5.6 ms for 12–32 new glyphs). A sub-rect seen once is now uploaded
  directly (second-chance admission) instead of being recorded as an rspq block that evicts a
  block in use; the batches are still 2.4–3 ms (the uploads themselves).
- **The story book** (frontend / game over): the 256x256 page texture is drawn as triangles
  through TMEM tiles, 64 tiles x 2 triangles, each with an upload, clipping and `rdpq_triangle`:
  ~16 ms per frame while the book is shown, 55–125 ms in its first frame. Not changed (not
  gameplay). Next: a tile-major loop over the batch (one upload per tile), 62x31 tiles (TMEM is
  full at 63x32 with the filter border: 45 instead of 64 tiles), the fast vertex path.
- **Level transitions**: the game-over → frontend frame is the frontend's level load inside
  `Game::tick` (~480 ms) plus 52 ms of texture conversion (was 122) in its first draw.
- Steady state: mixer refills no longer pile into one frame's submit; in gameplay the longest
  main-loop period (`longest loop` in the perf line) is the worst frame + ≤ 3 ms.

### Frame limit (30 / 20 fps setting)
`display_set_fps_limit(30)` shows frames only on every 2nd vblank at a fixed phase: with 3
framebuffers the average rate holds (it does not fall to 20 fps), but a frame that misses its
slot by a millisecond leaves the previous image up for 4 vblanks (66 ms). Garden with the limit
(`-DF2N64_FLIP_LIBDRAGON`, `F2N64_FRAME_LIMIT=30`): per 5 s, 132–136 images shown 2 vblanks and
7–9 shown 4. Now (default) the display flips on any vblank (`display_set_fps_limit(0)`), and
the RDP-done callback or a vblank handler hands a finished frame to `display_show` no earlier
than 2 vblanks after the last flip (3 at 20 fps): a late frame costs one vblank (50 ms) and the
cadence continues from there. Same Garden test: 104–110 images shown 2 vblanks, 27–30 shown 3,
none 4 (that build ran at 27 instead of 28.5 fps for code-layout reasons, hence more late
frames). The trade: when the frame time hovers around 33 ms, a few 66 ms holds become more
frequent 50 ms ones; an isolated late frame is always better off. FMV playback still sets its
own limit, and the game's is restored after it.

### Measurement notes
- **Code layout moves a build by up to 10 %**: the same commit padded by 0 / 1024 / 2048 /
  3072 bytes (`-DF2N64_CODEPAD`): Space 16.4 / 15.9 / 15.5 / 17.5 fps (actors0 14.6 / 13.5 /
  11.9 / 11.3 ms); Garden 27.6 / 28.8 / 29.8 fps (0 / 1024 / 3072). This branch's first full run
  looked 2 fps slower than its audio-only commit in Garden for that reason (an old build padded
  by the same 2.3 KB was not slower). Compare builds over several layouts (`F2N64_PADX` for
  data, `CODEPAD` for code).
- ares v148 memory costs (`-DF2N64_MEMBENCH`, CPU cycles per access in a loop): uncached
  32/64-bit store 5 / 4, uncached load 5, cached access with a line miss every 4 words ~22.
  Real hardware has much slower uncached accesses, so trading uncached for cached accesses can
  look worse in ares than it is on the console (the scratch experiments below).

### Tried and dropped
- ULC coefficient unpacking with the bit cursor in registers and a magnitude table (host test:
  identical over 96 M coefficients): no gain (581 vs 580 µs per Garden frame); decoding into a
  cached scratch + memcpy: slower in ares (807 µs). The loop is already ~24 cycles per
  coefficient.
- Column-sum scratch arrays for the texture box filters: slower in ares (D-cache conflicts).
- Preloading (resident) VADPCM effects: Lab opens 26 looped samples (~850 KB as VADPCM) and the
  frontend runs at 5.1 of 5.4 MB of heap; small loops are already pinned in their ring by the
  mixer.
- Muting silent voices: none of Lab's 10 voices is silent.

### Quality trade-offs measured (not shipped; owner's call)
- Mixer output at 22.05 kHz instead of 32 kHz (`-DF2N64_AUDIO_RATE=22050`): mixer CPU Lab
  77 → 74 ms/s, Garden 46 → 42 (−5–8 %); with the music re-encoded at 22 kHz the ULC unpacking
  would drop by another ~30 % of ~17 ms/s. It loses everything above 11 kHz in the music for
  ≤ 1 % of the CPU: **not recommended**.
- Mono or lower-bitrate music: ULC unpacking is ~17 ms/s for 128 kbps stereo and scales with
  the coded coefficients; halving it saves ≤ 1 % of the CPU: **not recommended**.
- The effect voices (~5 ms/s each in Lab) are the remaining audio lever; a voice cap below the
  game's would change which sounds play (parity): not proposed.

### Risks
- libdragon-3a is a third patched prefix (`~/opt/n64root-3a`); builds without it still work (a
  NOTE is printed; the mixer then costs ~2x the CPU). The round-cap change relies on a ring's
  mirrored tail being at least the window it hands out (true for every waveform: the tail is
  `max(128, append_units)` units); libdragon's asserts (compiled in) checked it in every run.
- The ULC read-ahead is one static 4 KiB window (one ULC stream at a time: the music; a second
  stream stays correct, refilling on every switch).
- Triangle-data slot reuse depends on the rdpq overlay keeping TRIANGLE_DATA in its saved state
  and the triangle command not modifying it (true at the pinned 39d0d60); the slots are
  forgotten at each batch and after any `rdpq_triangle`.
- Paced flips call `display_show` from the RDP-done interrupt (as `rdpq_detach_show` does) or
  from a vblank handler; `-DF2N64_FLIP_LIBDRAGON` restores the old path.

## 3D2: draw-loop actor cost, second round (2026-10-12)
Scope: the CPU work of `f2_draw::render::draw_actor_list` and what it calls on the N64
(`f2-draw`, `f2-engine`, r3d's recording side), worst case Space 1. Branch
`worktree-agent-a13e17187dd926045` from `350099b`. No change to `f2-game`.

### Before / after (ares v148, `tools/n64/bench.sh NAME "level-hop W 0" 40`, last windows, frog hopping)
Draw phases are the `draw phases` line (µs/frame; actor phases include the r3d recording).

| level | fps | draw | draw loop | actors0 + actors1 | shadows | sprites | r3d queue (batch) |
|---|---|---|---|---|---|---|---|
| Space 1 (2-0) | 16.1 → **21.2** | 37 → 25–26 ms | 23 → **15 ms** | 10.1 + 5.3 → 3.0 + 3.9 ms | 2.4 → 2.05 | 2.2 → 2.1 | 5.2 → 2.3 ms |
| Garden 1 (0-0) | 28.2 → **34.0** | 21–22 → 17 ms | 12–13 → **9–10 ms** | 7.2 + 1.65 → 4.5 + 1.9 ms | 0.84 → 0.74 | 1.0 → 0.9 | 4.4 → 2.9 ms |
| Lab 1 (5-0, windows before the game over) | 17.0 → **22.7** | 33–34 → 23–24 ms | 21 → **14 ms** | 5.8 + 5.5 → 1.85 + 3.7 ms | 2.4 → 2.1 | 5.3 → 4.5 | 5.2 → 2.3 ms |

The targets (Space ≤ 12 ms, Garden ≤ 7 ms of draw loop) are not reached; what is left is
listed under "Where the time is now". Space's `specfx` phase reads 1.3 → 1.8 ms in these
runs; builds with the shadow or texture change reverted show the same, so it is the
different game state at 21 fps (more effects alive), not code.

### What changed
1. **Actors wholly outside the view are left out before any work** (the big one). In a
   hopping run r3d's batch culled ~88 % of all actor draws (Space 396 of 449, Garden 328 of
   374 per frame): the draw loop interpolated, slerped, transformed and recorded ~110 actors a
   frame that showed nothing. Now each actor slot carries a radius bound of its model
   (`f2_engine::anim::model_bound`, computed once per model from r3d's mesh spheres through
   `MeshBackend::object_sphere`): rotations keep lengths and interpolated move/scale keys stay
   within their largest key, so `max|actor scale| · (|c|+r+WAVE_MAX)·max|s_o| + Σ|t_k|·max|s_k-1|`
   encloses every vertex the RSP draws in any animation pose and actor rotation (MODGE/WAVE
   included: WAVE raises a vertex by ≤ 10 object units; r3d's sphere gets one `pos_scale` step
   of slack). The test uses the current tick position with the Manhattan distance to the
   previous one (the drawn, interpolated position lies on that segment) and r3d's own frustum
   (`R3d::sphere_culled`, the planes `cull_world` uses), so no interpolation is needed for it.
   A culled actor only tells the back end how many draws it would have made
   (`MeshBackend::skip`): r3d keeps the draw slots, so later actors keep last frame's slots and
   r3d's per-slot caches. `None` (never culled): skinned models (the frog), unknown spheres,
   non-finite data, move/scale key times that do not increase (the key search could
   extrapolate). `lean` (N64) only; the PC has no mesh sink.
   - Correctness: `F2N64_CULL_CHECK=1` draws culled actors anyway with a marker bit and r3d
     checks it culls every such draw wholly. Hopping runs (30–40 s) of all 20 arcade levels that
     boot (W 0–7; 0-1, 0-2, 3-1, 3-2 do not boot in this harness): ~1.6 million draws of culled
     actors, **0 visible**. Host test `model_bound_encloses_every_pose` (random hierarchies,
     keys, rotations, scales, WAVE heights).
2. **Per-actor code in one I-cache region**: `draw_actor_list`, `draw_listed_actor`,
   `quat_slerp`, `sin_fast`/`acos_fast` joined `.text.f2hot_d` (f2-draw's `forbid(unsafe_code)`
   is `deny` on the N64 only, like f2-game); rare paths out of line (slot creation, override
   texture CRC, portrait Gouraud, skinned recording, draw-list builds); `actors[id]` without
   f2-game's out-of-line `Index`. The sin/acos tables are read through a relaxed pointer on
   the N64 (`OnceBox::get` cost a `sync` per lookup and its init was inlined four times into
   `quat_slerp`, 2 KiB → 0.6 KiB). Space actors 15.4 → 13.3 ms on its own. The loop's per-actor
   code is now ~13 KiB contiguous from `queue_raw` (Makefile comment updated).
3. **One back-end call per actor** (`MeshBackend::actor`, `ObjDraw` list cached per actor and
   flags; `R3dLevel::record_actor` resolves the override texture once). Same draws, slots and
   queue entries. Small on its own (−0.2 ms).
4. `xform_actor_cached` split: the key test inline, `pose_update` out of line with one
   `L × R` loop (`rotl` kept as before: second unchanged rotation); `Vec` resizing moved to the
   hierarchy rebuild.
5. Animated hierarchies (`transform_object`): the keyframe search tests the hinted pair first
   (`key_search`; the original's search starts at `low = e_key, high = e_key + 1`, so the
   result is identical for any key array: host test `key_search_matches_find_to_from`);
   `lean`: a non-unit object scale scales the rows instead of a full `mtx_cat` with
   `gu_scale` (equal up to the sign of zeros, `lean_transform_matches_full_products`);
   `xform_into` no longer refills the output with identities. Synthetic 10-object model:
   6570 → 6120 cycles per object.
6. Shadows (`lean`): `rot · (translate · I)` built directly (−0.3 ms Space);
   `TextureRegistry::entry` has a 64-slot CRC → index memo (`lean`; exact: entries are only
   replaced in place).
7. Culled path made cheap: `ActorSlot` is `repr(C)` with what a culled actor reads first
   (model, bound, draw count, last snapshot); the cull test uses `pos × 0.1` (no divisions).

PC frames: `frogger --headless --frames {90,300} --level W 0 --profile {authentic,enhanced}
--size 320x240` for W = 0,1,2,3,5,8 (24 PNGs) byte-identical between `350099b` and the head.
Host tests: `cargo test -p f2-draw -p f2-engine` (and `-p f2-engine --features lean`),
`cargo +nightly test` in `crates/frogger-n64`. N64 captures before/after (Garden, Space,
Lab; `target/n64/bench-d2/`, not in the repo): same scenery, actors at the screen edges
present; differences are animation phase and timers.

### Measurement notes
- ares costs measured in a boot-time microbenchmark (cycles): `mul.s`/`add.s` ~5 (dependent
  or not), `div.s` ~29, `cvt`/`trunc` ~5, `mult` ~5 (not 25 in this context), `lw`/`lwc1`
  hit ~2, D-cache read miss ~40, **store miss ~80** (fill plus the dirty victim's write-back),
  I-cache line miss ~40–50, call+return ~19, `sync` ~6. A warm second run of the same
  per-actor code took 65–70 % of the first, i.e. instruction count and misses both matter.
  Real functions (warm): `quaternion_to_matrix` 240, `quat_slerp` ~700, `mtx_cat` 510,
  13-float array compare ~270. (A first microbenchmark charged `quat_slerp` 2500 cycles: the
  lazy table build landed in its timing loop.)
- `F2N64_CULL_CHECK` builds are for checking only (they draw everything).

### Tried and dropped
- Per-object pre-cull at record time (each object's mesh sphere through its matrix against
  r3d's frustum before queueing): r3d's batch got 0.15–0.65 ms faster, the draw loop 0.4–0.8 ms
  slower (Garden +0.15 ms net). Recording a draw costs about as much as testing it.
- Reusing the simulation's matrices: `f2-game` only computes the root object's matrix (for
  the bounding-box test) at tick time; the draw side interpolates, so nothing to share.

### Where the time is now / next
- Space 1 (15 ms): actors 7 ms (the frog alone ~1.25 ms: 18 animated bones at ~5000 cycles
  each; ~15 visible actors at 60–100 µs; ~110 culled at ~10–15 µs), shadows 2.1, sprites 2.1,
  specfx 1.8, overlays 1.2. Garden (10 ms): actors 6.4 (many visible multi-object actors; after
  the actor test r3d still culls ~2/3 of the recorded draws, mostly objects of partly visible
  multi-object models), overlays 1.1.
- Fx: every `push_tri` writes a 152-byte `Tri` (~1000 cycles of store misses): ~160 a frame in
  Space and Lab ≈ 1.5–2 ms across shadows / sprites / specfx. A compact console quad/sprite
  record (consumed by `render_cpu`) is the next big item there.
- Animated hierarchies: `quat_slerp` + keys + two products per object; caching the slerp's
  per-key-pair setup would save ~400 cycles per object.

### Risks
- The actor test is only as good as r3d's mesh spheres and frustum (checked in ROM by
  `F2N64_CULL_CHECK`, all levels); a back end that changes how far vertices move (a new
  vertex animation like WAVE) must widen `model_bound`.
- **Memory**: in a Space 3 (`level-hop 2 2`) run the main heap fills during the level on the
  baseline too (`bigpool: main heap full, small blocks from the pool`, ~4000 small blocks);
  on the head the frontend load after the game over then failed to place `titles.ntx`
  (439 KB) in the fragmented pool and asserted. Heap use is not higher (level peak 3057 vs
  3141 KiB Rust); the code grew ~10 KiB, the allocation pattern moved. Pre-existing
  fragility (see 3R's Lab game-over note) that this change exposes in that run; needs a fix in
  the pool / level-load path.
- Timing is layout-sensitive (±5–10 %); the numbers above are single builds per level.

## 3S3: simulation tick, third round (2026-10-12)

Branch `worktree-agent-a08011cde1a4eb6cc` (from `350099b`). Goal: `Game::tick` Space 1 ≤ 10 ms,
Lab/Subterranean/Halloween 1 ≤ 8 ms, Garden 1 ≤ 6 ms, bit-identical. Result: **−14…−22 %**
(Space 14.7 → 12.5 ms, Lab 10.6 → 8.9, Subterranean 10.5 → 8.5, Halloween 10.5 → 8.2,
Garden 8.5 → 7.3). The targets are not reached.

### Result (ares v148, `level-hop W 0`, 80 s runs, mean of the windows after the first two; µs per tick)

Both rows of a level are builds made in this worktree on the same day (`base` = `350099b`),
run in parallel ares instances. Lab 1 ends in a game over after ~15 s, so its row is one window.

| level | | tick | actordraw | enemies | platforms | specfx | draw2d | loop-rest | frogs | events | draw3d | fps |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Space 1 (2-0) | base | 14 672 | 5 873 | 2 852 | 1 817 | 1 237 | 1 189 | 700 | 373 | 332 | 258 | 16.0 |
| | after | 12 542 | 5 483 | 1 864 | 1 625 | 959 | 1 106 | 593 | 288 | 377 | 215 | 16.5 |
| Lab 1 (5-0) | base | 10 613 | 5 021 | 2 364 | 737 | 192 | 675 | 870 | 425 | 33 | 260 | 16.9 |
| | after | 8 938 | 4 762 | 1 489 | 580 | 165 | 673 | 623 | 321 | 44 | 242 | 17.8 |
| Subterranean 1 (4-0) | base | 10 457 | 4 438 | 766 | 888 | 614 | 741 | 1 459 | 753 | 448 | 310 | 18.4 |
| | after | 8 509 | 4 030 | 544 | 790 | 568 | 616 | 740 | 458 | 493 | 227 | 19.8 |
| Halloween 1 (6-0) | base | 10 508 | 3 546 | 2 468 | 1 207 | 287 | 827 | 774 | 518 | 620 | 219 | 21.9 |
| | after | 8 246 | 3 218 | 1 156 | 1 033 | 271 | 664 | 633 | 372 | 682 | 183 | 23.6 |
| Garden 1 (0-0) | base | 8 504 | 3 632 | 1 626 | 834 | 204 | 813 | 670 | 454 | 76 | 163 | 27.9 |
| | after | 7 286 | 3 316 | 1 136 | 698 | 172 | 781 | 560 | 343 | 87 | 138 | 29.3 |

Build-to-build layout noise is still ±5 % per phase (platforms moved by ±200 µs between
builds with identical platform code until the last region fixes); the "after" rows match
the last five builds of the round within that.

### Cost model (measured, ares v148)
A temporary calibration build (`F2N64_TUNE_AB` cycling through masks; 20 000 operations
per tick, Garden 1) gave per operation: `mul.s`/`add.s` 5.3–5.5 cycles (dependent or not),
`mul.d` 8.4, `div.s` 30, 32-bit integer `mult` ~4.4 (not ~25), `FMul` (64-bit product)
~13, integer `div` ~41, `i32→f32` ~7, `u32→f32` ~11, **Rust's saturating `f32 as i64`
~36**, `mtx_cat` ~520, `quaternion_to_matrix` ~240, `MakeUnit` ~376, a D-cache load miss
~47 extra and a **store miss ~85** (allocate + later write-back). Host callgrind with the
VR4300 geometry (`--D1=8192,1,16 --I1=16384,1,32 --toggle-collect='*tick_frames*'` on a
host copy of `level-hop`) counts ~10 000 D-cache misses per Space 1 tick: memory, not
arithmetic, is about a third of the tick. The sampling profiler attributes a long
branch-free block's time to the *next* block (ares checks interrupts at block ends), so
per-function numbers around calls mislead; only same-ROM A/B or whole-phase numbers are
trustworthy.

### What changed (all exact)
1. **I-cache regions** (`tools/n64/Makefile`): a new `.text.f2hot_x` between the enemy and
   platform regions holds the fixed-point maths both loops call per entity (`Orientate`,
   `MakeUnit`, `IQuatSlerp`, `fixedRotateVectorByRotation`, `MagnitudeF`, the quaternion
   helpers, `ProcessAttachedEffects`' test, the node-position lookup): e + x = 15.6 KiB and
   x + p = 12.4 KiB (the enemy region was 25 KiB and evicted itself). Cold parts moved out of
   line so the regions fit: `ProcessAttachedEffects`' effect creation (8 KiB; the test left
   in the region also skips the body for actors outside the timer/distance window — Space
   ran it for 26 far enemies per tick), `UpdatePathNME`'s node arrival and push blocks,
   `DoEnemyCollision`'s multiplayer branch, `UpdatePlatforms`' crumble and multiplayer
   parts, `UpdateFlappyThing` (Garden only), the float fallbacks of `rsin`/`rcos`/`arccos`
   (`#[cold]`: `IQuatSlerp` 5.7 → 1.8 KiB, `Orientate` 3.4 → 2.1 KiB),
   `transform_actor_sprites`' body, `find_to_from`, `calc_view_matrix`;
   `copy_and_draw_actors` is no longer inlined into `after_game_loop` (its loop shared I-cache
   lines with `copy_actor`). Every remaining out-of-region callee of the three loops is once
   per tick or rare (checked from the ELF: `jal` targets of region functions).
2. **Allocations per tick.** The N64 heap is full during Subterranean 1, where small blocks
   fall back to the big-block pool's list walk (`big_alloc` + `big_free` were 0.46 ms/tick).
   The HUD's numbers are formatted on the stack and copied into the overlays' `String`s only
   when they change (`hud::StackStr`, `set_text`); `sync_collect_text` and the time overlay
   no longer clone overlays; `take_effects` leaves a list with the last call's capacity
   (ambient sounds emit several `UpdateHandle`s per frame; a host can also `Vec::append`
   out of `Game::effects` to keep both buffers); swarm and lightning effects walk their
   particle chains in place; `ActorListDraw(1)` and `DrawSpecialFX` snapshot into reused
   buffers (`Game::scratch`). Subterranean loop-rest 1.46 → 0.74 ms.
3. **Nearest tile** (`nearest.rs`): `FindNearestTileS` scanned all ~950 tiles (a D-cache miss
   each; Halloween's rotating enemy: ~1 ms/tick). A uniform x/z grid built with the collision
   map gives the same tile: while no squared distance can wrap (checked against the bounding
   box of all centres) the scan's `DistanceBetweenPointsSS >> 12` is a non-decreasing
   function of the exact squared distance, so cells farther than the best tile in that
   measure (strictly) are skipped, and ties keep the lowest index. A replaced tile list or a
   far point falls back to the scan; debug builds compare every answer with the scan; tested
   on random maps. Halloween enemies 2.5 → 1.2 ms.
4. **No struct copies in hot loops**: `tile_ref`/`node_tile_ref` return `&GameTile` (an
   84-byte clone per call before; a static zero tile stands for NULL), `FroggerHop`'s
   per-platform scan reads three fields instead of cloning each 256-byte platform
   (Subterranean frogs 0.75 → 0.46 ms), `DoEnemyCollision` and `UpdatePathNME` read fields
   instead of cloning the enemy, and `copy_actor` compares its source copies word by word
   (`==` on the arrays had become `memcmp` calls outside the region).
5. **Node positions** (`path::NodePosCache` in `Platform` and `Enemy`, `skip` in
   `ser_order!`, not state): path platforms and path enemies reuse their from/to node
   positions while the node block generation (`PathState::touch`, called by every node
   writer) and the node indices are unchanged; debug builds recompute on every reuse.
   Same-ROM A/B: platforms −0.45…−0.73 ms, enemies −0.13…−0.35 ms.
6. **Actor pass**: the sphere's position-independent part (`t2`, radius) survives a
   position-only move (`SphereCache::rot_valid`; A/B −130 µs); the sphere test reuses the
   view-space `z` of its depth test (`xfm_point_f_z`; A/B −60…−100 µs); `object_local_r`
   has one `quaternion_to_matrix` call site.
7. **f2-math**: `utilSqrt` converts `num < 2^31` through the signed conversion and takes the
   product's integer part from the float's bits (no saturating conversion; equal for all
   2^32 inputs, test `util_sqrt_bits_all`); `MakeUnit`'s top-bit search is four compares
   (MIPS III has no `clz`; tested over all `u32`); `Random` is `#[inline]` (constant limits
   become multiplications); `DistanceBetweenPoints*` are `#[inline]` and
   `normalise_components`/`reduced_sqrt` `#[inline(always)]` (they had stopped being
   inlined). Same-ROM A/B of the first two: −150 µs in Space.
8. **Procedural textures**: the filters read `buf1` in place except for the first and last
   row (fire/candle/steam, water) or the first and last four groups (forcefield, circular),
   which go through small padded copies (before: a 1.1 KiB zeroed and copied stack buffer
   per texture per frame). Equal to the scalar references on the existing 4 000-case tests.
   Space draw2d 1.19 → 1.1 ms, Halloween 0.83 → 0.66.

### Parity
- `demo_trace 0 0 / 1 0 / 6 1` and the 102 `f2-parity check --corpus all` Rust traces are
  sha1-identical to `350099b`'s (105/105 files; the check: 102 ok, 0 regressed).
- Host simbench checksums unchanged (frontend `7054b682…`, 0-0 `5aff35da…`, 2-0
  `9fd3d559…`, 2-2 `29f4158d…`, 4-1 `4e0873f9…`, 6-0 `c0075ee8…`, demos `2d7f69cc…`,
  `dc71fff9…`, `b9bfab76…`); level-hop checksums (2 frames per tick, 600 ticks) unchanged
  on 0-0, 2-0, 4-0, 5-0, 6-0; debug runs (cache and grid asserts) on 1-0, 2-0, 2-2, 4-0,
  6-0, 6-1. Not rerun: the N64 = host simbench comparison (needs an `F2N64_DATA=prep` DFS).
- f2-game and f2-math tests pass (debug), clippy `-D warnings`, the `thumbv7em` no-std check.

### Tried and dropped
- Key times in a separate array per channel (one search instance): +0.5…1.4 ms actordraw;
  the times array is one more D-cache miss per channel.
- `quaternion_to_matrix` out of line: the 64-byte matrix returned through memory cost more
  than the I-cache it saved.
- Bit-level float→int conversions for `sin_fast`/`acos_fast`/`ftol16`/`oozd` (equal on all
  f32 and dense f64 tests): A/B 0.
- Writing `SphereCache::src_*` only when changed: A/B 0.
- An 8-pixel palette blit: the blit is bound by the 2 KiB surface write misses; the surface
  is serialized game state, so it stays.
- A per-platform `IQuatSlerp` set-up cache and an `Orientate` memo: estimated ≤ 50 µs each
  after their extra cache lines; not built.

### Where the time is now / next
- Actor pass (3.2–5.5 ms): ~10 D-cache lines per actor per tick in `copy_actor` and the
  sphere test (~2.5 ms of misses in Space by callgrind) and the float work of moving visible
  actors (root object matrix + bounding box ≈ 3 200 cycles each, ~13 per tick in Space;
  their object-local part could be cached for static-key models, ~50 µs). The s region is
  16.0 KiB, at the I-cache limit: new per-actor code needs room first.
- Enemies/platforms: `MakeUnit`/`Orientate` per path enemy (three integer divisions each)
  and entity memory. Space's lightning makes 160 `MakeUnit` calls per tick (~0.5 ms).
- `FindNearestTileS`-like scans elsewhere, events (`snap_at_flag` walks all platforms and
  enemies each tick: ~0.3 ms in Subterranean).

## After the 3A + 3D2 + 3S3 merges (2026-10-12, `e9c7447`)
Same benchmark (`bench.sh m4-wW "level-hop W 0" 40`, clean build, frog hopping, last 3 windows).
fps: first measurement this push → after 3S2/3E/3R → now.

| level | fps | sim | draw (loop / r3d queue / flush) | Game::tick | audio / frame | worst frame |
|---|---|---|---|---|---|---|
| Garden 1 | 15.4 → 28.4 → **37.2** | 8 ms | 17 (9 / 2.9 / 1.5) | 6.9 ms | 1.3 ms | 29 ms |
| Ancients 1 | 19.0 → 29.4 → **42.7** | 6 | 15 (8 / 1.9 / 1.5) | 5.4 | 1.3 | 25 |
| Space 1 | 10.2 → 16.6 → **24.3** | 13 | 25 (14 / 2.3 / 1.5) | 11.8 | 2.5 | 42 |
| City | 39.8 → 59.0 → **59.8** | 4 | 11 (3 / 3.1 / 0.8) | 3.0 | 0.9 | 18 |
| Subterranean 1 | 12.6 → 19.3 → **29.7** | 11 | 20 (12 / 1.9 / 1.3) | 8.4 | 2.4 | 35 |
| Lab 1 | 10.6 → 17.5 → **26.6** | 11 | 24 (14 / 2.4 / 1.3) | 8.5 | 3.3 | 47–50 |
| Halloween 1 | 13.5 → 22.2 → **35.4** | 9 | 17 (9 / 2.2 / 1.2) | 8.0 | 1.6 | 31 |
| Retro 1 | 28.7 → 45.6 → **59.8** | 6 | 9 (5 / 1.0 / 0.9) | 5.4 | 0.8 | 19 |
| frontend | 16.5 → 25.5 → **29.5** | 6 | 25 (12 / 5.0 / 1.6) | 5.5 | 2.0 | 38 |

Lab 1's last window contains the game-over load (565 ms frame), excluded above. No assertions
or panics in the sweep; host tests pass. Open: Space/Lab/frontend still above 33 ms; the
Space 3 game-over → frontend out-of-memory risk (3D2); 2D submit (3A); book overlay (3A).

## Stretched-triangle slivers after audio preemption (2026-10-13)
Symptom: thin triangles from the top-left corner of the screen to scene vertices, drawn over
everything, a few frames at a time (worst at the start of Lab 2 "Research Facility", also near
the camera elsewhere). The PC build does not show them.

Diagnosis (`F2N64_FREEZE`, `-DF2N64_NO_MIXER`, see tools/n64/README.md): in one frozen scene
the sliver came and went (2-3 shots in 20, different triangles each time), so a race, not
geometry (every F2NM batch only uses the cache slots it loads: `rmodels::batch_tests`). With
no mixer rounds: 0 in 32. Mechanism: the mixer/ULC run as high-priority rspq bursts that can
preempt the magma queue between a group's vertex load and its triangles. rspq then reloads the
magma *core* overlay, whose IMEM overlay region holds the stock bootstrap; a triangle needing
guard-band clipping before the next vertex load enters the core's `_MG_CLIPPING`, which loads
`MG_CLIPPING_TEXT` = libdragon's stock clipper. That clipper clips near-plane faces MDX drops
and projects new vertices with magma's viewport (never set here: screen 0,0, z 0).

Fix: `n64_r3d_init` points `MG_CLIPPING_TEXT`/`_SIZE` in `rsp_magma`'s data image (what rspq
reloads) at `rsp_f2clip` (asserted to replace the stock value; DMEM offset from the Makefile's
`r3d_rsp_syms.h`). Frozen scene with audio: 0 in 40 shots; level start unfrozen: 0 in 32.

Cost: none (one pointer). `bench.sh NAME "level-hop W 0" 40`, last 4 windows, before → after:
Garden 1 36.8 → 37.9 fps, Ancients 1 43.1 → 43.5, Space 1 23.7 → 24.3, City 59.8 → 59.8,
Subterranean 1 30.4 → 30.7, Lab 1 (first two windows) 25.9 → 27.0, Lab 2 (`5 1`) 28.0 → 28.4,
Halloween 1 36.3 → 37.3, Retro 1 59.8 → 59.8, frontend 30.9 → 31.2; r3d flush 0.1-0.3 ms
lower everywhere. `mem-tour`: all 26 steps, no ALLOC FAILED, stack guard untouched.
