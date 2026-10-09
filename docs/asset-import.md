# Asset import (`f2-import`)

How a player turns *their own copy* of Frogger 2 into the data directory the Rust engine runs
from. Facts about the media: [`../research/retail-media.md`](../research/retail-media.md).

## 1. User flow

```
frogger2 import <path>          # path to .cue, .bin, .iso, .zip (Redump), a mounted disc, or an install folder
frogger2 import --list          # show installed data sets
frogger2 import --verify        # re-hash installed data against manifests
frogger2                        # first run with no data → launcher shows "Import your game" with a file picker
```

The importer never writes next to the source and never modifies it. Output:

```
<data_dir>/frogger2-rs/data/<rendition>/        e.g. usa, eu
  manifest.json            source identification, hashes, importer version
  game/                    DATA1.CAB "Data files" tree, original names, lowercased
    collision/ maps/ objects/ scenics/ sfx/ textures/ demos/
  video/                   *.bik copied as-is (decoded at runtime by f2-bink)
  music/track02.flac … track12.flac
  cache/                   optional converted forms (decoded textures/meshes), rebuildable
```

`<data_dir>` = platform data dir (`~/.local/share`, `%APPDATA%`, `~/Library/Application Support`).
Original files are kept intact so parsers stay the single source of truth and mods can override
them; converted caches are disposable.

## 2. Supported inputs

| Input | Handling |
|---|---|
| Redump `.zip` | read entries in place (`zip` crate, streaming), then treat as multi-file cue |
| Redump `.cue` + 12 `.bin` | `FILE` per track; MODE1/2352 data + AUDIO tracks; skip INDEX 00 pregaps |
| Single `.bin` + `.cue` (MyAbandonware) | one `FILE`, tracks by `INDEX 01` MSF offsets |
| `.iso` (2048-byte sectors) | data only → music missing; prompt for audio tracks or continue without music |
| Mounted disc / drive letter | read filesystem directly; CD-DA via OS raw read (Linux `CDROMREADAUDIO`, Windows `IOCTL_CDROM_RAW_READ`) — Phase 1b |
| Installed folder (has `Frogger2.exe`, `objects/`) | copy data dirs; no videos unless `Video/` present; no music |
| `.ccd/.img`, `.mds/.mdf`, `.chd` | stretch (CHD via `chd` crate) |

## 3. Pipeline

1. **Open** source → `MediaSource` (sector reader + optional audio tracks).
2. **Identify**: ISO9660 PVD volume id `FROGGER2`; hash `FROGGER2/DATA1.CAB` → known rendition
   (USA/EU table). Unknown hash → warn, continue in "unverified" mode (e.g. other regions, demos).
3. **Extract** `DATA1.CAB` (InstallShield 5, single volume, "old compression") — file group
   `Data files` only; also `Program Executable Files/Frogger2.exe` **into a separate
   `oracle/` folder only if the user opts in to developer tools** (never needed to play).
4. **Copy** `VIDEO/*.BIK`.
5. **Rip** CD-DA tracks 2–12 → FLAC (lossless, `flacenc` crate) — verify against Redump track
   SHA-1s when the source is a Redump dump.
6. **Validate**: run every `f2-formats` parser over the extracted tree (fast), fail loudly on any error.
7. **Write** `manifest.json`.

## 4. InstallShield 5 CAB notes

- `unshield` (MIT, C) is the reference implementation; our files need its `-O` path.
- Header (`ISc(` magic, version `0x01000004`) embedded in `data1.cab`; file descriptors give
  compressed size/offset/flags; data is zlib-compressed in chunks.
- Write a clean-room Rust reader from the documented structure (`unshield`'s `libunshield/` is MIT,
  so porting it is also licence-compatible with attribution).
- Golden test: extracted file SHA-1s must match the manifest of `unshield -O` output.

## 5. Privacy / legal

The importer only reads what the user points it to, never phones home, and the output stays on the
user's machine. Releases contain **no** game data, hashes only.

## 6. Reuse

Port `cue.rs`, `image.rs`, `iso9660.rs`, `identify.rs`, `hashdb.rs`, `store.rs` from
`../theme-park-world-recomp/crates/tpw-import` (see `research/sibling-tpw-reuse.md`).
