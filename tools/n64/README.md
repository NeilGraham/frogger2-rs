# N64 build tools (phases 1B, 2G, 2R, 3I)

Builds the Nintendo 64 ROM of the port (`crates/frogger-n64`, a Rust `no_std` + `alloc`
static library linked into a small libdragon C host) and runs it in ares. Plan and
budgets: `docs/N64-PLAN.md`; measured numbers: `research/n64-performance.md` (§3I for the
game on the RSP renderer from the 64 MB pack).

| File | Purpose |
|---|---|
| `build.sh` | full build → `target/n64/frogger2.z64` (asset pack + ROM) |
| `mips-nintendo64-none.json` | rustc target spec: VR4300 (MIPS III), big-endian, **n32** |
| `cargo-config.toml` | `-Zbuild-std=core,alloc`, JSON target specs (passed with `--config`) |
| `n32-to-o64.py` | relabels the Rust objects as o64 so GNU ld links them with libdragon |
| `Makefile` | libdragon link + ROM (`n64.mk`): title `FROGGER 2`, Expansion Pak *required* |
| `gen-probe-data.py` | synthetic 1 MiB level-like file for the DFS/decompression probe (no game data) |
| `run-ares.sh`, `ares-shot.ps1`, `ares-settings.py` | ares runner: screenshots, key presses, debugf log |
| `stage-f2z-probe.py` | stages pack files + their mkasset re-compressions into the DFS root for the F2Z decode probe (`--bringup` builds; game-derived: `target/` only) |
| `prof.py` | maps the sampling profiler's `PROF` log lines to functions (`addr2line`) |
| `../f2-n64pack` | the asset pack (phase 1C/3I): levels F2Z, object banks + F2NM meshes, NTX1 textures, VADPCM sfx + index, ULC music, H.264 FMV; budget table |
| `../f2-n64prep` | phase-2G pre-pass (loose DFS files; `F2N64_DATA=prep`) |
| `export/` | `n64r-export` (own workspace): level landscape + actors -> F2NM test scene in `target/n64/filesystem/scene*.f2nm/.f2ns`, plus the PC reference PNG (f2-engine CPU renderer, same camera) |
| `compare.sh` | side-by-side + PSNR of a PC reference and an ares capture |

## RSP renderer test ROM (phase 2R)
`tools/n64/build.sh --r3d-test` (crate feature `r3d-test`) builds `src/r3dtest.rs` instead of
the game: cull check, synthetic benchmarks and the DFS scenes (`R3D` lines in the log). It uses
whatever `target/n64/filesystem` holds, so build it in a separate checkout/target or restore
the game filesystem afterwards. Typical loop:
```sh
cargo build --release --manifest-path tools/n64/export/Cargo.toml --target-dir target/n64/export
F2_PRINT_CAMERA=1 target/release/frogger --headless --frames 90 --level 0 0 --size 320x240   # prints camera/far/clear
target/n64/export/release/n64r-export --data game-data/usa --camera "-50,211.2,1181.2,-50,0,1025,0,0.1,0" \
    --far 700 --out target/n64/filesystem/scene --ref target/n64/scene_ref.png
echo 0 > target/n64/filesystem/r3d_show.txt     # pin a scene (else they cycle every 6 s)
tools/n64/build.sh --r3d-test && cp target/n64/frogger2.z64 target/n64/f2r3d.z64
N64TEST=/mnt/c/f2-n64test-r3d tools/n64/run-ares.sh target/n64/f2r3d.z64 -- -delay 30
```
Use your own `N64TEST` directory and ROM name when other agents run ares at the same time
(the default `C:\f2-n64test\frogger2.*` files are shared). Design and numbers:
`research/n64-performance.md` (phase 2R), format: `docs/n64-model-format.md`.

