# Source map: original C/C++ → Rust

The tracking table for the port. One row per translation unit in the shipping PC build
(`FroggerProject.dsp`, PSX/Editor excluded). **Update the Status column in the same PR that changes
the port.**

Status values: `todo` → `wip` → `ported` (compiles, unit tests) → `verified` (difftest or
trace-parity passes, reviewed) · `replaced` (platform code re-designed, not translated) ·
`skip` (not needed).

Conventions for function-level tracking: each ported Rust function gets a doc comment
`/// Port of `funcName` (Common/file.c:LINE).` so `grep "Port of"` produces the function-level map.
Generated 2026-10-09 from `FroggerProject/FroggerProject.dsp` (LOC via `wc -l`).

| C/C++ file | LOC | Rust target | Status |
|---|---:|---|---|
| `Common/DefData.c` | 41 | f2-game::defaults | todo |
| `Common/E3_Demo.c` | 255 | (skip — E3 build) | skip |
| `Common/actor2.c` | 1254 | f2-engine::actor | ported |
| `Common/anim.c` | 511 | f2-game::anim | ported |
| `Common/babyfrog.c` | 468 | f2-game::babyfrog | ported |
| `Common/bbtimer.c` | 44 | f2-game::timer | ported |
| `Common/cam.c` | 1012 | f2-game::camera | partial |
| `Common/collect.c` | 688 | f2-game::collect | ported |
| `Common/crc32.c` | 106 | f2-formats::crc | ported |
| `Common/enemies.c` | 2900 | f2-game::enemies | ported |
| `Common/evenfunc.c` | 525 | f2-game::script::funcs | todo |
| `Common/event.c` | 372 | f2-game::script::event | todo |
| `Common/fixed.c` | 69 | f2-math::fixed | ported |
| `Common/flatpack.c` | 340 | (PSX only — skip) | skip |
| `Common/frogger.c` | 295 | f2-game::frog | ported |
| `Common/frogmove.c` | 2190 | f2-game::frog::movement | ported |
| `Common/frontend.c` | 2048 | f2-game::modes | todo |
| `Common/game.c` | 759 | f2-game::ingame | partial |
| `Common/general.c` | 485 | f2-game::tiles | ported |
| `Common/hud.c` | 1184 | f2-game::ui::hud | todo |
| `Common/language.c` | 243 | f2-game::lang | todo |
| `Common/layout.c` | 1329 | f2-game::level::layout | partial |
| `Common/levplay.c` | 173 | f2-game::demo | ported |
| `Common/map.c` | 847 | f2-game::level::map (+ f2-formats::col/sce) | ported |
| `Common/maths.c` | 1848 | f2-math::maths | ported |
| `Common/memload.c` | 504 | f2-formats::entity | ported |
| `Common/menus.c` | 2177 | f2-game::ui::pause (+cheats) | todo |
| `Common/multi.c` | 1821 | f2-game::multi | todo |
| `Common/newpsx.c` | 55 | f2-math::psx | ported |
| `Common/options.c` | 3253 | f2-game::ui::frontend | todo |
| `Common/overlays.c` | 150 | f2-engine::overlay | todo |
| `Common/particle.c` | 365 | f2-game::fx::particle | todo |
| `Common/path.c` | 274 | f2-game::path | ported |
| `Common/platform.c` | 1243 | f2-game::platform | ported |
| `Common/ptexture.c` | 497 | f2-engine::proctex | todo |
| `Common/savegame.c` | 180 | f2-game::save | todo |
| `Common/script.c` | 1432 | f2-game::script::vm | todo |
| `Common/sndview.c` | 25 | (debug — skip) | skip |
| `Common/specfx.c` | 2457 | f2-game::fx | todo |
| `Common/sprite.c` | 502 | f2-engine::sprite | todo |
| `Common/story.c` | 92 | f2-game::story | todo |
| `Common/textover.c` | 453 | f2-engine::text | todo |
| `Common/tongue.c` | 822 | f2-game::frog::tongue | ported |
| `Common/training.c` | 148 | f2-game::training | todo |
| `Common/types.c` | 37 | f2-math::types | ported |
| `PC/Main.cpp` | 1025 | frogger (bin) + f2-platform | todo |
| `PC/actor.cpp` | 293 | f2-engine::actor | ported |
| `PC/backdrop.c` | 192 | f2-engine::backdrop | todo |
| `PC/banks.cpp` | 647 | f2-formats + f2-game::level::banks | ported |
| `PC/controll.cpp` | 1464 | f2-platform::input | todo |
| `PC/drawloop.cpp` | 408 | f2-engine::frame | todo |
| `PC/dx_sound.cpp` | 430 | f2-audio (replaced) | replace |
| `PC/fadeout.cpp` | 105 | f2-engine::fade | todo |
| `PC/islfile.cpp` | 35 | f2-mods::vfs | todo |
| `PC/isllocal.c` | 105 | f2-game::lang | todo |
| `PC/isltex.cpp` | 27 | f2-engine::texture | todo |
| `PC/islutil.cpp` | 81 | f2-platform::util | todo |
| `PC/net/dputil.cpp` | 81 | (Phase 6 — redesign) | deferred |
| `PC/net/netchat.cpp` | 435 | (Phase 6 — redesign) | deferred |
| `PC/net/netconnect.cpp` | 779 | (Phase 6 — redesign) | deferred |
| `PC/net/netgame.cpp` | 634 | (Phase 6 — redesign) | deferred |
| `PC/net/netrace.cpp` | 457 | (Phase 6 — redesign) | deferred |
| `PC/net/network.cpp` | 530 | (Phase 6 — redesign) | deferred |
| `PC/pcaudio.c` | 1518 | f2-audio | todo |
| `PC/pcgfx.c` | 862 | f2-engine::fx | todo |
| `PC/pcmisc.cpp` | 521 | f2-engine::misc | todo |
| `PC/pcsprite.c` | 471 | f2-engine::sprite | todo |
| `PC/video.cpp` | 251 | f2-bink + f2-engine::fmv | todo |

