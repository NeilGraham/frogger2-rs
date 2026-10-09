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

- [ ] Arbitrary resolution, 32-bit colour, borderless/fullscreen/windowed, vsync / frame cap
- [ ] Widescreen: Hor+ FOV, HUD anchored to screen edges, backdrops/FMVs pillarboxed
- [ ] High refresh (interpolated rendering)
- [ ] MSAA, anisotropic filtering, optional nearest-neighbour "PSX look"
- [ ] Integer-scaled or high-res UI fonts
- [ ] Draw distance / fog options

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
