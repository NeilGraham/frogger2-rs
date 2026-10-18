# Enhanced water (Graphics: Enhanced)

`crates/f2-engine/src/water.rs` (simulation), `gpu_mesh.rs` (water pass, caustics),
`crates/frogger/src/app.rs` (`water_tick`: landings), `crates/f2-draw/src/render.rs`
(`DrawCtx::dips`). *Original* graphics, the authentic profile and the N64 draw the water as
before.

## What the original does

Water is ordinary meshes with `OBJECT_FLAGS_MODGE` / `OBJECT_FLAGS_WAVE`, drawn on the CPU:
`PCCalcModgeValues` wobbles the texture coordinates (MODGE), and WAVE meshes are lifted by a slow
swell with their alpha computed from the wobble (`50 + (mx + mz)·400`). The opaque river beds
under the water are MODGE-only meshes: their moving texture is the original's stand-in for
caustics. Garden 1-1, for example, has an additive `00wate04` surface over a `riverbottom` bed.

## What Enhanced does

- **Classification** (`gpu_mesh::classify`, once per mesh): a mesh the CPU would draw (MODGE,
  WAVE or translucent) whose faces are mostly water textures (`lighting::is_water`: `wat<n>`,
  `ai_water`, `ai_ripple`, `00wate`, `00nwat`, `aniswamp`, `aqua0`, `aquatop`, `ripple`,
  `sludge`, `swampi`, `swampyi`) goes to the **water pass**; an opaque MODGE-only mesh (river
  bed) goes to the lit **GPU mesh pass without its wobble** (the real caustics replace it).
  Anything else stays on the CPU.
- **Geometry:** water meshes are subdivided once at upload (each triangle into n², n ≤ 8, edges
  ≤ 12 units), and the MODGE wobble, WAVE swell and alpha are computed per vertex on the GPU with
  the original formulas, so the surface keeps its look with smoother motion.
- **Waves:** a 512² damped wave equation (3-unit cells, ~1500 units square around the camera's
  target, scrolling with it a cell at a time), stepped at 120 Hz in pairs from the host's
  clock (`Renderer::water_clock`, game ticks + interpolation). A top-down depth render of the
  water meshes (the surface map) says where water is and how high; dry cells stay flat and
  reflect waves. Disturbances (`water::Splash`) push the surface down where there is water near
  their height:
  - actors moving through water make a wake each 60 Hz tick (`Renderer::actor_wake`, from the
    distance moved and the model's size);
  - the frog landing on a platform splashes at the platform (`App::water_tick`), and
  - the platform and the frog dip by a couple of units on a damped spring (render-only
    `DrawCtx::dips`, interpolated between ticks).
- **Surface shading:** per-pixel normals from the simulated height plus small travelling
  ripples; the texture is bent by the normal (cheap refraction); Fresnel reflection of the sky
  colour and a sharp sun glint (shadowed). Additive water stays additive, alpha-blended stays
  alpha-blended; drawn after the opaque geometry, before the CPU's translucent triangles.
- **Caustics** (performant, Enhanced): every lit GPU surface under the surface map's water gets
  a procedural caustic network (two layers of warped wave interference, ~25-unit cells),
  shifted along the sun direction by the depth, focused or spread by the simulated surface's
  curvature (its Laplacian), fading with depth (`exp(-depth/240)`), shadowed with the sun.
  Without a sun (caves) the sky light makes them. The Path-Traced preset will trace them.

## Tests

`crates/f2-engine/tests/headless.rs`: `enhanced_water_takes_splashes` (a pond drawn by the water
pass; a splash makes visible waves). `lighting::tests::water_names`.
