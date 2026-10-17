# PS4 / PS5 homebrew package

`tools/ps4/build-pkg.sh` builds a PS4 fake package (fPKG) of the port. It installs on a
jailbroken **PS4** (GoldHEN) and on a jailbroken **PS5** running PS4 homebrew in
backward-compatibility mode (etaHEN / kstuff with PS4 fPKG support, installed through
etaHEN's package installer or Itemzflow). It needs no Sony SDK: the
[OpenOrbis toolchain](https://github.com/OpenOrbis/OpenOrbis-PS4-Toolchain) links
against stubs of the console's own system libraries.

```sh
tools/ps4/build-pkg.sh                          # data copied to the console separately
tools/ps4/build-pkg.sh --data game-data/usa     # your imported data bundled in the package
tools/ps4/build-pkg.sh --icon boxart.png        # home-screen icon / background from an image
tools/ps4/build-pkg.sh --enhanced               # Pro mode + 7-CPU + Piglet auth info (FROG20002)
tools/ps4/build-pkg.sh --smoke                  # minimal Rust smoke test (FROG20001)
```

Output: `target/ps4/IV0000-FROG20000_00-FROGGER2RUSTPORT[-with-data].pkg` (title ID
`FROG20000`). Variants, each with its own title ID so they can be installed side by side:

| | Title | param.sfo ATTRIBUTE | auth info (create-fself) |
|---|---|---|---|
| default (conservative) | FROG20000 | 0 | create-fself's default |
| `--enhanced` | FROG20002 | Pro mode (bit 23) + 7-CPU (bit 16) + Cross = enter (bit 1) | the Piglet set (OpenOrbis Piglet sample / RetroArch) |
| `--smoke` | FROG20001 | 0 | default |

`F2_PS4_ATTRIBUTE=<int>` and `F2_PS4_AUTHINFO=default|piglet|<hex>` override either. The
first hardware attempt with the enhanced settings crashed on a PS5 (CE-108255-1); the
conservative package matches the very first build's settings, so the two can be bisected.
`tools/ps5/build-hw-test.sh` builds all three into the hardware test ladder.

**Pro bisect.** The enhanced package crashed again on the second hardware round (PS5 FW
11.40, kstuff 1.13, CE-108255-1) while the native PS5 package runs.
`tools/ps4/build-pro-bisect.sh [--data DIR] [--icon IMG] [--copy-to DIR]` builds five
packages that each change one thing against the conservative one (A FROG20003 Pro bit only,
B FROG20004 7-CPU bit only, C FROG20005 Piglet auth info only, D FROG20006 Pro bit with
framebuffers forced to 1920x1080 (`F2_PS4_FB_1080=1`), E FROG20007 the enhanced ATTRIBUTE
with the default auth info) into `target/ps4-bisect/` with `tools/ps4/pro-bisect-README.txt`.
`F2_PS4_TITLE_ID` / `F2_PS4_LABEL` / `F2_PS4_TITLE` give `build-pkg.sh` builds their own
identity. The video set-up logs each step before taking it (`video: sceVideoOutOpen`, the
reported mode, direct memory, `sceVideoOutRegisterBuffers WxH linear`, the first two
frames' render / scale / flip) and falls back to 1920x1080 framebuffers when VideoOut
refuses the output-mode size. shadPS4 in Neo mode runs E (4K linear framebuffers, 60 fps). A `--data` package contains copyrighted game data from your own disc: it is
for your console only and must not be shared. Neither the package nor the artwork is
committed (`target/`, `game-data/` are ignored).

**Players don't need any of this.** The desktop app exports the same package from the
player's imported game on Windows, macOS or Linux without OpenOrbis, PkgTool or .NET
(`frogger2 --export ps4`, docs/EXPORT.md "PS4"): `crates/f2-package/src/ps4*` writes the PKG and
both PFS images in Rust from the data-free runtime `tools/runtime/build-ps4.sh` makes (which
runs this script with `--stage-only`: program files only, no PkgTool needed). The two
packages agree structurally (`compare_with_pkgtool` in `crates/f2-package/src/ps4/tests.rs`).

## Game data on the console

Without `--data`, import your CD on a PC (`f2-import`, see `docs/BUILDING.md`) and copy
the rendition folder (the one containing `game/`, `music/`, `video/`) to the console,
e.g. over etaHEN's FTP server:

| Looked up in order | |
|---|---|
| `/app0/data` | bundled with `--data` |
| `/data/frogger2` (or `/data/frogger2-rs`) | internal storage |
| `/mnt/usb0/frogger2`, `/mnt/usb1/frogger2` | USB drive, folder `frogger2` at the root |

Until data is found the game shows a dark red screen with a notification and keeps
checking, so you can copy it while it runs. Config, saves (`saves/savegame.dat`) and
`frogger.log` go to `/data/frogger2` (fallback `/download0/frogger2`).

Controls: the DualShock 4 / DualSense works like the PC game's pad (d-pad or left stick
to hop, Cross jump, Circle tongue, Options pause, touch-pad click = the PC pad's
Select). The settings screen (pause menu -> Settings) works with the pad.

## How it works

- **Renderer**: two back ends for the same draw list, picked at start-up: the CPU
  renderer by default, Piglet only when `renderer.txt` in the save directory says
  `piglet` or Piglet shader binaries are present (see
  [GPU rendering (Piglet)](#gpu-rendering-piglet)):
  - **GPU**: `f2-engine`'s GLES2 back end (`gles.rs`, feature `gles2`) on Piglet, the
    system's OpenGL ES 2 library. The game renders at the output resolution (3840x2160
    on a PS4 Pro / PS5 with a 4K TV), no scaling.
  - **CPU** (default): `f2-engine`'s CPU back end (`soft.rs`), verified near
    pixel-identical against wgpu. The internal resolution is dynamic (`host::DynRes`:
    480, 540, 720, 900, 1080, 1440 lines at the display's aspect, stepped by measured
    render time) and scaled (multithreaded, nearest) to the VideoOut framebuffer.
- **Output resolution**: the framebuffer follows the output mode VideoOut reports:
  3840x2160 on a PS4 Pro or PS5 with a 4K TV (the package declares PS4 Pro support,
  param.sfo ATTRIBUTE bit 23; also 7-CPU mode, bit 16), 1920x1080 otherwise (the base
  PS4 cannot output more). PS4 titles are limited to 60 Hz, also on a PS5.
- **Rust on the console**: `crates/frogger-ps4` is a staticlib for a custom target: the
  stock `x86_64-unknown-freebsd` spec with `tls-model: emulated`, `cpu: btver2`,
  `panic-strategy: abort` (`tools/ps4/console-target.py`; std rebuilt with
  `cargo +nightly -Zbuild-std`), linked against `libkernel` and `libSceLibcInternal`
  instead of OpenOrbis' musl:
  - thread locals go through `__emutls_get_address` (`crates/console-host/csrc/emutls.c`,
    on a pthread key), so the executable has no `PT_TLS` and does not depend on the
    loader's static TLS layout (the build checks); see tools/ps5/README.md;
  - `orbis/shim.rs` translates the calls whose structs changed between FreeBSD 9 (the
    console kernel) and the FreeBSD 12+ ABI std uses (`stat`, `fstat`, `readdir`, ...)
    and supplies symbols the system libraries lack;
  - `orbis/alloc.rs` is the heap (dlmalloc over libkernel `mmap`), libm maths and
    `memcmp`, so the program does not depend on the system libc's heap setup;
  - `csrc/crt_shim.c` replaces musl's `__libc_start_main` and adds `memmove`;
  - `link.x` is OpenOrbis' linker script adapted to Rust's per-symbol sections (and
    asserting there is no TLS data).
- **Packaging**: `create-fself` -> `eboot.bin`, `PkgTool.Core` -> `param.sfo` and the
  `.pkg`; `gp4-dirs.py` fixes the directory tree `create-gp4` writes; `make-art.sh`
  makes `icon0.png` (512x512) and `pic0/pic1.png` (1920x1080).

## Requirements

- OpenOrbis PS4 toolchain v0.5.4 (`toolchain-llvm-18.tar.gz`) extracted, with
  `OO_PS4_TOOLCHAIN` pointing at `.../OpenOrbis/PS4Toolchain` (default
  `~/opt/openorbis/OpenOrbis/PS4Toolchain`).
- `clang-18` and `ld.lld-18` (`apt install clang-18 lld-18`), `ffmpeg`, `python3`.
- the nightly toolchain with `rust-src` (`rustup toolchain install nightly --component rust-src`).
- `PkgTool.Core` is .NET Core 3.1 and needs OpenSSL 1.1, which Ubuntu 24.04+ lacks:
  extract `libssl1.1_1.1.1f-1ubuntu2.24_amd64.deb` (Ubuntu archive) with
  `dpkg-deb -x` and set `F2_LIBSSL11` to its `usr/lib/x86_64-linux-gnu` (default
  `~/opt/libssl11/usr/lib/x86_64-linux-gnu`).

## GPU rendering (Piglet)

The console's own GPU APIs (GNM) are proprietary, but the system also contains
**Piglet** (`libScePigletv2VSH.sprx`), an OpenGL ES 2.0 / EGL 1.4 implementation used by
the system UI and WebKit, which homebrew can load ([OpenOrbis piglet sample][oo-piglet],
[flatz's ps4_gl_test][gltest], [RetroArch's PS4 port][ra-ctx]). `orbis/piglet.rs`:

1. loads the module with `sceKernelLoadStartModule` (development-kit copies in
   `/data/self/system/common/lib` first, then `/<sandbox word>/common/lib`, then
   `/system/common/lib`) and resolves every EGL/GL function with `sceKernelDlsym`;
   nothing is linked at build time, so a missing module is an ordinary error;
2. `scePigletSetConfigurationVSH` (flags composite + flexible memory, as the sample and
   RetroArch), `eglCreateWindowSurface` with an `OrbisPglWindow {id 0, width, height}`
   at the VideoOut output size (queried first), RGBA8 + depth 24, swap interval 1;
3. gets the shader program (below), then renders each frame with
   `Renderer::render_gles` and `eglSwapBuffers`.

Piglet is only tried when `renderer.txt` in the save directory contains `piglet` or a
shader binary (below) is present: on a retail console loading the module and calling EGL
may crash rather than fail cleanly, and without shaders it cannot render anyway. Any
failure is logged (`renderer: Piglet unavailable (...)`) and the CPU renderer is used.
`renderer.txt` containing `cpu` forces the CPU renderer.

**Shaders** are the catch. Retail firmware ships Piglet without its GLSL compiler: the
compiler module `libSceShaccVSH.sprx` exists only on development kits, and the compiler
hooks were removed from retail Piglet ([flatz's write-up][flatz]); `glShaderBinary`
accepts only Piglet's own PSSL shader binaries (format 0), which need Sony's compiler
(the open SPIR-V -> GNM compilers [psbc][psbc] / [opengnm-psbc][ognm] target GNM, not
Piglet's container with GL reflection data). Homebrew therefore either uses devkit
modules (ps4_gl_test, RetroArch, PPlay) or ships binaries compiled once on such a
console (WebKit's `libScePrecompiledShaders`, mpv ports dumping
`glPigletGetShaderBinarySCE` output). The port tries, in `<save dir>/shaders` and
`/app0/shaders`:

1. `f2_gles2.prog`: a program binary (`glProgramBinary`, format u32 + blob);
2. `f2_vs.sb` + `f2_fs.sb`: shader binaries (`glShaderBinary`);
3. the GLSL source (`gles::VERTEX_SHADER_SRC` / `FRAGMENT_SHADER_SRC`) when
   `GL_SHADER_COMPILER` is true, i.e. with a devkit `libScePigletv2VSH.sprx` and
   `libSceShaccVSH.sprx` copied to `/data/self/system/common/lib`. The linked program
   is then saved to `<save dir>/shaders/f2_gles2.prog`, so later runs need no compiler
   and the file can be reused on consoles with the same firmware. Piglet's own shader
   cache is also pointed at that directory (`scePigletSetShaderCacheConfiguration`).

Without any of these (a stock retail console) the CPU renderer runs. Loading system
libraries needs the auth info the OpenOrbis Piglet sample and RetroArch use: the
`--enhanced` package has it (or `F2_PS4_AUTHINFO=piglet`).

The GLES2 back end is checked on the build machine against the CPU renderer through
Mesa's EGL (surfaceless, llvmpipe):

```sh
cargo test --manifest-path crates/frogger-ps4/Cargo.toml --target-dir target/ps4-host --test gles_host
cargo run --release --manifest-path crates/frogger-ps4/Cargo.toml --target-dir target/ps4-host \
    --example gles_shot -- game-data/usa out 0 0 120 3840x2160 [--compare wgpu.png]
```

`gles_host` covers every render state on a synthetic scene (blends, depth modes,
alpha test, fog, affine/perspective, nearest/bilinear, non-power-of-two textures with the
shader wrap used without `OES_texture_npot`, viewport scissor); `gles_shot` renders a
level with both back ends from the same draw list (Garden 1: 0.002 % of pixels differ by
more than 8/255 at 640x480, 1080p and 2160p).

[oo-piglet]: https://github.com/OpenOrbis/OpenOrbis-PS4-Toolchain/tree/master/samples/piglet
[gltest]: https://github.com/flatz/ps4_gl_test
[ra-ctx]: https://github.com/libretro/RetroArch/blob/master/gfx/drivers_context/orbis_ctx.c
[flatz]: https://www.psx-place.com/threads/opengl-es-implementation-on-the-ps4-write-up-provided-by-flatz.21600/
[psbc]: https://github.com/lateleite/psbc
[ognm]: https://github.com/PS4-OpenGNM/opengnm-psbc

## Testing without a console

The [shadPS4](https://github.com/shadps4-emu/shadPS4) emulator runs the staged build:
`shadPS4.exe -g <stage>/eboot.bin` with the data in `<stage>/data`. Verified (shadPS4
v0.19.x on Windows): boot, Bink intro FMVs with audio, frontend, story book, training
level with HUD and PlayStation button prompts, pad input, steady 60 fps.

shadPS4 has no Piglet (no system modules); with `renderer.txt` = `piglet` the log shows
`piglet: load ...: 0x80020002` and `renderer: Piglet unavailable (...); using the CPU
renderer`. Re-verified 2026-10-11 with emulated TLS: the smoke test passes every check
(thread locals and their destructors, threads, files) and pulses green; the conservative
game boots (`main entered` ... intro FMVs) and with `autostart.txt` = `level 1 1` runs
Ancients 1 at 60 fps.

Pro mode / 4K: set `neo_mode` and a 3840x2160 internal resolution in shadPS4's config;
the log then shows `video out: mode 3840x2160` and the render size stepping up.
For scripted runs, `autostart.txt` in the save directory (`level W L`, `demo W L`,
`frontend`) skips the start-up and menus, and `shot.txt` holding a frame number N writes
that frame (as rendered, before scaling) to `shot.png` there.

Not verified: real PS4/PS5 hardware (no console here), in particular the whole Piglet
path. The kernel log shows how far a run got: `[frogger2] main entered` is the first
statement of `main`, then one line per start-up step; `frogger.log` goes to the first
writable of `/data/frogger2`, `/user/data/frogger2`, `/mnt/usb0|1/frogger2`,
`/download0/frogger2` (app-private), `/user/temp/frogger2`, and klog names it. How to
capture klog on a PS5: tools/ps5/README.md "Hardware test ladder and klog".

## Native PS5

A native PS5 build (a PS5 app folder, not a PS4 fPKG; 4K/120 Hz video out, optional wgpu
on the PS5 Vulkan driver) is `crates/frogger-ps5` + `tools/ps5/build.sh`: see
[tools/ps5/README.md](../ps5/README.md). The pad mapping, scalers, std shims and heap are
shared between both hosts in `crates/console-host`.
