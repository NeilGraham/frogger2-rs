# Dynamic lighting (Graphics: Enhanced)

The original has no run-time lights: its lighting is baked into the vertex colours. The
*Enhanced* graphics preset (`display.graphics = "enhanced"`, the enhanced profile's default)
keeps those colours as the base of every surface and adds lights on top, per pixel, in the GPU
mesh shader (`crates/f2-engine/src/gpu_mesh.rs`). *Original* and the authentic profile draw
exactly as before (CPU path, no lights). The N64 build has neither.

## What is lit

- **GPU meshes** (opaque, non-wobbling objects, skins, the landscape) are lit.
- **CPU-drawn geometry** (translucent and animated meshes such as water and lava, sprites,
  particles, shadows) is drawn as before. Glowing CPU meshes and fire sprites still *emit*
  light (below).

## Baked shading and blob shadows

With the dynamic lights on, the original's own shadows would double up:

- **Blob shadows** (the dark discs under the frog, enemies and platforms, `fx::process_shadows`)
  are not drawn when the sun casts shadows; the frog's is kept while it jumps (it shows where the
  jump lands). Levels without a sun (caves) keep them.
- **Baked shading** in the vertex colours is faded: each colour keeps its hue, its brightness is
  lifted towards 0.85 (about the art's average) keeping `Lighting::baked` (0.35) of the original
  shading, and near-black colours (pits, fade-outs to black) are kept as they are.

## Terms

For a surface with texture × vertex colour `base`, normal `n` (turned to face the viewer: the
game's meshes are one-sided and their normals are not always consistent) and world position
`p`:

```
light = mix(ground, sky, n.y / 2 + 1/2)                         hemisphere ambient
      + sun_color × max(n·L, 0) × shadow(p)                      sun / moon
      + Σ point_color × (1 − d/r)² × clamp(0.75 n·l + 0.25)      point lights (wrapped)
colour = base × light + sun_color × spec × (n·h)^48 × shadow × mean(vertex colour)
```

- **Balance:** each world's setup (`crates/frogger/src/lights.rs`) puts a surface facing up at
  about 1.2× the original, one facing the sun side-on at about 1×, one facing away at about
  0.75×, so a level keeps its art direction. The highlight is scaled by the vertex colour so
  corners the artists darkened stay dark.
- **Sun direction:** the game's cameras mostly look from +z down at the level and the screen
  mirrors x (world +x is screen left), so suns come from +x/+z: the faces the player sees are
  lit and shadows fall to the right, away from the camera.
- **Shadow map:** 2048², `Depth32Float`, orthographic from the sun over every GPU mesh draw (on
  screen or not), centred on the camera's target and sized by the camera distance
  (1.6×, 500..3000 units), snapped to whole texels so it does not crawl as the camera moves.
  3×3 PCF with a hardware comparison sampler, normal offset 2 units, slope-scaled bias.
- **Glowing surfaces:** textures whose names contain `lava`, `fire`, `flame`, `glow`, `burn`,
  `bulb`, `lairfl`, `ember`, `candle`, `neon` or `lamp` (`lighting::is_emissive`) are drawn
  unlit (×1.15) and emit light: each glowing texture group is split into 220-unit cells and
  every cell gets a point light (the texture's mean colour pushed warm, raised above the surface,
  radius 140 + 1.5 × the cell's size). Animated meshes drawn on the CPU are offered to the GPU
  side only for these lights (`GpuObj::light_only`). Fire sprites add a light each
  (`Renderer::light_sprite`, from `print_sprites`). Lights closer than 40 units merge; the 32
  nearest the camera's target are used.

## Per-world setups

| World | Light |
|---|---|
| Garden | warm late-afternoon sun |
| Ancients | high, hot sun, warm bounce from the sand |
| Space | hard white starlight, blue fill |
| Subterranean | no sun (under rock: it would be in shadow); bright ambient, lava lights |
| Laboratory | cool overhead light |
| Halloween | blue moonlight |
| others | neutral daylight (`Lighting::daylight`) |

## Tests

`crates/f2-engine/tests/headless.rs`: `gpu_mesh_lighting_balances_and_casts_shadows` (ambient 1
reproduces the baked colours exactly; a card casts a shadow of the expected size).