Game-side .c/.cpp total: **47824** lines (headers excluded).

| MDX Library file | LOC | Rust target | Status |
|---|---:|---|---|
| `fxBlur.cpp` | 175 | f2-engine (replace) | replace |
| `mdxActor.cpp` | 1025 | f2-engine (replace) | replace |
| `mdxCRC.cpp` | 111 | f2-engine (replace) | replace |
| `mdxD3D.cpp` | 731 | f2-engine (replace) | replace |
| `mdxDDraw.cpp` | 1203 | f2-engine (replace) | replace |
| `mdxDrivers.cpp` | 106 | f2-engine (replace) | replace |
| `mdxDText.cpp` | 327 | f2-engine (replace) | replace |
| `mdxException.cpp` | 142 | f2-engine (replace) | replace |
| `mdxFile.cpp` | 134 | f2-engine (replace) | replace |
| `mdxFont.cpp` | 610 | f2-engine (replace) | replace |
| `mdxInfo.cpp` | 399 | f2-engine (replace) | replace |
| `mdxLandscape.cpp` | 180 | f2-engine (replace) | replace |
| `mdxMath.cpp` | 568 | f2-engine (replace) | replace |
| `mdxObject.cpp` | 876 | f2-engine (replace) | replace |
| `mdxPoly.cpp` | 2235 | f2-engine (replace) | replace |
| `mdxPrcTex.cpp` | 101 | f2-engine (replace) | replace |
| `mdxProbe.cpp` | 29 | f2-engine (replace) | replace |
| `mdxProfile.cpp` | 415 | f2-engine (replace) | replace |
| `mdxRender.cpp` | 2266 | f2-engine (replace) | replace |
| `mdxSprite.cpp` | 256 | f2-engine (replace) | replace |
| `mdxTexture.cpp` | 1019 | f2-engine (replace) | replace |
| `mdxTiming.cpp` | 70 | f2-engine (replace) | replace |
| `mdxWindows.cpp` | 252 | f2-engine (replace) | replace |
| `mgeReport.cpp` | 107 | f2-engine (replace) | replace |
