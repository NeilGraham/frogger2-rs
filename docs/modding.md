# Modding architecture

Target: Phase 7, but the VFS foundation lands in Phase 1 so every asset load already goes through it.

## 1. Mod layout

```
mods/
  my-mod/
    mod.toml              # id, name, version, authors, engine version range, load order hints, profile flags
    game/                 # overrides/additions mirroring the imported tree (case-insensitive)
      textures/Garden/00wate04.png      # loose replacement for one texture in a bank (by name/CRC)
      sfx/garden/hopongrass.wav         # loose replacement for one sample (by name/CRC)
      objects/garden1/jetty.gltf        # replacement model
      maps/script-1-0.fes               # script source, compiled at load by f2-etc
    music/track02.ogg
    levels/<new-level>/level.toml       # new levels (see §3), + any files the level brings
    scripts/*.f2s                       # optional behaviour scripts (P7b, §4)
```

`config.toml` lists enabled mods in order; later mods win. The VFS resolves
`mods/<n>/game/<path>` → … → `data/<rendition>/game/<path>`; each mod's `levels/` dir is
mounted at the virtual `levels/`.

Enabling mods (`frogger`):

```toml
# config.toml
[mods]
enabled = ["hello-mod"]          # load order, later wins
dir = "/path/to/mods"            # optional; default <config dir>/mods or $F2_MODS_DIR
```

