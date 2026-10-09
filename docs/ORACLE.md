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
trace.enable=0          # f2-trace.jsonl, one line per GameLoop tick
trace.file=f2-trace.jsonl
trace.from=level        # level | all
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

The trace schema, binary encoding and diff live in `tools/f2-trace` (`TraceFrame`, schema v1);
f2fix has a dependency-free encoder for the same line format, kept byte-compatible by the
`oracle_encoder_matches_schema` test there.

## Build notes

`crates/f2fix` is a **standalone cargo workspace** (its own `[workspace]` table, with
`panic = "abort"` in its profiles). The root `cargo test --workspace` therefore does not build
it, and it can be built for i686 on the Windows side without the root crates. On Linux
`cargo test --manifest-path crates/f2fix/Cargo.toml` runs the portable tests (`cfg`,
`fbconv`). `F2_WIN_CLIPPY=1 tools/run/build_f2fix.sh` lints the `cfg(windows)` code with the
Windows toolchain. This is clippy-clean with `-D warnings`.
