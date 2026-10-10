# Frogger 2: Swampy's Revenge — Rust Port Plan

> **Codename:** `frogger2-rs` (working title; final name TBD — see [Decisions needed](#decisions-needed-from-the-project-owner))
> **Status:** see [Status](#status) below (updated 2026-10-09)

## Status

Audited 2026-10-09 against the code ([docs/source-map.md](docs/source-map.md): 68 game-side units,
41 ported, 5 partial, 17 replaced by platform redesigns, 4 skipped, 1 todo; open items in
[docs/PORT-GAPS.md](docs/PORT-GAPS.md)). Single-player parity is verified: all 26 levels and the 3
demos are identical to the retail oracle trace.

| Phase | State | What exists | Open |
|---|---|---|---|
| 0 Foundations | done | research, plan, workspace, oracle (`f2fix` proxy, trace schema v2), `f2-parity` trace diff, packaging scripts and CI ([docs/BUILDING.md](docs/BUILDING.md)) | P0-2 owner decisions ([section 9](#9-decisions-needed-from-the-project-owner)) |
| 1 Media and import | done | `f2-media`, `f2-formats`, `f2-import` (incl. FLAC music), exporters, `f2-viewer` | none |
| 2 Engine core | done | `f2-math` (difftests vs C, deterministic libm / x87 PC24 float paths), `f2-engine` (wgpu), `f2-audio`, `f2-platform`, `f2-bink`, `frogger` binary | none |
| 3 Gameplay port | done, parity verified | all of `f2-game` except the items in PORT-GAPS; 26 single-player levels and 3 demos trace-identical to retail | multiplayer HUD (`hud.c`), `UpdateCompletedLevel`, script `TrainingHint` wiring, `DefData` hi-score defaults |
| 4 Full single-player | done | frontend, menus, options, story, savegame, language, training, cheats, FMV, fades | none known |
| 5 Multiplayer and hardening | partial | local multiplayer and split-screen game logic (`multi.c`) | multiplayer HUD; retail parity for multiplayer; packaging (P5-2) |
| 6 Enhancements and online | done (first cut) | display profiles (authentic/enhanced: Hor+, ultrawide, MSAA, supersampling, aniso, fog, frame cap), gamepad and rebinding, `f2-net` rollback/lockstep netcode with UDP/loopback, lobby, spectators, wired into `frogger` | broader enhancements ([docs/enhancements.md](docs/enhancements.md)); real-network soak testing |
| 7 Modding | done (first cut) | `f2-mods` layered VFS, `f2-etc` script tool, `f2-modkit`, level editor (`--editor`, `f2-formats` writers) | glTF/PNG/WAV replacement polish, level manifests |


## 1. Goal

Rebuild the PC version of *Frogger 2: Swampy's Revenge* (Blitz Games / Hasbro Interactive, 2000) as a
modern, open, cross-platform **Rust** program that:

1. **Ships zero copyrighted game data.** The player points the importer at *their own copy*
   (Redump `.cue/.bin`, any single-bin CD image, a physical disc, or an existing install folder) and
   the tool extracts and converts what it needs into a local data directory.
2. **Plays identically to the retail PC game first** ("parity"), validated with the game's own
   demo-recording format and per-tick trace diffs against the user's own retail exe run headless
   and muted in the background (plus difftests against the original C for pure functions).
3. **Then becomes a platform** for fixes, enhancements (widescreen, high refresh rate, gamepads,
   modern audio), and **mods** (loose-file overrides, new levels via the original script language,
   model/texture replacement).

## 2. What we have (summary of research)

Full details live in [`research/`](research/). Highlights:

| Thing | Finding | Doc |
|---|---|---|
| Source code | `frogger2-vss` = end-of-project VSS backup (Apr 2001). Shipping PC build = `teamSpirit/FroggerProject` (`Common/` + `PC/`) + `MDX Library`. **~71.5k LOC** C/C++. | [research/source-tree.md](research/source-tree.md) |
| Engine shape | Win32 + DirectX 7 (DDraw/D3D7 IM, DInput7, DSound3D, DirectPlay), Bink video, MCI CD-audio music. 60 Hz logical tick with variable timestep. Gameplay maths is **20.12 fixed-point**, PSX-style angles (4096 = 360°). Y-up. | [research/source-tree.md](research/source-tree.md) |
| Retail media | Mixed-mode CD: Track 1 = ISO9660 data (InstallShield 5 `DATA1.CAB` + Bink videos), Tracks 2–12 = Red Book CD-audio music. USA and Europe (En/Fr/De/It) Redump dumps verified; MyAbandonware ISO is a single-bin image of the same layout. | [research/retail-media.md](research/retail-media.md) |
| Asset formats | Object/model banks are **Win32 DLLs** containing static MDX structs (no code to run — parse PE + apply relocations). Textures/backdrops/text in FLA2 (LZSS) files, sounds in `.wus` CRC-keyed banks, tile collision `.col`, entity `.dat`, compiled event scripts `.fev`, scenics `.sce`, demo input `.key`. | [research/asset-formats.md](research/asset-formats.md) |
| Sibling project | `../theme-park-world-recomp` already has Redump/cue/ISO import, rendition hashing, a headless + muted + no-focus harness for running original exes in the background, framebuffer dumps, difftest harness, task orchestration. | [research/sibling-tpw-reuse.md](research/sibling-tpw-reuse.md) |
| Community | Highway Frogs preserved the source; `frogger2-pc` is a VC6-buildable tree; its `fixes` branch is the *Frogger 2 Enhanced* mod (good list of known bugs). No license on any of it. No existing Rust/other-language reimplementation. | [research/community-and-legal.md](research/community-and-legal.md), [research/known-bugs.md](research/known-bugs.md) |

**Verified hands-on during research:** the USA + EU disc images were parsed (ISO9660 from raw
MODE1/2352 sectors), `DATA1.CAB` extracted (needs InstallShield 5 *"old compression"* mode), the
extracted data is byte-identical to the MyAbandonware "RIP" folder, object DLLs export exactly one
symbol (`GetbankAddress`) with identical stub `.text`, and the Bink files are `BIKi` (Bink 1,
640×480 @ 25 fps) which FFmpeg decodes. Python prototypes (`research/prototypes/`) then parsed
**every** texture bank, backdrop, sound bank and the language table byte-exactly, walked object
banks out of the DLLs, and confirmed the CRC-32 naming scheme (`CRC("frogger.obe")` = object id).

## 3. Strategy

### 3.1 "Port, then improve" — not a clean-room reimplementation

We have the real source, so the fastest route to an *exact* game is a **systematic, module-by-module
translation of the C/C++ into idiomatic-but-faithful Rust**, keeping:

- the original **fixed-point maths bit-exact** (`i32` 20.12, PSX trig tables) so behaviour matches;
- the original **data layouts** in the asset parsers (we read retail files, never re-author them);
- a **1:1 traceable mapping** from every C file/function to its Rust home
  ([docs/source-map.md](docs/source-map.md)) so reviewers can diff behaviour.

Platform layers (DirectX, Win32, MCI, DirectPlay, Bink SDK, SoftStation) are **replaced, not
translated** — they are re-designed on modern Rust crates behind small traits.

> ⚠️ This makes the Rust code a *derivative work* of unlicensed source. See §8 and
> [research/community-and-legal.md](research/community-and-legal.md). Choosing the project license
> and distribution posture is an owner decision.

### 3.2 Determinism is the backbone of verification

The original has **demo record/playback** (`levplay.c`, `Demos/record-W-L.key`), a fixed-seed RNG
(`SeedRandom()` → 398623) and integer gameplay maths — so it is very traceable. We will:

1. Run the game simulation on a **fixed 60 Hz tick** (the original's logical rate) decoupled from rendering.
2. Build the **retail oracle**: `f2fix.dll`, a 32-bit Rust shim injected (via a `dinput.dll`
   proxy) into the *user's own* retail `Frogger2.exe`. It runs the original game **in the
   background, off-screen, muted, never stealing focus**, with registry/CD virtualised (no crack,
   no exe patch), clock forced to exact 1/60 s steps, input fed from `.key` files/scenarios, and
   per-tick state traces (frog, actors, tiles, score, timers, RNG) + framebuffer PNG dumps.
   This is lifted almost wholesale from the sibling **Theme Park World recomp**
   (`../theme-park-world-recomp`: `tpwfix` headless platform layer, `audio.mute`, `run.py`,
   framebuffer dumps, `tpw-hook`) — see [research/sibling-tpw-reuse.md](research/sibling-tpw-reuse.md).
3. `f2-game` replays the same input headless and emits the same trace format; `f2-trace` diffs
   them. Any divergence points at the exact tick and field. Many oracle instances can run in
   parallel at faster-than-real-time.
4. For pure functions (fixed-point, maths, tile lookups) we also difftest against the **original C
   compiled in tests** (`cc` crate) — the cheapest oracle, since we have the source.

The original uses a variable timestep (`gameSpeed = 4096*elapsed/16.67ms`); forcing a fixed step on
both sides makes traces comparable and also **fixes the frame-rate–dependent bugs** (e.g. glide
duration at high FPS).

Parity hazards already identified (see [docs/porting-guide.md](docs/porting-guide.md) §3):
`FMul` is a 4-part split multiply; `FDiv` is a *reciprocal approximation using unsigned division*;
list iteration order; MSVC `long` = 32-bit.

### 3.3 Technology choices (recommended)

| Concern | Original | Rust replacement | Notes |
|---|---|---|---|
| Window / events | Win32 | `winit` | Cross-platform; Linux/macOS/Windows; later web. |
| Rendering | MDX → D3D7 HAL / SoftStation, 16bpp | `wgpu` | Re-implement MDX's draw model (sorted poly lists, additive/alpha modes, fog, sprites). 32-bit colour, any resolution, MSAA. |
| Input | DirectInput 7 | `winit` keyboard + `gilrs` gamepads | Keep PSX-pad abstraction (`padData`/`PAD_*`) internally. |
| SFX | DirectSound + DS3D | `kira` (or `cpal` + own mixer) | `.wus` → PCM; 3D pan/attenuation re-done in software. |
| Music | MCI Red Book CD audio | Ripped tracks → FLAC/WAV at import, streamed | Fixes "music doesn't loop on Vista+" bug for free. |
| FMV | Bink 1 SDK | Native Rust Bink-1 decoder (`f2-bink`), FFmpeg fallback at import | 14 videos, all `BIKi`. See §6 Phase 4. |
| Networking | DirectPlay | Phase 6: `quinn`/UDP + lockstep/rollback (`ggrs`) | Local multiplayer first. |
| Files | Win32 + `LoadLibrary` for banks | `f2-formats` pure parsers | DLL banks parsed as data; nothing is executed. |
| Text | `language.fla` | parsed; UTF-8 internally | Adds room for fan translations. |

All crates chosen are pure Rust or have pure-Rust paths, so building needs only `cargo`.

### 3.4 Cargo workspace layout

```
frogger2-rs/
├─ crates/
│  ├─ f2-media      # cue/bin + ISO9660 reader, Red Book audio ripper, InstallShield-5 CAB extractor
│  ├─ f2-formats    # parsers: PE-bank (MDX objects), .fla (FLA2 LZSS), .wus, .col, .dat, .fev, .sce, .key, savegame, language
│  ├─ f2-import     # CLI + library: detect version, verify hashes, extract & convert into data dir
│  ├─ f2-bink       # Bink 1 video+audio decoder (pure Rust)
│  ├─ f2-math       # fixed-point (FVECTOR, IQUATERNION, rsin/rcos tables), matrix utils — bit-exact
│  ├─ f2-engine     # MDX replacement: object hierarchy, keyframe anim, sprites, fonts, renderer (wgpu)
│  ├─ f2-audio      # sample banks, 3D voices, music streaming
│  ├─ f2-game       # gameplay port of Common/*.c — no platform deps, deterministic, testable headless
│  ├─ f2-platform   # winit/gilrs glue, config, paths, save dirs
│  ├─ f2-mods       # virtual filesystem with mod overlays, manifest, load order
│  └─ frogger2      # the executable
├─ tools/
│  ├─ f2-viewer     # model/texture/level/collision viewer (egui) — doubles as debugging tool
│  ├─ f2-etc        # port of the original `Etc` event-script compiler/decompiler (.fev)
│  └─ f2-trace      # trace-diff tool for parity testing
├─ docs/  research/  PLAN.md
```

Detailed design: [docs/architecture.md](docs/architecture.md).

## 4. Asset import flow (user-facing)

```
$ frogger2 import "/path/to/Frogger 2 - Swampy's Revenge (USA).cue"
  ✓ Detected: Frogger 2 (USA) — Redump-verified data track
  ✓ Extracted DATA1.CAB (383 files)       ✓ Videos 14/14
  ✓ Ripped music tracks 2–12 → FLAC       ✓ Converted object banks 72/72
  → Game data installed to ~/.local/share/frogger2-rs/data/usa
$ frogger2
```

Supported inputs (priority order): Redump multi-file `.cue`; single-file `.cue/.bin` (e.g.
MyAbandonware); `.iso` + separately supplied audio; mounted physical disc; existing Windows install
folder (no CD audio → music optional / user-supplied). Details: [docs/asset-import.md](docs/asset-import.md).

## 5. Scope

**In scope for 1.0 (parity):** all single-player content (Story + Arcade), all 4 shipped
playable worlds' levels incl. bosses and Super Retro levels, training, frontend 3D menus, extras
(art gallery, FMV viewer), cheats, saves, all languages present on the disc, local multiplayer
(race/collect/battle), FMVs, music, SFX.

**Post-1.0:** online multiplayer, in-game editor (the original had one, `Editor/`, debug-only),
mod tooling, new features.

**Out of scope:** PSX/N64/DC versions (though PSX/DC sources are useful cross-references), the
SoftStation software renderer, DirectPlay compatibility with original clients.

## 6. Phased roadmap

Each phase has an **exit criterion** that is testable. Estimates assume 1–2 regular contributors;
they are rough.

### Phase 0 — Foundations *(current)*
- [x] Inventory source, retail media, formats, community work, legal context.
- [x] This plan, research notes, docs skeleton.
- [ ] Owner decisions in §9 (license, name, repo hosting, target platforms).
- [ ] Create Cargo workspace, CI (fmt, clippy, test on Linux/Windows/macOS), contribution guide.
- [ ] Adopt TPW's execution tooling: `CLAUDE.md` session rules, `work/tasks.json` + `tools/tasks.py`
      task graph, `work/LOG.md`, lean `docs/ORCHESTRATION.md` (one implementer per task, executable
      acceptance), `tools/build.sh` (WSL → Windows `cargo.exe` for `i686-pc-windows-msvc`).
- [ ] Port TPW's import crate pieces (cue/ISO9660/identify/hashdb/store) as the `f2-media` seed.
- [ ] **Oracle spike**: `f2fix.dll` proxy-loaded into retail `Frogger2.exe` (copied from the
      user's `DATA1.CAB` into a run root) reaches the frontend headless + muted and writes one
      framebuffer PNG. Reuses `tpwfix` platform/headless code.
- [ ] Optional: build `HighwayFrogs/frogger2-pc` Debug (VC6 or VS2005) as a symbol-rich secondary oracle.

**Exit:** workspace builds in CI; oracle spike runs the retail game invisibly and muted from WSL.

### Phase 1 — Media & asset pipeline (`f2-media`, `f2-formats`, `f2-import`, `f2-viewer`)
1. `.cue` parser (multi-FILE and single-FILE/INDEX offsets), raw 2352 sector reader, ISO9660.
2. InstallShield 5 cabinet extractor (header in `data1.cab`, zlib "old compression" blocks) —
   reference: `unshield` (MIT) behaviour with `-O`.
3. Red Book audio rip → FLAC (via `flacenc`) or WAV, with pregap handling.
4. Version detection by SHA-1 (table in [research/retail-media.md](research/retail-media.md)).
5. Parsers, each with golden tests against both regions:
   PE bank loader + MDX object structs → `ObjectBank` (meshes, hierarchy, keyframes, texture refs);
   FLA2 LZSS → texture banks + 640×480 backdrops; `.wus` → PCM samples by CRC; `.col` tile graph;
   `entity-W-L.dat`; `script-W-L.fev`; `.sce`; `language.fla`; `.key`; `savegame.dat`.
6. `f2-viewer` (egui + wgpu): browse banks, render static models, play anims, show tile graph.
7. Export helpers: models → glTF, textures → PNG, sounds → WAV (for modders and debugging).

**Exit:** both Redump images + the single-bin image import cleanly; every file in `DATA1.CAB`
parses with no unknown bytes flagged; viewer shows every object bank.

### Phase 2 — Engine core (`f2-math`, `f2-engine`, `f2-audio`, `f2-platform`)
1. `f2-math`: port `fixed.c`, `maths.c`, trig/acos tables; property tests vs C (compile the C file
   with `cc` in a test-only build script and compare outputs on random inputs).
2. Renderer: MDX object/actor drawing model, materials & blend modes, z-sorted sprites/overlays,
   4096×4096 virtual overlay space, fonts (`textures/font`), fades, procedural textures (`ptexture.c`).
3. Audio: sample playback, 3D voices, music streaming with loop points.
4. Input: PSX-pad abstraction, keyboard defaults from `controll.cpp`, gamepads, rebinding.
5. Main loop: fixed 60 Hz simulation + interpolated rendering; game-mode state machine skeleton
   (`define.h` `gameModes`).

**Exit:** a Garden level renders with correct textures/animation and a free camera; SFX/music play.

### Phase 3 — Gameplay port, vertical slice (`f2-game`)
Port order (dependency-driven): `types/fixed/maths` → `map` + `general` (tiles) → `actor2`/`anim`
→ `path` → `platform` → `enemies` → `frogmove`/`frogger`/`tongue` → `collect` → `event`/`evenfunc`/`script`
→ `cam` → `hud`/`overlays`/`textover` → `specfx`/`particle` → `game.c` loop → `levplay` (demos).

Before the first gameplay module lands, extend `f2fix` with the **state-trace hook**: locate the
globals (`gameState`, `frog[]`, `player[]`, actor/enemy lists, `rseed`) in the retail exe by
matching it to source in Ghidra (strings + call graph make this quick), and dump them per tick.

**Exit:** Garden 1 is fully playable start→finish; the shipped `record-*.key` demos replay with
**zero trace divergence** vs the retail oracle.

### Phase 4 — Full single-player
Frontend (`options.c`, `menus.c`, `charsel.c`), story flow (`story.c`), FMV (`f2-bink`),
training, bosses, Super Retro levels, saves (`savegame.c` — keep compatible format + new JSON
slot), language switching, cheats, extras.

**Exit:** game completable in Story and Arcade on both regions; all FMVs play; saves round-trip.

### Phase 5 — Local multiplayer & parity hardening
`multi.c` modes with split-screen; record new demo traces covering every level; fix divergences;
performance pass; packaging (Windows/macOS/Linux, Flatpak/AppImage).

**Exit: v1.0** — parity test suite green on all levels; no known divergences flagged "unexpected".

### Phase 6 — Enhancements, fixes & online
Online multiplayer (rollback/lockstep), widescreen/ultrawide with FOV rules, uncapped framerate
via interpolation, rebinding UI, accessibility (colour-blind-friendly HUD, assist modes),
speedrun timer, controller rumble, optional bug-fix toggles ("authentic" vs "fixed" profile).
See [docs/enhancements.md](docs/enhancements.md) and [research/known-bugs.md](research/known-bugs.md).

### Phase 7 — Modding platform
Virtual filesystem with mod overlays; loose PNG/WAV/OGG/glTF replacements; level packs using the
ported `Etc` script compiler; Lua/Rhai scripting hooks for new behaviour; in-game editor revival.
See [docs/modding.md](docs/modding.md).

## 7. Testing strategy

| Layer | Method |
|---|---|
| Parsers | Golden tests on real data, *gated* behind `F2_DATA_DIR` env var (no assets in repo, CI uses a private cache or skips). Fuzzing (`cargo fuzz`) for every parser. |
| Maths | Differential tests vs original C compiled in tests. |
| Gameplay | Demo replay trace-diff vs instrumented reference build; headless `f2-game` runs thousands of ticks per second in CI. |
| Rendering | Screenshot comparison vs reference captures at fixed ticks (tolerant diff). |
| Import | Synthetic CUE/ISO fixtures generated in tests (no game data needed). |

## 8. Legal & distribution posture (read this)

- The Frogger 2 source was **publicly released for preservation but carries no license**; rights
  ownership (Hasbro Interactive → Infogrames/Atari; Frogger IP → Konami) is unclear.
- A Rust **translation** of that source is a derivative work; it does not become "clean" by being
  re-typed. This is the same risk profile as decomp-based ports (re3 received DMCA notices; Ship of
  Harkinian and others have been tolerated).
- Mitigations we will follow regardless: **no game assets in the repo or releases**, **no original
  binaries/DLLs redistributed**, import only from a user-owned copy, non-commercial, clear
  attribution to Blitz Games / Interactive Studios and the Highway Frogs preservation effort,
  prompt response to any rights-holder request.
- Platform-layer code (importer, renderer, audio, Bink decoder, mod system) can be written
  independently and is cleanly licensable on its own.

## 9. Decisions needed from the project owner

1. **License** for our original code (suggest: **GPL-3.0-or-later** for the game crates à la
   OpenMW/OpenRCT2, and **MIT/Apache-2.0** for reusable crates like `f2-media`, `f2-bink`), with
   the derivative-work caveat above documented in the README.
2. **Project name & hosting** (GitHub org? Keep "Frogger" out of the repo name to reduce
   trademark friction, e.g. "Swampy's Revenge Engine"?).
3. **Platforms for 1.0** — suggest Windows + Linux + macOS; Steam Deck as a stretch; web later.
4. **Oracle environment** — OK to run the retail `Frogger2.exe` (extracted from your discs) on the
   Windows side of this WSL machine via the TPW-style headless/muted harness? (Same setup as TPW:
   Windows Rust toolchain at `/mnt/c/Users/…/.cargo/bin/cargo.exe`, VS on Windows.) Does DirectX 7
   D3D HAL init work there in windowed mode, or do we need a ddraw wrapper (dgVoodoo2/DDrawCompat)?
   We'll test in the Phase 0 spike, but a VM may be needed if the desktop GPU path misbehaves.
5. **Shared crates with TPW** — copy now and diverge, or extract a shared `recomp-kit` workspace
   used by both projects? (Recommendation: copy now, extract after both stabilise.)
6. **Do you have (or want to fetch) the Frogger 2 dev archive** (archive.org `frogger2-dev-archives`)?
   It contains the original object converter and the `.fev` compiler/decompiler + docs that would
   speed up Phase 1 and Phase 7.
7. **Authentic vs fixed defaults** — should 1.0 default to authentic behaviour (incl. known bugs)
   with an "enhanced" toggle, or the reverse?

## 10. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Rights-holder takedown | High | §8 posture; keep platform crates separable; no assets/binaries distributed. |
| Behaviour drift during translation | High | Trace-diff harness from Phase 3 onward; fixed-point kept bit-exact. |
| Variable-timestep logic assumes FPS-dependent quirks | Med | Run reference at fixed 60 Hz too; document intentional deviations. |
| MDX renderer subtleties (sorting, blending, fog) | Med | Screenshot diffs; study `mdxRender.cpp`/`mdxPoly.cpp` closely. |
| Bink decoder effort | Med | Import-time FFmpeg transcode fallback; FMVs aren't on the gameplay critical path. |
| Prebuilt-only libs (gelf, SoftStation, Bink) not in source | Low | Only used for platform functions we replace anyway. |
| Scale (~71.5k LOC + 15k MDX) | Med | Strict phase ordering; vertical slice first; source-map tracking. |

## 11. Index of documents

- `research/source-tree.md` — architecture of the original code, subsystems, LOC, entry points.
- `research/retail-media.md` — disc layouts, hashes, CAB details, file inventory, music track map.
- `research/asset-formats.md` — every on-disk format, loader references, layouts.
- `research/community-and-legal.md` — Highway Frogs, dev archive, legal precedents.
- `research/known-bugs.md` — known PC bugs/quirks and the Enhanced mod's fixes.
- `research/sibling-tpw-reuse.md` — what to lift from the Theme Park World recomp (headless/muted oracle, importer, orchestration).
- `research/prototypes/` — Python format-verification scripts (no game data).
- `docs/architecture.md` — Rust design: crates, data flow, main loop, traits.
- `docs/porting-guide.md` — conventions for translating C → Rust faithfully.
- `docs/source-map.md` — C file → Rust module tracking table with status.
- `docs/asset-import.md` — importer design and user flow.
- `docs/enhancements.md` — fixes & features backlog.
- `docs/modding.md` — modding architecture.
