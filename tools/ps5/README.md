# Native PS5 homebrew app

`tools/ps5/build.sh` builds a **native PS5 app** of the port (not the PS4 fPKG of
`tools/ps4/`): a directory title (`eboot.bin`, `sce_sys/`, `sce_module/libc.prx`) and, from
the same content, an installable **PS5 fake package** (`.pkg`, the format kstuff 1.13+
installs) and mountable `.ffpkg` / `.ffpfsc` images for ShadowMountPlus.

```sh
tools/ps5/build.sh                                   # target/ps5/PPSA99920/ (data copied separately)
tools/ps5/build.sh --data game-data/usa --pkg        # + target/ps5/UP9000-PPSA99920_00-FROGGER2RUSTPORT.pkg
F2_ICON_CROP_X=85 tools/ps5/build.sh --icon boxart.png
tools/ps5/build.sh --ffpkg --ffpfsc                  # + PPSA99920.ffpkg (UFS2Tool), PPSA99920.ffpfsc (MkPFS)
tools/ps5/build.sh --smoke                           # Rust std smoke test (PPSA99921)
tools/ps5/build.sh --hello                           # C hello with PS5_Vulkan's packaging (PPSA99922)
tools/ps5/build.sh --gpu                             # wgpu on the PS5 Vulkan driver (see below)
tools/ps5/build-hw-test.sh --data game-data/usa      # the whole PS4/PS5 hardware test ladder
```

