# Frogger 2 on the Nintendo 64 — plan

Goal: the full game (all 26 single-player levels, frontend, story, multiplayer, music,
FMV) as a **64 MB cartridge ROM** for a console with the **Expansion Pak (8 MB RDRAM)**,
built from the player's own PC data (never distributed, like the `--data` console packages).
Frame rate: 60 fps target; 15–30 fps acceptable in heavy scenes (the PSX version dipped there).

Blitz started an N64 version of the early Frogger 2 codebase (`frogger2-vss/teamSpirit/
FroggerII/N64/`) and never shipped it. This port reuses the *final* game's simulation
unchanged (parity-verified `f2-game`), so the N64 build plays exactly like the PC release.

## 1. Architecture

```
 ROM (64 MB)                         RDRAM (8 MB)
 ┌──────────────────────────┐        ┌───────────────────────────────────────┐
 │ IPL3 + ELF (compressed)  │  boot  │ code + libdragon + Rust core   ~1.5 MB│
 │ DFS asset pack           │ ─────▶ │ framebuffers 3× 320×240×16b  450 KB   │
 │  ├ global (fonts, UI)    │        │ z-buffer                      150 KB  │
 │  ├ per-world banks       │ stream │ game state (f2-game)        ≤ 0.9 MB  │
 │  ├ per-level data        │ ─────▶ │ level working set (models,    ~3.5 MB │
 │  ├ sfx banks (deduped)   │        │   textures, collision, script)        │
 │  ├ music (Opus)          │        │ audio (mixer, music stream)   ~300 KB │
 │  └ FMV (MPEG-1 / h264?)  │        │ heap headroom                         │
 └──────────────────────────┘        └───────────────────────────────────────┘
```

- **Toolchain**: libdragon *preview* branch (pinned `39d0d60`, installed at
  `~/opt/n64root/opt/libdragon`, `N64_INST`), GCC 16 `mips64-elf`; tools `mkdfs`, `mkasset`
  (built-in compression levels 1 = LZ4, 2 = aPLib, 3 = Shrinkler), `audioconv64` (VADPCM,
  Opus), `videoconv64`, `n64tool`. Test emulator: **ares v148**
  (`C:\Users\<you>\Emulation\emulators\ares-v148\ares.exe`), the emulator libdragon
  documents as accurate.
- **Language split**: a small **C host on libdragon** (boot, display, RDP, audio, joypad,
  DFS, FMV) linking a **Rust `no_std` + `alloc` static library** (MIPS III, big-endian,
  `-Zbuild-std=core,alloc`, nightly) containing `f2-math`, `f2-game`, the needed parts of
  `f2-formats`/`f2-engine` and an `n64` front-end crate. Rust owns the frame: the C side
  exposes a narrow FFI (`n64_*`: present, draw batch, audio, pad, file).
- **Simulation**: unchanged `f2-game` at the fixed 60 Hz tick (integer fixed point; the
  original ran it on a 33 MHz R3000). Parity traces must stay identical after the `no_std`
  conversion.
- **Rendering**, phase 1: reuse `f2-engine`'s faithful CPU transform (`scene.rs`, MDX
  maths) to produce screen-space TL triangles and submit them with `rdpq_triangle`
  (shade + texture + z) at 320×240, 16-bit colour, RDP blend modes matching
  Semi/Additive/Subtractive, alpha compare, fog via the blender. Phase 2 (if the CPU
  transform is the bottleneck): move transform/lighting to the RSP (libdragon's GL 1.1
  RSP pipeline or a custom ucode) for 60 fps.
- **Textures**: converted at build time to RDP formats (CI4/CI8 with TLUT, IA, I,
  RGBA16) chosen per texture by error budget, and sized/split to fit 4 KB TMEM
  (2 KB for CI with TLUT). Full-screen backdrops: 320×240 RGBA16 tiles.