## Replays (PC recording -> N64)
A play session recorded on the PC replays in ares tick for tick, checked against the PC's
state checksums (`Game::checksum`, every 60 ticks): real play, not a scripted test.
```sh
frogger --practice 0 0 --mods "" --record garden1.txt     # PC: play, close the window when done
frogger --story 1 --mods "" --record story1.txt           # or story mode (its video first)
frogger --headless --practice 0 0 --frames N --pad-script garden1.txt --record check.txt   # optional: PC re-check
F2N64_BOOT=replay F2N64_KEEP_DFS=1 F2N64_REPLAY=garden1.txt tools/n64/build.sh
tools/n64/run-ares.sh target/n64/frogger2.z64 -- -delay 600 -stopon "replay done|REPLAY DESYNC"
tools/n64/replay-all.sh /mnt/c/Frogger2-rs/recordings     # or: build + run each recording, one summary line each
```
- The recording (`crates/frogger/src/record.rs`) is a `--pad-script` with `# h TICK HEX`
  checksum lines, their parts (`# p`), `# mode TICK M` lines (3 = level complete), the best-time
  name entry's input (`# n`: the PC feeds it the debounced pad, not a pad line) and `# video`.
  While recording, the PC session matches the N64's: authentic options with the console and
  settings-menu flags, no save, no mods, the default language.
- One level per recording: 180 ticks after the level is complete the recording ends and the
  game closes; a game over or a quit to the frontend deletes the recording and closes the game.
- Videos take no ticks on either build (the simulation waits), so the replay skips them after
  their first frames (`rt::set_fmv_autoskip`), as a player skipping them would.
- The ROM (`src/replay.rs`) starts the session as the PC's `--practice` / `--story` does, runs one 60 Hz
  `tick_frames(.., 1)` per game frame owed (at most `MAX_TICKS_PER_FRAME`; slower than real time
  when frames are slow), and logs `replay: tick T checksum ok`, `replay: tick T mode M`,
  `REPLAY DESYNC at tick T` with the differing parts, and `replay done: ...`.
- First results (2026-10-13): a 100 s Garden 1 run (level complete at tick 3621), all 101
  checksums equal; a story session Garden -> video 03S -> Boulder Canyon, all 126 equal.

## Prerequisites
- libdragon **preview** (pinned `39d0d60`) + its toolchain at `N64_INST`
  (default `~/opt/n64root/opt/libdragon`: GCC 16 `mips64-elf`, `mkdfs`, `mkasset`, `n64tool`…).
- Recommended (phase 3P): the libdragon variant with a 2 x 32 KiB rspq ring,
  `tools/n64/libdragon-3p.sh` (copies the prefix to `~/opt/n64root-3p` and the source to
  `~/opt/libdragon-3p`, applies `libdragon-3p.patch`, installs; the stock install is left
  alone). `build.sh` uses `~/opt/n64root-3p/opt/libdragon` when it exists (override with
  `N64_INST`) and prints a NOTE with the stock ring (the CPU then waits for the RSP: ~7 ms
  per Garden frame). After switching prefixes, `rm -rf target/n64/build` (C objects).
- Recommended (phase 3A): `tools/n64/libdragon-3a.sh` = the 3P variant plus
  `libdragon-3a.patch` (mixer rounds sized by the ring's real mirrored tail: the 32 kHz ULC
  music no longer caps every round at 111 samples; ULC compressed blocks read ahead by
  asynchronous PI DMA; profile slots `ulc_io` / `ulc_decode` / `ulc_coef`), in its own copies
  `~/opt/n64root-3a` and `~/opt/libdragon-3a`. The mixer then costs about half the CPU
  (research/n64-performance.md §3A). `build.sh` prefers `-3a`, then `-3p`, then the stock
  prefix, and prints a NOTE without the 3A patch.
- Rust nightly with `rust-src`: `rustup toolchain install nightly --component rust-src`.
- ares (v148 tested) on the Windows side, reached through WSL interop.

