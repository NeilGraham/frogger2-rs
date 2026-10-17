# Contributing

Thanks for helping. A few rules keep the project safe to host and the port faithful.

## Never commit game data
No files from your disc or install, no extracted or converted assets, no exes or DLLs, no saves,
no videos, music, textures or decoded dumps, and no screenshots that are mostly game art. Hashes,
file layouts, names and notes are fine. Bug reports are the same: describe the problem or attach
a recorded input (`.key`) or a trace, not game files.

## Parity first
The simulation (`f2-game`) is a faithful port of the original C, checked tick for tick against
the retail exe. Do not change behaviour while porting: log the bug in
[research/known-bugs.md](research/known-bugs.md), mark the code `// BUG(original):`, and put the
fix behind an option of the *Enhanced* profile ([docs/enhancements.md](docs/enhancements.md)).
Fixed-point maths and the RNG must stay bit-exact ([docs/porting-guide.md](docs/porting-guide.md)).
`f2-game` stays platform-free and deterministic (fixed 60 Hz tick).

## Building and testing
See [docs/BUILDING.md](docs/BUILDING.md). `cargo fmt`, `cargo clippy` and `cargo test --workspace`
should pass. Tests that need your own game data are `#[ignore]`d or gated on an environment
variable; the parity tools are described in [docs/TOOLS.md](docs/TOOLS.md) and
[docs/ORACLE.md](docs/ORACLE.md).

## Licensing
Contributions are under GPL-3.0-or-later (`f2-bink`: LGPL-2.1-or-later). Say where code adapted
from elsewhere comes from, and keep its attribution header.
