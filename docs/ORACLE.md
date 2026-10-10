# Retail oracle: headless, muted runs of the user's own `frogger2.exe`

Status (2026-10-09, task P0-3): **working.** From WSL, one command builds a run root on the
Windows side, starts the retail USA `frogger2.exe` off-screen and muted, and collects PNG
framebuffer dumps. A run plays the logo FMVs (Hasbro, Blitz, intro `01S`), reaches the
**frontend main menu** after about 58 s, and then the attract-mode **demo** (3D in-game). The owner's
desktop is not disturbed: the game never takes the foreground, its window is never on a
monitor, the display mode doesn't change, and no audio is heard.

The design and some code come from the sibling project `../theme-park-world-recomp`
(`crates/tpwfix`, `tools/run`, `docs/NOTES-headless.md`; MIT, same owner). The copied files
carry an attribution header.

## Quick start

```bash
tools/run/build_f2fix.sh                       # -> target/oracle-bin/f2fix.dll (i686, Windows cargo.exe)
python3 -I tools/run/run.py --id smoke --watch # 60 s run, a dump every second
#   -> target/oracle-runs/smoke/{f2-shot-*.png, f2fix.log, fgwatch.jsonl, result.json}
```

Useful options: `--exit-seconds S` (default 60; f2fix dumps a `final` frame and ends the
process), `--exit-frames N`, `--dump-interval-ms MS` (default 1000), `--dump-every N`
(presents), `--window offscreen|hidden|off`, `--timeout S` (hard kill), `--keep-root`,
`--game-dir` (default `$F2_GAME_DIR` or `game-data/usa`), `--compat ''` (no compat layer,
see Known issues), `--audible` (debug only). The exit code is 0 when at least one non-black dump
was written.

The prerequisites are the files that `f2-import --oracle` writes:
`game-data/<region>/{oracle/frogger2.exe, oracle/binkw32.dll, game/, video/}`.

## What a run does (`tools/run/run.py`)

