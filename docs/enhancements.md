# Enhancements & fixes backlog

Everything here comes **after** parity for the affected subsystem (Phase 6+), and is selectable via
profiles:

- **Authentic** — matches retail behaviour (parity tests run in this profile).
- **Enhanced** (default for players, owner decision) — bug fixes + QoL.
- Per-option overrides in `config.toml`.

## Fixes (see `research/known-bugs.md` for root causes)

- [ ] Camera divide-by-zero crash (#4)
- [ ] Pause resumes wrong music (#7)
- [ ] Particle over-spawn/render (#10)
- [ ] Stuck after vacuum tube on last level (#11)
- [ ] Frame-rate–dependent glide (#9) — fixed by architecture; verify
- [ ] Audit every `gameSpeed`-scaled update for other FPS-dependent behaviour

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
- [x] High refresh (interpolated rendering; follow camera derives from the interpolated frog
  position, free camera moves per frame; `video.frame_cap`)
- [x] MSAA (`display.msaa`, 4×), supersampling (`display.render_scale`), anisotropic/trilinear
  filtering with generated mipmaps (`display.filter`, `display.anisotropy`), nearest-neighbour
  "PSX look" (`filter = "nearest"`), perspective-correct texturing (`display.perspective_correct`)
- [ ] Integer-scaled or high-res UI fonts
- [x] Draw distance (`display.draw_distance`, far-clip multiplier) / fog (`display.fog`, linear
  to the clear colour ending at the far clip; never on the sky backdrop)
- [ ] HUD anchoring of the real game HUD/menus: the ported overlays/text go through
  `draw_sprite_overlay`/`draw_text_overlay_ex` (in-game arcade HUD verified at 16:9; level
  complete / pause / multiplayer screens still to verify per screen). Authentic keeps the
  original stretch and `SPRITECLIP*` 10-pixel clipping (scaled with the resolution).

## Input

- [ ] Native gamepad support (Xbox/PS/Switch via gilrs) with button prompts
- [ ] Full rebinding UI, multiple bindings per action
- [ ] Rumble (events already exist for PSX vibration — check `#ifdef PSX_VERSION` code)
- [ ] Input buffering option for hops

## Audio

- [ ] Music from FLAC/OGG with seamless loops
- [ ] Separate music/SFX/voice volume sliders
- [ ] Optional HRTF / better 3D panning

## Gameplay & QoL

- [ ] Save slots + auto-save
- [ ] Skip FMV, fast-forward cutscenes
- [ ] Level select / practice mode (E3 level-select code exists: `E3_Demo.c`)
- [ ] Speedrun timer (in-game time + real time), splits, ghost replays (we have deterministic input recording!)
- [ ] Assist options: infinite lives, slower enemies, checkpoint tweaks
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
