# Swampy's Revenge Recomp

<img src="packaging/icon/png/128.png" width="96" align="right" alt="App icon: a cartoon frog in a crocodile's jaws (original artwork)">

**Swampy's Revenge Recomp** (`swampys-revenge-recomp`) is a Rust recompilation of the engine of
**Frogger 2: Swampy's Revenge** (PC, Blitz Games / Hasbro Interactive, 2000) that runs on modern
systems using **your own copy** of the game.

> Status: **in development, playable**. Import pipeline, engine, audio, video, the full single-player
> game (26 levels, 3 demos trace-identical to the retail exe), frontend/menus, rebindable controls,
> display enhancements, rollback netplay, mods and a level editor exist. Open: the multiplayer HUD and
> a few small items in [docs/PORT-GAPS.md](docs/PORT-GAPS.md). Not yet release-ready. See
> [PLAN.md Status](PLAN.md#status) and [docs/source-map.md](docs/source-map.md).

- No game assets are included or will ever be distributed. You import them from your own disc
  image (e.g. a Redump dump) or install.
- The port is based on the original source code preserved by
  [Highway Frogs](https://highwayfrogs.net/) (`HighwayFrogs/frogger2-vss`). That source carries no
  license; see [research/community-and-legal.md](research/community-and-legal.md).
- Frogger is a trademark of Konami. This is an unaffiliated, non-commercial fan preservation project.
- The app icon is original artwork (`packaging/icon`, from `tools/icon/make_svgs.py`). After you
  import your game, the app switches to an icon cut from **your copy's** artwork, made on your
  machine (window/taskbar and Windows shortcuts; on Android the recents icon and an optional
  home-screen shortcut).

## Getting started (importing your copy)

```sh
cargo run --release -p f2-import -- "/path/to/Frogger 2 - Swampy's Revenge (USA).zip"   # or .cue/.bin/.iso, a disc folder, or an install folder
cargo run --release -p f2-import -- --list
```

Data goes to `$F2_DATA_ROOT` or the platform data dir (`~/.local/share/frogger2-rs/data/<rendition>`).
Use `--data-root game-data` to keep it inside the checkout (gitignored). `--oracle` additionally
extracts the original exe for developer parity testing.

## Documentation

- [docs/USER-GUIDE.md](docs/USER-GUIDE.md): playing it: import, controls, profiles, config, saves, troubleshooting
- [docs/TOOLS.md](docs/TOOLS.md): importer, viewer, exporter, script compiler, modkit, trace/parity tools, oracle runner
- [docs/BUILDING.md](docs/BUILDING.md): building, packaging, WSL notes

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
