# Frogger 2: Swampy's Revenge (Rust): plan and design rationale

Why the project is built the way it is, what it covers, and how it is verified. For the current
state of each area see [Status](#status); open items are in [PORT-GAPS.md](PORT-GAPS.md).

## Status

Audited against the code ([source-map.md](source-map.md): 68 game-side units, 41 ported 1:1, 5
partial, 17 replaced by platform redesigns, 4 skipped, 1 todo). Single-player parity is verified:
all 26 levels and the 3 demos are identical to the retail oracle trace.

| Phase | State | What exists | Open |
|---|---|---|---|
| 0 Foundations | done | research, workspace, oracle (`f2fix` proxy, trace schema v2), `f2-parity` trace diff, packaging scripts and CI ([BUILDING.md](BUILDING.md)) | none |
| 1 Media and import | done | `f2-media`, `f2-formats`, `f2-import` (incl. FLAC music), exporters, `f2-viewer` | none |
| 2 Engine core | done | `f2-math`, `f2-engine` (wgpu), `f2-audio`, `f2-platform`, `f2-bink`, the `frogger2` binary | none |
| 3 Gameplay port | done, parity verified | all of `f2-game` except the items in PORT-GAPS | multiplayer HUD |
| 4 Full single-player | done | frontend, menus, options, story, savegame, language, training, cheats, FMV, fades | none known |
| 5 Multiplayer and hardening | partial | local multiplayer and split-screen game logic (`multi.c`), packaging | multiplayer HUD |
| 6 Enhancements and online | done (first cut) | display profiles (authentic / enhanced), gamepads and rebinding, rollback netplay (`f2-net`), settings screen | culling for wider-than-4:3 views, see below |
| 7 Modding | done (first cut) | `f2-mods` layered VFS, `f2-etc` script tool, `f2-modkit`, level editor | glTF/PNG/WAV replacement polish |
| Platforms | see [PLATFORMS.md](PLATFORMS.md) | Windows, Linux, macOS, Android, PS4, PS5, N64 (experimental) | N64 black screen after a level; Linux not yet played on a desktop |

**Next:** drawing and animating what the original 4:3 view culled, on every platform except the
N64 (see [enhancements.md](enhancements.md), "Hor+ past the 4:3 frustum"), the multiplayer HUD,
and N64 stability and speed.

## 1. Goal

Rebuild the PC version of *Frogger 2: Swampy's Revenge* (Blitz Games / Hasbro Interactive, 2000)
as a modern, open, cross-platform **Rust** program that:

1. **Ships zero copyrighted game data.** The player points the importer at *their own copy*
   (Redump `.cue/.bin`, any single-bin CD image, a physical disc, or an existing install folder)
   and the tool extracts and converts what it needs into a local data directory.
2. **Plays identically to the retail PC game first** ("parity"), validated with the game's own
   demo-recording format and per-tick trace diffs against the user's own retail exe run headless
   and muted in the background (plus difftests against the original C for pure functions).
3. **Then becomes a platform** for fixes, enhancements (widescreen, high refresh rate, gamepads,
   modern audio), mods, and ports to other systems.

## 2. The original game

Full details live in [`research/`](../research/).

| Thing | Finding | Doc |
|---|---|---|
| Source code | `frogger2-vss` is an end-of-project VSS backup (Apr 2001). The shipping PC build is `teamSpirit/FroggerProject` (`Common/` + `PC/`) plus `MDX Library`: about **71.5k LOC** of C/C++. | [source-tree.md](../research/source-tree.md) |
| Engine shape | Win32 + DirectX 7 (DDraw/D3D7, DInput7, DSound3D, DirectPlay), Bink video, MCI CD-audio music. 60 Hz logical tick with a variable timestep. Gameplay maths is **20.12 fixed point** with PSX-style angles (4096 = 360°). | [source-tree.md](../research/source-tree.md) |
| Retail media | Mixed-mode CD: track 1 is ISO9660 data (InstallShield 5 `DATA1.CAB` plus Bink videos), tracks 2 to 12 are Red Book music. USA and Europe (En/Fr/De/It) Redump dumps verified. | [retail-media.md](../research/retail-media.md) |
| Asset formats | Object/model banks are **Win32 DLLs** containing static MDX structs (parsed as data, never executed). Textures and backdrops are FLA2 (LZSS) files, sounds are `.wus` CRC-keyed banks. | [asset-formats.md](../research/asset-formats.md) |
| Community | Highway Frogs preserved the source; `frogger2-pc` is a VC6-buildable tree whose `fixes` branch is the *Frogger 2 Enhanced* mod (a good list of known bugs). No license on any of it. | [community-and-legal.md](../research/community-and-legal.md) |

## 3. Strategy

### 3.1 "Port, then improve", not a clean-room reimplementation

With the real source available, the fastest route to an *exact* game is a **systematic,
module-by-module translation of the C/C++ into idiomatic-but-faithful Rust**, keeping:

- the original **fixed-point maths bit-exact** (`i32` 20.12, PSX trig tables);
- the original **data layouts** in the asset parsers (retail files are read, never re-authored);
- a **1:1 traceable mapping** from every C file and function to its Rust home
  ([source-map.md](source-map.md)) so reviewers can diff behaviour.

Platform layers (DirectX, Win32, MCI, DirectPlay, the Bink SDK, SoftStation) are **replaced, not
translated**: they are redesigned on modern Rust crates behind small traits. See
[architecture.md](architecture.md) and [porting-guide.md](porting-guide.md).

> This makes the Rust code a *derivative work* of unlicensed source; see [Legal](#7-legal-and-distribution-posture).

### 3.2 Determinism is the backbone of verification

The original has **demo record/playback** (`levplay.c`, `Demos/record-W-L.key`), a fixed-seed RNG
and integer gameplay maths, so it is very traceable:

1. The game simulation runs on a **fixed 60 Hz tick** (the original's logical rate), decoupled
   from rendering.
2. The **retail oracle** is `f2fix.dll`, a 32-bit Rust shim injected (via a `dinput.dll` proxy)
   into the *user's own* retail `Frogger2.exe`. It runs the original game in the background,
   off-screen, muted, never stealing focus, with the clock forced to exact 1/60 s steps, input
   fed from `.key` files, and per-tick state traces (frog, actors, tiles, score, timers, RNG).
   See [ORACLE.md](ORACLE.md).
3. `f2-game` replays the same input headless and emits the same trace format; `f2-trace` diffs
   them. Any divergence points at the exact tick and field.
4. Pure functions (fixed point, maths, tile lookups) are also difftested against the **original C
   compiled in tests**.

The original uses a variable timestep (`gameSpeed = 4096 * elapsed / 16.67 ms`); forcing a fixed
step on both sides makes traces comparable and also **fixes the frame-rate-dependent bugs** (for
example glide duration at high FPS).

### 3.3 Technology choices

| Concern | Original | Rust replacement |
|---|---|---|
| Window / events | Win32 | `winit` |
| Rendering | MDX on D3D7 / SoftStation, 16 bpp | `wgpu` (sorted poly lists, additive/alpha modes, fog, sprites; 32-bit colour, any resolution, MSAA) |
| Input | DirectInput 7 | `winit` keyboard + `gilrs` gamepads, PSX-pad abstraction internally |
| SFX | DirectSound + DS3D | `cpal` + own mixer; 3D pan and attenuation in software |
| Music | MCI Red Book CD audio | tracks ripped at import, streamed (fixes "music doesn't loop on Vista+") |
| FMV | Bink 1 SDK | native Rust Bink 1 decoder (`f2-bink`) |
| Networking | DirectPlay | UDP with rollback netcode (`f2-net`) |
| Files | Win32 + `LoadLibrary` for banks | `f2-formats` pure parsers; DLL banks are data |
| Text | `language.fla` | parsed, UTF-8 internally |

Everything is pure Rust or has a pure-Rust path, so building needs only `cargo`.

### 3.4 Workspace layout

See [architecture.md](architecture.md) for the crates, data flow and main loop. In short:
`crates/` holds the libraries (`f2-media`, `f2-formats`, `f2-import`, `f2-bink`, `f2-math`,
`f2-engine`, `f2-audio`, `f2-game`, `f2-platform`, `f2-mods`, `f2-net`, `f2-package`, and the
platform hosts), `frogger2` is the executable, and `tools/` holds the viewer, the script compiler,
the trace and parity tools, the packers and the console build scripts.

## 4. Asset import flow (user-facing)

```
$ f2-import "/path/to/Frogger 2 - Swampy's Revenge (USA).zip"
  Detected: Frogger 2 (USA), Redump-verified data track
  Extracted DATA1.CAB, videos, music tracks, object banks
  -> Game data installed to ~/.local/share/frogger2-rs/data/usa
$ frogger
```

Supported inputs: Redump multi-file `.cue`; single-file `.cue/.bin`; `.iso`; a mounted physical
disc; an existing Windows install folder (no CD audio, so music is optional). Details:
[asset-import.md](asset-import.md).

## 5. Scope

**In scope (parity):** all single-player content (Story and Arcade), every level including
bosses and Super Retro levels, training, the 3D frontend, extras, cheats, saves, all languages on
the disc, local multiplayer (race, collect, battle), FMVs, music and SFX.

**Beyond parity:** online multiplayer, the level editor, mod tooling, the display and input
enhancements ([enhancements.md](enhancements.md)), and the console and mobile ports
([PLATFORMS.md](PLATFORMS.md), [EXPORT.md](EXPORT.md)).

**Out of scope:** the PSX and Dreamcast versions (their sources are useful cross-references),
the SoftStation software renderer, DirectPlay compatibility with original clients.

## 6. Testing strategy

| Layer | Method |
|---|---|
| Parsers | Golden tests on real data, gated behind an env var (no assets in the repo); synthetic fixtures where possible. |
| Maths | Differential tests against the original C compiled in tests. |
| Gameplay | Demo and scenario replay trace-diff against the retail oracle; headless `f2-game` runs thousands of ticks per second. |
| Rendering | Screenshot comparison against reference captures at fixed ticks. |
| Import | Synthetic CUE/ISO fixtures generated in tests. |

## 7. Legal and distribution posture

- The Frogger 2 source was **publicly released for preservation but carries no license**; the
  rights ownership (Hasbro Interactive, then Infogrames/Atari; the Frogger IP, Konami) is unclear.
- A Rust **translation** of that source is a derivative work; it does not become "clean" by being
  re-typed. This is the same risk profile as decompilation-based ports.
- Mitigations: **no game assets in the repo or releases**, **no original binaries or DLLs
  redistributed**, import only from a user-owned copy, non-commercial, clear attribution to Blitz
  Games / Interactive Studios and the Highway Frogs preservation effort, and a prompt response to
  any rights-holder request.
- Platform-layer code (importer, renderer, audio, Bink decoder, mod system) is written
  independently of the original source.
- The project's own code is GPL-3.0-or-later (`f2-bink`: LGPL-2.1-or-later); see
  [DECISIONS.md](DECISIONS.md).

## 8. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Rights-holder takedown | High | Section 7 posture; keep platform crates separable; no assets or binaries distributed. |
| Behaviour drift during translation | High | Trace-diff harness; fixed point kept bit-exact. |
| Variable-timestep logic assumes FPS-dependent quirks | Med | Reference runs at a fixed 60 Hz too; intentional deviations documented. |
| Prebuilt-only libraries (gelf, SoftStation, Bink) not in the source | Low | Only used for platform functions that are replaced anyway. |

## 9. Index of documents

- [research/](../research/): `source-tree.md`, `retail-media.md`, `asset-formats.md`,
  `community-and-legal.md`, `known-bugs.md`, plus the N64 notes and Python format-verification
  prototypes (no game data).
- [architecture.md](architecture.md), [porting-guide.md](porting-guide.md),
  [source-map.md](source-map.md), [asset-import.md](asset-import.md),
  [enhancements.md](enhancements.md), [modding.md](modding.md), [ORACLE.md](ORACLE.md).
- Using it: [USER-GUIDE.md](USER-GUIDE.md), [BUILDING.md](BUILDING.md), [EXPORT.md](EXPORT.md),
  [TOOLS.md](TOOLS.md), [PLATFORMS.md](PLATFORMS.md).