- **Audio**: libdragon mixer; sound banks → deduplicated sample set (the 19 world banks
  share many samples) in VADPCM; music → Opus (RSP-decoded); positional volume/pan from
  `f2-audio`'s mixer maths.
- **FMV**: the 14 Bink movies (7.9 min) re-encoded for libdragon's video player at
  ~256×192–320×240, audio Opus; size-budgeted (see §2).
- **Saves**: Controller Pak / SRAM / EEPROM per the flashcart; the PC save block is 
  small (`MakeSaveGameBlock`).

## 2. Cartridge budget (64 MiB = 67,108,864 bytes)

Measured inputs (USA PC data): FMV 472 s (Bink 116 MB), music 1520 s (WAV 256 MB), sfx 45 MB
PCM in 19 banks, objects 19 MB (PE DLLs), textures 7.8 MB (LZSS FLA2), maps+collision+
scenics 3.6 MB.

| Asset class | Approach | Target | Measured (phase 1C) |
|---|---|---|---|
| boot + code (ELF, compressed) | n64elfcompress | 2 MB | 2.5 MiB reserve |
| models | parsed `ObjectBank`s → compact fixed-point vertices, quantised normals/UVs, delta-coded keyframes, deduplicated, then entropy-coded | 5 MB | 2.11 MB (lossless columnar F2MB + aPLib; 1.73 MB with F2Z) |
| textures + fonts + backdrops | RDP formats, perceptual palettes, TMEM-aware, deduplicated, then compressed | 6 MB | 2.75 MB (NTX1 + aPLib; 2.53 MB with F2Z) |
| levels (maps, collision, scenics, scripts, demos, text) | compact binary + compression | 1.5 MB | 0.39 MB (retail bytes + F2Z) |
| sound effects | cross-bank dedupe, resample where inaudible, VADPCM / Opus | 8 MB | 11.12 MB (all VADPCM) |
| music | Opus ~40 kbps stereo (25.3 min) | 7.5 MB | 18.50 MB (Opus 99 kbps, the maximum) |
| FMV | video ~350 kbps + Opus audio (7.9 min) | 25 MB | 19.31 MB (H.264 at the 288 kbps decoder cap) |
| headroom / higher FMV quality | | 8 MB | 9.8 MiB |

The asset packer (`tools/f2-n64pack`) prints this table from real outputs and fails the build
if the ROM exceeds 64 MiB; music/FMV are filled up to their codec caps and the rest goes to
quality upgrades. Measurements, codec comparison and formats: `research/n64-compression.md`.

### Compression ("invent something if necessary")
Per asset class, pick the best decoder that is fast enough on the VR4300 (93.75 MHz) / RSP:
- baselines: libdragon's LZ4 (fast), aPLib, Shrinkler (best ratio, slow) via `mkasset`;
- **F2Z** (our codec, to evaluate): an LZ77 + rANS coder with a **dictionary trained across
  the whole game's data per asset class** (the 26 levels share most model/texture/script
  structure, so a shared primed dictionary resident in RAM wins big on small files), plus
  **structure-aware transforms before entropy coding**: vertex/keyframe delta + zigzag,
  splitting interleaved struct fields into planes, texture palette-index planes. Decoder
  target: ≥ 4 MB/s on the VR4300, small tables.
- textures: optionally **VQ** for large RGBA textures (decoded at load), when it beats
  CI8+LZ for the same PSNR.
Ratios and decode speeds are measured on the real data and recorded in
`research/n64-compression.md`; the packer chooses per file.

## 3. RAM budget (8 MiB)
Measured by phase 1 (the plan is invalid if this does not fit): `size_of::<Game>()`, the
largest level's decoded working set (models for generic + world + level banks, textures,
collision, script), mixer and music buffers, FMV decoder buffers (FMV runs with the game
unloaded). Levels load from ROM at level start (PI DMA ~5 MB/s + decompression).

