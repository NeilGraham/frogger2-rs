# Asset import (`f2-import`)

How a player turns *their own copy* of Frogger 2 into the data directory the Rust engine runs
from. Facts about the media: [`../research/retail-media.md`](../research/retail-media.md).

## 1. User flow

```
frogger                                   # first run with no data → import screen (drives, image file, folder)
f2-import <source>                        # any source of §2 (image, drive letter, /dev/sr0, folder)
f2-import --list-drives                   # optical drives, labels, Frogger 2 disc?, TOC
f2-import <iso or folder> --music DIR     # data-only source + music tracks 02–12 as WAV/FLAC
f2-import --list                          # installed renditions
f2-import --verify-installed              # re-hash installed data against manifests
f2-import --convert FMT IMAGE OUTDIR      # developer tool: re-encode an image (cue|ccd|mds|mds-sub|nrg|nrg1|toc|iso)
```

**First-run screen** (`crates/frogger/src/firstrun.rs`): when `Data::locate` finds nothing, the
windowed game shows an import screen drawn with a built-in 5×7 font (no game data exists yet). It
lists optical drives (a Frogger 2 disc is highlighted: "Import from the Frogger 2 disc in D:"),
"Import from an image file…" and "…a folder…" (native dialog: PowerShell `OpenFileDialog` on
Windows, `zenity`/`kdialog` on Linux), a typed path, and drag & drop of a file onto the window.
The import runs on a worker thread with a progress bar and log; on success the game restarts
itself with the same arguments (winit allows one event loop per process) and boots normally.

The importer never writes next to the source and never modifies it. Output:

```
<data_dir>/frogger2-rs/data/<rendition>/        e.g. usa, eu
  manifest.json            source identification, hashes, music source/note, drive-rip report
  game/                    DATA1.CAB "Data files" tree, original names, lowercased
  video/                   *.bik copied as-is (decoded at runtime by f2-bink)
  music/track02.wav … track12.wav   (.flac with --flac; or the files given with --music)
```

`<data_dir>` = platform data dir (`~/.local/share`, `%APPDATA%`, `~/Library/Application Support`).

## 2. Supported sources

