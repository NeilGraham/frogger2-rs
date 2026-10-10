# Decisions log

Owner decisions from `PLAN.md` §9. Until the owner answers, the **recommended defaults are
applied** so work can proceed; each is easy to revisit.

| # | Question | Status | Applied default |
|---|---|---|---|
| 1 | License | default | Translated game code (`f2-math`, `f2-game`, `frogger`): **GPL-3.0-or-later** (derivative of unlicensed source; see PLAN §8). Independent platform/format crates (`f2-media`, `f2-formats`, `f2-import`, `f2-engine`, `f2-audio`, `f2-platform`, `f2-bink`, `f2-mods`, tools): **MIT OR Apache-2.0**. Code adapted from `theme-park-world-recomp` (MIT, same owner) is marked in file headers. |
| 2 | Name & hosting | default | Working title `frogger2-rs`; crate prefix `f2-`. No public hosting decided. |
| 3 | Platforms for 1.0 | default | Windows x64, Linux x64, macOS. CI on Linux first. |
| 4 | Oracle environment | default | Retail `Frogger2.exe` extracted only with `f2-import --oracle`, run on the Windows side via WSL interop, headless + muted (TPW harness). |
| 5 | Shared crates with TPW | default | Copy now (attributed), extract a shared kit later. |
| 6 | Dev archive | open | Not available locally; work proceeds from `frogger2-vss`. |
| 7 | Authentic vs fixed default | default | Parity work targets **authentic**; players' default profile decided before 1.0. |

## Technical decisions made during implementation

| Date | Decision | Why |
|---|---|---|
| 2026-10-09 | `f2-media` implements InstallShield 5 "old compression" (per-chunk raw deflate split at `00 00 FF FF`) natively | Output verified byte-identical to `unshield -O` on USA + EU |
| 2026-10-09 | Object-bank pointers are trusted only if the PE `.reloc` table relocates that field | Some banks (e.g. `language.dll`) contain junk in unused pointer fields (`bBox` = `0xffff`) |
| 2026-10-09 | Stale `entity-*.dat` files (version 15: 4-5, 4-6, 6-2, 6-3, 6-5, 8-10) are skipped, as the game rejects them | Matches `MemLoadEntities` |
| 2026-10-09 | Music imported as WAV (lossless, simple); FLAC optional later | No extra dependency; 6 s import |
| 2026-10-09 | Game data paths lowercased on import; VFS is case-insensitive anyway | Linux/macOS friendliness |
| 2026-10-09 | `f2-bink` is LGPL-2.1-or-later (close translation of FFmpeg's Bink decoder) | Fastest route to a verified decoder; isolated crate keeps licensing clear |
| 2026-10-09 | Tools/game may link LGPL `f2-bink` (dynamic relinking not required: full source is public; revisit if binaries are ever distributed) | Keep FMV support simple |
| 2026-10-09 | Under WSLg, windows use X11 (Wayland crashes Weston); `F2_WAYLAND=1` overrides | Observed compositor crashes |
| 2026-10-09 | Float maths that feeds game decisions emulates the retail x87 control word (24-bit precision, set by Direct3D): `f2_math::arccos` rounds each step to `float` (arccos(0)=1024, exact 90-degree `Orientate`), MDX view/culling maths in `f2-game/src/view.rs` is plain `f32`. The f2-math difftest build rewrites `arccos` in maths.c the same way (`arccos_f64` keeps the 53-bit host semantics) | Verified against the oracle: 0-0, 1-0, 6-1 demos identical on all v2 sections |
| 2026-10-09 | The simulation owns a 640x480 MDX view model (`view.rs`: `CalcViewMatrix`, `XfmPoint`, `CheckBoundingBox`, `ActorListDraw` sphere test, `CopyActorList`, frog head matrix); `IsPointVisible`/`clipped`/`distanceFromFrog`/animation advance never depend on the host display | Retail decides effect creation, collisions and animation from the drawn view |
| 2026-10-09 | `Game::tick` = `tick_game_loop` (trace point, as the oracle) + `after_game_loop` (CopyActorList + DrawLoop state); demo start runs `after_game_loop` once (InitLevel happens inside the frontend's GameLoop) | Oracle trace lines are written before DrawLoop |
| 2026-10-09 | RETAIL: `CalcTongueSource` evaluates `(up*5 + pos + fwd*12) * SCALE` (FUN_00434370 order) | Shipped exe differs in evaluation order from the VSS source |
| 2026-10-09 | RETAIL: `rsin`/`rcos` round `x/4096`, the angle and `sin*4096` to `float` (24-bit x87, as `arccos`); the f2-math difftest build rewrites the `libgte.h` macros the same way | Parity corpus 1-1: `UpdateGallopingEnemy`'s unsigned `clam` (~2e8) gave y 480 vs retail 479 with 53-bit maths |
| 2026-10-09 | Draw-time state the original changes in `DrawLoop` is simulated: `UpdateFrogCroak` (croak ring, timer, baby reply, head/breast post-matrix swelling that moves `headMatrix` and so the tongue source) and `DrawFXRing`'s `FXTYPE_TWINKLE` trails (odd frames, odd on-screen ring segments; consumes `Random`) | Parity corpus: croak/tongue/specfx divergences in 15 levels |
