# Enhancements & fixes backlog

Everything here comes **after** parity for the affected subsystem (Phase 6+), and is selectable via
profiles:

- **Authentic** — matches retail behaviour (parity tests run in this profile).
- **Enhanced** (default for players, project decision) — bug fixes + QoL.
- Per-option overrides in `config.toml`.

## Fixes (see `research/known-bugs.md` for root causes)

Gameplay options (P6-1) live in `f2_game::enhance::GameOptions` (part of `SessionConfig`,
`Game::set_game_options`); every hook is a no-op in the authentic profile and in recorded
demos (`DEMO_MODE` always replays retail behaviour), so the 0-0 / 1-0 / 6-1 demo traces stay
identical to the oracle. Host side: `crates/frogger/src/gameplay.rs`, config `[gameplay]`
(`docs/BUILDING.md`), edited in game with Settings (pause menu / Extras / F10) -> Gameplay.

- [x] Camera divide-by-zero crash (#4): `SlurpCamPosition`'s `(gameSpeed<<12)/FMul(camSpeed,
  transCamSpeedMult)` with a zero divisor; the port guards it in both profiles (logs, factor
  4096 = snap) — a crash is never "authentic" (`cam.rs` `slurp_factor`). The multiplayer
  `camDiv` division is guarded too (#26).
- [x] Pause resumes wrong music (#7): free by construction — retail `UnPauseAudio` resumes with a bare
  MCI `PLAY` (no track / position) after `PauseAudio`'s `STOP` (suspected cause: the device does not
  resume the level's track; Garden is the first audio track); the port pauses and resumes the same music stream (`f2-audio`
  `pause_audio`/`unpause_audio`), so the level's own track continues.
- [x] Particle over-spawn/render (#10): a frame-rate artefact, resolved by the fixed 60 Hz tick.
  Retail spawns smoke on `frameCount&1` (rendered frames, `enemies.c:1239`), repeats
  `actFrameCount`-gated spawns when several frames share one count, and specfx fades use
  `(fade*gameSpeed)>>12`, which truncates to 0 at high FPS so effects never fade and fill the
  pools (512 particles, `maxSpecfx`); new effects then silently fail. Pools are capped, no leak.
  In the port one tick = one retail 60 FPS frame and effects spawn only in the tick (never per
  rendered frame), so no toggle is needed.
- [x] Stuck after vacuum tube on last level (#11): toggle `fix_tube_softlock` (enhanced
  default on). Swampy 3 (`script-6-4.fev`): `KillFrog` leaves `currPlatform` set, so a frog that
  dies on a tube still satisfies `FrogOnPlatform` and is fired out by `SpringP`; if it respawns
  mid-flight the landing trigger never fires and the tube is parked in the shaft -> softlock.
  Fix: a dead frog is not on a platform (`evenfunc.rs` `frog_on_platform`). Root cause found by
  source/script reading, not yet reproduced in a run.
- [x] Frame-rate–dependent glide (#9) — fixed by architecture (fixed 60 Hz tick, `gameSpeed`
  is always 4096).
- [x] Audit every `gameSpeed`-scaled update for other FPS-dependent behaviour: all 81 uses in
  `f2-game` are evaluated with `gameSpeed == 4096`, i.e. exactly retail at 60 FPS. In retail
  they fall in four groups: linear (FPS-independent up to rounding: frog/enemy/platform motion,
  idle timers, camera, multiplayer timers), truncating `>>12` (frozen at high FPS: baby idle,
  specfx/particle/sprite/overlay/text fades, procedural textures), clamped `.min(4092)` (enemy
  slerps slow down at low FPS) and per-frame exponential curves (tongue, `enemies.rs:743`). None
  needs a fix while the simulation runs at a fixed 60 Hz; anything that renders between ticks
  must interpolate, never re-run simulation code.

## Display

All options: `docs/BUILDING.md` § Configuration (`profile`, `[video]`, `[display]`). Authentic
display = retail and pixel-identical to the pre-enhancement renderer; `[display]` overrides
apply to the enhanced profile only. F-keys for quick A/B: F5 profile, F3 perspective-correct,
F4 render scale, F6 MSAA, F7 filter, F8 widescreen, F9 fog, F12 draw distance.

- [x] Arbitrary resolution, 32-bit colour, borderless/fullscreen/windowed, vsync / frame cap
  (`video.width/height/fullscreen/vsync/frame_cap`)
- [x] Widescreen: Hor+ FOV (`display.hor_plus`: projection scale `min(w/640, h/480)` instead of
  `w/640`), HUD re-anchoring (`display.hud_anchor`: 4096-space overlays scaled to a centred
  4:3 area; elements centred in the left/right third stick to the screen edges, centred and
  full-screen ones stay centred, i.e. backdrops pillarboxed), FMVs pillarboxed (aspect kept),
  ultrawide clamp (`display.max_aspect`: pillarboxed 3D viewport, HUD anchored to it)
- [x] Hor+ past the 4:3 frustum: the simulation keeps culling against its 640×480 view
  (`f2-game` `view.rs`: `ActorListDraw` sphere test, `CheckBoundingBox` → `ACTOR2::clipped`,
  `IsPointVisible`/`OutcodeCheck` for effects) because those results feed game state and
  must not depend on the window. When Hor+ shows more than 4:3 (`View::hor_plus_extent`,
  wide or tall), the host draws what that culling dropped: (1) object sprites (e.g. the
  sarge ants' bodies in Subterranean 4-1) that `KillObjectSprites` turned off are posed
  render-side (`Game::posed_object_sprites`, read-only; `render::widened_object_sprites`),
  so off-4:3 actors are no longer drawn as legs only; (2) enemy/platform shadows ignore
  the 4:3 `clipped` flag (stale for culled actors); (3) `XfmPoint`'s camera-space
  `horizClip`/`vertClip` box scales with the visible area (sprites, effects). Meshes and
  the landscape were already drawn whole (the renderer's screen rejection uses the real
  viewport). Known limits, kept for sim parity: actors outside the 4:3 frustum do not
  animate (`UpdateAnims` only runs for actors `ActorListDraw` accepts, as in retail), so
  they hold the pose of their last on-screen frame; special effects whose origin is
  outside 4:3 are not spawned/updated by the simulation (`IsPointVisible`). Authentic and
  4:3 are unchanged (extent `[1, 1]`, empty pose set).
- [x] Extended polygon culling (`display.clip_faces`, enhanced default; *Polygon culling* in the
  settings): MDX drops a whole face when one corner is outside the near / far range
  (`PCPrepareObject` stores `vz = 0`), which on wide screens makes big polygons beside and in
  front of the camera blink out (100-350 faces a frame in a moving camera, measured on levels
  1-1, 2-2, 6-1). Extended clips them at the near plane (Sutherland-Hodgman in view space,
  colour / uv / fog / motion interpolated), keeps faces that only reach past the far plane, and
  replaces the screen test (no corner inside the screen on an axis) with an all-corners-past-one-
  edge test. Faces with every corner in range take the original path unchanged; *Original* and
  the authentic profile are the retail culling. Not compiled into `no_std` builds (N64).
  Remaining edge effects that are not culling: level art that ends past the 4:3 view (the clear
  colour shows), and the simulation's 4:3 culling (actors there hold their pose, see above).
- [ ] Modern renderer (in progress; *Graphics* in the settings, `display.graphics` =
  `original` / `enhanced` / `path_traced`, separate from the gameplay profile; enhanced default,
  the authentic profile is always Original).
  - [x] Milestone 1: GPU meshes (`crates/f2-engine/src/gpu_mesh.rs`; also `display.gpu_meshes`
    on its own). Opaque meshes (`Normal` / `Wrap` frames, no MODGE/WAVE) are uploaded once
    (corners expanded, grouped by texture; skins with a bone per vertex) and drawn with their
    object matrices, so the GPU has the scene's geometry. The vertex shader uses the game's
    projection with the exact depth (`View::exact_depth`, which the CPU path then uses too, so
    sprites, shadows and effects line up) and the CPU path's linear depth; per-pixel far cut
    instead of per-face popping; motion vectors and upscaler depth from last frame's matrices.
    Translucent and wobbling meshes stay on the CPU path. Matches the CPU picture (headless test;
    levels 1-1, 2-2, 4-1, 6-1 differ only by ~1 px on edges from the exact depth). Falls back to
    the CPU path on adapters without vertex storage buffers.
  - [x] Milestone 2: dynamic lighting (Enhanced; [LIGHTING.md](LIGHTING.md)): hemisphere
    ambient, a sun / moon per world with a 2048² PCF shadow map, Blinn-Phong highlight, up to 32
    point lights found on glowing textures (lava, fire, bulbs; also on CPU-drawn animated lava
    and fire sprites); glowing surfaces drawn unlit.
  - [ ] Milestone 3: water (wave simulation, objects and landings make waves, platforms dip,
    caustics).
  - [ ] Milestone 4: path tracing (Path-Traced preset).
- [x] High refresh (interpolated rendering; follow camera derives from the interpolated frog
  position, free camera moves per frame; `video.frame_cap`)
- [x] MSAA (`display.msaa`, 4×), supersampling (`display.render_scale`), anisotropic/trilinear
  filtering with generated mipmaps (`display.filter`, `display.anisotropy`), nearest-neighbour
  "PSX look" (`filter = "nearest"`), perspective-correct texturing (`display.perspective_correct`)
- [x] Temporal upscaling and frame generation (`[upscale]`, Windows / Direct3D 12): NVIDIA DLSS
  (DLAA to Ultra Performance, presets), AMD FSR 3.1 / FSR 4, DLSS (multi) frame generation 2x / 3x /
  4x / dynamic, FSR frame generation 2x, Reflex; per-vertex motion vectors and reversed depth for
  the upscalers; DLSS 5 Neural Rendering through RenoDX's add-on (`docs/UPSCALING.md`)
- [x] High-res UI fonts (`display.modern_font`): `bigfont`/`smallfont` re-rendered from their
  typeface, Gill Sans Ultra Bold, read from the user's system fonts (`crates/f2-fontgen`)
- [x] Draw distance (`display.draw_distance`, far-clip multiplier) / fog (`display.fog`, linear
  to the clear colour ending at the far clip; never on the sky backdrop)
- [ ] HUD anchoring of the real game HUD/menus: the ported overlays/text go through
  `draw_sprite_overlay`/`draw_text_overlay_ex` (in-game arcade HUD verified at 16:9; level
  complete / pause / multiplayer screens still to verify per screen). Authentic keeps the
  original stretch and `SPRITECLIP*` 10-pixel clipping (scaled with the resolution).

## Input

- [x] Native gamepad support (Xbox/PS/Switch via gilrs), PS1 layout by position ([CONTROLLERS.md](CONTROLLERS.md)): 4-way stick with hysteresis, Nintendo label option, per-player device memory across hot-plug, ✕○□△ glyphs, pad name entry; host prompts UI pending
- [~] Full rebinding UI, multiple bindings per action (f2-platform `Bindings`/`Rebind` API + config done; in-game UI is host work)
- [~] Rumble (`Gamepads::rumble` + DualShock motor model `controller::ShockMotors` done and wired; the PS1 game never vibrates — the shock code has no call sites — so nothing emits `Effect::Rumble` yet; any events would be a new enhancement)
- [~] Input buffering option for hops (`HopBuffer`, enhanced default 6 frames; host must apply it)

## Audio

- [ ] Music from FLAC/OGG with seamless loops
- [ ] Separate music/SFX/voice volume sliders
- [ ] Optional HRTF / better 3D panning

## Gameplay & QoL

- [x] Save slots + auto-save: `gameplay.save_slot` 1..4 (slot 1 = `saves/savegame.dat`, slot N =
  `saves/slotN/savegame.dat`, format unchanged); Gameplay page: change slot, Enter loads it
  (restarts at the title), "Save now" writes `MakeSaveGameBlock` to the slot. `auto_save = false`
  stops writing the game's own saves (level complete, resets).
- [x] Skip FMV: any key / pad button skips (`skip_fmv_any_key`, enhanced; authentic keeps
  Enter/Esc); `skip_intro_logos` drops the Hasbro/Blitz logo FMVs. [ ] fast-forward cutscenes
- [x] Level select / practice mode: Shift+F10 -> Level select lists every story chapter (`BookStart`
  from that chapter) and every single-player level (`ArcadeStart`, practice), unlocked or not;
  `--story N` / `--practice W L`. Built as a host page over `Game::start_story/start_arcade`
  instead of porting the unused `E3_Demo.c` state machine.
- [x] Speedrun timer: IGT = 60 Hz ticks in `INGAME_MODE` (the `actFrameCount` advance while
  playing; pause, level complete and loading excluded), per level and per run, split at
  `LEVELCOMPLETE_MODE`; RTA wall clock from the first level of a run; shown top right;
  splits appended to `<config>/saves/splits.csv`, best per level shown as +/- delta.
  [ ] ghost replays
- [x] Assist options: infinite lives (`KillFrog` skips `lives--`), invulnerability (enemy hits
  ignored in `NMEDamageFrog`; hazard tiles/falls/script deaths still kill), slower enemies
  (`enemy_speed` 25..100 %: on skipped ticks every enemy and platform is frozen in time — path
  time stamps advance with `actFrameCount` — so they resume where they were; at <100 % motion
  is stepped like a lower frame rate). [ ] checkpoint tweaks
- [ ] Accessibility: colour-blind safe HUD, text scaling, reduced flashing
- [ ] Unlock cut content where it exists in data (City single-player levels were commented out; check data)
- [ ] Debug/dev menu (original debug code under `#ifndef FINAL_MASTER`)

## Multiplayer

- [ ] Local split-screen improvements (2–4 players, per-player gamepads)
- [ ] Online: rollback netcode (`ggrs`) over UDP, lobby via join codes; deterministic sim makes this feasible
- [ ] Remote Play Together–friendly

## Platform

- [ ] Steam Deck verified layout, Flatpak, AppImage, macOS bundle
- [ ] Web build (WebGPU) with File System Access import
- [ ] Localisation: UTF-8 text pipeline, fan translations via mods