1. **Stage** (once per data set): `game/*` and `video/` (as `Video\`) are copied to
   `C:\f2-runs\_stage\<region>-<fingerprint>\` (~190 MB) and marked read-only.
2. **Root** `C:\f2-runs\<id>\` gets hardlinks to every staged file (instant, no extra disk
   space). Because the staged files are read-only, a game write can never corrupt the stage.
   It also gets copies of `frogger2.exe` and `binkw32.dll`, `f2fix.dll` as **`dinput.dll`**,
   and a generated `f2fix.ini`.
3. **Launch** `frogger2.exe` from WSL (cwd = root), with `__COMPAT_LAYER=16BitColor` in its
   environment only. With `--watch`, `tools/run/fgwatch.ps1` samples the foreground window,
   the display mode and the game's windows every second, using read-only Win32 queries.
4. **Collect** `f2fix.log`, `f2-shot-*.png` and `fgwatch.jsonl` into
   `target/oracle-runs/<id>/`, and write a `result.json` (dumps with mean and non-black %, the
   mute status, suppressed dialogs, a watch summary). Then the root is deleted.

Nothing is written to the registry, the system mixer, a drive letter or AppCompat settings.
Nothing in `game-data/` runs except the retail exe inside its run root.

## `f2fix.dll` (`crates/f2fix`)

The DLL is a 32-bit cdylib. It is loaded because the exe imports `DINPUT.dll`: Windows
searches the application directory first. It forwards `DirectInputCreateEx/A/W`,
`DllGetClassObject` and `DllCanUnloadNow` to `%SystemRoot%\System32\dinput.dll`, which
WOW64 resolves to SysWOW64. The exe is never patched on disk.

`DllMain` only reads the ini, opens the log and patches import slots. The real dinput.dll,
ole32 and the audio thread are loaded on an init thread, outside the loader lock.

### Shims

All of these are typed hooks in the exe's import table (`win/shims.rs`) or vtable hooks on the
game's own COM objects.

| Area | Source reference | Hooked | Behaviour |
|---|---|---|---|
| Registry | Main.cpp `GetRegistryInformation`/`SetRegistryInformation`, controll.cpp | `RegOpenKeyExA`, `RegCreateKeyExA`, `RegQueryValueExA`, `RegSetValueExA`, `RegCloseKey` | `HKLM\Software\Hasbro Interactive\Frogger2` is a virtual key. `InstallDir` = run root, `Resolution` = 640x480 (`(w<<16)\|h`), `Fullscreen` = 0, `VideoDevice` = the primary display's DirectDraw description (`auto`), `Language` = `US`, `Control1..4` = `Keyboard`. `Keymap`, `netplayer` and `netgame` are "not found", which selects the defaults. Writes are dropped. |
| CD check | Main.cpp `FindFrogger2CD`, video.cpp `StartVideoPlayback` | `GetLogicalDriveStringsA`, `GetDriveTypeA`, `GetVolumeInformationA`, `CreateFileA` and `FindFirstFileA` (exe), and `CreateFileA` (binkw32) | One virtual CD-ROM drive `Q:\` with the label `FROGGER2`. `Q:\…` paths are redirected to `cd.root` (the run root, which holds `Video\`). |
| CD audio | pcaudio.c (`mciSendCommand` "cdaudio") | `mciSendCommandA` | Every command succeeds silently. `MCI_OPEN` returns a fake device id. |
| Mixer | (retail only) | `mixerSetControlDetails` | Dropped. The game would otherwise change the owner's system mixer volumes. |
| Audio | pcaudio.c, Bink | `DirectSoundCreate` (ordinal 1) + WASAPI | The process's audio session is muted, as in TPW `audio.mute`: `ISimpleAudioVolume::SetMute` on the default render device's session, re-asserted every 0.25–2 s. `DirectSoundCreate` waits until the mute is confirmed, and if it isn't confirmed within 5 s it fails (`DSERR_NODRIVER`), so sound can never be audible. Bink's DirectSound and waveOut output joins the same session. |
| Window | mdxWindows.cpp `WindowsInitialise`, mdxDDraw.cpp | `RegisterClassA`, `CreateWindowExA`, `ShowWindow`, `SetWindowPos` | `WS_EX_NOACTIVATE\|WS_EX_TOOLWINDOW` (no taskbar or Alt-Tab entry), placed at `SM_XVIRTUALSCREEN - 12000`. `ShowWindow` → `SW_SHOWNOACTIVATE` (`hidden`: never shown). `SetWindowPos` gets `NOACTIVATE\|NOZORDER` and stays off-screen. The window procedure is wrapped so that `WM_ACTIVATEAPP(FALSE)` and `WM_ACTIVATE(WA_INACTIVE)` are dropped: `WM_ACTIVATEAPP(FALSE)` would clear `winActive`, and drawloop.cpp `DrawLoop` stops drawing while it is 0. |
| Modal UI | Main.cpp (insert-CD / bad-video boxes), mdxDDraw.cpp `DDrawInitObject` (video-device dialog) | `MessageBoxA`, `DialogBoxParamA` | Never shown. They are logged and answered with cancel/no. The device dialog only opens when `VideoDevice` matches no enumerated device. `auto` prevents that, so it has not appeared in any run. |
| DirectInput | controll.cpp `InitInputDevices` | proxy `DirectInputCreateEx` → `IDirectInput7`/`IDirectInputDevice7` vtables | `EnumDevices(joystick)` returns none, so the owner's pads are never opened. The cooperative level is forced to `BACKGROUND\|NONEXCLUSIVE`. `GetDeviceState`/`GetDeviceData` report nothing pressed, so the owner's typing never leaks in. |
| DirectDraw | mdxDDraw.cpp `DDrawCreateSurfaces`/`DDrawFlip`, video.cpp | `DirectDrawCreateEx` → `IDirectDraw7` vtable (`CreateSurface`, `SetCooperativeLevel`, `SetDisplayMode`) → **the game's own primary surface** vtable (`Blt`, `Flip`) | Fullscreen/exclusive requests are forced to `DDSCL_NORMAL`, and `SetDisplayMode` is refused (neither is reached with `Fullscreen=0`). Each `primary->Blt(…, src, …)` counts as a present (game frames come from `RENDER_SRF`, windowed FMV frames from Bink's temporary surface). When a dump is due, the source is locked read-only, converted (565/555/24/32 → RGB, `fbconv.rs`) and written as `f2-shot-<frame>.png` (or `final`). |

Following the TPW notes, the hooks go on the vtables of the objects the game itself got back,
not on objects obtained through `QueryInterface`. A compat shim may wrap ddraw, and the wrapper's
vtable is the one the game calls.

### `f2fix.ini`

The ini sits next to the exe. Its syntax follows TPW hlcfg; the parser is `src/cfg.rs`, which
is host-tested.

```text
window=offscreen        # off | offscreen | hidden
audio.mute=1
input.virtual=1
reg.install_dir=        # empty = exe dir
reg.language=US         # English | Français | Deutsch | Italiano | US
reg.resolution=640x480
reg.fullscreen=0
reg.video_device=auto   # or an exact DirectDraw description, e.g. "Software Rendering"
reg.control=Keyboard
cd.drive=Q
cd.root=                # empty = install dir; must contain Video\
dump.first=1
dump.every=0            # presents
dump.interval_ms=0      # wall-clock spacing
dump.max=100
exit.frames=0
exit.seconds=0          # a final dump is attempted, then TerminateProcess
log.presents=0
time.fixed=0            # trace mode: one 60 Hz frame per main-loop iteration, uncapped
fmv.skip=0              # BinkOpen fails -> every FMV skipped
start.demo=             # W-L: frontend -> attract demo of world W level L
start.level=            # W-L: frontend -> arcade level (scenario mode)
start.keys=             # scenario .key file (relative = run dir)
start.after=2           # frontend frameCount before start.* fires
start.multi=0           # 2..4: start.level as a local multiplayer match (GoMulti) with N frogs
start.chars=0,1,2,3     # start.multi: options.playerChar[] (frogger.h FROG_*)
start.mp_mode=0         # start.multi: override multiGameTypes[W] (1 race, 2 battle, 3 collect)
trace.enable=0          # f2-trace.jsonl, one line per GameLoop tick
trace.file=f2-trace.jsonl
trace.from=level        # level | all
trace.version=1         # 2 = + optional sections (script, triggers, camera, ...)
snap.ticks=             # breakpoint snapshots at these act_frame_count values
snap.regions=           # NAME@0xADDR:LEN or NAME@*0xADDR:LEN (pointer), space/comma separated
exit.ticks=0            # end after N trace lines
exit.level_end=1        # end when the started demo/level returns to the frontend
```

Without an ini the defaults apply. They are already headless and muted, with a single dump of
the first frame.

## Verified runs (2026-10-09, Ryzen 9 9950X, RTX 5090, Windows 11, 7680x2160x32)

| Run | Settings | Result |
|---|---|---|
| `smoke1` | no compat layer | FMVs correct; title backdrop garbled (see Known issues) |
| `compat16` | `16BitColor` | render surface 640x480 **565**; title screen correct |
| `front1`, `accept1` | 95 s / 75 s, offscreen, `--watch` | logos → title → intro FMV → frontend menu (≈58 s) → attract demo. 74/74 watch samples with the game running: foreground never the game (it stayed with the owner's apps), 0 game windows on a monitor, one display mode throughout. Session muted at t = 0.02 s, before `DirectSoundCreate`. |
| `hidden1` | `--window hidden` | Same output; 0 game windows visible at all |

All runs: no dialog or message box, no registry key created (HKLM and VirtualStore both
checked), no `frogger2.exe` left running.

## Known issues / notes

- **32-bpp desktop.** Windowed, the original creates `RENDER_SRF` (and the backdrop surfaces)
  in the primary's pixel format but writes 16-bit pixels into some of them. On a 32-bpp desktop
  the 2D backdrops come out half-width with wrong colours (FMV and 3D are fine). In 2000 the
  game ran on 16-bit desktops. `run.py` therefore sets the per-process AppCompat layer
  `__COMPAT_LAYER=16BitColor` (Windows emulates a 16-bit display for this process only; the real
  display mode stays the same, as the watch confirmed). `--compat ''` turns it off.
- **Frame rate.** Without `time.fixed` the game runs uncapped on its variable-step timing
  (`UpdateTiming`), so frame counts are not comparable across runs; use `dump.interval_ms`.
  With `time.fixed=1` (trace mode) every main-loop iteration is exactly one 60 Hz frame, still
  uncapped (~600-700 ticks/s in a level).
- **Input.** Live input is always virtual (nothing pressed). Demos replay their own `.key`
  files; `start.level` + `start.keys` replays a recording into `padData` (see Trace mode).
  Menu navigation by key queue is not implemented (not needed: start control writes memory).
- **FMVs play in real time** (25 fps, on the game's video thread) unless `fmv.skip=1`.
- **Hardware D3D only.** With `VideoDevice=auto` the HAL device of the primary GPU is used.
  `reg.video_device=Software Rendering` selects the MDX software rasteriser; it is untested.
- **Primary blit is clipped away.** The window is off-screen, so the present blit draws
  nothing and the dump reads the source surface. `--window off` shows the window normally, for
  debugging only, because it takes focus.
- The dumps are RGB PNGs with stored deflate (about 920 KB per frame, uncompressed).
- `C:\f2-runs\_stage\` stays between runs. Delete it to re-stage after re-importing data.

## Trace mode (deterministic oracle, task P3-T)

```bash
python3 -I tools/run/run.py --id g1 --trace --demo 0-0      # Garden 1 attract demo, ~1556 ticks, ~5 s
python3 -I tools/run/run.py --id a1 --trace --demo 1-0      # Ancient 1 demo (6-1 = Halloween 2)
python3 -I tools/run/run.py --id s1 --trace --level 0-0 --keys path/to/record-0-0.key --exit-ticks 2000
target/agent-f2-trace/release/f2-trace diff target/oracle-runs/g1/f2-trace.jsonl other.jsonl
```

`--trace` implies `--fixed-time --skip-fmv`; the trace is collected to
`target/oracle-runs/<id>/f2-trace.jsonl`, and `result.json` gets `trace.{lines,level_ticks,sha1}`.
Addresses come from `symbols/frogger2_usa.csv` (USA retail exe; matched in Ghidra against the
source) and live in `crates/f2fix/src/addrs.rs`. `crates/f2fix/src/win/oracle.rs` patches three
code sites in memory, each verified byte-for-byte first:

| Site | Patch | Effect |
|---|---|---|
| `RunWindowsLoop` `call UpdateTiming` (0x455b6e) | call `h_tick_timing` | `timeInfo.frameCount += 1`, `tickCount = frame*1000/60`, `speed = 1.0`: `LoopFunc` computes `actFrameCount += 1`, `gameSpeed = 4096` |
| `UpdateTiming` entry (0x44b430) | `jmp h_hold_timing` (no-op) | backdrop loads (`InitBackdrop`) no longer read the wall clock |
| `LoopFunc` `call GameLoop` (0x43cdd0) | call `h_game_loop` | start control, scenario pads, then `GameLoop`, then one trace line |

- **Start control.** `start.demo=W-L`: once the frontend runs (`frameCount >= start.after`,
  `fadingOut == 0`) f2fix writes `demoLevels[0] = {W,L}`, `nextDemo = 0`, `goingToDemo = 1`;
  the frontend itself then calls `InitDemoMode` → `InitLevel` → `LoadDemoFile`
  (`DEMOS\RECORD-W-L.KEY`). `start.level=W-L` does what `InitDemoMode` does but as an arcade
  game: `gameState = {multi 1, single ARCADE, mode INGAME, difficulty NORMAL}`, `InitLevel(W,L)`
  (that iteration's `GameLoop` is replaced by the call and `frameCount++`).
- **Local multiplayer** (`run.py --level W-L --multi N [--chars a,b,c,d] [--mp-mode M]`,
  `start.multi`): f2fix fills what the frontend's multiplayer menus leave behind,
  `options.{numPlayers, currentPlayer} = N`, `playerChar[] = chars`, `levelSelection` = the
  level's index in `multiWorldNum/multiLevelNum` (1-3, 3-0 race; 5-3, 5-4, 5-5 battle; 7-10,
  7-11, 7-12 collect), optionally `multiGameTypes[W] = M`, and calls the retail **`GoMulti`**
  (0x422e40) instead of that iteration's `GameLoop`: `NUM_FROGS = N`, `gameState = {MULTILOCAL,
  INVALID_MODE, INGAME}` (difficulty stays the frontend's NORMAL), `multiplayerMode`,
  characters, `fog.max = fogStore` (10000), `InitLevel`. (A level outside the table, or a
  frontend without its option overlays, gets the same writes done by f2fix.) The scenario
  `.key` drives all four pads (record field `player`). The log line `start multi ...` carries
  `fogStore` and the rseed for the Rust side.
- **Trace window.** With `trace.from=level` the first line is the first `GameLoop` tick of the
  started level (`act_frame_count == 1`); each line is the state after `GameLoop` returned. The
  run ends when the game is back in the frontend (demo timeout ≈ 25 s = 1556 ticks) or after
  `exit.ticks`.
- **Scenario input.** A `.key` file is `(actFrameCount, digital, pad)` u32 triplets
  (`RecordButtons`). It is replayed as *held* state: before each `GameLoop`, `padData.digital[p]`
  = buttons of the last record with `frame < actFrameCount`, `debounce = ~prev & digital`
  (what `ProcessUserInput` derives from live keys). Demo mode keeps the game's own replay.
- **Determinism fix (BUG(original)).** `RunDemoMode` builds the first replayed key's debounce
  from `playKeyList[-2]` (the heap header in front of the `fileLoad` buffer), so whether the
  demo's first press registers varied between runs. f2fix replays from a copy whose two leading
  words are 0 (first press always registers) and restores the game's pointer before it is freed.
  The Rust port must use the same rule (`prev = 0` for the first record) to match.
- **Verified (2026-10-09).** `--demo 0-0` twice → byte-identical 1556-line traces (sha1
  `44059c7e…`); `--demo 1-0` 3 runs (two in parallel) identical (`605d5851…`); `--demo 6-1`,
  `--level 0-0 --keys record-0-0.key` (2000 ticks, twice, identical). Every line has
  `game_speed 4096` and consecutive `act_frame_count`.

### Schema v2 sections (`--trace-version 2`, `trace.version=2`)

`"v":2` lines are v1 lines plus these keys after `cam_target`, in this order (types are the
`tools/f2-trace` structs; offsets in `symbols/frogger2_structs.md`). Every section is
optional: an emitter (e.g. `f2-game`) may write any subset, the diff only compares the
sections both sides have and notes the rest, and v1 traces still read and diff.

| key | content |
|---|---|
| `players_ext` | per player `i < NUM_FROGS`: `frog_facing` (`frogFacing[i]`), `cam_facing` (`camFacing[i]`), `frogon`, `frogunder`, `old_lives`, `input_pause`, `spawn_timer`, `spawn_score_level`, `idle_time`, `idle_enable`, `has_double_jumped`, `extended_hop_dir`, `stun_time`, `croak_time`, `autohop_time`, `quickhop_time`, `slowhop_time` (TIMER `.time`), `jump_start_frame`, `jump_speed`, `jump_multiplier`, `jump_fwd`, `jump_up` (FVECTOR), `jump_origin` (SVECTOR) |
| `garib_ext` | `blocks`, `max_coins`, `pause_flag`, `pause_timer`, `drop_speed[]` (per garib, list order as `garibs`) |
| `babies` | per `babyList[i]`, `i < numBabies`: `is_saved`, `idle`, `collect`, `pos` (baby actor, or null) |
| `camera` | `source`/`target` (`camSource`/`camTarget`), `cam_vect`, `curr_cam_dist`, `cam_dist` (SVECTOR), `curr_cam_offset`, `cam_look_ofs`, `trans_cam_speed_mult`, `y_fov_new`, `fixed` [`fixedDir`,`fixedPos`,`fixedUp`], `cam_shakiness`, `trans` (highest `tcList` index whose tile is `currTile[0]`, -1), `trans_count`, `fog` [r,g,b,mode,min,max] |
| `script` | `flags` (`scriptFlags[8]`), `counters` (`scriptCounters[16]`) |
| `triggers` | `count` (`triggerList.count`, includes AND/OR/NOT sub-triggers), `list[]` {`flags`,`count`,`delay`} of the active ring from `head.next` |
| `specfx` | `count` (`sfxList.count`), `list[]` {`kind` (type), `origin` (SVECTOR), `lifetime` (absolute actFrameCount), `update_type`} from `head.next` |
| `sprites` | `count` (`sprList.count`), `particles` (`partList.count`) |
| `tongue` | per player: `flags`, `radius`, `progress`, `kind`, `can_tongue`, `source`/`target`/`pos` (SVECTOR), `has_thing` |
| `hud` | `score_timer`, `mode_timer`, `go_timer`, `demo_timeout` (TIMER `.time`), `hand_angle`, `store_frame_count` |
| `mp` | only while `gameState.multi == MULTILOCAL` (diff section `multi`): `mode` (`multiplayerMode` 1 race, 2 battle, 3 collect), `started`, `num_multi_items`, `game_winner`, `match_winner` (-1, a player, 127 draw), `multi_timer`, `end_timer` (TIMER `.time`), `initial_cam_height`, `mpl[]` per player `i < NUM_FROGS`: `wins`, `lap`, `timer`, `penalty`, `check`, `score`, `trail`, `ready`, `lasttile` |

`f2-trace diff` reports the first divergence per section (`core players enemies platforms
garibs babies camera hud script triggers specfx sprites tongue multi`; v1 fields map to the
same names, e.g. `garib_count` -> `garibs`, `cam_source` -> `camera`) and `--sections
camera,triggers` restricts the comparison. The binary encoding (header version 2) appends the v2
sections of each frame as a JSON blob and still decodes version-1 files.

**Verified (2026-10-09).** v2 `--demo 0-0` twice (in parallel) -> identical (sha1
`b0818758...`); `--demo 1-0` twice -> identical (`148391ab...`). With the v2 sections
stripped the lines are byte-identical to the v1 traces (`44059c7e...`, `605d5851...`).

### Breakpoint snapshots

```bash
python3 -I tools/run/run.py --id s --trace --demo 0-0 --snap-tick 600 --snap 'player,rseed,*frog:64,tcl=*tcList:72'
target/agent-f2-trace/release/f2-trace snap A/f2-snap-600.json B/f2-snap-600.json
```

`--snap-tick N` (repeatable, comma lists) dumps raw bytes after the `GameLoop` of
`act_frame_count == N` (in the started level) to `f2-snap-N.json`; `--snap` names CSV
globals (`NAME`, `NAME:LEN`, `*NAME:LEN` = LEN bytes where the pointer in NAME points,
`0xADDR:LEN`, `LABEL=SPEC`), default every CSV global/data entry with a size. run.py
resolves names from `symbols/frogger2_usa.csv` into `snap.regions`. Unreadable memory
gives `"hex":null` (checked with `IsBadReadPtr`). Format: `tools/f2-trace/src/snap.rs`;
`f2-trace snap A B` lists differing byte ranges per region. Snapshots do not change the trace.

`run.py --data DIR` (= `--game-dir`) selects the rendition; by default a worktree checkout
uses the main checkout's `game-data/usa`.

The trace schema, binary encoding and diff live in `tools/f2-trace` (`TraceFrame`, schemas v1/v2);
f2fix has a dependency-free encoder for the same line format, kept byte-compatible by the
`oracle_encoder_matches_schema` test there.

## Parity corpus (`tools/f2-parity`)

Beyond the three attract demos, `f2-parity` checks every single-player level of the PC level
table (26 levels) against the retail exe with a seeded scripted input (`.key`, generator
`f2-parity-gen-1`, seed 1, 1800 ticks):

```bash
cargo run -p f2-parity -- run     # oracle runs (cached, 2 at a time) + Rust runs + report
cargo run -p f2-parity -- check   # no Windows: Rust vs cached oracle, exit 1 on regression
cargo run -p f2-parity -- check --update   # record improvements in tools/f2-parity/baseline.json
```

- Oracle traces are cached by script hash in `target/parity/oracle/` (`<W-L>-<key>.jsonl` +
  `.meta.json` with the rseed before `InitLevel`). A worktree without its own cache uses the
  main checkout's; `--cache DIR` overrides. `--refresh` re-runs the oracle, `--no-oracle`
  skips it.
- The Rust side is the `f2-game` example `scenario_trace W L keys.key out.jsonl [ticks]`
  (`F2_START_RSEED` from the meta; `F2_LOG=1` prints level-load log lines such as failed
  enemy/platform creation).
- `target/parity/report.md` lists per level the matched leading ticks, the first divergent
  field and a likely module. Status 2026-10-09: **26/26 levels identical** for the full
  oracle length (the demos stay identical too).
- **Local multiplayer** (`--corpus single|multi|all`, default all): every level of the
  frontend's multiplayer table (`scenario_trace --list-multi`) × 2 and 4 frogs in its own
  mode (`W-L-mpN`), plus race and collect forced on the other levels (`W-L-mpN-race`,
  `-collect`, `run.py --mp-mode`): 38 cases, generator `f2-parity-mp-gen-1` (one seeded
  stream per player, merged by frame). The oracle runs `run.py --level W-L --multi N`; the
  Rust side `scenario_trace --multi N [--mp-mode M]` (`F2_FOG_STORE` from the meta). Battle
  forced on a non-battle level is not in the corpus: the retail exe dies during
  `GoMulti`/`InitLevel` (no trace), and the frontend never starts that combination.
  Status 2026-10-09: **38/38 identical** for 1800 ticks, `mp` section included (battle
  rounds won and drawn, mode 16 win screens; races reach the countdown and running state
  but no lap is completed with random input, so lap/match-end logic is still unexercised).
- **Finish corpus** (`--corpus finish`, cases `fin-W-L` / `fin-W-L-mpN`, generator
  `f2-parity-finish-1`): planned scripts that play to the end state random input never
  reaches — single player: every baby saved, `LEVELCOMPLETE_MODE`, CROSS through the
  level-complete screen, `SaveGame`, the next start; multiplayer: rounds until a match
  winner (race: every frog over the finish line, battle: 3 wins, collect: unassailable
  leads), the mode 14–16 win screens, `DoEndMulti` "quit" and back to the frontend. The
  scripts are committed in `tools/f2-parity/finish/` (`manifest.json` + `.key`, with the
  planner's summary) and planned by the Rust sim itself:
  `scenario_trace [--multi N] --plan OUT.key [--plan-seed S] W L - out.jsonl 1`, or
  `f2-parity plan --levels fin-..` (re-plan + manifest). The planner is a Go-Explore archive
  search over the deterministic sim (single player, race one frog at a time, collect) and
  receding-horizon random shooting (battle); `F2_PLAN_DEBUG=1` prints targets and, on
  give-up, the most advanced cell; `F2_PLAN_SIM_BUDGET`, `F2_PLAN_MAX_CELLS` (memory:
  ~0.6 MB per cell; use `MEMCAP=12G` above 6000). Levels finished under par would open the
  name entry (keyboard text the pad script cannot type), so the planner then starts later.
  The report adds the end state of each trace and the `savegame.dat` comparison (run.py
  collects `SaveData\savegame.dat`; Rust: `F2_SAVE_OUT`); `check` fails when a baselined
  end state or save stops matching. Needs an `f2fix.dll` with `start.multi`
  (`tools/run/build_f2fix.sh`; f2-parity now refuses a multiplayer trace without a
  multiplayer start — an old DLL silently ran them single-player).
  Status 2026-10-09: **34 cases baselined, every end state and save identical**:
  all 16 multiplayer cases (race 1-3/3-0, battle 5-3..5, collect 7-10..12, 2 and 4 frogs)
  identical for the whole run, `mp` section and win screens included; 18 single-player
  levels (0-0, 1-1, 2-1, 2-2, 4-1, 4-2, 5-1, 6-0, 7-0..7-9) complete with the oracle's
  level-complete afc, the next start and a byte-identical `savegame.dat` (retro levels
  7-x write none on either side). 15 of them are identical throughout; two have a
  cosmetic-RNG divergence that does not change the outcome:
  - `fin-2-2` @8836 (a baby rescue in LEV_SPA3): the oracle draws 1061 `Random()` calls
    that tick, Rust 37 — an effect created at draw time on the exe side (not found yet).
  - `fin-4-1` @8763 (level-complete screen, CROSS held): Rust draws one more `Random(1000)`
    (frontend.c:1990 celebrate check: Rust's frog is in looping `FROG_ANIM_BREATHE` one
    tick before the exe's; anim queue timing in `RunLevelComplete`).
  - `fin-1-1` @10693 (level-complete screen, 510 ticks after `complete`): Rust 31 draws,
    the exe 20 — a sparkle re-initialised (frontend.c:1910, `draw == 0` after `DEC_ALPHA`)
    plus the celebrate check; same `RunLevelComplete` cosmetic area as `fin-4-1`.
  Not finished by the planner yet (puzzle levels; the static tile graph does not see
  switches/teleports/lifts): 1-0, 1-2, 2-0, 4-0, 5-0, 5-2, 6-1, 6-2 (6-2 has no
  babies: it ends by script, `EndLevel` after a tube platform ride). Their manifest
  entries hold the partial 60000-tick plans (`goal no`) and are not baselined.
- To investigate a divergence take oracle snapshots at the tick
  (`run.py --level W-L --keys K --snap-tick N --snap NAME@ADDR:LEN`) and compare with
  `scenario_trace`; draw-time state (`DrawLoop`: effects created while drawing, croak,
  head matrix) and x87 24-bit float rounding were the causes so far (see `DECISIONS.md`).

## Build notes

`crates/f2fix` is a **standalone cargo workspace** (its own `[workspace]` table, with
`panic = "abort"` in its profiles). The root `cargo test --workspace` therefore does not build
it, and it can be built for i686 on the Windows side without the root crates. On Linux
`cargo test --manifest-path crates/f2fix/Cargo.toml` runs the portable tests (`cfg`,
`fbconv`). `F2_WIN_CLIPPY=1 tools/run/build_f2fix.sh` lints the `cfg(windows)` code with the
Windows toolchain. This is clippy-clean with `-D warnings`.
