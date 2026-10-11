# PS5 GPU rendering: wgpu on the PS5 Vulkan driver (RADV)

The native PS5 host (`crates/frogger-ps5`) draws with `f2-engine`'s CPU renderer today. This
document covers the GPU half: running the game's existing **wgpu** renderer on the PS5 GPU
through the open-source PS5 Vulkan driver from
[mihawk-99/PS5_Vulkan](https://github.com/mihawk-99/PS5_Vulkan) (Mesa **RADV** with a PS5
winsys, GPL-3.0-or-later). Status as of 2026-10-10:

| Piece | Status |
|---|---|
| Driver archive `libvulkan_radeon.ps5.a` | **builds** (`tools/ps5/build-vulkan-driver.sh`) |
| PS5_Vulkan's RADV smoke title, our `vktri` triangle title | **build and link** into signed `eboot.bin` app folders |
| `f2-engine` feature `ps5-vulkan` / `ps5-static-driver` | **implemented**; host-verified (identical frames, swapchain path validation-clean) |
| Running in KytyPS5 | **does not get past `vkCreateInstance`** (emulator gaps, §5) |
| Running on a PS5 | not tried (no console in this environment) |

Nothing from the driver, its SDKs or build outputs is in this repository: they live under
`~/opt` (scripts default to it; `PS5VK_OPT` / `PS5VK` override).

## 1. Building the driver

```sh
tools/ps5/build-vulkan-driver.sh          # ~10 min on 32 threads; idempotent
```

It clones and pins what PS5_Vulkan pins, then runs that project's own scripts:

