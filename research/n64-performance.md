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