## Build and run
```sh
F2_GAME_DIR=/path/to/game-data/usa/game tools/n64/build.sh   # the game: pack + ROM (64,323,584 bytes with the USA data)
F2N64_BOOT="level 0 0" tools/n64/build.sh   # boot straight into Garden 1 (arcade)
F2N64_KEEP_DFS=1 tools/n64/build.sh         # code-only rebuild, reuse target/n64/filesystem
tools/n64/build.sh --small                  # pack without music and FMV (20.5 MB): quick iteration
tools/n64/build.sh --bringup                # phase-1B demo + probes instead of the game
tools/n64/build.sh --r3d-test               # phase-2R renderer test app (see below)
tools/n64/run-ares.sh                       # boots target/n64/frogger2.z64, 1 screenshot after 8 s
tools/n64/run-ares.sh target/n64/frogger2.z64 -- -delay 28 -shots 8 -gap 5   # 8 shots, 5 s apart (-gap 0: back to back)
cargo +nightly test --manifest-path crates/frogger-n64/Cargo.toml   # host: portable modules
# host render of the N64 drawing path (320x240, CPU rasteriser; --n64 = texel round trip):
cargo +nightly run --release --manifest-path crates/frogger-n64/Cargo.toml --example host_shot -- GAME_DIR out.png garden|book [TICKS] [--n64] [--cmds]
```
Status / next steps: `research/n64-performance.md` §3P (end; §3I for the pack/ROM).
Link: `tools/n64/Makefile` generates `build/f2.ld` = libdragon's `n64.ld` with the per-frame
hot code placed first in `.text` (`.text.f2hot_d`, `.text.f2hot_q`, `.text.f2hot_f` and the
listed libdragon functions, `HOT_C`): the VR4300's 16 KiB I-cache is direct-mapped, and
link-order placement made the same frame cost 2.6 or 9.6 ms after unrelated edits. Rust
functions join a region with `#[cfg_attr(target_vendor = "nintendo64", link_section =
".text.f2hot_q")]`; keep each region under 16 KiB (`__f2_hot_*` symbols in the map).
After them come the simulation's loop regions (§3S2): `.text.f2hot_s` (the per-actor pass,
`Game::copy_and_draw_actors`; keep it under 16 KiB), `.text.f2hot_e` (enemy loop) and
`.text.f2hot_p` (platform loop, `iquat_slerp`/`make_unit`); f2-game/f2-math tag functions
with `#[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ...)]`
(their `forbid(unsafe_code)` is `deny` on the N64 only). `build/f2.ld` is regenerated when
the Makefile changes.
Paths in the ROM (panic and assert texts, the crash screen's backtraces) carry no home
directory or user name: `build.sh` remaps Rust's (`crates/...`, `/rust/...`, `/cargo/...`), the
Makefile C/asm's (`-ffile-prefix-map`), and `libdragon-3a.sh` builds libdragon the same way
(re-run it once on an older install). Check: `n64symdump target/n64/build/frogger2.elf.sym | grep -c /home/` → 0.
`build.sh` also localises compiler-builtins' weak `memcpy`/`memset`/… so everything links
newlib's.

`build.sh` steps: cargo (`--target tools/n64/mips-nintendo64-none.json`, target dir
`target/n64/cargo`, features from `F2N64_FEATURES`) → `n32-to-o64.py` + `ranlib` → DFS root
`target/n64/filesystem` → `make -f tools/n64/Makefile` (n64sym, n64elfcompress, n64tool
`--toc`, 16 KiB padding, ed64romconfig header) → size check (fails above 64 MiB; warns above
0x3FF0000 bytes, where ares disables ISViewer logging).

### Game data (derived data, never committed — it lives in `target/n64/`)
Default (phase 3I): `build.sh` runs `tools/f2-n64pack --rendition $F2_GAME_DIR/.. --out
target/n64/pack --cache target/n64/cache` (`--only levels,models,textures,sfx` and
`target/n64/pack-small` with `--small`) and copies the pack to `target/n64/filesystem`. The
media encodes are cached (first run: tens of minutes for the FMV/music searches). The ROM reads
it through `crates/frogger-n64/src/data.rs` (`PackStore`: magic dispatch `F2Z1` / `DCA` / raw;
layout table in the module docs). The full ROM (61.34 MiB) stays under ares' 0x3FF0000-byte
ISViewer limit, so `debugf` works on it too.

Phase 2G layout (`F2N64_DATA=prep`, crate feature `prep-data`): `tools/f2-n64prep <game dir> target/n64/filesystem target/n64/wav` lays out the DFS:
`maps/ collision/ scenics/ demos/ textures/` copied (lowercase); `objects/<x>.dll` →
`objects/<x>.dll.pcd` (the parsed `ObjectBank`, postcard: the N64 never parses PE files);
`sfx/<x>.wus` → `sfx/<x>.wus.meta` (the bank without PCM, postcard) + one WAV per sample,
which `audioconv64 --wav-compress vadpcm,huffman=false` turns into
`sfx/<x>/<uid:08x>.wav64` (VADPCM decoded on the RSP, streamed from ROM). Music:
`F2N64_MUSIC` tracks (default `02 09 10 12`) from `F2_MUSIC_DIR` (default
`$F2_GAME_DIR/../music`) → `music/trackNN.wav64` (22 kHz VADPCM). `mkasset -c 1` (LZ4)
compresses `objects/ collision/ maps/ scenics/` (`asset_load` decompresses transparently).
USA data: 36 MiB ROM.

| env | effect |
|---|---|
| `F2N64_BOOT` | `frontend` (default: logo FMVs, title, frontend; progress from the SRAM save), `frontend-trained` (training level marked done), `save-test` (writes the save block at boot), `level W L`, `story N` (story mode at `storySequence[N]`, FMV first), `story-finish N` (testing: `story N`, level completed after 4 s, results pressed through), `frontend-start` (testing: Start pressed on the title), `settings-test` (testing: then the settings page open, FPS counter on, 30 fps limit), `book-test` (testing: then Right, Up: the story book open), `book-select-test [N]` (testing: chapters 1..N open, FMVs skipped, title pressed through quickly, the book, select at once), `mem-tour [STEP]` (testing, memory: every story level then the retro levels 7-0..7-9, each left after 240 frames by Quit, a game over or straight into the next in turn; `mem` lines at every load, research/n64-memory.md "Level heap"), `level-book-test` (testing: story Garden 1 with its FMV, Quit to the frontend after 300 frames, then Right, Up), `gameover-book-test` (testing: story Gold Mine, game over after 300 frames, then the book), `replay` (testing: plays `rom:/replay.txt`, a PC recording, see "Replays") |
| `F2N64_REPLAY` | a PC recording copied to `rom:/replay.txt` for `F2N64_BOOT=replay` (see "Replays"); builds without it carry none |
| `F2N64_FEATURES` | `profile` (2 kHz sampling profiler, `PROF` lines with every perf log), `profile-sim` / `profile-draw` (sample inside `Game::tick` / `App::draw` only), `game-log` (the simulation's `Effect::Log`), `render-cpu` (phase-2G CPU renderer), `prep-data` (set by `F2N64_DATA=prep`), `simbench` (no game: runs the `f2_game::simbench` scenarios, logs per-tick times, phases and state checksums; research/n64-performance.md §3S) |
| `F2N64_DATA` | `pack` (default) or `prep` (phase-2G loose files) |
| `F2N64_CFLAGS=-DF2N64_NO_TRIS` | measurement: r3d does all its CPU work but sends no triangles (the ceiling of polygon reduction; touch `csrc/r3d.c` to rebuild) |
| `F2N64_CFLAGS=-DF2N64_NO_MUSIC` | measurement: no music (the `audio:` log line shows the mixer's CPU time per second; ULC music at 32 kHz is most of it) |
| `F2N64_R3D_PROF=1` | measurement: r3d per-phase timers (full / camera-only draws, culling, water, recording, batch, flush parts), `r3dprof` line every 150 frames |
| `F2N64_PADX=N` | measurement: N bytes of padding before the `R3d` struct (samples data layouts; research/n64-performance.md §3R) |
| `F2N64_CFLAGS=-DF2N64_CODEPAD=N` | measurement: N bytes of padding at the end of the hot flush region (everything after it moves: samples code layouts; one build's fps can be 10 % off, §3A) |
| `F2N64_CFLAGS=-DF2N64_AUDIO_PROF` | measurement: libdragon's mixer profile slots (fetch, sample buffers, ULC I/O / decode, volumes, rspq, ...) dumped every 5 s (needs the 3A libdragon for the ULC slots) |
| `F2N64_CFLAGS=-DF2N64_AUDIO_RATE=N` | measurement: mixer output rate (default 32000) |
| `F2N64_CFLAGS=-DF2N64_MAIN_RESERVE_KIB=N` | the main heap's share after boot (default 1500; the rest is the level heap); touch `csrc/main.c` after changing `F2N64_CFLAGS` (make does not track it) |
| `F2N64_CFLAGS=-DF2N64_STACK_GUARD_KIB=N` | painted guard below the 64 KiB stack during boot (default 192; 16 KiB stay painted afterwards) |
| `F2N64_CFLAGS="-DF2N64_MEM_TRACE_SIZE=A -DF2N64_MEM_TRACE_SIZE2=B"` | testing: backtrace of every level-heap allocation of A or B bytes (`memtrace size ptr: addrs`; match the `LEFT OVER` pointers, then `mips64-elf-addr2line -f -C -e target/n64/build/frogger2.elf`) |
| `F2N64_CFLAGS=-DF2N64_MEM_PEAK_TRACE=N` | testing: during play, the backtrace of every allocation that takes both heaps' bytes in use N bytes past the last logged high-water mark (`memgrow KiB +size: addrs`; the mark restarts at the end of each load). Blocks freed every frame stop setting new highs; note that the allocation crossing a mark is often a per-frame victim on top of slower growth elsewhere |
| `F2N64_CFLAGS=-DF2N64_MEM_LIVE_TRACE` (+ `F2N64_MEM_LIVE=1`) | testing: play-time level-heap blocks remember their allocation backtrace (interned, 512 sites; add `-DF2N64_MEM_LIVE_LOAD` for load-time blocks too, 2048 sites); a replay boot dumps the live blocks by site every 1200 ticks (`memlive [tN] id: blocks bytes: addrs`), the load's blocks at the start (`memlive [load]`, with `_LOAD`) and everything at an allocation failure (`memlive [fail]`). The tables cost 16-64 KiB, so the build runs out sooner. Symbolise with `mips64-elf-addr2line -f -C -i -e target/n64/build/frogger2.elf`. After changing `F2N64_CFLAGS`, touch `csrc/main.c` |
| `F2N64_CFLAGS=-DF2N64_MEM_TRACE_PLAY=N` | testing (with `F2N64_MEM_TRACE_SIZE`): backtrace of every play-time allocation of N bytes or more (`memtrace play`) |
| `F2N64_CFLAGS=-DF2N64_BIGPOOL_DUMP` | testing: backtraces of allocations ≥ 200000 bytes and the level heap's block list at each video |
| `F2N64_CFLAGS=-DF2N64_MEMBENCH` | measurement: at boot, CPU cycles per uncached / cached access and per `rdpq_set_prim_color` (`membench` line) |
| `F2N64_CFLAGS=-DF2N64_FLIP_LIBDRAGON` | frame limit (30 / 20 fps setting) through `display_set_fps_limit` (fixed vblank phase) instead of the paced flips (§3A) |
| `F2N64_FRAME_LIMIT=30` / `20` / `0` | testing: the frame limit at boot whatever the SRAM settings say |
| `F2N64_MODGE_CHECK=1` | testing: every 64 frames, compare the RSP's MODGE/WAVE vertices with the bit-exact CPU model (logged) |
| `F2N64_MODGE_CPU=1` | testing: animate MODGE/WAVE meshes on the CPU instead of the RSP |
| `F2N64_CULL_CHECK=1` | testing: actors the draw loop's view test would leave out (research/n64-performance.md §3D2) are drawn anyway, their draws marked; r3d logs `cull check: N draws of culled actors, M visible` every 2000 such draws and `CULL CHECK FAILED` (model, object) for any it does not cull wholly (must stay 0) |
| `F2N64_CVT_CHECK=1` | testing: at boot, check the shader's float -> s15.16 matrix conversion against `to_fixed` (logged) |
| `F2N64_R3DT_FRAME=N` | r3d-test ROM: freeze the synthetic scene's animation at frame N (screenshots comparable between builds) |
| `F2N64_MEASURE_SIZES=1` | measurement: log r3d draws/triangles per on-screen radius class (< 1, 1.5, 3 px) every 300 frames |
| `F2N64_NO_BACKEND=1` | measurement: the draw loop records meshes into the sink and drops them (no r3d): its own CPU cost alone (`draw phases` line) |
| `F2N64_KEEP_DFS=1` | skip the data step (reuse `target/n64/filesystem`) |
| `F2N64_FREEZE=N` or `N/M` | testing: the simulation stops after N game frames of each level and the same scene is drawn from then on (`/M`: then one tick every M drawn frames, logged `app: freeze step`); tells scene-dependent render bugs (every frame) from races (some frames) |
| `F2N64_CFLAGS=-DF2N64_NO_MIXER` | testing: no mixer rounds at all (no audio RSP work, silent): rules the audio's high-priority RSP bursts in or out of a bug |
| `F2N64_DUMP_FRAME=N` or `A-B` | log frame N's (or frames A..=B's) CPU draw commands and r3d draws (frames counted per level) |
| `F2N64_LOG_MODELS=1` | log each model the first time r3d draws it |
| `F2N64_ONLY_MODEL` / `F2N64_SKIP_MODEL` | comma-separated hex model ids: draw only / never these (bisecting render bugs) |
| `F2N64_TUNE` | with `simbench`: experiment masks run one after another in the same ROM (`f2_game::simbench::tune`), e.g. `"0 1"` |
| `F2N64_TUNE_AB` | in the game: `f2_game::simbench::TUNE` alternates between 0 and this mask every 5 s perf window (A/B of experiment code in one ROM, so both sides have the same code layout); the `sim phases` line prints `tune` and the phase `sum`. With `tools/n64/bench.sh`, `BENCH_PHASE_LINES=N` prints the last N `sim phases` lines |

Log lines (debugf → `C:\f2-n64test\run.txt`): `draw phases [W-L] us/frame` every 5 s (phase 3E:
the draw loop's parts — setup, world, actors0, shadows, actors1, sprites, specfx, overlays; the
actor phases include r3d queueing — plus the procedural texture refill, `render::snapshot` and
host overlays), `cputri:` every 300 frames (CPU batches: triangles, rects, mode resets, rect
paths, texture upload cache), `perf [W-L] ...` every 5 s (fps, sim ticks/s,
dropped ticks, per-frame sim and draw time split into draw loop / textures / CPU batches /
framebuffer wait, RSP and RDP busy %, `Game::tick` µs, audio poll µs, CPU triangles, r3d
draws / triangles queued and sent / batches / texture loads, audio voices, music),
`spike [W-L] mode M: ...` (phase 3A: a main-loop period over 50 ms and 1.5x the running
average — or the first frame after a load — with its breakdown: sim, ticks, draw loop, refill,
texture uploads, CPU batches, r3d, framebuffer wait, mixer time; at most 8 per 5 s window),
`slow cpu batch: ...` (a CPU batch over 2 ms: count, texture size/format, blend, rect paths;
plus the Rust-side line with the segment), `cpu texture: ...` (a CPU texture conversion —
TMEM / downscaled copy, additive variant — over 0.2–0.5 ms), `display [W-L]: N flips, the
previous image shown 1 vblank xA, 2 xB, ...` every 5 s (how long each image stayed on screen),
`audio: N polls/s, N us/s in polls` (all mixer time, the C main loop's polls included; the perf
line's `audio N us/frame` is the same per frame, its `audio poll` only the poll after each tick),
`app: frame pacing N fps` (phase 3P: `display_set_fps_limit` from the running tick/frame
costs, see `App::pace`; `pacing N fps` in the perf line, 0 = unlimited), the perf line's
`r3d queue`/`flush` µs (queue = the batch after the draw loop; the per-draw recording is part of the draw loop, §3R) and `(N reused)` draws, `heap [W-L] ...` (Rust live/peak, libdragon heap used/total, N64 texture bytes; level peak on
every level change), `heap step` (per-step memory of a level load), `game: ...` on mode or
pad changes, `app: level ... loaded in N ms`, `app: video ...`, `app: save game ...`,
`ALLOC FAILED` (heap state when an allocation fails).
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
`ares-shot.ps1` (adapted from the PS4/PS5 harnesses) captures the client area with
`PrintWindow` (`<name>.png`, `<name>_1.png`, …) without bringing ares to the foreground: the
harness runs while the user works on the same desktop. It sends **no keys** (`-keys` is
refused): `keybd_event` injects system-wide keystrokes into whatever window has focus, which
typed into the user's windows. Input for tests comes from inside the ROM (`F2N64_BOOT`
`story-finish N`, `frontend-start`, `level W L`, ...). At the end ares is closed through its
window first (forced after 8 s), so it writes the cartridge save `<rom>.ram` next to the ROM.
Pad mapping in the game
(`src/input.rs`): A = Cross (hop), B = Circle (tongue), Z / C-Left = Square (croak),
C-Up = Triangle, C-Down = Select, L/R = L1/R1, Start, D-pad and stick = directions.

## Settings page (host menu)
The pause menus (in levels and in the frontend) have a **Settings** item
(`GameOptions::settings_menu`; the frontend pause menu's quit item reads "Quit" and soft-resets
to the start-up logos, `GameOptions::console`). The page (`src/settings.rs`) has:
- **FPS counter** On/Off (bottom right, frames shown in the last second);
- **Frame limit** Off / 30 / 20 fps (Off = as fast as possible; 30 / 20: a finished frame is
  shown no earlier than 2 / 3 vblanks after the last one, so a late frame holds the previous
  image one extra vblank, not until the next 30 / 20 Hz slot — phase 3A; the old
  `display_set_fps_limit` behaviour with `-DF2N64_FLIP_LIBDRAGON`);
- **Controls** "A hop, B tongue" (standard) or "B hop, A tongue" (A/B swapped for the game; the
  page itself always uses A = change, B = back).

Up/Down choose, Left/Right/A change, B/Start/C-Up close. Settings are saved on close in SRAM at
`0x7F00` (`"F2ST"` record with a checksum, after the save game; a blank or corrupt record means
defaults).

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

## Crate layout (`crates/frogger-n64`, phase 3I)
| module | role |
|---|---|
| `app.rs` | front end: fixed 60 Hz `f2-game` ticks paced by C0 COUNT (catch-up ≤ 6 ticks/frame, the rest dropped and counted), level loads, effects → audio / FMV / saves, perf/heap logs |
| `data.rs` | `N64Data<S: AssetStore>`: the game's `DataSource` over the pack (`PackStore`: F2Z / DCA / raw by magic; F2SX sound index; `.prc` procedural sources) or the phase-2G layout |
| `render_r3d.rs` | the renderer: recorded `MeshDraw`s → r3d (F2NM meshes on the RSP), the CPU batches between r3d's lists, texture-map sync |
| `r3d.rs`, `csrc/r3d.c`, `csrc/rsp_f2*.S` | the RSP renderer (phase 2R; plus texture aliases, additive variants, level reset) |
| `render_cpu.rs` | textures (registry image → N64 texture: NTX adoption, RGBA conversion, in-place updates) and the CPU batches (`n64_draw_tris` / `n64_draw_rects`); the whole phase-2G renderer with `render-cpu` |
| `ntx.rs`, `ntxload.rs` | NTX1 reader; texture banks / backdrops into the registry without RGBA8 copies |
| `save.rs` | the PC save block in SRAM |
| `texconv.rs` | RGBA8 → RGBA16/RGBA32 + TMEM-fitting downscale (procedural textures, book pages) |
| `audio.rs` | `Effect`s → mixer voices (wav64), PC `PlaySample` positional maths; music |
| `input.rs` | N64 pad → PSX pad bits |
| `rt.rs`, `ffi.rs`, `csrc/` | runtime glue and the C host (`n64_*`) |

The draw loop itself (level scene, actors, shadows, sprites, effects, overlays, text) is the
desktop's, moved to the `no_std` crate `crates/f2-draw` and shared.

## Prior art
nust64 (ELF → ROM packer), n64-heart-rust and the nintendo64-hal crate are pure-Rust N64
stacks that own the boot and hardware access and do not link libdragon; this port instead
keeps libdragon (RDP/RSP, audio, DFS, video, joybus) and only bridges the ABI.
