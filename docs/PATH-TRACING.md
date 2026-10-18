# Path-Traced graphics

`display.graphics = "path_traced"` (*Graphics: Path-Traced* in the settings): the Enhanced
renderer ([LIGHTING.md](LIGHTING.md), [WATER.md](WATER.md)) with its lighting traced on the GPU
(`crates/f2-engine/src/rt.rs`). Needs hardware ray queries (wgpu
`EXPERIMENTAL_RAY_QUERY`: DXR 1.1 / `VK_KHR_ray_query`); without them the setting reads
*Path-Traced (no ray tracing: Enhanced)* and draws Enhanced. `F2_NO_RAY_TRACING=1` turns ray
queries off for testing.

On Windows (DX12) ray queries need shader model 6.5, i.e. Microsoft's DXC compiler:
put `dxcompiler.dll` and `dxil.dll` beside `frogger2.exe` (`tools/run/fetch-dxc.sh GAME_DIR`;
packages built with them in the build folder include them). Without DXC wgpu compiles with FXC
and reports no ray tracing. `logs/upscale.log` says `ray tracing: true/false`. (Statically linked
DXC, wgpu's `static-dxc`, would need the Visual Studio ATL libraries to build.)

## What is traced

- **Scene:** each rigid GPU mesh gets a BLAS of its opaque-textured groups (textures with more
  than 2% cut-out pixels, e.g. leaves and fences, and skinned meshes are left to the sun's shadow
  map, which still applies: shadow = min(map, rays)); each frame's draws are the TLAS instances,
  each with its mean texture colour.
- **Sun / moon:** soft shadows, rays into a 1.7° disc, 260 units long: far casters fade out as
  in the shadow map ([LIGHTING.md](LIGHTING.md)).
- **Point lights:** the 8 nearest the camera's target cast shadows (one ray each to a jittered
  point on the light).
- **Ambient and bounce:** cosine-weighted rays (90 units): the openness scales the ambient
  (softened, the baked colours already darken corners); a hit adds its albedo lit by half the
  ambient and, if a ray from it reaches the sun, half the sun (one bounce of colour).
- **Refraction:** the view ray bent by the wavy water surface is traced to what it lands on, so
  what lies under the water wavers where the waves really send the rays
  ([WATER.md](WATER.md)).
- **Caustics, by photon tracing:** a compute pass sends one photon per water-simulation cell (1.5 units)
  from the sun through the simulated surface (sim waves + ripples, refracted at 1.33), traces
  each to what it lands on and counts landings per cell. A surface under the water is lit by the
  sun in proportion to the photon density over it (5×5 tent filtered): bright where the waves
  focus the light, dimmer between. This replaces the procedural caustics of Enhanced.

Noise: with a temporal upscaler (DLSS, FSR) the ray directions change every frame and the
upscaler averages them; without one the pattern is fixed and twice the rays are traced (4 sun,
4 ambient). DLSS / FSR (quality or DLAA) is the intended pairing.

Ray queries live in WGSL functions, never directly inside loops: Mesa's lavapipe (and maybe
other drivers) fail to compile a `ray_query` declared in a loop body.

## Cost

RTX 5090, 3024×1890, DLSS Quality: about 300 FPS Path-Traced against about 520 FPS Enhanced
(Garden 1-1, Halloween 1-1).

## Tests

`crates/f2-engine/tests/headless.rs` (lavapipe has ray queries):
`ray_traced_lighting_casts_shadows` (a card's shadow from the rays alone, no shadow map), and
`enhanced_water_takes_splashes` runs with the photon pass where available.
