# Frogger 2 Rust port — instructions for every Claude session

Rust port of *Frogger 2: Swampy's Revenge* (PC, 2000) from the preserved original source, running
on assets imported from the user's own copy. Strategy: **port, then improve**, verified by
deterministic trace parity against the retail exe.

## Read first
- `docs/PLAN.md` (strategy, status, legal posture) — once per session.
- `docs/porting-guide.md` before translating any C; `docs/source-map.md` to pick/track work.
- `research/asset-formats.md` before touching parsers.

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
- WSL2 Ubuntu; Windows interop available (Windows-side toolchains for the oracle and Windows builds).
- `unshield` (use `-O`, lowercase `data1.cab`) and `ffmpeg` are installed; passwordless sudo.
- Quick data extraction for research: `research/prototypes/README.md`.

**Memory caps:** cargo test/run binaries are capped at 8 GB via `tools/memcap.sh` (the cargo runner). Wrap other heavy commands with `tools/memcap.sh <cmd>`. See "Memory caps" in docs/BUILDING.md.