| Input | Pin | Why |
|---|---|---|
| `PS5_Vulkan` | `main` (README state 2026-10-01) | build scripts, link recipe, title tools |
| `PS5_PayloadSDK` (author's fork) | `b83202be` = upstream **payload SDK v0.42** zip + `libps5platform.a` + fixed headers | the archive is built against it; titles must link its platform layer |
| `PS5_Mesa` (author's fork) | `47e9eb61` = Mesa 26.2.0 + `-Dradv-winsys=ps5` | the driver |
| PS5 OpenGL SDK `opengnm-psbc` | not needed | only the older `ps5vk` driver uses it; RADV compiles SPIR-V with ACO on the console |

Outputs (under `~/opt/PS5_Vulkan`):

- `.deps/native/radv-release/lib/libvulkan_radeon.ps5.a` (≈270 MB: RADV + ACO + NIR + Mesa's
  Vulkan runtime + WSI), headers in `.deps/native/radv-release/include/vulkan/`.
  Exports `vk_icdGetInstanceProcAddr` (also `vk_icdNegotiateLoaderICDInterfaceVersion`,
  `vk_icdGetPhysicalDeviceProcAddr`); there is no loader.
- `build/host/ps5-native-tool` (ELF -> PS5 module converter and fSELF signer) and
  `runtime/libc.prx` (clean-room libc module every title ships in `sce_module/`).
- `dist/PPSA99014/`: the project's RADV smoke-test title.

Host requirements met on this WSL2 Ubuntu 24.04 machine (installed system-wide with apt, plus
meson in `~/.local`): meson ≥ 1.4 (`uv tool install meson`; apt has 1.3), ninja,
`llvm-18-dev libclang-18-dev libclc-18-dev llvm-spirv-18 libllvmspirvlib-18-dev` (host-side
`mesa_clc`/`vtn_bindgen2` for RADV's OpenCL kernels), `python3-mako python3-yaml`,
`glslang-tools`, `bison flex rsync`, and **lld 21** (`apt.llvm.org` `lld-21` only; no
`llvm-config-21` is installed, so the payload SDKs keep using LLVM 18).

Gotchas found:

- **`-z nodynamic-undefined-weak` needs lld ≥ 21.** Mesa's dispatch tables refer to
  per-driver entry points weakly (`radv_EnumeratePhysicalDevices` falls back to
  `vk_common_*`). With lld 18/20 those unresolved weak symbols become dynamic imports and
  `ps5-native-tool` stops with `no public SDK stub exports required symbol
  radv_EnumeratePhysicalDevices` (lld 18 and 20 warn `unknown -z value`). The script points
  only the title link at lld 21 through a tiny `llvm-config` shim
  (`~/opt/ps5vk-tools/llvm-config-lld21`; `prospero-lld` finds `ld.lld` via
  `llvm-config --bindir`).
- PS5_Vulkan's root `make` fails to link its sample app (`ps5_fp_ieee` undefined; the app
  link omits `libps5platform.a`) but has already built `ps5-native-tool` and `libc.prx`,
  which is all we need.

## 2. How a title links the driver and presents

**Linking** (PS5_Vulkan `tools/radv-link.sh`, used verbatim by `tools/ps5/build-vktri.sh`):

- the archive `--whole-archive` (weak dispatch references pull nothing in otherwise);
- the SDK fork's `libc++.a libc++abi.a libunwind.a` (ACO is C++), Clang's builtins
  (`__emutls_get_address`), and `libps5platform.a`;
- `--wrap=malloc,calloc,realloc,free,...` onto the platform's direct-memory heap (libc's
  private heap is far too small), `--wrap=pthread_create/join/detach` (2 MiB stacks in
  direct memory), `--wrap=fclose/fflush` (open_memstream), and ~90 `--defsym=X=ps5_X` for libc
  functions the console lacks or refuses to titles, kept local by a version script (the
  converter refuses exports);
- host-link stubs for `libSceAgc` / `libSceAgcDriver` (the winsys submits through the
  console's own exported AGC driver functions: `sceAgcDriverSubmitDcb`, etc.);
- then `ps5-native-tool link` + `self --sign` into an app folder (`eboot.bin`, `sce_sys/`,
  `sce_module/libc.prx`). This is the same native-app format `crates/frogger-ps5` already
  produces, so the Rust staticlib can be added to this link.

**Presenting**: no window system and no custom extension. RADV exposes **`VK_KHR_display`**
on VideoOut (Mesa `src/vulkan/wsi/wsi_common_videoout.c` in the fork): one display (the TV,
3840x2160 when it accepts 4K), its modes (60 Hz; 120 Hz when `param.json` `attribute3`
declares it and the display takes it), a plane surface from
`vkCreateDisplayPlaneSurfaceKHR`, and an ordinary `VkSwapchainKHR`: `B8G8R8A8_UNORM` or
`_SRGB`, FIFO only, opaque alpha, 2..5 images (3 used), extent = the mode's. Its buffers are
registered with VideoOut and flipped at vblank (`sceVideoOutSubmitFlip` inside the WSI).
So a title presents exactly like a Linux KMS/`VK_KHR_display` application.

## 3. wgpu integration (`f2-engine`, features `ps5-vulkan` and `ps5-static-driver`)

`crates/f2-engine/src/vkraw.rs`:

1. **Entry without a loader**: `entry_from_gipa(gipa)` = `ash::Entry::from_static_fn` over any
   `vkGetInstanceProcAddr`. With `ps5-static-driver` the module declares the archive's
   `extern "system" fn vk_icdGetInstanceProcAddr` and `vkraw::ps5_entry()` wraps it. The
   feature compiles anywhere (`cargo check` on Linux and `x86_64-unknown-freebsd` pass); it
   only links against the driver archive.
2. **Instance/adapter/device**: `GpuContext::from_vulkan_entry(entry, extra_exts)` creates the
   `VkInstance` itself (wgpu-hal's desired extensions + the extras the driver offers, API
   version capped at 1.3 like wgpu-hal), wraps it with
   `wgpu_hal::vulkan::Instance::from_raw` + `wgpu::Instance::from_hal::<Vulkan>`, then
   requests adapter and device exactly like `GpuContext::with_instance` (same features and
   limits), so rendering is unchanged. wgpu-hal enables `VK_KHR_swapchain` on the device by
   itself.
3. **Presentation**: wgpu cannot wrap a foreign `VkSurfaceKHR` (wgpu-hal's
   `create_surface_from_vk_surface_khr` is crate-private, and its DRM path needs a DRM fd), so
   `VkPresenter` owns the swapchain, created with ash on wgpu's `VkDevice` and queue family:
   - `VkPresenter::new_display(&ctx)`: first `VK_KHR_display` display, first mode, a plane
     that supports it; or `VkPresenter::from_vk_surface(&ctx, surface, w, h)` for any surface;
   - swapchain images are imported as wgpu render targets
     (`hal::vulkan::Device::texture_from_raw` with `TextureMemory::External` and a no-op drop
     callback, then `Device::create_texture_from_hal`, initial state `UNINITIALIZED`);
   - per frame: `acquire()` returns the image's `TextureView`; the caller encodes the frame
     into it (`Renderer::encode`); `present()` submits **one** `queue.submit` of three
     command buffers: a raw-API encoder (`CommandEncoder::as_hal_mut`; wgpu forbids mixing
     raw and wgpu commands in one encoder) with `UNDEFINED -> COLOR_ATTACHMENT_OPTIMAL`, the
     caller's encoder, and a raw encoder with `COLOR_ATTACHMENT_OPTIMAL -> PRESENT_SRC_KHR`.
     The acquire semaphore (wait at colour-attachment output) and a per-image "rendered"
     semaphore are attached through wgpu-hal's `Queue::add_wait_semaphore` /
     `add_signal_semaphore`, then `vkQueuePresentKHR` runs on wgpu's raw queue. wgpu's tracker
     always believes the image is in `RENDER_ATTACHMENT`, which the first barrier restores;
     acquire semaphores are recycled per slot after `device.poll(Wait{submission})`.
   - `ERROR_OUT_OF_DATE_KHR` recreates the swapchain (the VideoOut WSI has a fixed mode, so
     this should not occur on the console).

### API for the PS5 host

`f2-engine = { ..., features = ["ps5-static-driver"] }` and `frogger` with the new **`gpu`**
feature (the wgpu `App::new` without `desktop`'s winit/gilrs; `desktop` now implies it):

```rust
use f2_engine::{vkraw::VkPresenter, GpuContext};

let ctx = GpuContext::new_ps5()?;                     // RADV, linked statically, + VK_KHR_display
let mut out = VkPresenter::new_display(&ctx)?;        // VideoOut swapchain (3840x2160 on a 4K TV)
let (w, h) = out.size();
let mut app = frogger::app::App::new(data, &ctx.device, &ctx.queue, out.format(), audio, mapper);
loop {
    // ... app.update(...) / app.tick(...) as the CPU host does ...
    app.draw(w, h);
    let (frame, view) = out.acquire(&ctx)?;           // blocks on the next free VideoOut buffer
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let clear = app.clear_color();
    app.r.encode(&mut enc, view, w, h, clear);         // the unchanged wgpu renderer, at 4K
    out.present(&ctx, frame, enc)?;                    // submit + vkQueuePresentKHR (FIFO, vsync)
}
```

`present` paces the loop to vblank (FIFO). Lower internal resolution, MSAA etc. are the
renderer's existing display options (it renders internally and filters to `w`x`h`). The
link of `frogger-ps5` then adds the driver per §2: `--whole-archive libvulkan_radeon.ps5.a`,
the SDK fork's `libps5platform.a` + `libc++/libc++abi/libunwind`, the `radv-link.sh`
`--wrap`/`--defsym` set (its `--wrap=malloc...` also moves Rust's allocator onto the
platform heap; drop `console-host`'s own heap or keep only one), the AGC stubs, lld 21 with
`-z nodynamic-undefined-weak`. Set `MESA_VK_IGNORE_CONFORMANCE_WARNING=1` before
`new_ps5()` to keep Mesa's non-conformance banner off stderr.

## 4. Verification on the host (evidence)

- **Identical frames through the raw-entry path**: `F2_VK_RAW=1` makes
  `GpuContext::new_headless` use `from_vulkan_entry(ash::Entry::load())`, i.e. exactly the
  PS5 code path with the system loader standing in for the static driver. Garden 1,
  1280x720, 120 ticks (`frogger --headless --frames 120 --level 0 0 --size 1280x720
  --screenshot ...`, built with `--features f2-engine/ps5-vulkan`): both PNGs are
  byte-identical (sha256 `945b5371...85fc`), log line `f2-engine: Vulkan via raw
  vkGetInstanceProcAddr: llvmpipe (LLVM 20.1.2, 256 bits)`.
  Test without game data: `cargo test -p f2-engine --features ps5-vulkan --test vkraw`
  (raw-entry render == loader-path render, pixel for pixel; `entry_from_gipa` with a plain
  function pointer).
- **Swapchain path**: `cargo run --release -p frogger --features ps5-vulkan --example
  vk_present -- <data> 300` (X11 surface instead of `VK_KHR_display`, everything else as on
  the console): 300 frames of Garden 1 presented (llvmpipe, 1280x720 `Bgra8Unorm`), **0
  messages from `VK_LAYER_KHRONOS_validation` with synchronization validation**
  (`VK_KHRONOS_VALIDATION_ENABLES=VK_VALIDATION_FEATURE_ENABLE_SYNCHRONIZATION_VALIDATION_EXT`).
  Negative control: changing the post barrier's old layout to `TRANSFER_SRC_OPTIMAL` produces
  20 errors (`VUID-VkImageMemoryBarrier-oldLayout-01212`,
  `UNASSIGNED-CoreValidation-DrawState-InvalidImageLayout`), so the layer was watching.

## 5. KytyPS5 (2026-10-10 build f63d3be): what fails

Script: `~/opt/ps5vk-tools/kyty-run.sh <title-dir> <log>` (copies to `C:\f2-ps5vk\`, runs
`kyty_emulator.exe --game ... --no-splash --printf-direction Console`). Both titles load,
relocate and start; the driver never reaches the GPU:

1. **PS5_Vulkan smoke title (PPSA99014)** dies at its first `fprintf(stderr, ...)`:
   `--- Fatal Error --- Not implemented (stream != stdout) in ...\src\libs\libC.cpp:639`.
   Kyty's libc HLE only writes to `stdout`; it also lacks `pipe` (`-Jp7F+pXxNg`), used by
   the platform's klog capture.
2. **`vktri`** (`tools/ps5/vktri`, our minimal title: `vk_icdGetInstanceProcAddr` ->
   instance -> device -> `VK_KHR_display` swapchain -> triangle, logging on stdout only;
   `fflush` also aborts in Kyty, so it is compiled out by default):
   - prints `[vktri] start` and `vk_icdGetInstanceProcAddr(vkCreateInstance): 0`;
   - `vkCreateInstance` then calls `getenv`, which Kyty does not provide for
     `libSceLibcInternal` (`Unresolved import stub called: smbQukfxYJM[LibcInternal_v1]`,
     resolved NID = `getenv`), and Mesa aborts: `"%s: %d: out of memory." ...
     src/util/xmlconfig.c` (line 389, `strdup`).
   - With emulator-only shims (`KYTY_SHIM=1 tools/ps5/build-vktri.sh`:
     `vktri/kyty_shim.c` wraps `getenv`/`setenv` with a title-local table and
     `strdup`/`strndup` via the platform heap, whose Kyty `strdup` returns NULL), instance
     creation runs further but never returns: the title heap keeps mapping 16 MiB
     direct-memory segments (`sceKernelAllocateDirectMemory` + `MapDirectMemory`, `[Ok]`
     800 times up to phys `0x31f000000`) until Kyty's direct memory is exhausted
     (`[Fail]`), then the run times out. No AGC or VideoOut call is ever made.
   Other imports Kyty patches to "unresolved" stubs at load: `mmap` (`BPE9s9vQQXo`),
   `readlink` (`pd02UI9TbOA`), `sceKernelDebugOutText` (`9JYNqN6jAKI`), Agc `kW3GLb7QfPg`.

Conclusion: KytyPS5 cannot currently run titles built on PS5_Vulkan's RADV + platform layer
(its libc/libkernel HLE lacks what Mesa and the platform layer use, and memory behaviour
differs). Whether its AGC HLE would accept RADV's raw PM4 submissions is untested. Real
hardware is the next step: PS5_Vulkan's own smoke title (`dist/PPSA99014`) is the first
thing to run on the console (it validates driver + VideoOut on that firmware), then `vktri`,
then the game.

## 6. Remaining blockers / next steps

1. **Run on a PS5** (etaHEN/kstuff, fPKG or a debug-installed app folder; klog over the
   network shows the driver's stdout/stderr, PS5_Vulkan `tools/ps5_console.py klog`).
2. **frogger-ps5 link**: add the driver to the staticlib's link (§3); reconcile its v0.43
   payload SDK with the v0.42-based fork the archive needs (same crt/libc ABI family, but the
   archive references `libps5platform.a` `ps5_*` symbols and the fork's libc++); one heap only
   (`radv-link.sh` wraps `malloc`; `console-host`'s `dmem-heap` and the platform heap must
   not both own direct memory); confirm Rust std's `x86_64-unknown-freebsd` imports are all
   in the SDK stubs `ps5-native-tool` checks (wgpu pulls `libloading`/`dlopen` through ash's
   `loaded` feature even though `ps5_entry()` never calls it).
3. **Shader compile time**: RADV compiles wgpu's SPIR-V with ACO at pipeline creation; enable
   its disk cache (`MESA_SHADER_CACHE_DIR`/`MESA_DISK_CACHE_DATABASE` in a writable title
   folder, as vkQuake does) to avoid recompiling on every start.
4. `param.json` `attribute3` 120 Hz bit if 120 Hz output is wanted (see the WSI source).
5. Licensing: the driver is GPL-3.0-or-later; the `frogger` executable is already
   GPL-3.0-or-later, `f2-engine` stays MIT/Apache (it only declares the symbol).
