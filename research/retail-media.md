# Retail media inventory

Verified hands-on 2026-10-09 from the files in `Redump/` and `MyAbandonware/` (gitignored).
Scratch tools used: a ~40-line Python ISO9660 reader over raw 2352-byte sectors, `unshield 1.5.1`,
`ffprobe`.

## 1. Disc images available

| Source | Layout | Notes |
|---|---|---|
| `Redump/Frogger 2 - Swampy's Revenge (USA).zip` | 12 `.bin` + `.cue` (one FILE per track) | **canonical** |
| `Redump/Frogger 2 - Swampy's Revenge (Europe) (En,Fr,De,It).zip` | 12 `.bin` + `.cue` | multi-language |
| `MyAbandonware/…_ISO-Version.zip` | single `FROGGER2.bin` (490 MB) + `.cue` with INDEX offsets | same USA layout, one file; all 12 tracks hash-identical to the Redump USA dump (checked 2026-10-09 with `f2-import --verify`) |
| `MyAbandonware/…_RIP-Version.zip` | installed-style folder (scene rip "MYTH") | data byte-identical to USA `DATA1.CAB`; **exe is cracked** (different hash) — do not use as reference exe |
| `MyAbandonware/Game Extras/…NoCD-Patcher.zip` | crack | not used; the port has no CD check |

## 2. Disc layout (both regions)

Mixed-mode CD, volume label **`FROGGER2`**, created 2000-09-06.

- **Track 1** — MODE1/2352 data, ISO9660:
  ```
  /AUTORUN.EXE /AUTORUN.INF
  /DEMOS/BREAKO~3.EXE                    (Breakout demo; EU: BREAKO~6.EXE)
  /FROGGER2/DATA1.CAB                    InstallShield 5 cabinet — ALL game data + exe
  /FROGGER2/_SYS1.CAB _USER1.CAB SETUP.* _INST32I.EX_ …   installer
  /FROGGER2/DIRECTX.CAB DXSETUP.EXE …    DirectX 7 redistributable
  /FROGGER2/SETUPDIR/{0007,0009,0010,040C}/_SETUP.DLL     (EU only — localized installer)
  /VIDEO/01S.BIK … 12S.BIK, BLITZS.BIK, HASBROS.BIK      Bink FMVs (read from CD at runtime)
  ```
- **Tracks 2–12** — Red Book CD-DA music (USA durations, EU identical except tracks 7/8 by a few sectors):

| Track | Use (`pcaudio.c`) | Length |
|---|---|---|
| 2 | Garden | 2:50 |
| 3 | Ancients | 3:20 |
| 4 | Space | 3:20 |
| 5 | Subterranean | 3:04 |
| 6 | Laboratory | 3:18 |
| 7 | Halloween | 3:05 |
| 8 | Super Retro (+City) | 2:59 |
| 9 | Frontend | 3:04 |
| 10 | Level complete | 0:13 |
| 11 | Game over | 0:11 |
| 12 | Level complete (loop) | 0:18 |

Redump cue sheets give tracks 2–7, 9–12 a 2-second (`00:01:74`) INDEX 00 pregap; track 8 has none.
The importer must strip INDEX 00 pregaps.

## 3. `DATA1.CAB`

- Magic `ISc(`, version field `0x01000004` → **InstallShield 5.x**, single volume, header embedded
  (no `data1.hdr`).
- `unshield` lists 383 files but **extraction fails unless `-O` (old compression) is used** — it
  otherwise tries to read a non-existent `data2.cab`. Also requires the lowercase filename
  `data1.cab` on Linux (copy/symlink). Our Rust extractor must implement the IS5 "old" variant.
- File groups: `Program Executable Files` (`Frogger2.exe`, `binkw32.dll`, `HSBR.exe` — USA only),
  `Data files`, `English` (+ `French`, `German`, `Italian` on EU — readme translations only).
- Extracted `Data files` = 75 MB:

| Dir | Files | Content |
|---|---|---|
| `collision/` | 47 `.col` | tile graphs per level |
| `maps/` | 151 | `entity-W-L.dat` (67), `script-W-L.fev` (36), `sfxanimWL.sam` (~45), `sscript*`, `language.fla` |
| `objects/` | 72 `.dll` | MDX object banks as PE DLLs (19 MB) |
| `scenics/` | 46 `.sce` | scenic placement |
| `sfx/` | 20 `.wus` | sample banks (47 MB) |
| `textures/` | 38 `.fla` + `font/` (BMP) + `procdata/` (BMP) | texture banks, backdrops, art gallery (`artwork00-20.fla`) |
| `Demos/` | 3 `.key` | attract-mode input recordings (record-0-0, 1-0, 6-1) |

- `savedata/savegame.dat` is created at runtime (present in the RIP).

## 4. Region differences (USA vs EU data)

Only these differ: `maps/language.fla`, `textures/loading.fla`, `textures/title.fla`,
`Frogger2.exe`, readmes; USA adds `HSBR.exe` (Hasbro promo launcher). Everything else is
byte-identical → one engine, region-specific language/title assets.

## 5. Identification hashes (SHA-1)

| File | SHA-1 |
|---|---|
| USA Track 01 `.bin` | `6cbaea5810a99483a123a7e6d10e9f3a3ebbebfd` |
| EU Track 01 `.bin` | `66b2157b62bffe43d89143a26572b98f4ddbd3a3` |
| USA `DATA1.CAB` | `b4f7cca8cab0820ffca02d7e8bbfdfa8f792dc55` |
| EU `DATA1.CAB` | `9866393ca8f50b1917db6c7e473fb12aca9b104e` |
| USA `Frogger2.exe` | `51ca60c39a6784a4073d145f6d330f86ad07bc7a` |
| EU `Frogger2.exe` | `66986191e1ba65b1db72b264f6d8e9ceef2c37bb` |
| RIP `Frogger2.exe` (cracked) | `95e2f9c9e4facfa9bc8d5c5ba8bb0d333a4e3437` |

**Per-track SHA-1s (all 12 tracks)** are in `crates/f2-import/src/lib.rs` (`KNOWN_TRACKS`) and
checked by `f2-import --verify`. EU: matches the redump.org entry (serial 99220.DL, Sono press,
write offset -12, PVD created 2000-09-09) — confirmed track-for-track 2026-10-09. USA: computed
from a Redump-named dump; its audio hashes differ from EU (different pressing/offset), still to be
cross-checked against redump.org. Redump track lengths include the 149-sector (00:01:74) pregap
that the importer strips (e.g. EU track 2: 12754 = 12605 + 149).

TODO (Phase 1): hash every extracted file
into a manifest (`research/manifests/{usa,eu}.json`) like the TPW project does.

## 6. Executable facts

`Frogger2.exe` (USA): PE32, image base `0x400000`, **no relocations**, sections `.text`
447 KB, `.rdata`, `.data` 52.7 MB (mostly BSS-style static buffers), `.rsrc`. Imports
`DDRAW`, `DINPUT`, `DSOUND`, `WINMM`, `binkw32`, `ADVAPI32`, `COMCTL32`, `GDI32`, `KERNEL32`…
→ a `dinput.dll`/`ddraw.dll` proxy is a viable injection point for the oracle harness
(see docs/ORACLE.md).

## 7. Video

All 14 `.BIK` are Bink 1 revision `i` (`BIKi`), 640×480, 25 fps, Bink audio (RDFT) — 22 kHz mono
for story FMVs, 44.1 kHz stereo for logos. FFmpeg decodes them (`binkvideo`, `binkaudio_rdft`).
Total ≈ 121 MB.
