# Upscaling and frame generation (DLSS, FSR)

NVIDIA DLSS, AMD FSR 3.1 and AMD FSR 4 super resolution, DLSS frame generation (including
multi frame generation) and FSR frame generation, plus the depth and motion vectors they need,
so DLSS 5 Neural Rendering can run on the game's picture as well. Enhanced profile only;
Windows (Direct3D 12) only. The game doesn't need any of this at its original detail level; it is
here for heavier content (higher-poly models, ray tracing) and to try the technology.

## What is supported

| | Modes | GPUs |
|---|---|---|
| **DLSS** (super resolution) | DLAA, Quality, Balanced, Performance, Ultra Performance; presets Default / J / K / L / M | GeForce RTX |
| **FSR 3.1** | Native AA, Quality, Balanced, Performance, Ultra Performance; sharpening | any DX12 GPU |
| **FSR 4** (4.1 with SDK 2.3) | as FSR 3.1 | Radeon RX 7000 / 9000 (falls back to FSR 3.1 elsewhere) |
| **DLSS Frame Generation** | 2x | GeForce RTX 40 / 50 |
| **DLSS Multi Frame Generation** | 3x, 4x, dynamic (up to the driver's maximum, e.g. 6x on RTX 50 with DLSS 310.8, to a target frame rate) | GeForce RTX 50 |
| **FSR Frame Generation** | 2x (AMD offers one generated frame; the ML generator 4.x on RX 9000 when FSR 4 upscales) | any DX12 GPU |
| **NVIDIA Reflex** | Off, On, On + Boost (on whenever DLSS frame generation is) | GeForce |
| **DLSS 5 Neural Rendering** | through RenoDX's ReShade add-on, on our DLSS output | GeForce RTX 50 (what NVIDIA's runtime supports) |

Super resolution and frame generation are chosen independently, so e.g. DLSS with FSR frame
generation (RTX 20 / 30) or FSR with DLSS frame generation both work.

Verified on an RTX 5090 (driver 32.0.16.1742) at 3024x1890 / 3360x1890: DLSS 310.8.0 (Quality, DLAA),
DLSS-G 4x and dynamic (reports up to 5 generated frames), FSR 3.1.5 + FSR Frame Generation
3.1.6, FSR 3.1.5 on the Ryzen's Radeon iGPU (`adapter = "Radeon"`), DLSS 5 NR through RenoDX
2026.1003. FSR 4 could not be run here (no RX 7000 / 9000).

## Setting it up

