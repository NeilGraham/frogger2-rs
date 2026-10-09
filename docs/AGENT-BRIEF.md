# Standing rules for implementer agents

You are implementing ONE task of the Frogger 2 Rust port (`<repo>`).
Read `CLAUDE.md` (hard rules) and only the docs your brief names.

1. **Scope**: touch only the files/crates your brief lists. Do NOT edit the root `Cargo.toml`,
   `work/`, `docs/source-map.md`, `PLAN.md`, or other agents' crates. Need a change there? Put it
   in your final report. You may add dependencies to *your* crate's `Cargo.toml`.
2. **Builds**: use your own target dir to avoid lock contention:
   `CARGO_TARGET_DIR=target/agent-<crate> cargo test -p <crate>`. Keep `cargo clippy -p <crate>`
   warning-free where reasonable. Rust 1.98, edition 2021.
3. **Game data**: imported retail data is at `game-data/usa/` (`game/`, `video/`, `music/`,
   `oracle/`) and `game-data/eu/` when present. It is gitignored and copyrighted — never copy it into
   tracked files, test fixtures, or code. Tests that need it must read the path from an env var
   (`F2_GAME_DIR`, default `game-data/usa`) and skip gracefully when absent.
4. **Original source** (read-only reference): `frogger2-vss/teamSpirit/FroggerProject/{Common,PC}`
   and `frogger2-vss/teamSpirit/MDX Library`. Format docs: `research/asset-formats.md`.
   Parsers already exist in `crates/f2-formats` (use them; don't re-parse files).
5. **Faithfulness** (gameplay/maths): follow `docs/porting-guide.md` — wrapping 32-bit ints,
   exact operator semantics, original names in snake_case, `/// Port of `Func` (file.c:LINE).`
6. **Untrusted data**: run Python with `python3 -I`; never execute anything from `game-data/`.
7. **Done means** the command in your brief passes. Stop after the budget in your brief even if
   unfinished and report partial progress honestly.
8. **Report** (final message, ≤ 15 lines): status (done/partial/blocked), what you built (files),
   the acceptance command output summary, deviations/open issues, requested changes to shared files.
