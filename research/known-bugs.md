# Known bugs, quirks & community fixes (PC)

Sources: Frogger 2 Enhanced (`HighwayFrogs/frogger2-pc` `fixes` branch, `Files/FEATURES.MD`,
`Files/RELEASE.MD`), source reading, and research 2026-10-09. TODO: read the `fixes` branch
diffs in full and turn each into a tracked issue with a root cause.

Legend for "Port": **free** = disappears by construction in the Rust architecture;
**toggle** = keep authentic behaviour by default, fix behind the "enhanced" profile;
**fix** = always fix (crash/corruption).

| # | Bug / limitation | Root cause (known/suspected) | Enhanced mod | Port |
|---|---|---|---|---|
| 1 | Max resolution 1024×768-ish; no widescreen | Config dialog list; 4:3 projection & overlay maths | High-res + widescreen, sprite/coin scaling | free (any res) + widescreen rules |
| 2 | 16-bit colour only | `DDrawCreateSurfaces(…,16…)` hard-coded | 32-bit | free |
| 3 | Half-screen / wrong-colour fullscreen images, garbled level-select book in 32-bit | 16bpp assumptions in backdrop/surface copies | fixed | free |
| 4 | Camera divide-by-zero crash | `cam.c:539` `SlurpCamPosition`: `(gameSpeed<<12)/FMul(camSpeed,transCamSpeedMult)` with a zero divisor | fixed | fix (guarded in both profiles, `cam.rs` `slurp_factor`) |
| 5 | Config window sometimes empty resolution list / hidden mouse | device enumeration | always show window | free (own launcher/settings) |
| 6 | **Music doesn't loop / end-of-level music skipped on Vista+** | MCI CD-audio notify no longer delivered | WAV music from `music\` | free (streamed ripped tracks) |
| 7 | Pause always resumes Garden track | `UnPauseAudio` (pcaudio.c:764) resumes with a bare MCI `PLAY` (no track/position) after `STOP` (suspected; Garden = first audio track) | fixed | free (music stream paused/resumed) |
| 8 | Needs CD in drive for videos | `FindFrogger2CD` | play from `Video\` | free (imported data dir) |
| 9 | **Gliding lasts longer at high FPS (windowed)** | variable-timestep integration in frog glide | not fixed | free (fixed 60 Hz sim) — (The opt-in retail timestep `Game::tick_timed`/`tick_frames` reproduces it.) |
| 10 | Particles render wrong / too many | frame-rate artefact: smoke on `frameCount&1` (rendered frames, enemies.c:1239), `actFrameCount`-gated spawns repeated when frames share a count, specfx fades `(fade*gameSpeed)>>12` truncating to 0 at high FPS so pools (512 particles / `maxSpecfx`) fill and new effects fail | not fixed | free (fixed 60 Hz tick) — Reproduced on purpose by the N64 variable timestep (phase 3T, `f2_game::timing`): at even k `actFrameCount & 1` effects never fire, smoke is per tick, `GTUpdate` timers run 1.6–4.9 % slow at k = 2–4 (research/n64-performance.md §3T). |
| 11 | Last level: can get stuck after vacuum tube | Swampy 3 (`script-6-4.fev`): `KillFrog` leaves `currPlatform` set, so a dead frog still satisfies `FrogOnPlatform` (evenfunc.c:90) and is fired out by `EV_SPRING_P` (script.c:1071, no dead check); respawning mid-flight skips the landing trigger and the tube is parked -> softlock. Designers guarded tube 5 only. (Source reading; not reproduced yet) | not fixed | toggle `fix_tube_softlock` (enhanced): dead frog is not on a platform |
| 12 | Software renderer half-screen, purple/green | SoftStation 565/555 mismatch | n/a | free (no software renderer) |
| 13 | Settings corrupted after DirectX failure | registry write on failure path | — | free (own config file) |
| 14 | No FPS cap (game runs uncapped in windowed mode) | — | FPS cap | free (vsync/pacing) |
| 15 | Keyboard-only rebinding via dialog; DInput joystick only | DX7 era | — | free (gilrs + rebinding UI) |
| 16 | Online multiplayer via DirectPlay (dead) | — | — | Phase 6 replacement |

Bugs discovered during the port should be appended here with: repro, root cause (file:line in
original), authentic-vs-fixed decision, test (demo/trace) that pins it.

## Found during the port

| # | Bug | Root cause | Port |
|---|---|---|---|
| 17 | First keypress of a demo replay registers randomly | `RunDemoMode` (`Common/levplay.c`) reads `playKeyList[-2]` — heap header bytes before the key buffer — to decide whether the first recorded press applies | Treat that value as 0 (oracle does the same) → first press always registers |
| 18 | Volume 238 plays at full volume | typo in the original volume table (`PC/pcaudio.c`) | reproduced, `BUG(original)` |
| 19 | Looping sounds resumed after unpause play once | `UnPauseAudio` | reproduced in authentic profile |
| 20 | Music never loops on PC even pre-Vista; end-of-level loop handoff never fires | retail never calls `LoopSong` | port loops (intended behaviour) |
| 21 | Demo pad-clear loop clears pad 0 four times | `levplay.c` loop uses index 0 | reproduced |
| 22 | Uninitialised `dirNumber`/`res` in `GetTilesMatchingDirection`/`PushFrog` | `frogmove.c` | deterministic value reproduced (see BUG(original)) |
| 23 | `FroggerHop` indexes `inTile[]` by player number | `frogmove.c` | reproduced |
| 24 | `AddEnemy` compares array addresses → every enemy inserted at list head | `enemies.c` | reproduced |
| 25 | `CreateFrogger` checks `> FROG_NUMFROGS` instead of `>=` | `frogger.c` | reproduced |
| 26 | Multiplayer camera divides by `camDiv[index]` that can be zero/mis-indexed | `multi.c` | guarded, BUG(original) |
| 27 | `babiesSaved` never incremented in multiplayer | `multi.c` | reproduced |
| 28 | `Animate` only rejects `animNum > numAnimations`; `== numAnimations` (the frog's celebrate anim 42 on the level-complete screen) reads the 12 bank bytes after the table | `mdxActor.cpp` | reproduced (bank bytes, f2-formats `anim_past_end`) |
