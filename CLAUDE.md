# Frogger 2 Rust port — instructions for every Claude session

Rust port of *Frogger 2: Swampy's Revenge* (PC, 2000) from the preserved original source, running
on assets imported from the user's own copy. Strategy: **port, then improve**, verified by
deterministic trace parity against the retail exe.

## Read first
- `PLAN.md` (strategy, phases, open owner decisions) — once per session.
- `docs/porting-guide.md` before translating any C; `docs/source-map.md` to pick/track work.
- `research/asset-formats.md` before touching parsers.
- Sibling project with reusable tooling: `../theme-park-world-recomp` (see `research/sibling-tpw-reuse.md`).

## Hard rules
- **Never commit game data**: no files from `Redump/`, `MyAbandonware/`, extracted CAB contents,
  exes/DLLs, videos, music, textures, saves, or decoded dumps. Hashes, layouts, names, notes are fine.
- Run Python on untrusted/extracted data with `python3 -I`, scripts outside the data dir.
  Never execute anything from extracted media except the retail exe inside an oracle run root.
- Extracted media goes to the session scratchpad or the gitignored `game-data/`.
- Reference sources (`frogger2-vss/`) are read-only.
- Parity first: no behaviour fixes while porting; log bugs in `research/known-bugs.md` and mark
  `// BUG(original):`.
- Fixed-point and RNG must be bit-exact (`FMul`/`FDiv`/`Random` semantics in porting guide §3).
- `f2-game` stays platform-free and deterministic (fixed 60 Hz tick).

## Environment
- WSL2 Ubuntu; Windows interop available (Windows-side toolchains as in the TPW project).
- `unshield` (use `-O`, lowercase `data1.cab`) and `ffmpeg` are installed; passwordless sudo.
- Quick data extraction for research: `research/prototypes/README.md`.
