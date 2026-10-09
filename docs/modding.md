# Modding architecture

Target: Phase 7, but the VFS foundation lands in Phase 1 so every asset load already goes through it.

## 1. Mod layout

```
mods/
  my-mod/
    mod.toml              # id, name, version, authors, engine version range, load order hints, profile flags
    game/                 # overrides/additions mirroring the imported tree (case-insensitive)
      textures/Garden/00wate04.png      # loose replacement for one texture in a bank (by name/CRC)
      sfx/garden/hopongrass.ogg         # loose replacement for one sample (by name/CRC)
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
| Sounds | PCM in `.wus` | WAV, OGG, FLAC |
| Models | MDX structs in PE `.dll` | glTF 2.0 (meshes, hierarchy, keyframe anims) |
| Music | CD-DA | OGG/FLAC with loop points |
| Text | `language.fla` TSV | TSV/TOML string tables (new languages possible) |
| Levels | `.col` `.sce` `entity.dat` `.fev` | same binary formats **or** text equivalents (TOML/RON) + script source |
| FMV | Bink 1 | Bink 1, or WebM (if a decoder is enabled) |

Exporters in `f2-viewer` (PNG/WAV/glTF/TOML) give modders editable starting points.

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
