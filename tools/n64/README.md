# N64 build tools (phases 1B, 2G)

Builds the Nintendo 64 ROM of the port (`crates/frogger-n64`, a Rust `no_std` + `alloc`
static library linked into a small libdragon C host) and runs it in ares. Plan and
budgets: `docs/N64-PLAN.md`; measured numbers: `research/n64-performance.md` (§2G for the
playable build).

| File | Purpose |
|---|---|
| `build.sh` | full build → `target/n64/frogger2.z64` (game data pre-pass + ROM) |
| `mips-nintendo64-none.json` | rustc target spec: VR4300 (MIPS III), big-endian, **n32** |
| `cargo-config.toml` | `-Zbuild-std=core,alloc`, JSON target specs (passed with `--config`) |
| `n32-to-o64.py` | relabels the Rust objects as o64 so GNU ld links them with libdragon |
| `Makefile` | libdragon link + ROM (`n64.mk`): title `FROGGER 2`, Expansion Pak *required* |
| `gen-probe-data.py` | synthetic 1 MiB level-like file for the DFS/decompression probe (no game data) |
| `run-ares.sh`, `ares-shot.ps1`, `ares-settings.py` | ares runner: screenshots, key presses, debugf log |
| `prof.py` | maps the sampling profiler's `PROF` log lines to functions (`addr2line`) |
| `../f2-n64prep` | host pre-pass: game files → DFS layout (postcard object banks, split sound banks) |

## Prerequisites
- libdragon **preview** (pinned `39d0d60`) + its toolchain at `N64_INST`
  (default `~/opt/n64root/opt/libdragon`: GCC 16 `mips64-elf`, `mkdfs`, `mkasset`, `n64tool`…).
- Rust nightly with `rust-src`: `rustup toolchain install nightly --component rust-src`.
- ares (v148 tested) on the Windows side, reached through WSL interop.

## Build and run
```sh
F2_GAME_DIR=/path/to/game-data/usa/game tools/n64/build.sh   # the game (needs your PC data)
F2N64_BOOT="level 0 0" tools/n64/build.sh   # boot straight into Garden 1 (arcade)
F2N64_KEEP_DFS=1 tools/n64/build.sh         # code-only rebuild, reuse target/n64/filesystem
tools/n64/build.sh --bringup                # phase-1B demo + probes instead of the game
tools/n64/run-ares.sh                       # boots target/n64/frogger2.z64, 1 screenshot after 8 s
tools/n64/run-ares.sh target/n64/frogger2.z64 -- -delay 28 -shots 8 -gap 5 -hold 900 -keys "Right;;Up;;X"
cargo +nightly test --manifest-path crates/frogger-n64/Cargo.toml   # host: portable modules
```
`build.sh` steps: cargo (`--target tools/n64/mips-nintendo64-none.json`, target dir
`target/n64/cargo`, features from `F2N64_FEATURES`) → `n32-to-o64.py` + `ranlib` → DFS root
`target/n64/filesystem` → `make -f tools/n64/Makefile` (n64sym, n64elfcompress, n64tool
`--toc`, 16 KiB padding, ed64romconfig header) → size check (fails above 64 MiB; warns above
0x3FF0000 bytes, where ares disables ISViewer logging).

### Game data (phase 2G; derived data, never committed — it lives in `target/n64/`)
`tools/f2-n64prep <game dir> target/n64/filesystem target/n64/wav` lays out the DFS:
`maps/ collision/ scenics/ demos/ textures/` copied (lowercase); `objects/<x>.dll` →
`objects/<x>.dll.pcd` (the parsed `ObjectBank`, postcard: the N64 never parses PE files);
`sfx/<x>.wus` → `sfx/<x>.wus.meta` (the bank without PCM, postcard) + one WAV per sample,
which `audioconv64 --wav-compress vadpcm,huffman=false` turns into
`sfx/<x>/<uid:08x>.wav64` (VADPCM decoded on the RSP, streamed from ROM). Music:
`F2N64_MUSIC` tracks (default `02 09 10 12`) from `F2_MUSIC_DIR` (default
`$F2_GAME_DIR/../music`) → `music/trackNN.wav64` (22 kHz VADPCM). `mkasset -c 1` (LZ4)
compresses `objects/ collision/ maps/ scenics/` (`asset_load` decompresses transparently).
USA data: 36 MiB ROM. The compressed pack of phase 1C replaces this layout behind
`crates/frogger-n64/src/data.rs::AssetStore`.

