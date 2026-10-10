# Source map: original C/C++ → Rust

The tracking table for the port. One row per translation unit in the shipping PC build
(`FroggerProject.dsp`, PSX/Editor excluded). **Update the Status column in the same PR that changes
the port.**

Status values: `todo` → `wip` → `ported` (compiles, unit tests) → `verified` (difftest or
trace-parity passes, reviewed) · `replaced` (platform code re-designed, not translated) ·
`skip` (not needed).

Conventions for function-level tracking: each ported Rust function gets a doc comment
`/// Port of `funcName` (Common/file.c:LINE).` so `grep "Port of"` produces the function-level map.
Generated 2026-10-09 from `FroggerProject/FroggerProject.dsp` (LOC via `wc -l`).

**Status summary (game-side, 68 units, audited 2026-10-09 against the Rust code):** ported 41, partial 5, replaced 17, skip 4, todo 1. Function-level gaps: [PORT-GAPS.md](PORT-GAPS.md). Statuses in use: `ported`, `partial`, `replaced`, `skip`, `todo`. Rust paths are flat modules (`crates/f2-game/src/<name>.rs`). Parity: all 26 single-player levels and 3 demos match the retail oracle (see PLAN Status).

| C/C++ file | LOC | Rust target | Status | Notes |
|---|---:|---|---|---|
| `Common/DefData.c` | 41 | f2-game::defdata | ported | dead data (`scoreName`, `hiScoreData`, `saveSlots` unreferenced) |
| `Common/E3_Demo.c` | 255 | (skip: E3 build) | skip | `E3_LEVELSELECT_MODE` is dead in retail (frontend.rs:458) |
| `Common/actor2.c` | 1254 | f2-game::actor2 + f2-engine | ported | `DrawActorList`/`GetHeadMatrix` replaced by the wgpu renderer |
| `Common/anim.c` | 511 | f2-game::anim | ported |  |
| `Common/babyfrog.c` | 468 | f2-game::babyfrog | ported |  |
| `Common/bbtimer.c` | 44 | f2-game::bbtimer | ported |  |
| `Common/cam.c` | 1012 | f2-game::cam | ported | `ControlCamera` (debug) and `SetCamFF` (PSX) intentionally absent |
| `Common/collect.c` | 688 | f2-game::collect | ported | `CHEAT_MAD_GARIBS` sample swap TODO (collect.rs:567); `TransformPoint` for garib fx |
| `Common/crc32.c` | 106 | f2-formats::crc | ported |  |
| `Common/enemies.c` | 2900 | f2-game::enemies | ported |  |
| `Common/evenfunc.c` | 525 | f2-game::evenfunc + event | ported | trigger/assign helpers live in `event.rs` enums; FindVoice per-character TODO (evenfunc.rs:255) |
| `Common/event.c` | 372 | f2-game::event | ported | `DeallocTriggerBlock` unnecessary (Rust ownership) |
| `Common/fixed.c` | 69 | f2-math::fixed | ported |  |
| `Common/flatpack.c` | 340 | (PSX only: skip) | skip |  |
| `Common/frogger.c` | 295 | f2-game::frogger | ported |  |
| `Common/frogmove.c` | 2190 | f2-game::frogmove | ported |  |
| `Common/frontend.c` | 2048 | f2-game::frontend | ported | multiplayer win screens on the real overlays; level-complete/game-over texts still `UiOverlay` stand-ins (PORT-GAPS P2) |
| `Common/game.c` | 759 | f2-game::game + session | ported | game loop / level init flow in `game.rs` and `session.rs` |
| `Common/general.c` | 485 | f2-game::general | ported | `cmemcpy`/`lmemcpy`/`smemcpy` not needed |
| `Common/hud.c` | 1184 | f2-game::hud | ported | multiplayer HUD included |
| `Common/language.c` | 243 | f2-game::language | ported |  |
| `Common/layout.c` | 1329 | f2-game::layout + session | ported | `InitTiming` has no state (fixed 60 Hz); `InitSpriteSortArray` is renderer state |
| `Common/levplay.c` | 173 | f2-game::levplay | ported |  |
| `Common/map.c` | 847 | f2-game::map (+ f2-formats) | ported |  |
| `Common/maths.c` | 1848 | f2-math::maths | ported | PSX matrix-stack helpers not needed |
| `Common/memload.c` | 504 | f2-game::memload (+ f2-formats::entity) | ported | `MemLoadEvents`/`MemLoadString` folded into f2-formats |
| `Common/menus.c` | 2177 | f2-game::menus | ported | pause menu, cheats, frontend loop, `CheckForExtras` |
| `Common/multi.c` | 1821 | f2-game::multi | ported | no retail trace for local multiplayer yet (PORT-GAPS) |
| `Common/newpsx.c` | 55 | (PSX only: skip) | skip |  |
| `Common/options.c` | 3253 | f2-game::options | ported | `RunTeaserScreens`/`ToggleStereo` unused on PC; Gouraud helpers done in-line |
| `Common/overlays.c` | 150 | f2-game::overlays | ported |  |
| `Common/particle.c` | 365 | f2-game::particle | ported |  |
| `Common/path.c` | 274 | f2-game::path | ported |  |
| `Common/platform.c` | 1243 | f2-game::platform | ported |  |
| `Common/ptexture.c` | 497 | f2-game::ptexture | ported |  |
| `Common/savegame.c` | 180 | f2-game::savegame | ported |  |
| `Common/script.c` | 1432 | f2-game::script | ported | all commands ported |
| `Common/sndview.c` | 25 | (debug: skip) | skip |  |
| `Common/specfx.c` | 2457 | f2-game::specfx | ported |  |
| `Common/sprite.c` | 502 | f2-game::sprite | ported | `Print*Sprite` drawing replaced by `frogger::fx` |
| `Common/story.c` | 92 | f2-game::story | ported |  |
| `Common/textover.c` | 453 | f2-game::textover | ported | `PrintText*` drawing replaced by `frogger::overlay` |
| `Common/tongue.c` | 822 | f2-game::tongue | ported | `CalcTongueNodes` done in the renderer |
| `Common/training.c` | 148 | f2-game::training | ported | hints from the level script (`EV_HINT`) |
| `Common/types.c` | 37 | f2-math::types | ported |  |
| `PC/Main.cpp` | 1025 | frogger (bin: main/app) + f2-platform | replaced | window, loop, args; registry/CD-check/installer dropped |
| `PC/actor.cpp` | 293 | f2-game::pc_actor | ported |  |
| `PC/backdrop.c` | 192 | f2-game::backdrop | ported | draw side in `frogger::render` |
| `PC/banks.cpp` | 647 | f2-game::pc_banks + f2-formats | ported |  |
| `PC/controll.cpp` | 1464 | f2-platform::{input,bindings,gamepad} + frogger::controls | replaced | DirectInput/dialogs redesigned (rebinding UI, gamepad) |
| `PC/drawloop.cpp` | 408 | frogger::{app,render} + f2-engine | replaced |  |
| `PC/dx_sound.cpp` | 430 | f2-audio | replaced |  |
| `PC/fadeout.cpp` | 105 | f2-game::fadeout | ported |  |
| `PC/islfile.cpp` | 35 | f2-mods (layered VFS) | replaced |  |
| `PC/isllocal.c` | 105 | f2-game::language | ported |  |
| `PC/isltex.cpp` | 27 | f2-engine::textures | replaced |  |
| `PC/islutil.cpp` | 81 | f2-math | replaced |  |
| `PC/net/dputil.cpp` | 81 | f2-net | replaced | DirectPlay -> rollback/lockstep UDP + loopback |
| `PC/net/netchat.cpp` | 435 | f2-net | replaced | chat not carried over |
| `PC/net/netconnect.cpp` | 779 | f2-net::lobby | replaced |  |
| `PC/net/netgame.cpp` | 634 | f2-net + frogger::netplay | replaced |  |
| `PC/net/netrace.cpp` | 457 | f2-net + frogger::netplay | replaced |  |
| `PC/net/network.cpp` | 530 | f2-net | replaced |  |
| `PC/pcaudio.c` | 1518 | f2-game::pcaudio + f2-audio | ported | CD-audio/MCI/DirectSound parts replaced by `f2-audio`; ambient scheduling in `pcaudio.rs` |
| `PC/pcgfx.c` | 862 | frogger::fx + f2-engine | replaced |  |
| `PC/pcmisc.cpp` | 521 | frogger::{render,overlay} + f2-game::frontend | replaced | `PcNameEntryInit` ported (frontend.rs:1320) |
| `PC/pcsprite.c` | 471 | frogger::fx + f2-engine | replaced |  |
| `PC/video.cpp` | 251 | f2-bink + frogger::video | replaced |  |

