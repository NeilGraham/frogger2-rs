# f2-game porting conventions

Applies to every agent porting `FroggerProject/Common/*.c` (and gameplay-relevant PC files) into
`crates/f2-game`. Read together with `docs/porting-guide.md`.

## Layout

- One Rust module per C file, same base name in snake_case: `Common/frogmove.c` →
  `src/frogmove.rs`, `PC/actor.cpp` → `src/pc_actor.rs`.
- Each module defines `pub struct <Name>State` holding the globals **defined** in that C file (e.g.
  `enemyList` in `enemies.c` → `EnemiesState { enemy_list: EnemyList, ... }`) and adds one field to
  `Game` in `lib.rs` (`pub enemies: enemies::EnemiesState`). Globals keep their C names in
  snake_case (`actFrameCount` → `act_frame_count`).
- Functions are `impl Game` methods in the module's file when they touch state
  (`impl Game { pub fn update_enemies(&mut self) { ... } }`), free functions when pure. Keep the C
  name in snake_case and add `/// Port of `UpdateEnemies` (Common/enemies.c:123).`
- Constants/flags keep their C names (`ENEMY_NEW_FOLLOWPATH`) as `pub const`.

## Data model

- Pointers → typed ids from `ids.rs` (`TileId`, `EnemyId`, `PlatformId`, `ActorId`, …);
  nullable → `Option<…>`. Add new id types to `ids.rs` with the `id!` macro if needed.
- Block-allocated arrays (`AllocNmeBlock`, `AllocPlatformBlock`, …) → `Vec<T>` indexed by id.
- Intrusive lists with sentinel heads → `list::Links<Id>` embedded in the element + `list::Head<Id>`
  in the owning list struct. Preserve insertion (`push_front` for C "add at head") and removal
  semantics; iterate like C (`let mut c = head.first; while let Some(id) = c { let next = links.next; … c = next; }`).
- `PATH*` owned by an enemy/platform (memload copies the path) → owned `Path` inside the entity.
- Function pointers (`Update[]`, trigger funcs, death anims) → `enum` tags + `match` dispatch.
- `void*` params smuggling ints/pointers (e.g. `MakeTrigger(func, (void*)tile, …)`) → typed enum payloads.
- Strings (`char name[16]`) → `String` (Latin-1) unless a fixed byte layout is observable.

## Determinism & numerics

- Integer maths exactly as C (wrapping 32-bit, truncating division, arithmetic shifts on signed).
  Use `f2_math` functions (`fmul`, `fdiv`, vector helpers) — never re-implement them.
- `Random()` → `self.rng.random(n)`; call order must match the C.
- Floats only where C used floats; `f32`, never `f64`.
- No wall-clock, no HashMap iteration in logic (use `Vec`/`BTreeMap`), no threads.
- `gameSpeed` is 4096 (one frame per tick); keep the multiplications by `gameSpeed` so behaviour
  matches the original at 60 fps exactly.

## Platform boundary

- Audio, music, video, saves, vibration → push `effects::Effect` (`self.effects.push(…)`).
- Sample lookups (`FindSample(UpdateCRC("name"))`, `genSfx[...]`) → sample uid `u32` (CRC);
  the audio layer resolves it.
- Rendering-only code (D3D, sprites drawn by `pcsprite.c`) is NOT ported here; but *state* that
  the simulation creates for rendering (actors, sprites, special effects, overlays, text overlays,
  fog/colour settings) IS, so the renderer can read it from `Game`.
- Model facts (animation tables, radius, sub-object names) come from `self.models`
  (`assets::ModelTable`, filled from object banks). Level files come from an `assets::DataSource`.

## Things that are deliberately different

- `#ifdef PSX_VERSION` / `DREAMCAST_VERSION` / `N64_VERSION` branches are dropped; keep
  `PC_VERSION` (+ `FINAL_MASTER`) behaviour. `#ifdef E3_DEMO` etc. dropped unless PC uses it.
- Debug-only code (`#ifndef FINAL_MASTER`, `DEBUG_*`, editor hooks) dropped, except harmless
  logging which becomes `Effect::Log` when useful.

## Tests

- Unit tests for leaf logic; real-data tests read `F2_GAME_DIR` (default `game-data/usa`) and skip
  when absent, e.g. "load every level's collision + entities + script and run 600 ticks without
  panic".
- Keep `cargo test -p f2-game` green after each wave; `#![forbid(unsafe_code)]` stays.

## Status tracking

After porting a file, the orchestrator updates `docs/source-map.md` (agents: list ported
functions + any stubs left (`// TODO(port): FuncName`) in the final report).
