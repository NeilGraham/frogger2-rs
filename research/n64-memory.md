# N64 RAM budget: measured simulation and level working set

Input for `docs/N64-PLAN.md` §3 (8 MiB RDRAM with the Expansion Pak). Measured 2026-10-10 on
the USA PC data after the `no_std` conversion (phase 1A).

## How

- `size_of::<f2_game::Game>()`: a compile-time probe (`const _: [(); 1] = [(); size_of::<Game>()]`,
  the error prints the size) built with nightly `-Zbuild-std=core,alloc` for phase 1B's N64
  target spec (`mips-nintendo64-none.json`: VR4300, n32, 32-bit pointers, big-endian).
- Heap: `cargo run --release -p f2-game --example mem_budget [ticks]`, a counting global
  allocator around the system allocator. Run for **i686** (32-bit pointers like the N64's n32
  ABI; `CARGO_TARGET_DIR=target/i686 cargo build --release -p f2-game --example mem_budget
  --target i686-unknown-linux-gnu`, needs `gcc-multilib`), once as is and once with
  `F2_MEM_STRIP_WUS=1`. Per level (frontend + the 26 single-player levels of the PC level
  table): a fresh `Game::new_session`, `start_arcade(W, L)` (`start_frontend` for 8-0), then
  1800 ticks of a deterministic pad stream (a hop every 12 ticks).
- Columns (KiB = 1024 bytes):
  - *live after load*: heap the simulation keeps after `InitLevel` (everything in `Game`:
    map, model facts, actors, parsed level files, game text, effect pools...). The component
    columns are what dropping that part frees (actors before models; `Arc`-shared model facts
    count under *models*).
  - *play peak*: peak over the 1800 ticks when the run had no level restart. Runs that died
    and restarted (`(reload)`) peak at a fresh load instead (≈ the stripped load peak + live).
  - *parsed object banks*, *texture banks*: what a renderer holds for the level when it keeps
    the `f2-formats` structures (`ObjectBank` with f32 vertices/keys; `TexBank` with 16-bit
    texels as stored: the texel data alone is within 1% of the column). Not part of `Game`.
  - *working set* = live + object banks + texture banks.
  - *load peak*: peak during `InitLevel` including the transient file reads and parses.
    The PC loader parses each sample bank (`.wus`, all PCM) only to read `uid`/`looped`
    (`load_sfx_set`); *PCM stripped* replaces that with metadata-only banks, as an N64
    `DataSource` would provide.

`size_of::<Game>()`: **28,408 bytes** on the N64 target (28,404 on i686, 30,688 on x86-64).
The struct holds the fixed pools (`TEXTOVERLAY`/`SPRITEOVERLAY` blocks, timers, players...);
the variable-size state is on the heap (below).

## Results (i686, 32-bit pointers)

