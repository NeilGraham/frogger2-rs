# ETC — the Frogger 2 event-script language

Each level has an event script, `maps/script-<world>-<level>.fev`. It is compiled from ETC
("Event/Trigger Compiler") source. `tools/f2-etc` is a port of the original compiler
(`frogger2-vss/teamSpirit/Etc`) and also has a decompiler:

```sh
f2-etc compile level.etc -o script-1-1.fev     # -d: add line-number records, -t: check only
f2-etc decompile script-1-1.fev -o level.etc   # recompiles to the identical bytes
f2-etc check script-*.fev                      # decode + decompile/recompile self-test
f2-etc header -o codes.h                       # the opcode header the game includes
```

Commands and triggers come from two tables, `tools/f2-etc/tables/commands.ec` and
`triggers.ec`. The original tables were lost. These ones were rebuilt from `codes.h` and the game's
interpreter (`Common/script.c`), and the script names in them were picked for this port. To use your
own tables, pass `--tables <dir>`.

## Lexical rules

- **Comments**: everything from `;` to the end of the line, even inside a string.
- **Names** (commands, triggers): letters and `_` only, no digits. Case does not matter.
- **Numbers**: an optional `+` or `-`, then digits, then optionally `.digits`. There are no
  exponents and no hex.
- **Strings**: `"..."` on one line, with no escapes. A string cannot contain `"` or `;`.
- **Variables**: `%name` (letters and `_` only, case-insensitive).
- **Symbols**: `{ } ( ) ,`. Whitespace is free-form.

## Statements

A script is a list of commands. Every command except `On`, `Set` and `Include` uses call syntax
with parentheses, even when it takes no arguments: `NoFog()`.

```etc
Set %TRIGGER_RISING 16
Set %door_tile 312                 ; Set %var <token>: compile-time constant (no output)
Include "common.etc"               ; splice in another source file

On FrogOnTile(0, %door_tile) %TRIGGER_RISING {   ; On <trigger> <flags 0-255> <block>
    PlaySound("door.wav", 5)
    If(FlagSet(2), { ChangeLevel(1, 2) })        ; If(<trigger>, <block>)
}

On Wait(2.5) 1 ShakeCamera(4.0, 0.5)             ; a block can also be a single command
```

- **`On`** registers the block to run whenever the trigger fires. The flags are the game's
  `TRIGGER_*` bits: `1` ONCE, `2` DELAY, `4` ALWAYS, `8` FIRED, `16` RISING (fire when the
  condition becomes true). The decompiler writes these as `Set %TRIGGER_… n` variables at the top of
  the file.
- **`If`** runs its block immediately, if the trigger holds at that moment.
- A **block** is either one command or `{ commands }`. `On`/`If` can be nested inside blocks.
- **`Set %name value`** stores the next token as text. You can then use the variable anywhere a
  number or string is expected. An integer variable must hold only digits (an optional `-` first).
  A float variable may also hold a `.fraction`. Variables are substituted when the script is
  compiled, so they cost nothing at runtime.
- **`Include "file"`** carries on compiling from that file. The compiler looks for the file as
  given, then next to the current file, then in the `-I` directory.

## Argument types

| Table type  | Written as                       | Stored as                                          |
|-------------|----------------------------------|----------------------------------------------------|
| `p_byte`    | integer 0–255                    | `u8`                                               |
| `p_word`    | integer −65535…65535             | low 16 bits                                        |
| `p_integer` | integer                          | `i32`                                              |
| `p_float`   | number                           | `i32` = `(int)((float)x * 65536)` (16.16, truncated) |
| `p_string`  | `"text"` (max 255 bytes)         | `u8` length + bytes                                |
| `p_trigger` | trigger call, e.g. `Wait(1.0)`   | trigger opcode + its arguments                     |
| `p_block`   | block                            | `i32` size, `i16` count, commands                  |

The game reads most `p_float` values as `FIXED` (`value >> 4`, so 20.12 fixed point) in seconds or
world units. Because floats are truncated, `0.1` is stored as 6553.

## Triggers

`EnemyOnTile(enemy, tile)`, `FrogOnTile(frog, tile)`, `FrogOnPlatform(frog, platform)`,
`RandomWait(min, max)`, `EnemyAtFlag(enemy, node)`, `PlatAtFlag(platform, node)`, `Wait(seconds)`,
`FlagSet(flag)`, `CounterEquals(counter, value)`, `FrogIsDead(frog)`, `LevelIsOpen(world, level)`,
`OnPath(frog, enemy)`, `And(t, t)`, `Or(t, t)`, `Not(t)`.

## Commands

| Command | Arguments |
|---|---|
| `SetEnemyFlag` / `ResetEnemyFlag` / `SetPlatFlag` / `ResetPlatFlag` | uid (word), flags (integer) |
| `SetEnemy` / `SetPlatform` | uid, on/off (byte) |
| `MoveEnemy` / `MovePlat` | uid, path node (word) |
| `AnimateActor` | uid, anim, flags, speed (float) |
| `AnimateFrog` | frog, anim, flags, speed (float) |
| `ScaleEnemySpeed` / `ScalePlatSpeed` | uid, factor (float) |
| `CollapsePlat` | uid, seconds |
| `ChangeVolume` | n (byte), delta (float), platform (word) |
| `ChangeLevel` | world, level |
| `Hop` | frog, tile |
| `Fog(r, g, b, min, max)` / `NoFog()` | colour bytes; distances ×100 |
| `CameoModeOn()` / `CameoModeOff()` | — |
| `SetFlag` / `ResetFlag` | flag (byte) |
| `IncCounter` / `DecCounter` | counter (byte) |
| `SetCounter` | counter (byte), value (word) |
| `SfxEnemy` / `SfxPlaceholder` / `SfxPlaceholderNoClip` | uid, type (byte), size, speed, accn, lifetime (floats) |
| `HurtFrog()` / `KillFrog(frog, death)` | — |
| `ChangeFrog` | frog, model name (string, < 20 chars) |
| `ShakeCamera` | shakiness, falloff (floats) |
| `SetTileP(uid, state)` / `SetStartTileP(uid)` | placeholder-enemy tiles |
| `TeleportP(frog, uid, seconds)` / `SpringP(frog, uid, height, time)` | |
| `PlaySound(name, uid)` / `StopSound(name)` | sample name (< 32 chars), owner uid |
| `Hint(string)` | language string index |
| `ResetScript()` / `EndLevel()` / `PokeFrogs()` | — |
| `Debug("text")` | debug builds only |

## Compiled layout (`.fev`)

`u8 version (3)`, `i32 size`, `i16 count`, then the commands. Each command is a `u8` opcode followed
by its arguments in table order. `On` is stored as `0x40, trigger, u8 flags, block`. With `-d`, each
command is preceded by a `0xFF, i16 0, i16 line` record.

## Differences from the original compiler

- Every error makes the compile fail. The original sometimes wrote a truncated file after a lexical
  error.
- There is no 80-character token limit. Include lookup also tries the including file's directory
  (the original computed that path but never used it).
- Kept for compatibility: any symbol (not only `{`) opens a `{ }` block. This is a bug in the
  original.