| env | effect |
|---|---|
| `F2N64_BOOT` | `frontend` (default; training marked done like an existing save, since there is no save medium yet), `frontend-fresh`, `level W L` |
| `F2N64_FEATURES` | `profile` (2 kHz sampling profiler, `PROF` lines with every perf log), `profile-sim` (sample inside `Game::tick` only), `game-log` (the simulation's `Effect::Log`) |
| `F2N64_KEEP_DFS=1` | skip the data pre-pass |

Log lines (debugf → `C:\f2-n64test\run.txt`): `perf [W-L] ...` every 5 s (fps, sim ticks/s,
dropped ticks, per-frame sim/draw/RDP time, `Game::tick` µs, triangles, audio voices, music),
`heap [W-L] ...` (Rust live/peak, libdragon heap used/total, N64 texture bytes; level peak on
every level change), `game: ...` on mode or pad changes, `app: level ... loaded in N ms`.
Profiling: build with `F2N64_FEATURES=profile` (or `profile-sim`), run, then
`python3 -I tools/n64/prof.py /mnt/c/f2-n64test/run.txt target/n64/build/frogger2.elf`.

### ares harness
`run-ares.sh` writes `C:\f2-n64test\settings.bml` (from ares' own `settings.bml` next to
`ares.exe`) with: Expansion Pak on, **HomebrewMode on** (ISViewer → ares stdout, so libdragon
`debugf` lands in `C:\f2-n64test\run.txt`), status bar off, 640×480 window, unfocused input
blocked, and the virtual pad bound to keys (ruby keyboard id `0x1`, rawinput key index):
arrows = D-pad, `X` = A, `Z` = B, `Return` = Start, `PageUp`/`PageDown` = L/R, `Home` = Z,
`I/K/J/L` = stick. ares is started as
`ares.exe --settings-file <bml> --system "Nintendo 64" --no-file-prompt <rom>`.
`ares-shot.ps1` (adapted from the PS4/PS5 harnesses) captures the client area
(`<name>.png`, `<name>_1.png`, …) and holds the keys of step *i* (comma separated
`System.Windows.Forms.Keys` names, `;` between steps, empty step = no key) for `-hold` ms
(default 400; the game runs at ~4 fps in ares, use 900) before capture *i*
(`<name>_<i>held.png` is taken while they are held). Pad mapping in the game
(`src/input.rs`): A = Cross (hop), B = Circle (tongue), Z / C-Left = Square (croak),
C-Up = Triangle, C-Down = Select, L/R = L1/R1, Start, D-pad and stick = directions; e.g.
frontend → Garden 1: `-keys "Right;;Up;;X"` (hop to the choice pad, up to the story book, A).

## ABI
libdragon is compiled with GCC `-march=vr4300 -mabi=o64`: MIPS III, 64-bit GPRs, FR=1
(32 × 64-bit FPRs), **32-bit pointers and `long`**, `.module fp=64 oddspreg`. LLVM cannot
generate o64, so Rust targets **n32**, the closest LLVM ABI — same ISA, same register widths,
same FPU mode, same 32-bit pointers, same sign-extension convention. The generated code is
ABI-neutral at the instruction level; only the calling convention and the ELF labelling
differ:

| | o64 (C) | n32 (Rust) | bridged by |
|---|---|---|---|
| int/pointer args 1–4 | `$a0–$a3` | `$a0–$a3` | identical |
| args 5+ | stack (8-byte slots) | `$a4–$a7` (= o64 `$t0–$t3`) | **rule: ≤ 4 args** |
| float args | `$f12,$f13` only while leading, then GPRs | `$f12+i` per slot | **rule: no float args** (pass structs by pointer) |
| return | `$v0` / `$f0` | `$v0` / `$f0` | identical (64-bit ints in one register in both) |
| arg home area | callee may write 32 bytes above `$sp` | not reserved | `RUST_TO_C` stubs in `csrc/abi.S` |
| callee-saved FPRs | `$f20–$f31` | even `$f20–$f30` | `f2n64_call` saves all of `$f20–$f31` |
| `$sp` alignment | 8 | 16 | `f2n64_call` aligns |
| FPU traps | libdragon enables V/Z/O | Rust assumes IEEE non-trapping | `f2n64_call` clears the enables, restores on return |
| ELF | `elf32-bigmips`, REL, `EF_MIPS_ABI_O64` | `elf32-nbigmips`, RELA, `EF_MIPS_ABI2` | `n32-to-o64.py` |

FFI rules (`crates/frogger-n64/csrc/n64_ffi.h` ↔ `src/ffi.rs`):
- every C function Rust calls takes **at most four arguments, each a 32/64-bit integer or a
  pointer**, and returns an integer/pointer or nothing; floats only inside structs behind
  pointers. It is declared with `RUST_TO_C` in `abi.S` (the stub `n64_x` calls `n64_x_impl`);
- C calls Rust **only through `f2n64_call(fn)`** (entry points `f2n64_init`, `f2n64_frame`);
  no C callbacks into Rust (mixer/interrupt callbacks stay in C);
- never call variadic C functions from Rust (`n64_log` takes pointer + length);
- memory: the Rust global allocator is libdragon's heap (`memalign`/`free`), so buffers
  from `asset_load` can be adopted by a `Vec` and freed by Rust (it counts live/peak bytes,
  `rt::rust_heap`).

`n32-to-o64.py` rewrites each archive member: e_flags (`ABI2` → `ABI_O64`), every
`SHT_RELA` → `SHT_REL` with the addends moved in place (`R_MIPS_32`, `R_MIPS_26`,
`R_MIPS_HI16`/`LO16` with HI16s reordered before a matching LO16), `.mdebug.abiN32` →
`.mdebug.abiO64`. Any other relocation type is a hard error. Small data is disabled on the
Rust side (`-mips-ssection-threshold=0`, no GP-relative relocations).

Validation: the ROM logs the f2-math self-test, the scene.rs-style f32 transform and a
fixed-point "tick" checksum; they are bit-identical to the host (`cargo test` asserts the
same constants), and pad / DFS / audio / RDP calls round-trip through the bridge.

## Crate layout (`crates/frogger-n64`, phase 2G)
| module | role |
|---|---|
| `app.rs` | front end: fixed 60 Hz `f2-game` ticks paced by C0 COUNT (catch-up ≤ 6 ticks/frame, the rest dropped and counted), level loads, effects → audio, perf/heap logs |
| `data.rs` | `N64Data<S: AssetStore>`: the game's `DataSource` over the pre-pass layout; `DfsStore` = loose DFS files. **Phase 1C hook**: implement `AssetStore` (logical lowercase paths → bytes) over the pack |
| `render_cpu.rs` | **Phase 2R hook**: `RenderCpu::{reset, convert_image, upload, submit}`; today `Renderer::build` → `n64_draw_tris` (3D, TMEM-sized textures) / `n64_draw_rects` (screen-aligned 2D quads via `rdpq_tex_blit`, full-size textures, >1024 px wide images split in C) |
| `texconv.rs` | RGBA8 → RGBA16/RGBA32 + TMEM-fitting downscale |
| `audio.rs` | `Effect`s → mixer voices (wav64), PC `PlaySample` positional maths; music |
| `input.rs` | N64 pad → PSX pad bits |
| `rt.rs`, `ffi.rs`, `csrc/` | runtime glue and the C host (`n64_*`) |

The draw loop itself (level scene, actors, shadows, sprites, effects, overlays, text) is the
desktop's, moved to the `no_std` crate `crates/f2-draw` and shared.

## Prior art
nust64 (ELF → ROM packer), n64-heart-rust and the nintendo64-hal crate are pure-Rust N64
stacks that own the boot and hardware access and do not link libdragon; this port instead
keeps libdragon (RDP/RSP, audio, DFS, video, joybus) and only bridges the ABI.
