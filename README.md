# Frogger 2: Swampy's Revenge (Rust)

A Rust port of **Frogger 2: Swampy's Revenge** (PC, Blitz Games / Hasbro Interactive, 2000). It
runs on modern systems and can build the game for consoles and phones, using **your own copy** of
the game. No game assets are included or ever distributed.

> **Status: in development, playable.** The whole single-player game (26 levels) runs and is
> checked frame by frame against the retail exe. Not yet release-ready: see
> [Known issues](#known-issues).

## Play it everywhere

Import your PC copy once, then play on the desktop or export a ready-to-run build for another
platform. Exporting needs no SDKs and no extra tools.

| Platform | How | State |
|---|---|---|
| **Windows**, **macOS** | run the app, or export a standalone folder / `.app` | tested, no issues found |
| **Linux** | run the app, or export an AppImage | built and tested in CI, not yet played on a desktop |
| **Android** | export an `.apk` with the game inside, sideload it | played start to finish on a handheld, no issues found |
| **PS4** | export a fake package (`.pkg`) for a jailbroken console | tested on a jailbroken PS5 (firmware 11.40), no issues found |
| **PS5** | export an app folder / `.ffpkg` for a jailbroken console | tested on a jailbroken PS5 (firmware 11.40), no issues found |
| **N64** | export a `.z64` ROM | **experimental**: runs on a ModRetro M64 and in the ares emulator, but can black-screen when leaving a level for the menu; still being optimised |

```sh
frogger2 --export windows,android,ps4,ps5,n64 --out exports     # or: frogger2 --export all
```

The exported builds are named and iconed from your own disc, and they contain your game data:
keep them for your own devices. Details in [docs/EXPORT.md](docs/EXPORT.md).

## Getting started

1. Build it ([docs/BUILDING.md](docs/BUILDING.md)):
   `cargo build --release -p frogger -p f2-import`
2. Start `frogger2` (`frogger2.exe` on Windows). With no game data it opens an import screen: pick your disc drive, an image
   (`.zip`, `.cue`, `.bin`, `.iso`), or an install folder, or drag one onto the window. The
   command line works too: `f2-import "Frogger 2 - Swampy's Revenge (USA).zip"`.
3. Play. The first start after importing gets you the logos, intro, title and the 3D frontend, as
   in the original.

Everything else (controls, saves, multiplayer, troubleshooting) is in
[docs/USER-GUIDE.md](docs/USER-GUIDE.md).

## Features

- **Faithful by default.** The *Authentic* profile matches the retail game, including its quirks.
  The *Enhanced* profile (the default) adds fixes and comforts. Switch any time in the settings.
- **Settings screen**, styled like the game's own menus (pause menu → *Settings*, or F10), with
  everything applied live and saved:
  - *Display*: windowed / borderless / fullscreen, resolution, VSync, frame cap up to 240,
    widescreen and ultrawide, render scale, anti-aliasing, texture filtering, perspective-correct
    textures, draw distance, fog, brightness, FPS counter, and a sharp high-resolution font.
  - *Audio*: master, music and effects volume.
  - *Controls*: rebindable keyboard and gamepad, stick dead zone, button layout, rumble, input
    buffering for hops.
  - *Gameplay*: save slots and auto-save, speedrun timer, intro and video skipping, assists
    (infinite lives, invulnerability), the stuck-in-the-tube fix, and the text language.
- **2 to 4 players**: local split-screen as in the original, and online play with rollback
  netcode.
- **Mods and a level editor**: layer new files, scripts and levels over the retail data.
- **High refresh rates** with interpolated rendering on a fixed 60 Hz simulation.

## Known issues

- **Pop-in at the edges of the screen and in the distance.** The game decides what to draw and
  animate using the original 4:3 view. On wider screens the picture shows more than that, so
  polygons and actors appear suddenly at the sides, and actors outside the old view hold still.
  On every platform except the N64 (which needs the culling to stay fast), nothing should pop in:
  the plan is to cull against the real view and a much longer draw distance there. Details and
  partial workarounds in [docs/enhancements.md](docs/enhancements.md).
- The multiplayer HUD and a few small items are still open:
  [docs/PORT-GAPS.md](docs/PORT-GAPS.md).
- **N64 is experimental.** On a ModRetro M64 the screen can go black when going from a level back
  to the menu, and performance is still being tuned.

## About the project

- The port is based on the original source code preserved by
  [Highway Frogs](https://highwayfrogs.net/) (`HighwayFrogs/frogger2-vss`). That source carries
  no license: see [research/community-and-legal.md](research/community-and-legal.md).
- Correctness is checked by playing the same inputs through the retail exe and this port and
  comparing the game state frame by frame ([docs/ORACLE.md](docs/ORACLE.md)). The simulation uses
  bit-exact fixed-point maths and a fixed 60 Hz tick.
- After you import, the app icon switches to one cut from **your copy's** artwork, on your
  machine. The project has no logo of its own yet.
- Frogger is a trademark of Konami. This is an unaffiliated, non-commercial fan preservation
  project and has no connection to Konami, Hasbro or the original developers.

## Documentation

- [docs/USER-GUIDE.md](docs/USER-GUIDE.md): playing: import, controls, config, saves, troubleshooting
- [docs/BUILDING.md](docs/BUILDING.md): building, packaging, releases
- [docs/EXPORT.md](docs/EXPORT.md): building for Android, PS4, PS5, N64 and the desktop
- [docs/TOOLS.md](docs/TOOLS.md): importer, viewer, script compiler, modkit, trace and parity tools
- [docs/PLAN.md](docs/PLAN.md), [docs/](docs/), [research/](research/): goals, design notes and findings

## License

GPL-3.0-or-later, except `f2-bink` (the Bink video decoder), which is LGPL-2.1-or-later. See
[NOTICE](NOTICE) and the `LICENSE-*` files. The license covers this project's own code only: it
cannot cover the preserved original source, which carries none, and no game data is included.

## Credits

Original game by Blitz Games (Interactive Studios). Source preservation by Highway Frogs
(Kneesnap and contributors). Format research builds on their archive and the Frogger 2 Enhanced
project.
