# Progress toward a full recomp

**As of 2026-10-10** (branch `dev`). Detailed phase table: [PLAN.md Status](PLAN.md#status);
open items: [docs/PORT-GAPS.md](docs/PORT-GAPS.md); per-file C→Rust tracking: [docs/source-map.md](docs/source-map.md).

## Summary

The game is **fully ported and playable** from the player's own disc or image: every single-player
level, local multiplayer, frontend, story, saves, FMV and CD music run in the Rust port. Behaviour is
**verified tick-for-tick against the retail exe** (the "oracle") for all 26 single-player levels, all
local multiplayer modes and the 3 attract demos. What remains is verification breadth (a few
"play to the end" cases), hardware testing on platforms not available here, and owner decisions
before a public release.

| Area | State |
|---|---|
| Asset import (disc, images, drives) | done |
| Engine (renderer, audio, video, input, windowing) | done |
| Gameplay port (`f2-game`, ~71.5k LOC of original C) | done; 41 of 68 units ported 1:1, 17 replaced by platform redesigns, 5 partial, 4 skipped as dead/PSX-only, 1 todo (all non-gameplay) |
| Retail parity, single-player | **verified**: 26/26 levels + 3/3 demos identical |
| Retail parity, local multiplayer | **verified**: 38 cases, 0 regressions |
| Retail parity, "play to the end" (finish corpus) | 38 cases identical incl. `savegame.dat`: 22/26 single-player levels played to the end, all 16 multiplayer match ends; last 4 in progress (below) |
| Enhancements | done (first cut) |
| Online play | done (first cut, not soak-tested on real networks) |
| Modding + level editor | done (first cut) |
| Platforms | Windows: supported, installed at `C:\Frogger2-rs`; Linux: supported (AppImage tested); macOS: compiles, first native build is in CI |

## Done

- **Import** (`f2-import`, `f2-media`): Redump zip / BIN+CUE (single or per-track), CloneCD,
  Alcohol MDS, Nero NRG, cdrdao TOC, plain ISO, zips of all of these, disc folders, install folders,
  physical drives (Windows and Linux, CD-DA ripping with read-offset detection against the Redump
  hashes), Windows-mounted images. Native InstallShield 5 CAB reader (byte-identical to `unshield`).
  First-run import screen in the game itself. No game data is ever shipped.
- **Formats** (`f2-formats`): object banks (PE DLLs, relocation-checked), FLA2 textures, WUS sound
  banks, collision, entities, scripts, scenics, demo input, language table; writers for the editor.
- **Engine**: wgpu renderer (authentic 640×480 look or enhanced), cpal audio with 3D sound and CD
  music, a pure-Rust Bink 1 decoder for the FMVs, winit/gilrs platform layer.
- **Gameplay** (`f2-game`): platform-free and deterministic at a fixed 60 Hz; bit-exact 20.12
  fixed-point maths, RNG and x87-precision float paths; snapshot/rollback for netplay.
- **Full game flow**: frontend, menus, options, story, training, cheats, saves, level complete /
  game over, multiplayer race/battle/collect with HUD and win screens.
- **Verification tooling**: `f2fix` oracle (retail exe run headless, muted, fixed timestep, per-tick
  traces and framebuffer dumps), `f2-trace`, `f2-parity` with the random-input corpus and a
  planner-driven finish corpus, difftests of pure functions against the original C.
- **Enhancements**: Authentic and Enhanced display profiles; Hor+ widescreen and ultrawide, now
  without the 4:3 culling at the screen edges; render scale, MSAA, anisotropic filtering, fog/draw
  distance, frame cap, VSync, FPS counter; quality-of-life options.
- **Settings screen** in the game's own style: from the pause menus and from Extras. Resolution
  (Auto or any mode, including 640×480), windowed / borderless / exclusive fullscreen, Alt+Enter
  following the chosen fullscreen type, keyboard rebinding.
- **Controllers**: native support with the PS1 layout, rebinding, remembered devices per player.
- **Online**: `f2-net` rollback/lockstep over UDP, lobby, spectators.
- **Modding**: layered mod file system, F2S script tool (`f2-etc`), `f2-modkit`, in-game level
  editor (`--editor`).
- **Cross-platform groundwork**: `f2-math` is `no_std`; CI matrix for Windows, Linux and macOS;
  packaging (Windows installer script, AppImage, macOS universal `.app`).

## In progress

- **Last 4 puzzle levels in the finish corpus** (agent running): 1-0, 1-2, 2-0 and 6-2 are not yet
  played to the end automatically (deadly-tile timing, a superhop, platform-heavy search, a gated
  level-ending trigger); hand-written waypoints and planner improvements.
- **`f2-game` `no_std` + `alloc`** (agent running) for future console targets (blockers listed in
  [docs/PLATFORMS.md](docs/PLATFORMS.md) §5).

Fixed today by the finish corpus: the frog's celebrate animation reads past the end of the animation
table in the original (an off-by-one the exe gets away with; ported faithfully, known-bugs #28), and
the PC-only teleport dissolve texture that the port had dropped.

## Remaining

- Hardware testing not possible here: CD-DA ripping from a real optical drive, a native macOS run,
  real HiDPI displays, online play over real networks.
- Small display-only gaps in [docs/PORT-GAPS.md](docs/PORT-GAPS.md) (e.g. level-complete texts
  drawn by stand-in overlays).
- **Known limit of ultrawide**: actors outside the original 4:3 view are now drawn, but they hold
  their last pose and effects there are not spawned, because the original simulation does not update
  them off-screen. Changing that would break retail parity, so it stays.
- **Owner decisions before a public release** ([PLAN.md §9](PLAN.md#9-decisions-needed-from-the-project-owner)):
  project license and distribution posture (the original source carries no license).
- **Future, not started**: console ports (PS2–PS5, N64). An N64 build looks size-feasible on a
  64 MB cartridge without FMV.

## Numbers

- ~95k lines of Rust across 14 crates (incl. the `f2fix` oracle DLL) and 5 tools; 139 commits since 2026-10-09.
- Task graph `work/tasks.json`: 41/41 tasks done.
- Parity: 3 demos + 26 single-player + 38 multiplayer random-input cases + 38 finish cases identical (102 cases, 0 regressions)
  to retail.
