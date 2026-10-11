# Native PS5 homebrew app

`tools/ps5/build.sh` builds a **native PS5 app** of the port (not the PS4 fPKG of
`tools/ps4/`): a directory title as used by etaHEN + ShadowMountPlus / Itemzflow under
`/data/homebrew/<TITLE_ID>/`, optionally also a mountable `.ffpkg` image.

```sh
tools/ps5/build.sh                                   # target/ps5/PPSA99920/ (data copied separately)
tools/ps5/build.sh --data game-data/usa              # your imported data bundled at /app0/data
F2_ICON_CROP_X=85 tools/ps5/build.sh --icon boxart.png
tools/ps5/build.sh --ffpkg                           # + target/ps5/PPSA99920.ffpkg (UFS2Tool)
tools/ps5/build.sh --smoke                           # Rust std smoke test (PPSA99921)
tools/ps5/build.sh --gpu                             # wgpu on the PS5 Vulkan driver (see below)
```

Output folder (`target/ps5/PPSA99920/`, title "Frogger 2: Swampy's Revenge", content ID
`IV0000-PPSA99920_00-FROGGER2RUSTPORT`):

| File | |
|---|---|
| `eboot.bin` | the game: PS5 executable (ELF type `0xFE10`, SCE dynamic imports by NID) in a fake-signed SELF |
| `sce_module/libc.prx` | the clean-room libc companion module every title carries (generated, hash-checked) |
| `sce_sys/param.json` | title metadata; `attribute3` `0x80040` declares 120 Hz support |
| `sce_sys/icon0.png` | 512x512 launcher icon (`tools/ps4/make-art.sh`) |
| `data/` | only with `--data`: copyrighted data from your own disc, never share the result |

Copy the folder to `/data/homebrew/` (FTP), or build `--ffpkg` and mount it with
ShadowMountPlus. Game data without `--data`: as for the PS4 build, the rendition folder
(`game/`, `music/`, `video/`, `manifest.json`) in `/data/frogger2`, on USB at
`/mnt/usbN/frogger2` or on extended storage at `/mnt/ext0/frogger2`; until found, a dark red
screen and a notification. Config, saves and `frogger.log` go to `/data/frogger2` (fallback
`/download0/frogger2`). Creating `/data/frogger2/60hz` keeps the output at 60 Hz.