`frogger2 --mods a,b` replaces `enabled` for one run (`--mods ""` = no mods), `--mods-dir DIR`
replaces `dir`. With no mods enabled every read is the plain retail file (authentic play is
unchanged; `hello_mod_pipeline` in `crates/frogger` checks Garden 1 renders pixel-identically
with a mod enabled that doesn't touch it).

## 2. Granularity: why per-entry overrides

Original containers are coarse (a `.fla` holds 100+ textures; a `.wus` holds dozens of samples;
a `.dll` holds a whole object bank). Since the original code already addresses entries **by CRC
of their name**, the engine's asset registry is keyed by CRC; mods can replace or add single
entries without rebuilding containers. Supported loose formats:

| Kind | Original | Mod formats |
|---|---|---|
| Textures | X1R5G5B5 in `.fla` banks | PNG (any size, RGBA) |
| Sounds | PCM in `.wus` | WAV, FLAC (OGG planned) |
| Models | MDX structs in PE `.dll` | glTF 2.0 (meshes, hierarchy, keyframe anims) |
| Music | CD-DA | WAV/FLAC (OGG + loop points planned) |
| Text | `language.fla` TSV | TSV/TOML string tables (new languages possible) |
| Levels | `.col` `.sce` `entity.dat` `.fev` | same binary formats **or** text equivalents (TOML/RON) + script source |
| FMV | Bink 1 | Bink 1, or WebM (if a decoder is enabled) |

Exporters in `f2-export` (PNG/WAV/glTF/TSV) give modders editable starting points; the
importers in `f2-modkit` are their inverse (see §6).

## 3. Levels

A level = object bank + collision tile graph + scenics + entities/paths + event script + sfx map +
world assignment. We will:

1. Port the original **`Etc` script compiler** (`frogger2-vss/teamSpirit/Etc`) as `f2-etc`, plus a
   decompiler (`.fev` → source) so existing levels become readable examples.
2. Define a level manifest (`level.toml`) that adds an entry to the world/level table
   (`worldVisualData` equivalent) without patching code.
3. Revive the in-game editor (`FroggerProject/Editor/`, debug-only in retail) as a dev-mode tool:
   tile graph editing, entity placement, path editing, save to text formats.
4. Blender workflow: glTF export from Blender → importer computes tile graph helpers; or a Blender
   add-on later.

### What works now (P7-1)

- **Script source** (`f2-etc`, see `docs/ETC-LANGUAGE.md`): a mod file
  `game/maps/script-W-L.etc` (or `.fes`) replaces the retail `MAPS\SCRIPT-W-L.FEV`; the host
  compiles it when the game reads the `.fev` (a source wins over a `.fev` in the same or a
  lower layer). A level manifest may also name an `.etc`/`.fes` file directly. Compile errors
  are logged and the level runs without a script, like a missing `.fev`.
- **Level manifests**: `levels/<dir>/level.toml` adds a level to (or overrides) one
  `worldVisualData[world].levelVisualData[level]` slot at session start, through the
  additive `f2_game::levels::LevelRegistry` (`SessionConfig::extra_levels`,
  `Game::install_level_registry`). `load_level` takes file names from the registry entry and
  falls back to the original switch tables, so no game code is patched. Files resolve
  through the mod VFS, so a level may reuse retail files by name.
- Not yet: text level formats, Blender workflow; new levels are not listed
  in the frontend menus (raising `numLevels` would change the save-game layout) — start them
  with `frogger2 --level W L` or reach them through `next` of another level.

### `level.toml` reference

```toml
world = 0              # worldVisualData slot, 0-9 (the original table has exactly 10)
level = 1              # levelVisualData slot, 0-19 (empty slots are free)
base = [0, 0]          # optional: copy this retail slot's row and files first
name = "Hello Garden"  # literal level name (Latin-1), or:
# text_id = "STR_..."  # name by string id
level_id = 0           # levelID (default ENTITY-/SCRIPT- file names); default from base/slot
next = [1, 0]          # [nextWorldID, nextLevelID] after completion
fog = [96, 64, 160]    # fog / clear colour (0-255)
fog_near = 500         # fogNearDist
fog_far = 700          # fogFarDist
far_clip = 6000        # farClip (fog.max), game units x 10
par_time = 450         # parTime (tenths of a second)
music = 4              # CD track (music/trackNN.*, may come from a mod); default: the world's
open = true            # levelOpen in a new game (default true)
scripts = ["./hazard.f2s"]  # P7b behaviour scripts whose hooks run on this level only (§4)

[files]                # VFS paths; "./x" = levels/<dir>/x. Unset = from base (or the slot's row)
collision = "COLLISION/GARDEN1.COL"
scenics = "SCENICS/GARDEN1.SCE"
entities = "MAPS/ENTITY-0-0.DAT"
script = "./script.etc"          # .fev, or ETC source (.etc/.fes) compiled at load
sfx_mapping = "MAPS/SFXANIM00.SAM"
texture_bank = "textures/garden.fla"         # replaces the world's masterTextureBank
world_object_bank = "objects/gardenmaster.dll" # replaces masterObjectBank
object_bank = "objects/garden1.dll"          # replaces objectBankToUse
sfx_bank = "sfx/garden.wus"                  # replaces the world's sample bank in LoadSfx

[world_info]           # optional changes to the slot's world
world_id = 9
name = "My World"      # or text_id = "STR_..."
open = true
```

Banks named here still get per-entry overrides (§6) when they live at `textures/<bank>.fla`,
`objects/<bank>.dll`, `sfx/<bank>.wus`. Literal names become extra strings
`GAMESTRING(LANG_NUM_STRINGS + i)`. A slot claimed by two manifests: the later directory
(alphabetical, top layer first per directory) wins, with a warning. `f2-modkit pack` parses
every manifest and checks the named files exist (all of them with `--rendition`, else only
the mod's own `./` files).

Example: `examples/mods/hello-mod` (a text override and a dusk copy of Garden 1 in slot 0-1
with its own ETC script; no game assets):

```
f2-modkit pack examples/mods/hello-mod --rendition game-data/usa
frogger2 --mods hello-mod --mods-dir examples/mods --level 0 1
```

### Level editor (P7, `crates/frogger/src/editor/`)

`frogger2 --editor --level W L [--mods ID | --mod ID] [--mods-dir DIR] [--editor-cmd "CMD; CMD"]` (Ctrl+E toggles
it on the current level in dev builds). The simulation pauses; a free camera (WASD/QE, arrows,
Shift) flies over the level. Modes (keys 1-5): **tiles** (graph colour-coded by state, links as
half-way lines coloured per slot n/s/e/w; PgUp/PgDn state, L/U link/unlink towards the pointer,
Ins extrude a new linked tile, Del delete with every tile reference renumbered), **entities**
(markers: enemy red, platform blue, placeholder grey, garib yellow, camera case magenta; Tab/Space
select, M move to the pointer tile — paths and camera tile groups move as a whole, N/G/C new
enemy/garib/camera case, Del), **paths** (`[`/`]` node, N add, M move, Del, PgUp/PgDn speed),
**scenics** (M, Ins copy, Del), **script** (decompiled ETC source view; O opens `$EDITOR`, the
result is compiled with `f2-etc` and the `.fev` only changes when it compiles). Enter opens a
command line (`help` lists commands; `set FIELD VALUE` edits any field of the selection, e.g.
`set speed 2.5`, `set flags 0x10`, `set model bfly.obe`). P play-tests the level from the edited
files in memory, R reloads it paused. `save [ID]` writes the changed files in the retail formats to
`<mods dir>/<ID>/game/{collision,scenics,maps}/…` (mods dir = `--mods-dir`, else `[mods] dir`, else the platform default) (+ `mod.toml` when missing and the `.etc` source next to
the `.fev`); `frogger2 --mods ID --level W L` plays it (`--mod ID` is an alias that can be repeated). Writers in `f2-formats`
(`Collision::write`, `write_scenics`, `EntityFile::write` incl. the low bits the loader drops,
`SamFile::write`, `Script::write`) reproduce every retail file byte-for-byte
(`crates/f2-formats/tests/roundtrip.rs`).

## 4. Behaviour scripting (P7b, `crates/f2-game/src/modscript/`)

The original event language (§3, ETC) covers triggers/actions. For new behaviour, mods ship
**F2S** behaviour scripts: a small integer-only language compiled to bytecode and run on a
sandboxed stack VM *inside* the deterministic 60 Hz tick. Rhai/Lua were considered; neither is
available offline in the build environment and both bring floats, allocation-heavy values
and host access that would need auditing, so the port has its own (~2 200 lines incl. tests).

### Loading

- `mods/<id>/scripts/*.f2s` — mod-wide scripts (sorted by file name); their hooks run on every
  level (check `world()`/`level()` yourself).
- `scripts = ["./x.f2s", ...]` in a `level.toml` — hooks run only on that level's slot.
- Order: enabled mods in load order (each mod's `scripts/`), then level-manifest scripts.
  Each script has its own globals; all of them see every hook.
- A mod with scripts is **gameplay-affecting** (the host forces the flag and warns if
  `mod.toml` says otherwise; `f2-modkit pack` warns too).
- Compile errors are logged (`frogger: …: line N: …`) and that script is skipped; the game runs.
- Netplay: every peer installs its own enabled mods' scripts; the script-set hash
  (`Game::modscript_hash`, name + scope + source of every script) is part of the
  `MatchSetup` start fingerprint (and of the state checksum), so online play only starts with
  identical script sets ("game data differs … behaviour scripts must be identical").
- Hooks never run during attract demos (`DEMO_MODE`). With no scripts installed every hook
  call returns immediately: authentic play, the demo traces and the parity corpus are unchanged.

### Determinism and sandbox

- Values: 32-bit integers (wrapping arithmetic, `/`/`%` by zero is an error) and immutable
  strings (for messages/names, ≤ 1024 bytes). No floats, no clock, no I/O, no host access;
  the only randomness is `random(n)` = the game's own `Random()` (so it advances the game's
  RNG exactly like game code would).
- Bounded: **200 000 instructions per script per tick** (all hooks together), 4096 stack
  values, 64 nested calls. Any runtime error or budget overrun **disables that script** for the
  rest of the session: an `Effect::Log` `modscript <name>: disabled: <why>` is emitted and
  `Game::modscript_errors()` lists it; its enemy claims are released. Other scripts keep running.
- State: script globals, enemy claims, queued events, shown texts and budgets live in
  `Game::modscript` (`ModScriptState`), so `Game::snapshot`/`restore` and `Game::checksum`
  (sub-state `modscript`) cover them; compiled code is an immutable `Arc`. A hook always runs to
  completion inside one tick — no VM state survives between calls.
- Hook timing: `on_level_start` at the end of `InitLevel`; land/pickup/death events are queued
  where they happen and dispatched with `on_tick` at the end of `RunGameLoop` (events first, in
  order; then `on_tick`); `on_enemy_update` runs inside `UpdateEnemies` for claimed enemies.

### Language

```
// line comment            /* block comment */
const PERIOD = 120;        // compile-time constant (integer or string literal)
let counter = 0;           // global: persists across ticks and levels (part of the state)

fn helper(a, b) {          // functions (forward references fine, recursion bounded)
    let x = a * b;         // locals
    if x > 10 { return x; } else if x < 0 { return -x; } else { x += 1; }
    while x < 100 { x = x << 1; if x == 64 { break; } if x == 8 { continue; } }
    return x;
}

fn on_tick() { counter += 1; if counter % PERIOD == 0 { log("tick " + counter); } }
```

Operators (C precedence): `|| && | ^ & == != < <= > >= << >> + - * / %`, unary `- ! ~`;
assignments `= += -= *= /= %= &= |= ^= <<= >>=` (statements only). `&&`/`||` short-circuit and
give 0/1. `true`/`false` are 1/0. `+` with a string operand concatenates (`"x" + 3` → `"x3"`).
Numbers: decimal, `0x` hex, `_` separators. Strings: `"…"` with `\n \t \" \\`. Truthiness:
non-zero / non-empty. A function without `return` returns 0. Wrong arity, unknown names and
hook signature mismatches are compile errors.

### Hooks (define any subset)

| Hook | When |
|---|---|
| `on_level_start(world, level)` | a level finished loading (claims/queue/texts were reset) |
| `on_tick()` | every in-game tick (`RunGameLoop`, not while paused / in the end-of-level sequence) |
| `on_frog_land(player, tile, tile_state)` | a frog landed (tile = -1 if none; state at landing) |
| `on_frog_death(player, cause)` | a frog's `FROGSTATUS_ISDEAD` went on (`cause` = `DEATH_*`) |
| `on_pickup(player, kind)` | a collectable was picked up (`kind` = `PICKUP_*`) |
| `on_enemy_update(e)` | each `UpdateEnemies` pass for an enemy this script claimed, *instead of* its built-in movement (collision still runs first; attached effects after) |

The global initialisers (`let g = expr;` at top level) run once when scripts are installed.

### API reference

Handles are plain integers checked on every call (bad handle → error → script disabled).
Players are 0–3; tiles are indices `0..tile_count()`; enemies are indices `0..enemy_count()`
(entries that are not in the enemy list are rejected). Positions are world units
(`SVECTOR`), speeds and fixed-point values are 20.12 (`FIXED_ONE` = 4096).

| Function | Returns / effect |
|---|---|
| `log(v)` | log line `modscript <name>: v` (host stderr) |
| `frame()` / `level_frame()` | `actFrameCount` (resets at level start) / `frameCount` |
| `world()` / `level()` / `num_frogs()` | current world/level slot, number of frogs |
| `random(n)` | game `Random(n)`: 0..n-1 (0 for n ≤ 0) |
| `abs(x)` `min(a,b)` `max(a,b)` `clamp(x,lo,hi)` `str(v)` | helpers |
| `frog_tile(p)` `frog_lives(p)` `frog_dead(p)` `frog_state(p)` | frog info (`frog_tile` -1 if none) |
| `kill_frog(p, cause)` | kills a living frog with death animation `cause` (`DEATH_*`); 1 if killed |
| `tile_count()` `tile_state(t)` `set_tile_state(t, s)` | collision tiles; `s` = `TILE_*` (0–255) |
| `tile_neighbour(t, d)` | linked tile `d` (0–3 = the tile's `tilePtrs` n/s/e/w) or -1 |
| `tile_x(t)` `tile_y(t)` `tile_z(t)` | tile centre |
| `enemy_count()` `enemy_find(uid)` `enemy_uid(e)` `enemy_model(e)` | enemy lookup (`enemy_find` -1 if none; model e.g. `"wasp.obe"`) |
| `enemy_active(e)` `enemy_set_active(e, b)` | update/draw on or off |
| `enemy_speed(e)` `enemy_set_speed(e, v)` | path speed (20.12) |
| `enemy_flags(e)` `enemy_set_flags(e, f)` | `ENEMY_NEW_*` flags (`ENEMY_*` constants) |
| `enemy_waiting(e)` `enemy_set_waiting(e, w)` | `isWaiting` (-1 = forever) |
| `enemy_node(e)` `enemy_to_node(e)` `enemy_num_nodes(e)` `enemy_set_nodes(e, from, to)` | position along the path |
| `enemy_start_frame(e)` `enemy_end_frame(e)` `enemy_set_frames(e, s, f)` | current path segment timing (`actFrameCount` units) |
| `enemy_tile(e)` `enemy_x/y/z(e)` `enemy_set_pos(e, x, y, z)` | tile / actor position |
| `claim_enemy(e)` / `claim_enemy_model(name)` | take over the enemy's update (`on_enemy_update`); the model form claims every enemy with that model and returns the count. Claims reset at level start |
| `enemy_run_default(e)` | run the built-in movement (waiting + update functions) once |
| `spawn_fx(type, t)` / `spawn_fx_ex(type, t, size, speed, accn, life)` | special effect at a tile centre (`FX_*`); 1 if created (visible) |
| `play_sample(name)` / `play_sample_at(name, t)` | sound by sample name (CRC of the name, like the game) |
| `show_text(text, frames)` / `show_text_at(text, x, y, frames)` | centred text overlay (x/y in 0–4096 screen units; default y 1400), removed after `frames` ticks (≤ 0 = until `hide_text`); returns an id or -1; at most 8 per script |
| `hide_text(id)` | remove one of this script's texts |
| `set_fog(r, g, b)` / `set_fog_range(near, far)` | fog / clear colour and distances |

Constants: `TILE_NORMAL DEADLY SINK ICE SUPERHOP JOIN SAFE FINISHLINE GRAPPLE SMASH BARRED FALL
DEADLYFALL HOT ELECTRIC NOSUPER PANTS NOSUPERHOT OCCUPIED CONVEYOR CONVEYOR_MED CONVEYOR_FAST
CONVEYOR_ONEWAY` (all `TILE_…`); `DEATH_NORMAL RUNOVER DROWNING SQUASHED FIRE ELECTRIC FALLING
WHACKING INFLATION POISON SLICING EXPLOSION GIBBING INCINERATE BURSTING SPIKED LIGHTNING
FALLINGFOREVER`; `PICKUP_SILVERCOIN EXTRALIFE AUTOHOP LONGTONGUE QUICKHOP INVULNERABILITY GOLDCOIN
SLOWHOP`; `FX_WATERRIPPLE GARIBCOLLECT SPARKLYTRAIL FROGSTUN SMOKE_STATIC SMOKE_GROWS DECAL SPLASH
SMOKEBURST GREENGLOOP BUBBLES SPARKBURST FIERYSMOKE POLYRING GLOW TWINKLE CROAK FLASH`;
`ENEMY_FOLLOWPATH PINGPONG CYCLE NODAMAGE ONEHITKILL FACEFORWARDS DONOTMOVE RADIUSCOLLISION`;
`FIXED_ONE`. (Source of truth: `modscript/api.rs`.)

### Example (`examples/mods/hello-mod`)

- `levels/hello-garden/hazard.f2s` (`scripts = ["./hazard.f2s"]` in its `level.toml`): a patch
  of deadly water walks back and forth along a row of tiles three tiles ahead of the start,
  moving every 2 seconds, with a smoke burst and a hint text.
- `scripts/messages.f2s` (mod-wide): `on_pickup` shows a coin counter / power-up message on
  Hello Garden only.

```
frogger2 --mods hello-mod --mods-dir examples/mods --level 0 1
```

### Tests

`cargo test -p f2-game modscript` (language, compile errors, runtime errors, budget exhaustion,
per-tick budget, determinism + snapshot/restore mid-script, scope, events/text, no-script
no-op), `cargo test -p f2-game --test modscript` (real data: hooks fire on Garden 1, two runs
identical, snapshot at tick 600 resimulates to the same checksums, an inert script leaves every
other sub-state and every effect identical), `cargo test -p frogger hello_mod_scripts`.

## 5. Compatibility rules

- Mods declare the engine API version; `f2-mods` refuses incompatible ones with a clear message.
- Mods must not contain original game assets (enforced socially + by a hash denylist warning in
  the packer tool).
- Gameplay-affecting mods are flagged so speedrun/online modes can require vanilla.

## 6. Asset overrides — what works now (`crates/f2-modkit`, P7-1)

`f2-modkit` turns a `Vfs` (retail rendition + enabled mods) into the *effective* containers the
engine should use. Every function parses the retail container through `f2-formats`, then applies
loose per-entry files from all layers (topmost layer wins per entry). Broken override files are
skipped and listed in `Loaded::warnings` — they never abort a game load.

| API | Base container | Override files (virtual paths under `game/`) |
|---|---|---|
| `tex_bank(&vfs, "garden")` | `textures/garden.fla` | `textures/garden/<name>.png` |
| `sound_bank(&vfs, "garden")` | `sfx/garden.wus` | `sfx/garden/<name>.wav` / `.flac` |
| `object_bank(&vfs, "garden1")` | `objects/garden1.dll` | `objects/garden1/<model>.gltf` / `.glb` (+ `.bin`) |
| `string_table(&vfs)` | `maps/language.fla` | `text/language.tsv`, `text/<lang>.toml` |
| `music_track(&vfs, n)` | `music/trackNN.wav` | `music/trackNN.wav` / `.flac` (mod root, not `game/`) |

A missing base container yields an empty one, so mods can add **new banks**. Whole-file
replacement (`game/textures/garden.fla`, etc.) also still works through the VFS.

### File conventions

- **Textures** (`textures/<bank>/<name>.png`, any PNG colour type): replaces the retail texture
  whose exported stem is `<name>` (keeps its CRC — retail names are truncated to 11 chars), or a
  raw CRC when `<name>` is 8 hex digits, else adds `crc("<name>.bmp")`. Converted to X1R5G5B5:
  alpha < 128 → magenta key `0x7c1f` (opaque magenta is nudged to `0x7c1e`). Type from the name
  prefix like the original bank tool: `ai_` alpha-intensity (PNG alpha, or luminance if opaque),
  `ac_` alpha-colour, `prc` procedural, else normal (non power-of-two → plain surface); replacing a
  retail `ai_`/`ac_`/`prc` entry keeps its type. Texture animations stay as in the base bank.
  Full-screen backdrops (`title`, `loading`, `artworkNN`) are not banks — replace the whole `.fla`.
- **Sounds** (`sfx/<bank>/<name>.wav|.flac`): uid = 8 hex digits as written by `f2-export`
  (`sfx/<bank>/<uid>.wav`), else `crc(lowercase name)` like `soundbanker`. Any PCM WAV
  (8/16/24/32-bit int, float, extensible) or FLAC, any channels/rate → 16-bit mono. Native rates
  (11025/11050/22050/44100) are kept; others are resampled to the replaced sample's rate (pitch
  shifts set absolute frequencies) or 22050 for new samples. Looping: a WAV `smpl` loop sets it,
  else the replaced sample's flag. `.ogg` is reported as unsupported.
- **Models** (`objects/<bank>/<model>.gltf|.glb`): `<model>` is the `f2-export` name (sanitized
  lowercase root name, `_<id>` suffix for duplicates) — replaces that model and keeps its id;
  otherwise adds a model with id `crc("<model>.obe")` (the `FindObject` key, verified against all
  retail banks). Importer conventions are exactly the exporter's: raw left-handed coordinates,
  UV = value × 0.000975, key times in seconds × 60 game frames offset by the clip's
  `extras.start`; animations with `extras.speed` (or no extras: laid out back to back, speed 1,
  loop 1) become `MDX_ANIMATION` clips; scene roots → root + `root_siblings`; a node with a `skin`
  is the skin mesh (vertices go to their highest-weight joint's `effectedVerts`, moved into joint
  space by the inverse bind matrix). Per-face texture CRC from material `extras.textureCrc`,
  material name `tex_XXXXXXXX`, or the base-colour image name/URI stem → `crc("<stem>.bmp")`.
  Triangles only, no sparse accessors, external buffers must be relative paths without `..`.
  Not carried by glTF (defaults): `faceFlags` (0, unused by MDX rendering; honoured from
  primitive/material `extras.faceFlags`), `LOCMesh` (none), `bBox` (AABB of the mesh),
  `numVtx` (skin vertex count; unused by MDX), `objectSize` (0, as in every retail model).
- **Text**: `text/language.tsv` has retail/exported columns `id, comment, <language>…`; columns
  match base languages by name, unknown names add a language; rows match by `STR_*` id; empty
  cells keep the base text; unknown ids are appended (existing indices never shift).
  `text/<lang>.toml` = optional `name = "Español"` + `[strings] STR_ID = "text"`; `<lang>` is a
  base name or alias (`uk`/`en`, `fr`, `de`, `it`, `us`) or a new language. Text files **merge
  across mods** (each layer bottom→top, TSV then TOMLs). New-language cells left empty fall back
  to the UK column. The game font is Latin-1 (`pack` warns otherwise).
- **Music**: `music/trackNN.wav` (16-bit PCM) or `.flac` in the mod root already resolve through
  the VFS; `music_track` picks the topmost layer's file (WAV preferred within a layer).

### Tooling: `f2-modkit` CLI

```
f2-modkit new <id> [--dir mods]                       # scaffold mods/<id>/ with mod.toml + dirs
f2-modkit pack <mod dir> [--rendition game-data/usa]  # validate; exit 1 on errors
```

`pack` checks the manifest (parse + engine compatibility), decodes every override through the same
resolver the engine uses (errors for unreadable files), and with a rendition warns when a file is
byte-identical to a retail file (sha1 list in `<rendition>/manifest.json`) or an override decodes
to exactly the retail entry it replaces. Validation only — it does not build an archive yet.

### Host wiring (done, P7-1)

- `frogger` (`crates/frogger/src/data.rs`) builds the `Vfs` with the enabled mods; `Data` is
  the game's `DataSource`. `f2_game::assets::DataSource` has default-implemented
  `object_bank` / `texture_bank` / `sound_bank` / `string_table` methods that `f2-game`
  (object banks, `PTTextureLoad`, `LoadSfx`, `gameTextInit`) and the renderer/audio host use;
  with mods `Data` answers them from `f2_modkit::{object_bank, tex_bank, sound_bank,
  string_table}` (memoised per session, `.warnings` logged as `frogger: mods: warning: …`).
- Music: `Effect::PlayMusic` plays `f2_modkit::music_track(&vfs, n)` when a mod provides the
  track, else the retail track as before.
- Level manifests → `LevelRegistry` (`crates/frogger/src/mods.rs`), passed to every session.
