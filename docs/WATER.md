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
- **Generated surface:** the flat parts of the water are drawn as our own surface instead of the
  original meshes: a 512² grid of quads (2×2 simulation cells, 3 units) over the simulation
  window, resting 2.5 units below the surface map's height and moved by the simulation, a slow
  swell (two long waves and three shorter ones, scaled per level by `Lighting::water_swell`:
  standing water keeps 15% of it, so what moves through it shows) and fine ripples (always
  there: they bend the light into caustics and catch the sun). `Lighting::water_flow` (world
  units per second; zero: standing water) carries the swell, the ripples, the texture detail
  and the caustics along, for rivers. The original meshes only say where water is and how high
  (surface map); their steep, tall faces (waterfalls, more than 24 units high) are still drawn
  from the mesh, subdivided (n² per triangle, edges ≤ 12 units) with the original wobble / swell
  / alpha per vertex. Crests reach the original surface height and troughs go below it, so the
  banks made for that height stay dry.
- **Depth** is measured from where the water rests, so a platform's top between the rest and
  the original surface stays dry. A second top-down render, the *bed map*, holds the highest level surface (landscape
  and river beds, not actors) under the water, so each point knows how deep the water is: shallow
  water is clear, deep water takes the tint (the original water texture's mean colour, its
  texture as world-space detail), and the surface fades out at the shore with a little foam (the
  thickness is blended between the four nearest cells, so shores are smooth). Level geometry up
  to 15 units above the water is a bank's lip (a stone rim, grass hanging over the water): under
  it the water goes on as deep as the nearest river bed beside it, up to the bank, which hides
  it; the grid points next to anything the crests could reach (a lip, a platform's top just over
  the water) keep still at rest, so waves never poke through. Open water with nothing under it is deep.
- **Edges of the grid:** a triangle is drawn when any of its corners has water within 2 cells;
  its other corners take the height of water within 4 cells, so triangles along the shore stay
  in place (their dry part is discarded per pixel).
- **Waves:** a 1024² damped wave equation (1.5-unit cells, ~1500 units square around the
  camera's target, scrolling with it a cell at a time), stepped at 240 Hz in pairs from the host's
  clock (`Renderer::water_clock`, game ticks + interpolation). A top-down depth render of the
  water meshes (the surface map) says where water is and how high; dry cells stay flat and
  reflect waves. Disturbances (`water::Splash`) push the surface down where there is water near
  their height:
  - actors moving through water make a wake each 60 Hz tick (`Renderer::actor_wake`, from the
    distance moved and the model's size);
  - the frog landing on a platform splashes at the platform, hopping off one lets it bob up
    with a smaller wave, and falling in makes a big splash (`App::water_tick`), and
  - the platform and the frog dip by a couple of units on a damped spring (render-only
    `DrawCtx::dips`, interpolated between ticks).
- **Surface shading:** per-pixel normals from the simulated height plus the swell, and fine
  ripples (wavelengths 9 to 25 units, faded out with distance) for sparkle; the body lit by the
  waves' slopes; a sharp sun glint and sparkles (shadowed). Drawn after the opaque geometry,
  before the CPU's translucent triangles (the waterfalls keep the original's additive or alpha
  blending).
- **Reflections and what shows through:** the frame's pass is split before the water and the
  frame so far (colour and depth) copied (`GpuMesh::use_scene_copy` / `scene_copy`; any
  single-sampled frame whose target can be copied: always with DLSS / FSR, not with MSAA). The
  generated surface then ray-marches its reflection through the copy (screen-space reflections,
  32 growing steps from a per-pixel jittered start (no dotted seams), 5 refining ones, and a
  hit only on a surface close behind the refined point; off the screen: the level's backdrop colour, paler to the
  horizon), weighted by a Fresnel term lifted for the game's steep cameras. What lies under the
  water is read from the copy, bent a little by the waves, and tinted along the path through
  the water (red absorbed first) into the water's own colour. Without a copy the surface is
  blended over the frame as before. The surface is premultiplied, so its reflection does not
  depend on how clear the water is.
- **Waterfalls** (the steep faces) stir the water where they land: each waterfall group keeps up
  to 12 of its lowest points, which push the simulated surface every step (moving about a little,
  so the water churns), reaching water up to 400 units above them (a fall's mesh may go on far
  under its pool). Where the simulated surface moves fast (churn: under falls, wakes,
  splashes) the water foams. Waterfalls are left out of the surface map (they would lift their
  pool to their top).
- **Refraction:** what lies under the water wavers as the waves bend the view rays: each pixel of
  a surface under the water samples its texture where the ray refracted at the wavy surface lands,
  minus where it would land under flat water (turned into texture space with the surface's screen
  derivatives). This is what the original faked by wobbling the river beds' textures (their MODGE
  wobble is off in Enhanced). Path-Traced traces the refracted ray against the scene.
- **River beds** move from the CPU's late pass to the GPU mesh pass; they are pushed half a unit
  back in depth so the banks they run under still win where the two meet (no z-fighting).
- **Caustics** (performant, Enhanced): every lit GPU surface under the surface map's water gets
  a procedural caustic network (two layers of warped wave interference, ~25-unit cells),
  shifted along the sun direction by the depth, focused or spread by the simulated surface's
  curvature (its Laplacian), fading with depth (`exp(-depth/240)`), shadowed with the sun.
  Without a sun (caves) the sky light makes them. The Path-Traced preset will trace them.

## Tests

`crates/f2-engine/tests/headless.rs`: `enhanced_water_takes_splashes` (a pond drawn by the water
pass; a splash makes visible waves). `lighting::tests::water_names`.