| level | live after load | play peak (no reload) | models | map | actor2 | parsed object banks | texture banks (16-bit texels) | **working set** | load peak, PC loader | load peak, PCM stripped | largest transient (stripped) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 8-0 frontend | 524 | 531 | 277 | 18 | 46 | 1373 | 1569 | **3466** | 2840 | 2840 | objbank objects\start.dll |
| 0-0 GAR1 | 596 | (reload) | 294 | 61 | 48 | 1323 | 1086 | **3005** | 3727 | 2149 | objbank objects\garden1.dll |
| 1-0 ANC1 | 703 | (reload) | 348 | 95 | 48 | 1654 | 1148 | **3505** | 11049 | 2852 | objbank objects\ancients1.dll |
| 1-1 ANC2 | 723 | (reload) | 348 | 119 | 47 | 1504 | 1148 | **3375** | 11049 | 2492 | objbank objects\ancients3.dll |
| 1-2 ANC3 | 887 | 892 | 321 | 104 | 185 | 1614 | 1148 | **3649** | 11049 | 2794 | objbank objects\ancientsboss.dll |
| 2-0 SPA1 | 667 | 673 | 271 | 74 | 93 | 1228 | 1072 | **2967** | 6072 | 2141 | texbank textures\System.fla |
| 2-1 SPA2 | 627 | 634 | 271 | 76 | 25 | 1155 | 1072 | **2854** | 6072 | 2144 | texbank textures\System.fla |
| 2-2 SPA3 | 823 | (reload) | 313 | 126 | 94 | 1622 | 1072 | **3517** | 6072 | 2834 | objbank objects\spaceboss.dll |
| 4-0 SUB1 | 740 | 745 | 323 | 87 | 94 | 1324 | 1086 | **3150** | 14497 | 2226 | texbank textures\System.fla |
| 4-1 SUB2 | 702 | 707 | 326 | 82 | 94 | 1442 | 1086 | **3230** | 14497 | 2294 | objbank objects\sub2.dll |
| 4-2 SUB3 | 731 | (reload) | 317 | 98 | 93 | 1318 | 1086 | **3135** | 14497 | 2249 | texbank textures\System.fla |
| 5-0 LAB1 | 706 | (reload) | 306 | 111 | 94 | 1294 | 1141 | **3141** | 7977 | 2273 | texbank textures\System.fla |
| 5-1 LAB2 | 706 | 711 | 302 | 78 | 94 | 1496 | 1141 | **3343** | 7977 | 2358 | objbank objects\lab3.dll |
| 5-2 LAB3 | 574 | (reload) | 303 | 49 | 48 | 1184 | 1141 | **2899** | 7977 | 2107 | texbank textures\System.fla |
| 6-0 HAL1 | 655 | 660 | 278 | 78 | 48 | 1334 | 1181 | **3170** | 13822 | 2301 | objbank objects\halloween1.dll |
| 6-1 HAL2 | 719 | 724 | 309 | 80 | 93 | 1396 | 1181 | **3296** | 13822 | 2423 | objbank objects\halloween2.dll |
| 6-2 HAL3 | 656 | (reload) | 285 | 45 | 93 | 1319 | 1181 | **3156** | 13822 | 2212 | objbank objects\halloweenbossb.dll |
| 7-0 RET1 | 490 | 490 | 254 | 24 | 47 | 675 | 922 | **2087** | 2744 | 1992 | texbank textures\System.fla |
| 7-1 RET1 | 450 | 451 | 254 | 22 | 24 | 675 | 922 | **2047** | 2744 | 1986 | texbank textures\System.fla |
| 7-2 RET1 | 458 | 459 | 254 | 27 | 24 | 708 | 922 | **2088** | 2744 | 2000 | texbank textures\System.fla |
| 7-3 RET1 | 445 | 446 | 254 | 19 | 24 | 687 | 922 | **2054** | 2744 | 1978 | texbank textures\System.fla |
| 7-4 RET1 | 437 | 437 | 254 | 19 | 24 | 686 | 922 | **2045** | 2744 | 1977 | texbank textures\System.fla |
| 7-5 RET1 | 581 | (reload) | 254 | 31 | 93 | 686 | 922 | **2189** | 2744 | 2010 | texbank textures\System.fla |
| 7-6 RET1 | 460 | (reload) | 254 | 27 | 24 | 686 | 922 | **2068** | 2744 | 1999 | texbank textures\System.fla |
| 7-7 RET1 | 452 | 453 | 254 | 23 | 24 | 678 | 922 | **2052** | 2744 | 1988 | texbank textures\System.fla |
| 7-8 RET1 | 576 | (reload) | 254 | 29 | 93 | 687 | 922 | **2185** | 2744 | 2004 | texbank textures\System.fla |
| 7-9 RET1 | 493 | (reload) | 254 | 28 | 47 | 687 | 922 | **2102** | 2744 | 2003 | texbank textures\System.fla |

Session overhead before any level (`Game::new_session`: game text etc.): 35 KiB. The live heap
returns to the start value after each session is dropped (no leaks across sessions).

64-bit host (x86-64) for comparison: live after load 487–983 KiB, parsed object banks
718–1,749 KiB, texture banks 927–1,576 KiB, max working set 3,830 KiB (1-2): 32-bit pointers
save 5–10%.

## Reading

- **Simulation**: ≤ 0.9 MiB live in every level (max 887 KiB, Ancients 3 boss level), plus
  28 KB static `Game`. Play adds ≤ 10 KiB (effects, particles) without restarts. Model facts
  (`ModelTable`, 250–350 KiB) are the largest part and are derived from the object banks:
  the N64 pack can store them pre-extracted.
- **Level working set** with the PC data structures: 2.0–3.6 MiB (max 3,649 KiB in 1-2;
  frontend 3,466 KiB). The plan's "level working set ~3.5 MB" holds even before any
  compaction. Object banks as parsed `f2-formats` trees (f32 positions, normals, UVs, keyframes
  in `Vec`s) are 0.7–1.65 MiB; the packer's compact fixed-point models (§2 of the plan)
  should roughly halve that. Textures are 0.9–1.5 MiB as 16-bit texels; RDP CI4/CI8 conversion
  reduces them further (and the system bank, 0.9 MiB of it, is shared by every level).
- **Load transients**: the PC loader's peak is dominated by parsing whole sample banks
  (up to 14 MiB for `sub.wus`): unusable on the N64, and unnecessary (only `uid`/`looped` are
  read). With metadata-only sound banks the load peak is ≤ 2.8 MiB, from parsing a whole object
  bank DLL (`ancients1.dll`, 2.2 MiB transient) or `System.fla` for `pt_texture_load` (1.5 MiB).
  An N64 `DataSource` should hand the simulation pre-extracted facts (sound list, model facts,
  procedural-texture sources) instead of PC containers; then the load peak is the working set
  plus a decompression buffer.
- **Budget sketch (8 MiB)**: code + libdragon + Rust ~1.5 MiB, frame/z buffers 0.6 MiB,
  audio ~0.3 MiB, simulation ≤ 0.9 MiB, level models + textures ≤ 2.8 MiB (PC formats; less
  after conversion), leaving ≥ 1.9 MiB for load buffers and headroom. Fits.

Reproduce: the commands above (game data via `F2_GAME_DIR`). The example prints the full
per-component breakdown per level.
