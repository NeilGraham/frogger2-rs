# Reusing work from the Theme Park World recomp (`../theme-park-world-recomp/`)

Surveyed 2026-10-09. TPW is a **hook-based incremental decomp** (no source available): Rust
functions are injected into the original `TP.exe` one at a time and difftested. Frogger 2 differs
fundamentally — **we have the source** — so we don't need function hooking to *discover*
behaviour. But TPW has built a lot of infrastructure we should reuse rather than rebuild.

## 1. Reuse directly (port/copy the crate, rename `tpw-` → `f2-`)

| TPW asset | What it does | Frogger 2 use |
|---|---|---|
| `crates/tpw-import/src/{cue,image,iso9660,identify,hashdb,store}.rs` | Redump zip / BIN-CUE / ISO / MDF readers, ISO9660+Joliet VFS, rendition identification by hash, data-store assembly | Core of `f2-media`/`f2-import`. Add: InstallShield-5 CAB, Red Book ripping, single-bin multi-track cues. (TPW's `cab.rs` is **MS-CAB/MSZIP**, not InstallShield — not reusable for `DATA1.CAB`.) |
| `research/redump-*/manifests/*.json` pattern + `tools/py/verify_media.py` | Per-file hash manifests for every disc | Same for USA/EU Frogger 2 |
| `crates/tpwfix/src/platform/win.rs`, `hlcfg.rs`, `docs/NOTES-headless.md` | **Headless runs** of the original exe: window off-screen/never activated, `WS_EX_NOACTIVATE`, no display-mode changes, cursor untouched, DirectInput forced `BACKGROUND|NONEXCLUSIVE`, virtual input devices, **`audio.mute=1` mutes the process audio session**, framebuffer dumps (PNG) from the game's own DDraw back buffer | **Oracle runs of retail `Frogger2.exe` in the background, muted**, in parallel, while the owner keeps using the desktop |
| `tools/run/run.py`, `mkroot.sh`, `launch.sh`, scenarios | Builds an isolated run root from the data store, launches N instances, watches foreground, collects `result.json` + screenshots | Same flow; Frogger 2 scenarios = `.key` demo files + scripted pad input |
| `crates/tpw-hook` (`patch.rs`, `x86.rs`, `reloc.rs`, `live.rs`) | Inline detours, prologue relocation, ini toggles | Needed only for the oracle shim (`f2fix`) to hook a few platform APIs/game functions (state-dump points). Retail `Frogger2.exe`: image base `0x400000`, **no relocs** — same situation as TPW. |
| `crates/tpw-harness` (`gen`, `float`, `mem`, `report`, `runner`) | Seeded input generators, float policies, memory snapshot/diff, reports | Function-level difftests. For Frogger 2 the *simplest* oracle is compiling the original C function natively via the `cc` crate (we have source) — use harness for generation/reporting. |
| `crates/tpw-render/src/{compare,png}.rs`, `crates/tpw-replay` | Frame capture → PNG, image diff vs original | Screenshot parity tests for the wgpu renderer |
| `docs/TIMING.md` approach | Fixed sim step + interpolated rendering at any refresh rate | Frogger 2: fixed **60 Hz** (from `InitTiming(60.0)`), see `docs/architecture.md` |
| `tools/tasks.py`, `work/tasks.json`, `work/LOG.md`, `docs/ORCHESTRATION.md`, `docs/AGENT-BRIEF.md`, `CLAUDE.md` | Durable task graph + lean orchestration (one implementer per task, executable acceptance, model tiering) | Adopt as-is for this project's execution (Phase 0 task) |
| `tools/build.sh` | WSL → Windows `cargo.exe` for `i686-pc-windows-msvc` | Building the 32-bit `f2fix.dll` oracle shim |

## 2. Frogger 2–specific oracle design (built from the TPW pieces)

Two oracles, used for different things:

1. **Retail oracle — `f2fix.dll` injected into the user's own `Frogger2.exe`** (from `DATA1.CAB`,
   never redistributed). Injection via a `dinput.dll` proxy (exe imports DINPUT) or `ddraw.dll`
   proxy. The shim provides:
   - headless/off-screen window, never steals focus, **muted** (DirectSound → null device or
     WASAPI session mute; MCI `cdaudio` → stub that returns success, optionally plays ripped tracks);
   - registry virtualisation (`HKLM\Software\Hasbro Interactive\Frogger2` → `InstallDir`, `Language`,
     `Resolution`, `Fullscreen=0`), CD virtualisation (`GetVolumeInformation` → `FROGGER2`,
     `Video\` redirected into the data store) — no exe patching, no crack;
   - **fixed-timestep mode**: hook `timeGetTime`/`UpdateTiming` so every frame advances exactly
     1/60 s → deterministic, faster-than-real-time runs;
   - **state trace**: at the end of `GameLoop` dump frog/actor/tile/score/timer/RNG state per tick
     (addresses found by matching the retail exe to source in Ghidra);
   - input from `.key` recordings or scenario files (virtual pad), framebuffer PNG dumps.
2. **Source oracle — the original C compiled natively** for pure functions (`fixed.c`, `maths.c`,
   tile lookups, script VM with stubs) via the `cc` crate in test-only builds; and optionally the
   `frogger2-pc` Debug build (VC6, Windows side) with `printf`-style trace hooks where the retail
   exe is hard to instrument.

The retail oracle is authoritative for parity (it is what players played); the source oracle is
for fast, fine-grained unit difftests.

## 3. Don't reuse

- The TPW function-by-function *replacement* workflow (`functions.csv`, `mark_ported.py`,
  `hooks/generated.rs`): with source available, we port whole subsystems into a standalone engine
  and verify via traces instead.
- SafeDisc handling: Frogger 2 retail has no disc DRM beyond a volume-label check.

## 4. Longer term: shared crates

Both projects now need cue/ISO/rendition identification, headless Windows runs, frame diffing
and task orchestration. Once both stabilise, extract them into a shared `recomp-kit` workspace
(git submodule or crates path dependency) instead of copy-paste divergence. Owner decision.