Output folder (`target/ps5/PPSA99920/`, title "Frogger 2: Swampy's Revenge", content ID
`UP9000-PPSA99920_00-FROGGER2RUSTPORT`, the form of PS5_Vulkan's and
UnleashedRecomp-Prospero's hardware-proven titles):

| File | |
|---|---|
| `eboot.bin` | the game: PS5 executable (ELF type `0xFE10`, SCE dynamic imports by NID) in a fake-signed SELF |
| `sce_module/libc.prx` | the clean-room libc companion module every title carries (generated, hash-checked) |
| `sce_sys/param.json` | title metadata, field for field as the proven titles; `attribute3` `0x80040` declares 120 Hz support (game only) |
| `sce_sys/icon0.png`, `icon0.dds` | 512x512 launcher icon (`tools/ps4/make-art.sh`) and its BC7 DX10 DDS |
| `sce_sys/pic0/pic1.png`, `.dds` | 3840x2160 backgrounds (PNG + BC7 DX10 DDS, as Unleashed / PS5_Vulkan ship them) |
| `data/` | only with `--data`: copyrighted data from your own disc, never share the result |

Install the `.pkg` like any PS5 fPKG, or copy the folder to `/data/homebrew/` (FTP) for
ShadowMountPlus, or mount the `.ffpkg` / `.ffpfsc`. Game data without `--data`: as for the
PS4 build, the rendition folder (`game/`, `music/`, `video/`, `manifest.json`) in
`/data/frogger2`, on USB at `/mnt/usbN/frogger2` or on extended storage at
`/mnt/ext0/frogger2`; until found, a dark red screen and a notification. Config and saves go
to `/data/frogger2` (fallbacks `/user/data/frogger2`, `/download0/frogger2`); `frogger.log`
to the first writable of `/data/frogger2`, `/user/data/frogger2`, `/mnt/usb0|1/frogger2`,
`/download0/frogger2`, `/user/temp/frogger2` (klog names it). Creating
`/data/frogger2/60hz` keeps the output at 60 Hz.

**PS5 fPKG (`--pkg`).** Built with PS5 UltraPack's `ffpfsc-pkg-tool`, a command-line
wrapper around drakmor's [LibProsperoPkg](https://github.com/drakmor/LibProsperoPkg) 1.2.0
(GPL-3); UltraPack reports its `HomebrewTest` fPKG launching on retail FW 11.60 with
kstuff-lite 1.13. The staging copy follows that sample: raw ELF modules (`eboot.elf` from
the converter, `libc.raw.elf`) go in and the package builder fake-signs them itself; it
adds the PlayGo files and keystone, and the tool's `validate` checklist runs after every
build (`<pkg>.validate.txt`; currently 33 passed, 0 failed for all three titles). Set up
once (outside the repository):

```sh
git clone https://github.com/knutwurst/ps5-ultrapack ~/opt/ref/ps5-ultrapack   # tested at 32d776f
# .NET SDK 9+ (here: 10.0.401 under ~/opt/dotnet10 via dotnet-install.sh --channel 10.0)
cd ~/opt/ref/ps5-ultrapack/backend/native/src/ffpfsc-pkg-tool
~/opt/dotnet10/dotnet publish PkgTool.csproj -c Release -r linux-x64 -p:RuntimeIdentifier=linux-x64 \
    --self-contained true -o ~/opt/ffpfsc-pkg-tool                         # or set F2_PS5_PKG_TOOL
```

`--ffpfsc` uses the MkPFS copy UltraPack vendors (`backend/mkpfs`, 64 KiB PFS blocks, the
size UltraPack found the console needs); `--ffpkg` PS5_Vulkan's UFS2Tool. The DDS artwork
needs [bc7enc_rdo](https://github.com/richgel999/bc7enc_rdo) at PS5_Vulkan's pinned
`b943862` (`cmake -DCMAKE_CXX_FLAGS="-include cstdint" -DCMAKE_EXE_LINKER_FLAGS=-no-pie`;
`$F2_BC7ENC`, default `~/opt/ref/bc7enc_rdo/build/bc7enc`); the headers are normalised as
PS5_Vulkan's `prepare-assets.sh` does (single level, BC7_UNORM, straight alpha).

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

**Rust std** works as on the PS4: `crates/frogger-ps5` is a staticlib for a custom target,
the stock `x86_64-unknown-freebsd` spec with `tls-model: emulated`, `cpu: znver2` and
`panic-strategy: abort` (generated by `tools/ps4/console-target.py` from the installed
nightly; std is rebuilt with `cargo +nightly -Zbuild-std=std,panic_abort`), linked against
the system libraries' stubs. The shared runtime is `crates/console-host`:

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

**Emulated TLS.** Every thread local (std's included) is reached through
`__emutls_get_address` (`crates/console-host/csrc/emutls.c`: compiler-rt's algorithm on one
pthread key, with UnleashedRecomp-Prospero's destructor-ordering fix: the emutls array
survives two key-destructor rounds so std's thread-local destructors, which run from a
later-created key, still see live storage). The executable has no TLS image: `ps5-app.ld`
asserts that `.tdata`/`.tbss` are empty, `build.sh` checks the intermediate PIE has no
`PT_TLS` and the converted ELF only the converter's empty record (memsz 0, align 1),
exactly as PS5_Vulkan's titles, which the PS5 Clang target builds with `-femulated-tls`.
The converter is PS5_Vulkan's, unmodified. (Earlier builds used native static TLS with a
patched converter and a `PT_TLS` segment; that ran in KytyPS5 but a PS5 on FW 11.40
refused to start it, CE-107694-7.)

**Start-up.** `crates/frogger-ps5/csrc/crt.c`: `_start(process_parameters, teardown)` as the
console's loader calls it, a raw `sceKernelDebugOutText` line after each start-up step
(`[frogger2] _start entered`, `_init_env done`, `constructors done, calling main`), `_init_env` (system libc set-up, as PS5_Vulkan's `app_crt.cpp`),
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

## Hardware test ladder and klog

`tools/ps5/build-hw-test.sh [--data DIR] [--icon IMG] [--copy-to DIR]` builds six titles
with their own title IDs into `target/console-test/` (README there:
`tools/ps5/hw-test-README.txt`): PS5 C hello (PPSA99922, `--hello`: `tools/ps5/hello/hello.c`
compiled and linked exactly as PS5_Vulkan's boilerplate titles: `app_crt.cpp`, its SDK fork
with `libps5platform`, `prospero-clang18`, `ps5-pie.ld`), the PS5 Rust smoke test
(PPSA99921), the PS5 game (PPSA99920), the PS4 smoke test (FROG20001), the PS4 game
conservative (FROG20000) and enhanced (FROG20002); the PS5 titles as `.pkg` plus folders,
the game also as `.ffpkg`/`.ffpfsc`; launch payloads (`tools/ps5/launch/launch.c`, from
UnleashedRecomp-Prospero: `sceSystemServiceLaunchApp` for one title ID, result to klog and a
notification) and ps5-payload-dev's klogsrv.

**klog.** Log lines go to the kernel log with `sceKernelDebugOutText` (`[frogger2] ...`; the
PS5 build also prints them to stdout). With elfldr listening on port 9021 (the loader the
jailbreak starts, which kstuff/ShadowMountPlus are sent to):

```sh
nc -q0 PS5_IP 9021 < target/console-test/payloads/klogsrv-ps5.elf   # "Serving /dev/klog on ...:3232"
nc PS5_IP 3232 | tee klog.txt                                       # then start the title
```

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
short: WSL's page cache counts; free it first); `Options=Escape` is not an accepted keymap;
Kyty does not resolve `sceKernelDebugOutText` (the stdout copy of each line shows instead)
and its libc HLE refuses `FILE` streams other than stdout.

Verified 2026-10-10 with native TLS (game boots, Bink intro with audio, frontend, training
level with pad input, 3840x2160 tiled buffers, 119.88 Hz with `--vblank-frequency 120`).
Re-verified 2026-10-11 with emulated TLS (logs only): the smoke test passes every check
including thread-local destructors at thread exit (all but `available_parallelism`, which
Kyty lacks); the game boots from `/app0/data`, plays the intro and frontend at a steady
60 fps (3840x2160 tiled buffers); the C hello runs its file probes and flips VideoOut
buffers. shadPS4 runs PS4 titles only (use `tools/ps4/`).

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
  The first hardware attempt (native TLS, IV0000 content ID, no DDS artwork, folder via
  ShadowMountPlus on FW 11.40) gave CE-107694-7; the test ladder above is the next round.
  On a console, klog (`[frogger2]` lines) and `frogger.log` show how far it got.
- Quitting from the menu calls `exit`, which PS5_Vulkan notes crashes its launch context.
- `sceVideoOutGetResolutionStatus` returns zeros in Kyty; its PS5 struct layout is assumed to
  match the PS4's (read defensively).

## Status / next steps (2026-10-11)

- **Emulated TLS: done** for PS4 and PS5 (custom target from `tools/ps4/console-target.py`,
  `-Zbuild-std`, `crates/console-host/csrc/emutls.c`; the converter TLS patch is gone). No
  `PT_TLS` in either executable (checked by both build scripts). Kyty and shadPS4 runs pass
  the smoke tests incl. thread-local destructors; the games boot in both emulators.
- **PS5 fPKG: done** (`--pkg`, LibProsperoPkg via UltraPack's `ffpfsc-pkg-tool`); all three
  PS5 packages validate 33/0. Untested on hardware.
- **icon0.dds / pic0.dds / pic1.dds** (+ pic PNGs) generated for every PS5 format.
- **PS4: conservative defaults** (ATTRIBUTE 0, create-fself default auth info, CPU renderer
  unless `renderer.txt` = `piglet` or shader binaries exist); `--enhanced` = Pro/7-CPU/Piglet
  auth info (FROG20002); `--smoke` = FROG20001.
- **Test ladder:** `tools/ps5/build-hw-test.sh` -> `target/console-test/`, copied to the
  user's `Desktop\Frogger2-HW-Test\`. Next: the user's hardware results per step
  (`tools/ps5/hw-test-README.txt` lists what to report).
