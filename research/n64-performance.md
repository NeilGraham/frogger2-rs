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
