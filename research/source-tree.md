# Original source tree — architecture map

Source: `frogger2-vss/` = git conversion of the end-of-project VisualSourceSafe backup
`FINAL 18042001.ssa` (Highway Frogs). Paths below are relative to `frogger2-vss/teamSpirit/`.
Researched 2026-10-09.

## 1. Which tree is the shipping PC game?

**`FroggerProject/`** (+ `MDX Library/`). Evidence:

| | `FroggerProject/FroggerProject.dsp` | `FroggerII/Frogger.dsp` |
|---|---|---|
| Release defines | `PC_VERSION FINAL_MASTER USE_BINK_VIDEO` (l.47) | `PC_VERSION DONTUSEJALLOC USE_EDITOR` |
| Output | `Release/Frogger2.exe` | — |
| Links | `mdx.lib softstation.lib d3dim ddraw dsound dinput dplay binkw32 gelf` | no MDX/Bink/DPlay |
| History | 2000-02-10 → 2000-12-08 (1547 commits) | 1999-06-25 → 2000-05-12 |

`FroggerII/` is the older N64-era codebase. `PSX DEMO/`, `dreamcast/`, `DC Frogger2 MsDev/` share
`Common/` lineage and are useful cross-references (e.g. functions missing in PC snapshot).
`Editor/` (6.3k LOC) is excluded from release (`#ifndef FINAL_MASTER`, `PC/Main.cpp:736`).

A build-ready copy lives at <https://github.com/HighwayFrogs/frogger2-pc> (VC6 + DX7 SDK; Debug
config only builds — see `community-and-legal.md`).

## 2. Size

| Area | LOC (.c/.cpp/.h) |
|---|---|
| `FroggerProject/Common` | 43,257 |
| `FroggerProject/PC` (incl. `net/` 3,098) | 12,703 |
| `MDX Library` | 15,557 |
| **Shipping PC total** | **≈71,500** |
| Editor (debug only) | 6,352 |
| Tools (`Etc`, `BankManager`, `Object Banker`, `Frogger2 Tools`) | ≈19,000 |

Largest files: `options.c` 3253, `enemies.c` 2900, `specfx.c` 2457, `mdxRender.cpp` 2266, `mdxPoly.cpp` 2234.

## 3. Entry point, loop, state machine

- `WinMain` — `FroggerProject/PC/Main.cpp:885`: registry/args → language → `LoadGame` → CD check
  (`FindFrogger2CD`, volume label `FROGGER2`) → `WindowsInitialise` → `StartNetworkGame` →
  `InitInputDevices` → DDraw/D3D init loop → `GameStartup` (`Main.cpp:777`) → `CommonInit`
  (`Common/layout.c:1287`) → `AppLoop = LoopFunc` → `RunWindowsLoop()` (`MDX/mdxWindows.cpp:216`).
- Per frame `LoopFunc` (`Main.cpp:686`): timing → `ProcessUserInput` → `GameLoop()` (or
  `NetgameRun`) → `CopyActorList` → `DrawLoop()` (`PC/drawloop.cpp:142`, ends with `DDrawFlip`)
  → `UpdateTiming()`.
- `enum gameModes` (`Common/define.h:19-53`): `FRONTEND, TRAINING, INGAME, LEVELCOMPLETE,
  WORLDCOMPLETE, GAMECOMPLETE, GAMEOVER, INTRO, CAMEO, PAUSE, DEMO, CHARSELECT, FMVPLAY,
  CONTROLLERVIEW, E3_LEVELSELECT, MULTI_WINRACE, MULTI_WINCOLLECT, MULTI_WINBATTLE, ARTVIEWER,
  STARTUP, TEASERSCREEN, THEEND` (`_MODE` suffix). Held in `gameState.mode`
  (`struct gameStateStruct`, `define.h:73`) with `multi` (SINGLE/MULTILOCAL/MULTIREMOTE),
  `single` (ARCADE/STORY), `difficulty`.
- `GameLoop` = `Common/frontend.c:144-283`; `INGAME_MODE` → `RunGameLoop` (`Common/game.c:529`).
  `STARTUP_MODE` plays Hasbro → Blitz → intro Bink, then `FRONTEND_MODE`.

## 4. Timing & maths (critical for parity)

- `InitTiming(60.0)` (`Main.cpp:827`): 60 Hz *logical* frames, **variable timestep**.
  `UpdateTiming` (`mdxTiming.cpp:50-69`): `speed = elapsed_ms*60/1000`, step clamp
  `maxTickIncrement = 500` ms. `gameSpeed = 4096*speed` (20.12; 4096 = one 60 Hz frame);
  `actFrameCount` = elapsed 60 Hz frames. Pause freezes via `pFrameModifier`.
- Gameplay maths **fixed-point 20.12**: `typedef long fixed`, `ToFixed(x)=x<<12`, `FMul/FDiv`
  (`Common/fixed.h`, `fixed.c`). `FVECTOR` (fixed), `SVECTOR` (i16), `IQUATERNION`.
- Angles PSX-style, 4096 = 360°: `rsin/rcos`, `bb_acostable[4097]` (`maths.h:65`).
- MDX rendering uses `float`.
- Y-up (`upVec = {0,4096,0}`, `frogger.c:64-67`); world `SCALE 10` (`newpsx.h:6`).
- Overlays use a virtual 4096×4096 space (`OVERLAY_X = xRes/4096.0`, `Main.cpp:981`).
- Default res 640×480, 16bpp (`DDrawCreateSurfaces(...,16,...)`, `Main.cpp:985`); config dialog
  320×240…1280×1024.

