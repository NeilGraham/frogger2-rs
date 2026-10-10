# Port gaps

Audit of 2026-10-09: grep of `crates/` for `TODO(port`, `TODO(merge`, `todo!(`, `unimplemented!`
(none found) and log-only stubs, cross-checked against the function lists of the original C
(`docs/source-map.md`). Single-player parity (all 26 levels and 3 demos) is verified against the retail
oracle, so the gaps below are mostly outside the single-player trace. Line numbers are as of this commit.

## P1: gameplay-affecting

| Where | Gap |
|---|---|
| `f2-game/src/hud.rs:9,323,375,403` | Multiplayer HUD not ported: `InitMultiHUD`, `EnableMultiHUD`, `DisableMultiHUD`, `UpDateMultiplayerInfo` (hud.c). Multi-player timers, trophies, penalty text and centre text are never shown or hidden. |
| `f2-game/src/multi.rs:807,1466,1469` | `multiTimer`/`mpl[]`/`gameWinner` HUD updates; trophies `draw = 0`; `centreText->draw = 1; EnableHUD()` left as TODO. |
| `f2-game/src/multi.rs:343` | `SlideTextOverlayToPos` penalty-text slide (needs hud/textover port; `SlideTextOverlayToPos` also absent from `frontend.rs`). |
| `f2-game/src/frontend.rs:1363,1380,1422,1448` | Multiplayer result screen: `multiHud.centreText` position, `countdownString`, `backChars`/`penaliseText` slides, trophies drawing. |
| `f2-game/src/multi.rs:811`, `collect.rs:242` | `TransformPoint` (view transform) returns the origin; affects multiplayer/garib screen-space effects. |
| `f2-game/src/script.rs:717-718` | Script `TrainingHint` command only logs; `Game::training_hint` (`training.rs:81`) exists but is not called. Training-level hint text never appears from scripts. |
| `f2-game/src/hud.rs:767-769` | `SetTimeForLevel()` (story/records) and `PrepareSong(AUDIOTRK_GAMEOVER)` not performed on game over. |
| `f2-game/src/layout.rs:570,644,647,764,769` | Level init/free: `StopSong`, `FreeAmbientSoundList`, `InitTiming(60)`, `StoryStartLevel()` in story mode, `InitSpriteSortArray`/`InitSampleList`/`InitAmbientSoundList`, moolard cheat file. `UpdateCompletedLevel` (layout.c) has no Rust function. |
| `f2-game/src/collect.rs:567`, `hud.rs:90` | `CHEAT_MAD_GARIBS` ("RGARIB01" garib sample/texture) not wired in collect/hud (frontend.rs:1021 does it). |
| `f2-game/src/frogmove.rs:1532` | Comment says cheats not ported; `CHEAT_INFINITE_LIVES` state should be checked against `menus.rs` cheat state. |
| `f2-game/src/evenfunc.rs:255` | `FindVoice` per-character resolution deferred to the audio layer. |
| Online multiplayer parity | Netplay (`f2-net`, `frogger/src/netplay.rs`) is a redesign, not verified against the retail exe; chat (`netchat.cpp`) not carried over. Local multiplayer has no retail trace parity yet. |

## P2: front-end and data

| Where | Gap |
|---|---|
| `Common/DefData.c` (no Rust) | Default hi-score names (`scoreName`, "jam" x N); no Rust equivalent found. Only matters for the high-score table defaults. |
| `f2-game/src/frontend.rs:520,285` | `FreeTextOverlayLinkedList(); InitTextOverlayLinkedList(2)` and `arcadeHud.coinsOver` (level-complete) marked `TODO(merge wave E)`; verify they are done in `hud.rs`. |
| `f2-game/src/frontend.rs:1630,1636` | Stale `TODO(merge multi)` comments (`mpl[i]` accessors); multi.rs now exists, confirm and remove. |
| `Common/E3_Demo.c` | E3 level select not ported; dead in retail (`frontend.rs:458`), kept as `skip`. |
| `options.c` `RunTeaserScreens`, `ToggleStereo` | Not ported; unused in the PC build. |
| `f2-game/src/actor2.rs:466-468` | Stale `TODO(port)` for `TeleportActorToTile`; it exists in `evenfunc.rs:291`. `ResetUniqueActorList` has no PC definition. |

## P3: rendering/glue and cosmetics

| Where | Gap |
|---|---|
| `frogger/src/render.rs:100,124` | Stand-in frog comment (`CreateFrogger` TODO) is stale if `f2-game` creates frogs; remove after check. |
| `f2-game/src/pc_actor.rs:10` | `TODO(port)` revisit camera interplay for the per-actor update. |
| `f2-game/src/babyfrog.rs:45`, `collect.rs:37,120` | Cosmetic: move `FXTYPE_*` and overlay types to `specfx.rs`/HUD. |
| `f2-game/src/tongue.rs:812` | Logs `CheckForAttachedNMEs: endless loop in the original` (BUG(original) path; keep). |
| `f2fix/src/win/shims.rs:441` | Oracle shim stubs MCI (CD audio) intentionally; test tool only. |

## Known intentional replacements (not gaps)

DirectX/Win32/MCI/DirectPlay layers (`PC/*.cpp`, MDX Library, `net/`), registry/CD checks, PSX-only
files (`flatpack.c`, `newpsx.c`, `sndview.c`), `ControlCamera` and `SetCamFF` (debug/PSX).
