# Port gaps

Audit of 2026-10-09: grep of `crates/` for `TODO(port`, `TODO(merge`, `todo!(`, `unimplemented!`
(none found) and log-only stubs, cross-checked against the function lists of the original C
(`docs/source-map.md`). Single-player parity (all 26 levels and 3 demos) is verified against the retail
oracle, so the gaps below are mostly outside the single-player trace. Line numbers are as of this commit.

## P1: gameplay-affecting

| Where | Gap |
|---|---|
| `f2-game/src/evenfunc.rs:255` | `FindVoice` per-character resolution deferred to the audio layer. |
| Local multiplayer oracle | `tools/run/run.py` / f2fix have no multiplayer start (`start.demo`/`start.level` only), so local multiplayer (and its HUD) has no retail trace yet; needs an f2fix `start.multi=W-L-N` (GoMulti: `multiplayerMode`, characters, `NUM_FROGS`). |
| Online multiplayer parity | Netplay (`f2-net`, `frogger/src/netplay.rs`) is a redesign, not verified against the retail exe; chat (`netchat.cpp`) not carried over. Local multiplayer has no retail trace parity yet. |

## P2: front-end and data

| Where | Gap |
|---|---|
| `f2-game/src/frontend.rs` (level complete, game over) | Level-complete and game-over texts/icons are still `UiOverlay` stand-ins drawn by `frogger/src/overlay.rs` (`ui_overlays`), not entries of the text/sprite overlay lists (draw order and the 40-entry list limit differ; display only). |
| `Common/E3_Demo.c` | E3 level select not ported; dead in retail (`frontend.rs:458`), kept as `skip`. |
| `options.c` `RunTeaserScreens`, `ToggleStereo` | Not ported; unused in the PC build. |

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

## Closed (2026-10-09, gameplay gap-filling)

Multiplayer HUD (`InitMultiHUD`/`EnableMultiHUD`/`DisableMultiHUD`/`UpDateMultiplayerInfo`, hud.rs),
`numText`/`penalText` as real text overlays with the penalty slide (multi.rs), the multiplayer win
screens and `DoEndMulti` on the real overlays (frontend.rs, `SlideTextOverlayToPos` in overlays.rs),
`TransformPoint` (view.rs; multi player numbers, penalty text, coin zoom), training hints from the
script (`EV_HINT` → `TrainingHint`, `STR_TRAINING1` = 92) on real overlays with `StringWrap`,
`CHEAT_MAD_GARIBS` garib texture and `CHEAT_INFINITE_LIVES` in `KillFrog`, `StopSong` in
`FreeAllLists`, moolard cheat file probe (`layout.moolard_cheat`, "mullard" croak), `InitTiming`
(no state) and `StoryStartLevel` (already in `init_level_after_free`) notes, `UpdateCompletedLevel`,
game-over `SetTimeForLevel` + `PrepareSong(AUDIOTRK_GAMEOVER)`, "the end" text on a 2-entry text
list, level-complete coin on the real `arcadeHud.coinsOver`, `CreateOverlaysFromLogo`, DefData.c
tables (`defdata.rs`), `FRONTEND_OBJ_BANK` loads nothing (banks.cpp:323), stale TODOs removed.