Phase 1A measurements (`research/n64-memory.md`, 32-bit pointers): `size_of::<Game>()` =
28,408 bytes on the N64 target; simulation heap ≤ 887 KiB live in any level; level working set
with the PC data structures (simulation + parsed object banks + 16-bit textures) 2.0–3.6 MiB
(max 3,649 KiB, Ancients 3). The PC loader's transient peak (whole `.wus` sample banks, up to
14 MiB) must not be reproduced: the N64 data source hands the simulation pre-extracted facts.

## 4. Phases and owners
1. **Foundations (parallel)**
   - A. `no_std` + `alloc` for `f2-game`, the used parts of `f2-formats`, and a
     render-only `f2-engine` core (scene/anim/textures registry without wgpu/png), per
     `docs/PLATFORMS.md` §5; parity corpus and demo traces identical.
   - B. N64 platform bring-up: Rust `no_std` staticlib on MIPS III linked into a libdragon
     ROM; boots in ares; draws RDP triangles, plays audio, reads the pad, reads DFS;
     CPU/RSP timing probes; ares screenshot/automation harness.
   - C. Asset pipeline + compression: `tools/f2-n64pack` (host Rust) producing the DFS pack
     from a rendition; codec evaluation incl. F2Z; budget report.
   - **B status (done, 2026-10-10)**: ROM boots in ares with the Rust core driving
     rdpq triangles (textured, Gouraud, z-buffered), joypad, mixer audio, DFS and a panic
     handler on the libdragon crash screen. ABI: Rust targets **n32** (LLVM has no o64);
     objects are relabelled o64 at build time and calls cross only a ≤4-integer-argument
     FFI with asm bridges — `tools/n64/README.md`. Measurements and the 60/30 fps analysis:
     `research/n64-performance.md` (faithful CPU render path ≈ 6100 CPU cycles per
     triangle → too slow; 30 fps needs a fused N64-specific CPU path, 60 fps RSP T&L).
   - **Phase 2R status (2026-10-11)**: RSP renderer `r3d` (libdragon magma + MDX vertex
     shader with exact BETWEEN/near/far face rejection, in-overlay texture loads, per-batch
     sphere culling, fog), F2NM format (`crates/f2-n64fmt`, `docs/n64-model-format.md`):
     ~4100 sent tris/frame @60 on the RSP in ares, Garden 1 scenes match the PC renderer —
     `research/n64-performance.md`.
2. **Integration**: `crates/frogger-n64` front end (game loop, DataSource over the pack,
   renderer on rdpq, audio, input, FMV, saves); first playable Garden 1 in ares.
   - **2G status (2026-10-10)**: boots title → frontend → Garden 1 (story book) with the
     pad in ares, sound effects (VADPCM wav64 streamed from ROM, PC positional maths) and
     music (VADPCM wav64), heap ≤ 4.2 MiB used of the 6.1 MiB libdragon heap. Data: loose
     DFS files from `tools/f2-n64prep` (object banks pre-parsed to postcard, `.wus` split
     into metadata + samples) behind `data::AssetStore` (the 1C pack plugs in there).
     Drawing: the shared `f2-draw` crate (the desktop draw loop, now `no_std`) +
     `render_cpu.rs` (CPU transform → rdpq; the 2R RSP renderer replaces it). **Blocker:
     speed** — the simulation alone costs 10–14 ms per 60 Hz tick in ares (Garden 1 / the
     frontend), so with ~150–200 ms of CPU rendering per frame the game runs at 3.5–4 fps
     and 35–40 % of real time. Numbers and profile: `research/n64-performance.md` §2G.
3. **Completeness + performance**: all levels, frontend/menus, multiplayer split-screen,
   FMV; profile and move hot paths to RSP; tune budgets to fill 64 MB.
4. **Polish**: 60 fps where possible, flashcart testing notes, docs.

## 5. Rules
Same as the repo: no game data or derived assets committed (the ROM and pack live in
`target/`/`game-data/`); parity first (simulation untouched except mechanical `no_std`);
untrusted data handled with `python3 -I`; worktrees per agent, local commits only.
