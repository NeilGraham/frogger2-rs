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
    levels/<new-level>/…                # new levels (see §3)
    scripts/*.rhai                      # optional behaviour scripts (Phase 7b)
```

`config.toml` lists enabled mods in order; later mods win. The VFS resolves
`mods/<n>/game/<path>` → … → `data/<rendition>/game/<path>`.

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

### Level editor (P7, `crates/frogger/src/editor/`)

`frogger --editor --level W L [--mod ID] [--mods-dir DIR] [--editor-cmd "CMD; CMD"]` (Ctrl+E toggles
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
`mods/<ID>/game/{collision,scenics,maps}/…` (+ `mod.toml` when missing and the `.etc` source next to
the `.fev`); `frogger --mod ID --level W L` plays it. Writers in `f2-formats`
(`Collision::write`, `write_scenics`, `EntityFile::write` incl. the low bits the loader drops,
`SamFile::write`, `Script::write`) reproduce every retail file byte-for-byte
(`crates/f2-formats/tests/roundtrip.rs`).

## 4. Behaviour scripting (Phase 7b)

The original event language covers triggers/actions. For new behaviour (new enemy types, modes),
expose a sandboxed scripting API (Rhai or Lua via `mlua`) with hooks: `on_level_start`,
`on_tick`, `on_frog_land(tile)`, custom enemy update functions. Scripts run inside the
deterministic tick (no wall-clock, seeded RNG only) so replays/netplay remain valid.

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

### Host wiring (not done yet — owned by the engine/game agents)

1. `frogger` builds the `Vfs` with `f2_mods::load_enabled_mods(rendition, mods_dir, &ids)`.
2. Wherever `f2-engine`/`f2-game` currently read and parse a bank file (texture bank load,
   `LoadSfxSet`, `LoadObjBank`, `gameTextInit`), call `f2_modkit::{tex_bank, sound_bank,
   object_bank, string_table}(&vfs, name)` instead and use `.value`; log `.warnings`.
   The returned types are the same `f2-formats` structs, so nothing downstream changes.
3. Music: replace `AudioEngine::set_music_dir` lookups with `f2_modkit::music_track(&vfs, n)` and
   `play_music_stream(WavStream::open_any(&path).ok(), n, looped)`.
4. Optional cache: the effective banks are deterministic in the enabled-mod list, so the host may
   memoise them per bank name for a session.
