# Architecture of the Rust port

Status: design (Phase 0). Mapping from the original: [`source-map.md`](source-map.md). Original
architecture: [`../research/source-tree.md`](../research/source-tree.md).

## 1. Principles

1. **Simulation is pure and deterministic.** `f2-game` has no platform dependencies: no
   windowing, GPU, audio device, filesystem or clock. It consumes `TickInput` (pad state per
   player) and produces a new `GameState` plus a list of *events* (sounds to play, music cue, FMV
   request, save request, rumble…). It can run headless at thousands of ticks/s.
2. **Fixed 60 Hz logic tick**, interpolated rendering at any refresh rate. The original runs
   variable steps scaled by `gameSpeed`. Our tick function always gets `gameSpeed = 4096` (one
   60 Hz frame). With the oracle in the same mode, behaviour is comparable tick for tick.
3. **Original numerics are kept.** Gameplay uses `f2_math::Fixed` (`i32` 20.12, wrapping
   semantics identical to MSVC `long`), PSX angle units and the original tables. Floats are only
   used where the original used floats (MDX render path, some camera/particle code — audit per file).
4. **Data in, data out.** Assets come only through `f2-formats` parsers from the user's imported
   data directory (or mod overlays). There are no asset blobs in the binary.
5. **Platform layers are traits** so tests can use fakes and new backends can be added.

## 2. Crate graph

```
                         ┌─────────────┐
                         │  frogger    │  (bin: CLI, launcher, settings, main loop)
                         └─────┬───────┘
     ┌───────────────┬─────────┼──────────────┬──────────────┐
     ▼               ▼         ▼              ▼              ▼
 f2-platform     f2-engine   f2-audio      f2-bink        f2-import ──► f2-media
 (winit, gilrs,  (wgpu       (kira/cpal,   (Bink 1 video  (detect, verify,    (cue, ISO9660,
  config, paths)  renderer,   sample banks, + audio)       extract, convert)   IS5 CAB, CD-DA)
     │            actors,     music)          │              │
     │            sprites,       │            │              ▼
     │            fonts)         │            │          f2-formats (pure parsers)
     ▼               │           │            │              ▲
  f2-game ◄──────────┴───────────┴────────────┘              │
  (simulation)  ───────────────────────────────────────────── uses
     │
     ▼
  f2-math (fixed point, PSX trig, vectors/quaternions)

  f2-mods: VFS with overlays; used by frogger / f2-engine / f2-audio to open assets
  tools/: f2-viewer, f2-etc (script compiler), f2-trace (parity diff), f2fix (oracle shim, Windows i686)
```

Dependency rules: `f2-game` depends only on `f2-math` and `f2-formats` (for parsed level data
types). The renderer reads a **read-only view** of `GameState` (+ previous state for
interpolation); it never mutates gameplay.

## 3. Main loop

```rust
let mut acc = Duration::ZERO;
loop {
    platform.pump_events(&mut input);
    acc += clock.delta().min(MAX_CATCHUP);          // e.g. 250 ms
    while acc >= TICK {                              // TICK = 1/60 s
        prev = state.clone_render_view();
        let events = game.tick(&mut state, input.snapshot());
        audio.handle(&events); video.handle(&events); save.handle(&events);
        acc -= TICK;
    }
    let alpha = acc.as_secs_f32() / TICK.as_secs_f32();
    renderer.draw(&prev, &state.render_view(), alpha);
}
```

FMV playback and loading screens are game modes like in the original (`FMVPLAY_MODE`), driven by
events; the video decoder runs on its own thread and hands frames to the renderer.

## 4. Game state model

The C code uses global structs and intrusive linked lists of actors/enemies/platforms with raw
pointers between them. The port uses:

- one `GameState` struct owning everything that was global (`gameState`, `frog[]`, `player[]`,
  enemy/platform/garib lists, camera, timers, script VM state, RNG);
- **arenas + typed indices** (`EnemyId(u16)`, `TileId(u16)`, `ActorId`) instead of pointers —
  generational indices where entities are freed mid-level;
- *iteration order identical to the original lists* (insertion order, same removal semantics) —
  ordering affects gameplay (collision, update order) and must be preserved for parity;
- the original RNG (port the exact generator; seed handling identical).

`GameState` is `Clone` (cheap enough for interpolation snapshots, rollback netcode and
save-states) and serialisable (`serde`) for debugging dumps / trace comparisons.

## 5. Rendering (`f2-engine`)

Replaces MDX + D3D7. Faithful first, enhanced later.

- **Object/actor system**: hierarchical `MDX_OBJECT` trees with keyframe animation (scale/move/
  rotate keys, quaternion slerp as in `mdxActor.cpp`), skinning where `isSkinned`.
- **Draw model**: MDX builds per-frame poly lists, z-sorts translucent polys, uses additive/alpha
  modes and fog. We reproduce this with a few wgpu pipelines (opaque, alpha-sorted, additive,
  sprites/overlays) and CPU-side sorting where order matters.
- **Textures**: X1R5G5B5 → RGBA8 at load, honouring the flag rules (colour key, `ai_`, `ac_`,
  procedural). Animated textures driven by the bank's anim table. Procedural fire/water
  (`ptexture.c`) ported on CPU and uploaded per frame (small textures).
- **2D**: overlays in the 4096×4096 virtual space mapped to a 4:3 safe area (authentic) or
  re-anchored for widescreen (enhanced). Bitmap fonts from `Font.fla`.
- **Backdrops/FMV**: full-screen textured quads.
- Enhancements later: MSAA, texture filtering choice, widescreen FOV, high-res UI scaling.

## 6. Audio (`f2-audio`)

- `.wus` samples decoded to PCM at load; looked up by CRC like the original.
- 3D voices: the original uses DirectSound3D; we compute pan/attenuation in software with the
  original distance parameters (`pcaudio.c`) for comparable mix.
- Music: ripped CD tracks (FLAC) streamed; per-world cue + loop semantics from `PrepareSong` /
  `LoopSong`; mod overrides can supply OGG/FLAC.

## 7. Input (`f2-platform::input`)

Keep the original abstraction: per-player `padData` with PSX button bits (`PAD_UP`, `PAD_CROSS`…).
Keyboard default map from `controll.cpp`; gamepads via `gilrs`; rebinding stored in our config
(TOML). Demo `.key` files feed the same `padData` stream → playback is just another input source.

## 8. Files & mods (`f2-mods`)

Virtual filesystem, case-insensitive, layered: `mods/<enabled…>/` over `data/<region>/` (imported).
All engine/game file access goes through it. Format details in [`modding.md`](modding.md).

## 9. Config & saves

- Settings: `config.toml` in the platform config dir (replaces registry + config dialog).
- Saves: original `savegame.dat` layout kept (2112 B) for compatibility/import from a real
  install; additional JSON sidecar for new features (slots, stats) — never breaks the original layout.

## 10. Oracle & testing infrastructure (Windows-side tools)

See [ORACLE.md](ORACLE.md): `f2fix.dll` (32-bit, injected into the user's retail `Frogger2.exe` via proxy DLL) for headless,
muted, fixed-timestep runs with per-tick state traces and framebuffer dumps; `f2-trace` diffs those
traces against `f2-game` running the same `.key` input.

## 11. Platform targets

Tier 1: Windows x64, Linux x64. Tier 2: macOS (arm64/x64), Steam Deck. Later: wasm (wgpu WebGPU +
data via File System Access API) — keep `f2-game`/`f2-formats` `no_std`-friendly-ish (no threads,
no std::fs) to make this possible.