Game-side .c/.cpp total: **47824** lines (headers excluded).

| MDX Library file | LOC | Rust target | Status |
|---|---:|---|---|
| `fxBlur.cpp` | 175 | f2-engine (replace) | replace |
| `mdxActor.cpp` | 1025 | f2-engine (replace) | replace |
| `mdxCRC.cpp` | 111 | f2-engine (replace) | replace |
| `mdxD3D.cpp` | 731 | f2-engine (replace) | replace |
| `mdxDDraw.cpp` | 1203 | f2-engine (replace) | replace |
| `mdxDrivers.cpp` | 106 | f2-engine (replace) | replace |
| `mdxDText.cpp` | 327 | f2-engine (replace) | replace |
| `mdxException.cpp` | 142 | f2-engine (replace) | replace |
| `mdxFile.cpp` | 134 | f2-engine (replace) | replace |
| `mdxFont.cpp` | 610 | f2-engine (replace) | replace |
| `mdxInfo.cpp` | 399 | f2-engine (replace) | replace |
| `mdxLandscape.cpp` | 180 | f2-engine (replace) | replace |
| `mdxMath.cpp` | 568 | f2-engine (replace) | replace |
| `mdxObject.cpp` | 876 | f2-engine (replace) | replace |
| `mdxPoly.cpp` | 2235 | f2-engine (replace) | replace |
| `mdxPrcTex.cpp` | 101 | f2-engine (replace) | replace |
| `mdxProbe.cpp` | 29 | f2-engine (replace) | replace |
| `mdxProfile.cpp` | 415 | f2-engine (replace) | replace |
| `mdxRender.cpp` | 2266 | f2-engine (replace) | replace |
| `mdxSprite.cpp` | 256 | f2-engine (replace) | replace |
| `mdxTexture.cpp` | 1019 | f2-engine (replace) | replace |
| `mdxTiming.cpp` | 70 | f2-engine (replace) | replace |
| `mdxWindows.cpp` | 252 | f2-engine (replace) | replace |
| `mgeReport.cpp` | 107 | f2-engine (replace) | replace |
