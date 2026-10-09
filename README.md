# frogger2-rs (working title)

A Rust port of **Frogger 2: Swampy's Revenge** (PC, Blitz Games / Hasbro Interactive, 2000) that
runs on modern systems using **your own copy** of the game.

> Status: **planning** (Phase 0). Nothing playable yet. Start with [PLAN.md](PLAN.md).

- No game assets are included or will ever be distributed. You import them from your own disc
  image (e.g. a Redump dump) or install.
- The port is based on the original source code preserved by
  [Highway Frogs](https://highwayfrogs.net/) (`HighwayFrogs/frogger2-vss`). That source carries no
  license; see [research/community-and-legal.md](research/community-and-legal.md).
- Frogger is a trademark of Konami. This is an unaffiliated, non-commercial fan preservation project.

## Getting started (importing your copy)

```sh
cargo run --release -p f2-import -- "/path/to/Frogger 2 - Swampy's Revenge (USA).zip"   # or .cue/.bin/.iso, a disc folder, or an install folder
cargo run --release -p f2-import -- --list
```

Data goes to `$F2_DATA_ROOT` or the platform data dir (`~/.local/share/frogger2-rs/data/<rendition>`).
Use `--data-root game-data` to keep it inside the checkout (gitignored). `--oracle` additionally
extracts the original exe for developer parity testing.

## Layout

| Path | What |
|---|---|
| [PLAN.md](PLAN.md) | Goals, strategy, phased roadmap, decisions needed |
| [research/](research/) | Findings: original source tree, retail media, file formats, legal, bugs, sibling-project reuse |
| [docs/](docs/) | Design: architecture, porting guide, source map, asset import, enhancements, modding |
| `frogger2-vss/`, `onstream-data-recovery/` | Reference clones (gitignored) |
| `Redump/`, `MyAbandonware/` | Local game media (gitignored, never committed) |

## Credits

Original game by Blitz Games (Interactive Studios). Source preservation by Highway Frogs
(Kneesnap and contributors). Format research builds on their archive and the Frogger 2 Enhanced project.