1. **Runtimes** (not shipped with the game; put them in `upscalers\` beside `frogger2.exe`, or
   beside the exe itself, or set `[upscale] runtime_dir`):
   - DLSS: the Streamline + DLSS DLLs (`sl.interposer.dll`, `sl.common.dll`, `sl.dlss.dll`,
     `sl.dlss_g.dll`, `sl.reflex.dll`, `sl.pcl.dll`, `nvngx_dlss.dll`, `nvngx_dlssg.dll`) from
     NVIDIA's [Streamline releases](https://github.com/NVIDIA-RTX/Streamline/releases) or a DLSS
     package such as the `DLSS*-Streamline*.zip` from the RenoDX Discord. If the NVIDIA app has
     a newer DLSS / Streamline override installed, Streamline uses that one (`sl.log` says so).
   - FSR: `amd_fidelityfx_loader_dx12.dll`, `amd_fidelityfx_upscaler_dx12.dll`,
     `amd_fidelityfx_framegeneration_dx12.dll` from the
     [FidelityFX SDK](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/releases)
     prebuilt package; `tools/run/fetch-fsr.sh GAME_DIR` downloads them.
   - `tools/run/setup-dlss5.sh GAME_DIR DOWNLOADS_DIR` unpacks the DLSS zip into `upscalers\`
     (and the DLSS 5 parts, below).
2. **Settings** › Display: *Upscaler*, *Upscaling mode*, *DLSS preset* / *FSR sharpening*,
   *Frame generation*, *Target frame rate* (dynamic), *NVIDIA Reflex*. Or `config.toml`:

```toml
[upscale]
upscaler = "dlss"            # "off" | "dlss" | "fsr3" | "fsr4"
quality = "quality"          # "native" (DLAA / Native AA) | "quality" | "balanced" | "performance" | "ultra_performance"
dlss_preset = "default"      # "default" | "j" | "k" | "l" | "m"
sharpness = 0                # FSR sharpening, 0..100
frame_generation = "off"     # "off" | "dlss_2x" | "dlss_3x" | "dlss_4x" | "dlss_dynamic" | "fsr_2x"
target_fps = 0               # dlss_dynamic: frame rate to reach (0 = display refresh)
reflex = "off"               # "off" | "on" | "boost"
# runtime_dir = 'D:\dlss'    # where the DLLs are (default: upscalers\ beside the exe, else the exe's folder)
# adapter = "Radeon"         # run on the GPU whose name contains this (default: the fastest)
dlss5_helpers = false        # DLSS 5 through RenoDX, see below
```

Turning upscaling or frame generation **on from off** needs a restart (the game then starts on
Direct3D 12 with a swapchain of its own); so does switching frame generation between DLSS and
FSR (each owns the swapchain). Everything else (upscaler, mode, preset, sharpening, 2x / 3x /
4x / dynamic, target, Reflex, VSync) applies live. The settings screen says when a restart is
needed and shows what is running.

`<config>\logs\upscale.log` lists what loaded and why something didn't (missing DLLs, an
unsupported GPU); Streamline writes `sl.log` beside it.

Frame generation works best with VSync off on a high-refresh (or VRR) display, and with the
frame cap off or set to the rendered rate you want.

## How it works

`f2-engine` (`upscale.rs`, `Renderer::render_upscaled`) and `crates/f2-upscale` (native glue);
the host side is `crates/frogger/src/upscaling.rs`.

- **Render size and jitter.** The 3D scene (backdrops and the seven `DrawAllFrames` passes) is
  drawn at the render size the upscaler asks for (DLSS: its optimal settings; FSR: the mode's
  ratio) with a Halton(2, 3) sub-pixel jitter (`ceil(8 × ratio²)` phases) added in the vertex
  shader. The original's projection and depth are untouched.
- **Motion vectors.** The game transforms vertices on the CPU (`scene.rs`, the MDX pipeline),
  so each opaque vertex also gets its **previous-frame screen position**: the actor's matrices
  from its last draw (`Actor::motion_prev`) through the last frame's view (camera and
  projection, including the original's integer-truncated depth). Interpolated actors, skinned
  bones, the wave lift of water and the landscape are all covered; the camera's motion is part
  of it. They are written by the depth-writing passes into an `Rg16Float` target (render
  pixels, previous minus current, without jitter); translucent passes (particles, shadows,
  sprites) leave the opaque surface's motion. Large camera jumps, size changes and the first
  frame are cuts (history reset).
- **Depth.** The original's depth buffer is linear (`vz × 0.00025`); upscalers want projection
  depth, so the opaque passes also write `R32Float` **reversed infinite depth** `near / z`
  (near = 10, the original's near clip), from the interpolated `1/z`.
- **Camera.** `UpscaleCamera` describes the original's projection as a standard perspective
  matrix (row-vector convention, as Streamline takes it): view-to-clip and back,
  clip-to-previous-clip, camera position and axes, vertical FOV. The engine test
  `projection_matches_the_game` checks the matrix puts points where the game draws them.
- **Native side.** The native code (`native/f2_upscale.cpp`, built with MSVC through `cc`; headers vendored,
  libraries loaded at run time) gets wgpu's `ID3D12Device`, queue and textures (`as_hal`), and
  records its own command lists on the same queue between wgpu's submissions; wgpu's
  `transition_resources` puts the inputs in shader-read and the output in UAV state. DLSS runs
  through Streamline in manual-hooking mode (frame tokens, `slSetConstants`,
  `slSetTagForFrame`, `slEvaluateFeature`); FSR through the FidelityFX API loader with the
  provider (3.x or 4.x) chosen by version override.
- **HUD.** The upscaled image is drawn to the output, then the 2D overlays (HUD, menus, text)
  are drawn over it at the output size: sharp, and never in the upscaler's history. That image
  without the HUD is what frame generation gets as the HUD-less colour (DLSS-G tag; FSR
  `HUDLessColor`, double-buffered), so generated frames do not smear the HUD.
- **Presentation.** Frame generation has to own the swapchain, so on this path the game does
  not use a wgpu surface: the finished frame is copied into the back buffer of the native
  swapchain (Streamline's proxy for DLSS / Reflex, AMD's frame generation swapchain for FSR,
  plain DXGI otherwise) and presented there.

### Debugging

- `F2_UPSCALE_DEBUG=motion` / `depth`: shows the motion vectors (grey = still, red / green =
  x / y) or the depth instead of the picture.
- `F2_UPSCALE_SHOT=out.png:FRAME` writes the presented image at that renderer frame;
  `F2_UPSCALE_EXIT=FRAME` quits then (unattended runs).
- `F2_FSR4_PROBE=1` logs every FSR upscaler version the library offers and creates each FSR 4
  one on the running GPU (an unlisted 4.1.1 with its derived id too). On the RTX 5090 and the
  Ryzen iGPU it offers only 3.1.5 / 2.3.4 and the forced 4.1.1 create returns 4
  (`FFX_API_RETURN_NO_PROVIDER`): FSR 4 needs a Radeon RX 7000 / 9000.
- `cargo test -p f2-engine --test upscale`: headless checks that a moving actor and a moving
  camera produce the right motion vectors and depth.

## DLSS 5 (Neural Rendering)

DLSS 5 Neural Rendering is not in the public Streamline SDK; RenoDX's ReShade add-on
(`renodx-dlss.addon64`, from the RenoDX Discord) applies it to a game's DLSS output by hooking
NGX. This port supplies what it needs: a real DLSS feature with jittered colour, depth and
motion vectors, and the HUD drawn after it.

1. `tools/run/setup-dlss5.sh GAME_DIR DOWNLOADS_DIR` with `DLSS*Streamline*.zip`,
   `ReShade_Setup_*_Addon.exe` and `renodx-dlss.addon64` in `DOWNLOADS_DIR`. It puts the runtimes in
   `upscalers\` (and `nvngx_dlssnr.dll` beside the exe), `ReShade64.dll` and the add-on beside the
   exe, and sets RenoDX's hook method to **Upscaled** in `ReShade.ini`
   (`[RENODX-DLSS] DirectNeuralRenderingHookStage=2`; 1 = Render applies it to DLSS's input).
2. `config.toml`: `upscaler = "dlss"` (any mode; DLAA = `quality = "native"` keeps the most detail),
   `dlss5_helpers = true`.
3. Start the game: it loads `ReShade64.dll` itself. Home opens ReShade's overlay; RenoDX's tab
   has the Neural Rendering options and status.

Why the game loads ReShade itself instead of ReShade being installed as `dxgi.dll`: wgpu's
Direct3D 12 start-up asks DXGI for an `IDXGIFactoryMedia`, and ReShade's `CreateDXGIFactory1`
hook crashes on that (inside `dxgi.dll`). So the game creates the wgpu instance first and then loads
`ReShade64.dll`, before the device and swapchain exist, so ReShade still sees both. With
`dlss5_helpers`, the game also loads `nvngx_dlssnr.dll` and, after 120 frames, creates a throwaway
WARP device so ReShade raises `init_device` again and RenoDX attaches its runtime to the live
device (MGS4 DLSS's `NrPreload` / `NrKick`).

`ReShade.log` shows it working: `DLSS-NR direct: attached snippet ...nvngx_dlssnr.dll`,
`NR Upscaled extents`, `CreateFeatureInstance end: result=0x00000001` and
`NR vendor evaluate end: result=0x00000001` each frame (verified 2026-10-17, RTX 5090, RenoDX DLSS
v0.2026.1003, ReShade 6.8.0). DLSS 5 together with frame generation was not tested.

## Limits

- Windows / Direct3D 12 only. Linux, macOS and the consoles keep the normal path (the options are
  greyed out there); NGX on Vulkan and FSR's Vulkan back end are possible later.
- The authentic profile never upscales (it is the retail picture); MSAA and render scale do not
  apply while an upscaler runs.
- No reactive / transparency mask yet: translucent effects (particles, water surfaces drawn in
  the translucent passes) rely on the upscalers' own detection.
- Actors that change model or appear mid-frame have no previous pose: one frame without object
  motion (camera motion still applies).
- Water and slime surface wobble (`OBJECT_FLAGS_MODGE` texture coordinates) is not in the motion
  vectors (it is texture animation, not movement).