**Installable PS5 fPKG.** The app folder is the content a PS5 fake package wraps (an fSELF
`eboot.bin`, `sce_sys/param.json`, `icon0.png`, `sce_module/libc.prx`), so a PS5 fPKG
builder can package `target/ps5/PPSA99920/` as it is. No such builder is available in this
environment (2026-10-10: none in the payload SDK or PS5_Vulkan, whose outputs are the
folder, `.ffpkg` (UFS2 image) and `.ffpfsc` (MkPFS); SvenGDK's PS-Mac-Tools "GP5 Creator"
writes a `.gp5` project but needs Sony's publishing tools), so `build.sh` does not produce
one yet; adding it is a matter of calling that tool on the folder.

## How it works

**Toolchain.** The public [ps5-payload-sdk](https://github.com/ps5-payload-dev/sdk)
(v0.43, `$PS5_PAYLOAD_SDK`, default `~/opt/ps5-payload-sdk`) provides the stub `.so`
libraries with the console modules' symbol names (`libkernel`, `libSceLibcInternal`,
`libSceVideoOut`, `libScePad`, `libSceAudioOut`, ...) and the `prospero-lld` wrapper. Its own
output format, ELF *payloads* for `elfldr` (its `crt1.o` carries a run-time linker that
needs the loader's kernel R/W pipe), cannot become a launchable app with an icon, so the
build uses the **native app** route of
[mihawk-99/PS5_Vulkan](https://github.com/mihawk-99/PS5_Vulkan) (`$PS5_VULKAN`, default
`~/opt/PS5_Vulkan`, GPL-3.0, tested at `252bf91`): its `tooling/native` C++ sources are
copied, patched (below) and compiled into `ps5-native-tool` (PIE -> PS5 executable with
NID imports, module/library tables, process parameters; fake-SELF signing) and
`libc-builder` (the `libc.prx` above; its SHA-256 is checked against PS5_Vulkan's recorded
`e6ff45d1...`). zlib for the tool is downloaded and built if `$F2_ZLIB` is unset.

**Rust std** works exactly as on the PS4: `crates/frogger-ps5` is a staticlib for the stock
`x86_64-unknown-freebsd` target (`-C target-cpu=znver2 -C panic=abort`), linked against the
system libraries' stubs. The shared runtime is `crates/console-host`:

- `sce/shim.rs`: the PS5 kernel is FreeBSD 11 based and keeps the pre-ino64 `struct stat`
  (120 bytes) and `struct dirent` (8-byte header) layouts: the payload SDK's own
  `sys/stat.h`/`sys/dirent.h` (`__ino_t` = `uint32_t`) are field-for-field the PS4's FreeBSD 9
  structs, so the PS4 shims (`stat`/`fstat`/`lstat` over `sceKernelStat`, `opendir`/`readdir`
  over `sceKernelGetdents`, `clock_nanosleep`, process stubs, ...) apply unchanged;
- `sce/alloc.rs`: dlmalloc; on the PS5 (feature `dmem-heap`) its segments come from direct
  memory (`sceKernelAllocateDirectMemory` + `MapDirectMemory`, tracked so they are released)
  rather than anonymous `mmap` (small flexible-memory budget; not implemented by KytyPS5);
- `frogger-ps5/src/compat.rs` (PS5 only): std's futex (`_umtx_op` `WAIT_UINT_PRIVATE` /
  `WAKE_PRIVATE`, routed with `--wrap=_umtx_op`) on one pthread mutex + condvar, `rename` via
  `sceKernelRename`, an empty `getenv`, log lines through `printf` (klog on the console);
- `frogger-ps5/src/unwind.rs`: `_Unwind_*` stand-ins (never reached with `panic=abort`).

**Native TLS.** PS5_Vulkan's titles use emulated TLS; Rust std uses static TLS. Two fixes,
found with the hello-world/smoke titles in KytyPS5:
`crates/frogger-ps5/ps5-app.ld` (the intermediate PIE layout) declares a `PT_TLS` segment
(lld cannot resolve TLS relocations without one) and puts `.tdata`+`.tbss` in one 32-byte
aligned, 32-byte padded section (the converter gives `PT_TLS` an alignment of >= 32 and
the loader rounds the block to it, so lld's local-exec offsets must already agree, as in
the PS4 `link.x`); `tools/ps5/ps5-native-tool-tls.patch` makes the converter's data
segment load the TLS image and points `PT_TLS` at it (upstream expected TLS in RELRO and
never loaded it: thread locals started as zero).

**Start-up.** `crates/frogger-ps5/csrc/crt.c`: `_start(process_parameters, teardown)` as the
console's loader calls it, `_init_env` (system libc set-up, as PS5_Vulkan's `app_crt.cpp`),
MXCSR reset to IEEE (titles start with FTZ/DAZ on), `.init_array`, `main`, `exit`; plus
`memmove`.

**Video out** (`frogger-ps5/src/prospero.rs`). PS5 VideoOut scans out *tiled* buffers
(`sceVideoOutSetBufferAttribute2` tiling 0, pixel format `0x8000000022000000` = RGBA8 sRGB,
`sceVideoOutRegisterBuffers2`); the console scales them to the TV's mode itself. The CPU
renderer draws 960x540 (4K, 1080p) or 852x480 (1440p) and `console_host::host::TiledScaler`
writes the 64 KiB 128x128 blocks in address order (write-combined memory) on the render
thread pool. Size: `sceVideoOutGetResolutionStatus` when it reports the output mode, else
3840x2160, falling back to 2560x1440 and 1920x1080 if registration fails.

**120 Hz.** As PS5_Vulkan measured on hardware (its `driver/ps5vk_wsi.c`, jobs r51/r92):
the title must declare high frame rate in `param.json` (`attribute3` `0x80040`, else
VideoOut refuses with `0x80290016`); on a fresh handle, before the flip rate and buffers,
`sceVideoOutIsOutputSupported(h, 15, ...)` then `sceVideoOutConfigureOutput(h, 15, ...)`
(mode 15 = 119.88 Hz, 1 = default 59.94 Hz). An accepted mode is not proof: the vblank period
is measured (`sceVideoOutWaitVblank` x30) and mode 1 restored unless it is ~8.3 ms. Flips
are at vblank (`sceVideoOutSubmitFlip` mode 1, flip rate 0 = every vblank). The game keeps
its 60 Hz tick; the extra frames are the front end's interpolated rendering.