Verified = an automated test imports it and checks the result. "Real data" = the
`crates/f2-import/tests/formats.rs` parity test (`cargo test --release -p f2-import --test formats
-- --ignored`): the owner's Redump USA image is converted into each format in a temp dir, imported,
and every file hash (data, videos, music) must equal the Redump import's; the per-track Redump
hashes must verify too. It passes on Linux and on Windows 11 (2026-10-09). "Windows mount" =
`crates/f2-import/tests/windows_mount.rs` (Windows only, `F2_TEST_ISO=…`): attaches an `.iso`
with `AttachVirtualDisk` (the API behind Explorer's "Mount"), then checks drive enumeration,
label, virtual-drive detection, the `Get-DiskImage` backing path, `IOCTL_CDROM_READ_TOC`, and that
importing via the backing image and directly from the drive's file system give identical files.

| Source | Data | Video | Music | Verified |
|---|---|---|---|---|
| Redump `.zip` (12 `.bin` + `.cue`, read in place) | yes | yes | yes | real data (reference) |
| `.cue` + per-track `.bin` (Redump) | yes | yes | yes | real data |
| Single `.bin` + `.cue` (INDEX 00/01 offsets, e.g. MyAbandonware) | yes | yes | yes | real data; the MyAbandonware USA image verifies against all 12 Redump track hashes (Windows CLI) |
| BIN/CUE with several data tracks | yes (first data track) | yes | yes | synthetic |
| CloneCD `.ccd` + `.img` (+ `.sub`, ignored) | yes | yes | yes | real data + synthetic |
| Alcohol `.mds` + `.mdf`, 2352 or 2448 RAW+SUB | yes | yes | yes | real data + synthetic |
| Nero `.nrg` v2 (`NER5`, DAOX) and v1 (`NERO`, DAOI); TAO `ETN2`/`ETNF` | yes | yes | yes | real data (DAO) + synthetic |
| cdrdao `.toc` + `.bin` (big-endian audio unless `SWAP`; `.wav` FILEs) | yes | yes | yes | real data + synthetic |
| `.zip` containing any of the above (stored or deflated) | yes | yes | yes | real data (zip of ccd) |
| Plain `.iso` (2048-byte, or raw 2352/2448 sniffed by sync) | yes | yes | no → note, or `--music` | real data |
| Physical disc in a drive (Windows `D:`, Linux `/dev/sr0` or its mount point) | yes (file system, or raw ISO9660 read if unmounted on Linux) | yes | yes: CD-DA rip with offset detection | rip logic: emulated drive over real data; Windows IOCTL TOC on a virtual drive; CD-DA raw reads and Linux ioctls untested on hardware (no disc available) |
| Windows-mounted image (Explorer "Mount" / `Mount-DiskImage`) | yes | yes | from the backing image (an `.iso` has none → note) | Windows mount (Windows 11) |
| Other virtual drives exposing CD-DA (Daemon Tools, WinCDEmu…) | yes | yes | yes: rip via IOCTL as for a physical drive | untested |
| Disc folder (copied disc root with `FROGGER2/DATA1.CAB`) | yes | yes | no → note, or `--music` | existing |
| Install folder (`objects/`, `collision/`, optional `Video/`) | yes | if present | no → note, or `--music` | existing |
| `--music DIR`: WAV (16-bit PCM) / FLAC files for tracks 02–12 | – | – | yes | real data (ISO + Redump WAVs) |
| MAME `.chd`, DiscJuggler `.cdi`, `.7z`, `.rar`, `.ecm`, MP3/Ogg music | rejected with a conversion hint (no pure-Rust readers available offline) | | | |

Unsupported formats print how to convert them (e.g. `chdman extractcd -i game.chd -o game.cue`).

### Physical drives

* **Enumeration** (`f2_media::cdrom::list_drives`): Windows — drive letters of type
  `DRIVE_CDROM`, `GetVolumeInformationW` label, `IOCTL_STORAGE_CHECK_VERIFY2` (media present),
  `IOCTL_STORAGE_QUERY_PROPERTY` (model, virtual bus); Linux — `/sys/block/sr*`, model from
  sysfs, `CDROM_DRIVE_STATUS`, PVD volume id read from the device, mount point from
  `/proc/mounts`. A disc counts as Frogger 2 when its label is `FROGGER2` or
  `FROGGER2/DATA1.CAB` exists.
* **Raw access**: Windows `\\.\X:` + `IOCTL_CDROM_READ_TOC` (MSF) + `IOCTL_CDROM_RAW_READ`
  (`TrackMode = CDDA`, 20 sectors per call) + `IOCTL_CDROM_SET_SPEED`; Linux `CDROMREADTOCHDR`,
  `CDROMREADTOCENTRY` (LBA), `CDROMREADAUDIO` (25 frames per call), `CDROM_SELECT_SPEED`.
  Hand-written FFI; every `unsafe` block is in `crates/f2-media/src/cdrom/{windows,linux}.rs`
  behind safe wrappers.
* **Read errors**: a failing batch is retried once, then the drive is slowed to 4× and the batch
  re-read sector by sector (5 attempts each); still-unreadable sectors become silence and are
  reported (`rip.unreadable_sectors` in the manifest).
* **Offset detection**: drives return audio shifted by their read offset, and the pressing has a
  write offset (EU −12); Redump tracks are corrected. The importer reads the whole audio area
  (≈ 270 MB) with a 7-sector margin, then searches the sample shift (common AccurateRip offsets
  first, then ±3000 samples) that makes a short track hash to its `KNOWN_TRACKS` value; tracks are
  cut with the Redump pregaps (`REDUMP_PREGAPS`: 149 sectors, none before track 8). Result:
  `verified (offset +N, 11 of 11 tracks match Redump)` — the music files are then byte-identical to
  a Redump import — or `unverified (offset 0 assumed)`. `--drive-offset N` forces an offset.
  Tested with `f2_media::cdrom::ImageDrive` (a drive emulated over the Redump image with read
  offsets +6/−12/+667 and injected transient errors); never run against real hardware here.
* **Mounted images**: Windows virtual drives (bus type virtual / "Virtual DVD-ROM") are looked up
  with `Get-Volume -DriveLetter X | Get-DiskImage` (PowerShell, hidden). If the backing image is
  found the import reads the image file instead (a mounted `.cue`/`.bin` cannot be mounted by
  Windows itself; a mounted `.iso` has no audio, so the manifest records the music note).

## 3. Pipeline

1. **Open** source → `MediaSource` (sector reader + optional audio tracks).
2. **Identify**: ISO9660 PVD volume id `FROGGER2`; hash `FROGGER2/DATA1.CAB` → known rendition
   (USA/EU table). Unknown hash → warn, continue in "unverified" mode (e.g. other regions, demos).
3. **Extract** `DATA1.CAB` (InstallShield 5, single volume, "old compression") — file group
   `Data files` only; also `Program Executable Files/Frogger2.exe` **into a separate
   `oracle/` folder only if the user opts in to developer tools** (never needed to play).
4. **Copy** `VIDEO/*.BIK`.
5. **Rip** CD-DA tracks 2–12 (INDEX 01 → end) → WAV (or FLAC with `--flac`); from a drive with
   offset detection (§2); `--verify` checks every track of any raw image against the Redump hashes.
   Missing music: `--music DIR`, else `manifest.music_note` explains what to do.
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
