# Community work & legal context

Web research 2026-10-09. Items marked *(unverified)* could not be confirmed from a primary source.

## 1. Highway Frogs (preservation group, led by Kneesnap)

GitHub org <https://github.com/HighwayFrogs>:

| Repo | What |
|---|---|
| `frogger2-vss` | Git conversion of the VSS backup `FINAL 18042001.ssa` (our local `frogger2-vss/`). All platforms, full history, **not set up to build**. |
| `frogger2-pc` | Buildable PC tree. Dirs: `Bink, Common, Editor, IDE, MDX, PC, SoftStation, gELF`. **Debug config only** (gELF libs are debug-only; no Release SoftStation.lib). Not byte-matching retail. Toolchain: VC++ 6.0 + DirectX 7 SDK (Win10 / XP VM tested); VS2005 guide exists; VS2019 is the last that can import `.dsp`; DirectPlay headers block modern MSVC. No assets — drop the exe into a retail install. |
| `frogger2-pc` branch **`fixes`** | **"Frogger 2 Enhanced" v1.0.0** (97 commits 2023-09 → 2025-01). Features: <https://github.com/HighwayFrogs/frogger2-pc/blob/fixes/Files/FEATURES.MD> — see `known-bugs.md`. |
| `frogger2-psx`, `frogger2-dc`, `frogger2-n64`, `frogger2-gbc` | Other platform trees |
| `frogger-psx`, `frogger-gb`, `blitz-psx-libraries` | Frogger 1997 etc. |

**License: none** on any of these (GitHub API `license: null`, no LICENSE file).

Local clone `onstream-data-recovery/` = Kneesnap's guide/tools for the OnStream tape the archive
came from (context only; not needed for the port).

## 2. The Frogger 2 development archive

- "Motherload Lot" acquired 2022; public release **1–2 Feb 2025**.
  - Hidden Palace: <https://hiddenpalace.org/News/Frogger_2_Full_Development_Archive>,
    <https://hiddenpalace.org/Frogger_2_Motherload_Lot>
  - archive.org: <https://archive.org/details/frogger2-dev-archives> (~73 GB) — "Provided for
    educational/non-commercial purposes only."
- Contents: ~85 GB uncompressed; >1000 pages of scanned design docs; ~200 music tracks; source
  repos for 7 games/platforms; **tools incl. 3D object converter and script
  compiler/decompiler**; 12 PSX, 2 N64, 7 PC prototype builds; the Enhanced PC port.
- **Useful to us:** design docs (intended behaviour), original tool binaries/sources, source
  for missing functions (`GameStartup`, `mdxLoadBackdrop`), prototype builds for comparison.

## 3. Rights & legal status

- The source release is **preservation without a license grant**. Hidden Palace's
  "public domain unless noted" footer is site boilerplate, not a rights-holder grant.
- Frogger 1997's README (HighwayFrogs/frogger-psx) states Atari Interactive (formerly Hasbro
  Interactive) "explicitly stated they no longer hold any rights" and rights may have been "lost in
  the bankruptcy". Highway Frogs says it would buy rights if an owner appears.
- **Konami** owns the Frogger trademark/characters (Hasbro published under licence). Copyright
  ownership of Swampy's Revenge code/assets is **unresolved** *(analysis, not legal advice)*.
- Oliver Twins approval of the lot purchase *(unverified forum claim)*; even if true, not an IP grant.
- No takedowns of Highway Frogs' repos/archives observed so far.

## 4. Precedents for engine-only projects using user-supplied data

| Project | Basis | Data | Outcome |
|---|---|---|---|
| OpenMW (GPLv3) | clean-room | user's Morrowind files | tolerated for years |
| OpenRCT2 (GPLv3) | decompilation → rewrite | user's RCT2 data | tolerated |
| ScummVM (GPLv2+) | reverse engineering | user's files | tolerated; some freeware deals |
| Ship of Harkinian | OoT decomp | extracted from user's ROM | tolerated so far |
| Perfect Dark port | decomp | user's ROM | tolerated so far |
| re3 / reVC | GTA decomp | user's data | **DMCA 2021 + lawsuit, settled 2023** |
| SM64 PC port | decomp | — | binaries/videos taken down 2020 |

Pattern of survivors: open-source code, **no assets**, data extracted at runtime from a legally
owned copy, no distributed binaries containing copyrighted data, non-commercial.

**Our position:** a translation of the unlicensed Blitz source is a *derivative work* (re3-like
risk profile, not OpenMW-like). Mitigations in [docs/PLAN.md](../docs/PLAN.md) (Legal and distribution posture). Platform crates (importer, Bink
decoder, renderer, mod VFS) are original work and can be cleanly licensed.

## 5. Tooling ecosystem

- **FrogLord** (<https://github.com/Kneesnap/FrogLord>, MIT, Java): Frogger 1997, Great Quest,
  others — **no Frogger 2 support** found.
- No public Frogger 2 format documentation was found (highwayfrogs.net wiki 404) →
  `asset-formats.md` is, as far as we know, the first consolidated write-up.
- No existing Frogger 2 reimplementation in Rust or any other language.