**Threads.** Rasteriser bands and the scaler run on a persistent pool
(`console_host::host::Pool`, through `f2_engine`'s new `Renderer::render_soft_with`):
spawning ~16 threads a frame cost 15 ms a frame in KytyPS5 (render 18 ms -> 2.6 ms).

## Testing in KytyPS5 (Windows)

KytyPS5 (`kyty_emulator.exe`, build 2026-10-10 f63d3be) runs the app folder directly:

```text
kyty_emulator.exe --game C:\f2-ps5test\frogger --printf-direction Console --no-splash ^
    --keymap Cross=Return --keymap Up=Up --keymap Down=Down --keymap Left=Left --keymap Right=Right ^
    [--vblank-frequency 120]
```

with the data copied to `C:\f2-ps5test\frogger\data\` (`/app0/data`). `C:\f2-ps5test\shot.ps1`
(adapted from the shadPS4 harness: finds the `[Official build Kyty...` window, client-area
capture, `keybd_event` key steps) takes the screenshots. Notes: Kyty commits 13.5 GiB of
"direct memory" up front (`could not reserve 13824 MB` when the Windows commit limit is
short: WSL's page cache counts; free it first); `Options=Escape` is not an accepted keymap.

Verified 2026-10-10: the smoke test passes everything but `available_parallelism` (Kyty
lacks `sysconf`/`cpuset_getaffinity`; the game then uses 8 threads); the game boots, plays
the Bink intro with audio, runs the frontend and the training level with pad input and
"Cross" prompts, 3840x2160 tiled buffers; `output: 119.88 Hz selected, measured 120.06 Hz`
with `--vblank-frequency 120` and a steady 119-120 fps (render 3.5 ms, scale+flip 4.6 ms),
59.94 Hz restored at the default 60. shadPS4 runs PS4 titles only (use `tools/ps4/`).

## GPU build (`--gpu`, experimental, untested on hardware)

`eboot.bin` becomes the game's wgpu renderer on PS5_Vulkan's statically linked RADV driver
(`f2-engine` `ps5-static-driver`, `GpuContext::new_ps5` + `vkraw::VkPresenter` on
`VK_KHR_display`, see [VULKAN.md](VULKAN.md)); the CPU build ships beside it as
`eboot_cpu.bin`. `frogger`'s `gpu` feature compiles the CPU renderer out of `App`, so when
Vulkan does not start the GPU build starts `eboot_cpu.bin` in its place
(`sceSystemServiceLoadExec`, untested). Needs `tools/ps5/build-vulkan-driver.sh` (the
driver archive and the driver author's SDK fork `b83202be` in
`$PS5_VULKAN/.deps/native/ps5-payload-sdk`) and lld 21 (`-z nodynamic-undefined-weak`;
`$F2_LLD21_BIN`, default `/usr/lib/llvm-21/bin`). The link follows PS5_Vulkan's
`tools/radv-link.sh`, with three reconciliations: the platform layer's `--wrap=malloc` heap
is the only heap (console-host feature `external-heap`); std's FreeBSD-12 `stat`/`dirent`
shims are renamed in the staticlib (`f2_stat`, `f2_readdir`, ...; `llvm-objcopy
--redefine-syms`) so the driver's C code keeps the console layouts and the platform's
`readdir`; the SDK's real libunwind replaces `unwind.rs`. Result: a 37 MB `eboot.bin` that
loads in KytyPS5 and reaches `vkCreateInstance`, where Mesa aborts (`xmlconfig.c: out of
memory`, Kyty's libc HLE), exactly as PS5_Vulkan's own titles do there (VULKAN.md §5).

## Not verified / known gaps

- No PS5 hardware here: everything above is KytyPS5 (CPU build) or link-level (GPU build).
  On a console, `frogger.log` in `/data/frogger2` and klog (`[frogger2]` lines) show how far
  it got; try `--smoke` first.
- Quitting from the menu calls `exit`, which PS5_Vulkan notes crashes its launch context.
- Backgrounds (`pic0.dds`/`pic1.dds`, 4K BC7) are not generated; only the icon.
- `sceVideoOutGetResolutionStatus` returns zeros in Kyty; its PS5 struct layout is assumed to
  match the PS4's (read defensively).