## 5. Subsystems (game-side files)

| Subsystem | Files (FroggerProject/…) | ≈LOC |
|---|---|---|
| Main loop / state | `PC/Main.cpp`, `Common/frontend.c`, `game.c`, `levplay.c` (demo playback), `story.c` (story order + FMV list), `training.c`, `E3_Demo.c`, `bbtimer.c` | 4.9k |
| Rendering (game side) | `PC/drawloop.cpp`, `pcgfx.c`, `pcsprite.c`, `backdrop.c`, `fadeout.cpp`, `actor.cpp`, `pcmisc.cpp`; `Common/actor2.c`, `anim.c`, `sprite.c`, `specfx.c`, `particle.c`, `ptexture.c`, `overlays.c`, `textover.c` | 10.4k |
| Rendering engine | `MDX Library/*` (D3D7 HAL + SoftStation software rasteriser) | 15.6k |
| Input | `PC/controll.cpp` (DirectInput 7; keyboard/joystick → PSX-style `padData`/`PAD_*`; key-map dialog; record/playback) | 1.6k |
| Audio | `PC/pcaudio.c` (sample banks, 3D/ambient voices, MCI `cdaudio` music), `PC/dx_sound.cpp` (DirectSound3D) | 2.3k |
| Video | `PC/video.cpp` (Bink → DDraw surface) | 0.3k |
| Level load | `Common/layout.c` (`worldVisualData`, `InitLevel`), `map.c`, `memload.c`, `flatpack.c`, `PC/islfile.cpp`, `PC/banks.cpp` | 5.3k |
| Tiles / collision | `Common/general.c` (`FindNearestTile*`, `FindJoinedTileByDirection*`), `enemies.c` (`DoEnemyCollision`, `RadiusCheck`); tile states `define.h:86+` | — |
| Frog | `frogmove.c` (hop/jump/land/death), `tongue.c`, `frogger.c`, `babyfrog.c`, `collect.c` | 5.1k |
| Enemies / platforms / paths | `enemies.c`, `platform.c`, `path.c` | 4.8k |
| Camera | `cam.c`, `Cam.h` (follow cam, camera boxes, transitions) | 1.2k |
| Frontend / menus / HUD | `options.c` (3D frontend), `menus.c` (pause + cheats), `hud.c`, `charsel.c` | 7.4k |
| Save | `Common/savegame.c` (PSX memcard-shaped block) → `savedata\savegame.dat` (`PC/banks.cpp:451-477`) | — |
| Multiplayer | `Common/multi.c` (race/collect/battle), `PC/net/*` (DirectPlay) | 5.0k |
| Scripting | `script.c` (FEV bytecode VM), `event.c`, `evenfunc.c` | 2.5k |
| Language | `PC/isllocal.c`, `Common/lang.h`, `language.c` | — |
| Cheats | `menus.c:128-137` `cheatCombos[]` (d-pad sequences) | — |
| Maths | `maths.c/.h`, `fixed.c`, `types.*`, PSX shims (`PC/lib*.h`, `gtemac.h`, `isl*.h`, `ultra64.h`, `sonylibs.h`) | 2.3k+ |

## 6. Worlds / levels / music (from `layout.c`, `pcaudio.c`)

- World IDs (`Common/layout.h:237`): GARDEN, ANCIENT, SPACE, CITY, SUBTERRANEAN, LABORATORY,
  HALLOWEEN, SUPERRETRO, FRONTEND, TEST. PC `origWorldVisualData` is at `layout.c:329`; each
  level row = name string, level/next-level IDs, object bank, collision bank, open state,
  camera params, credit initials (e.g. `"LOZ"`), timers, stak file.
- City world on PC has only the multiplayer level (single-player City levels commented out).
- CD-audio track enum (`PC/pcaudio.c:128-145`): 2 Garden, 3 Ancients, 4 Space,
  5 Subterranean, 6 Laboratory, 7 Halloween, 8 Super Retro (also City), 9 Frontend,
  10 Level complete, 11 Game over, 12 Level complete loop.

## 7. External dependencies (all replaced in the port)

DirectX 7 (DDraw7, D3D7 IM, DInput7, DSound3D, DirectPlay), Bink SDK (`binkw32`), `gelf` (ISL
image loader, prebuilt), SoftStation (prebuilt software rasteriser), WinMM (`timeGetTime`, MCI).
No Miles/Smacker.

## 8. Tools in the tree (valuable for modding)

- `Etc/` — compiler for the event/trigger language → `.fev` (`etc.cpp:29`); can emit C header.
- `BankManager/` — builds object banks; generates the C that became the `objects/*.dll`
  (`drawlists.cpp`).
- `Object Banker/` — Borland C++Builder bank editor.
- `Frogger2 Tools/` — `WorldConv` (world → C/collision), `ObjConv` (`.obe`→`.psi`),
  `texturebanker`, `soundbanker` (WAV → CRC-keyed `.wus`), `animtex`, `Autorun`.

## 9. Known gaps in this snapshot

- `GameStartup()` definition missing (PC calls to `gameTextInit`/`InitFont` presumably live there).
- `mdxLoadBackdrop()` declared (`mdxDDraw.h:122`) but body absent.
- Check `frogger2-pc` and the dev archive for these (they may be fixed there).
