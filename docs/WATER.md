# Enhanced water (Graphics: Enhanced)

`crates/f2-engine/src/water.rs` (simulation), `gpu_mesh.rs` (water pass, caustics, lava),
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
  Opaque meshes that are another liquid are **opaque liquids** (`Class::Liquid`, below): lava
  (texture names with `lava`), acid (toxic glop `glop` / `toxic`, sludge, and water textures
  whose colour as seen, texture mean × vertex colours, is green: Subterranean tints its blue
  `00nwat02` / `wat0` green, so its standing acid vats are acid, not water), and water whose
  texture slides (a stream and its fall: the frontend's). Anything else stays on the CPU.
- **Streams, from the animation:** an actor whose texture slides (`ACTOR_SLIDYTEX` /
  `ACTOR_SLOWSLIDE`, `CopyActorList`) shows something flowing. Its speed (`faceTC.v` units per
  game frame, `texture_slide_speed`) reaches the renderer as `Actor::slide`; the texture moves
  to +v, so the flow is along v's gradient on the surface at slide / |gradient|. Water of this
  kind (Garden's `wat_flo` / `wat_fstr`, Ancients' `wat_floa`, ...) writes its flow into the
  **flow map** (beside the surface map, x / z per second, highest surface wins); the generated
  surface's ripples, swell, texture detail and caustics are carried along it by flow mapping
  (two copies each carried for 2.5 s and put back, half a period apart, crossfaded), the flow
  smoothed over about 12 units (the art's texture density changes from face to face, and a
  stream runs into still water). Still water writes no flow. Sliding waterfalls drawn from
  the mesh slide as in the original (the uploaded mesh's v is moved per draw). The flow map's
  z holds the liquid there (0 water, 1 acid, 2 lava, 3 a stream drawn opaque), w coverage.
- **Opaque liquids** keep their own surface on the GPU mesh pass, moving as in the original
  where their texture slides (v moved per draw), and go into the surface map too (their steep
  faces left out per pixel), so the wave simulation runs on them, as thick as the liquid is
  (below). Their patterns are height fields in the texture's coordinates, so everything on
  them flows with the texture at the animation's speed, repeating with the texture (the art's
  faces meet whole repeats apart); detail finer than a few pixels fades out. Their faces also
  light what stands in them and their banks, dimly (point lights in their colour).
  - **Acid** (`fn liquid`): thick and glossy, the original texture's colour kept as its
    palette: slow, gloopy swells (no fine chop), emerald in the troughs and lime on the
    swells, a breathing glow of its own, foamy yellowish scum clustering and drifting on it
    (pitted with tiny bubbles), bubbles that swell slowly and pop into a spreading ring
    (more of them in standing acid), streaks along the flow and froth on the crests where it
    runs steep. The simulated waves bend its normal; where they move it fizzes, brightens,
    and their crests catch the light (the rings of a frog landing on a crate). Lit through
    the bumped normal: sharp highlights of the key light (the sun, else a light from above in
    a cave) and the point lights, duller on the scum; reflections of the sky (in a cave, of
    its own glow).
- **Generated surface:** the flat parts of the water are drawn as our own surface instead of the
  original meshes: a 512² grid of quads (2×2 simulation cells, 3 units) over the simulation
  window, resting 2.5 units below the surface map's height and moved by the simulation.
  **Standing water** (no flow there) is silky smooth: no ripples, no swell, its texture detail
  still and faint, so only what disturbs it shows (the simulation's waves, whose curvature also
  focuses its caustics). **Streams** (the flow map, plus `Lighting::water_flow`) ripple as
  they run, more the faster they flow: a slow swell (two long waves and three shorter ones,
  `Lighting::water_swell`, at least 30%), riffles across the flow (short crests, long across
  it) and fine ripples (they bend the light into caustics and catch the sun), all carried along
  with the texture detail and the procedural caustics. The original meshes only say where water is and how high
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
  - actors moving through water make a wake each 60 Hz tick (`Renderer::actor_wake`, from
    their speed through the water: their movement less the flow there, with a little slack,
    so a log or a crate drifting with a stream makes none; the sim reads the flow map);
  - creatures (`Actor::wake`, from the host: scenery `ENEMY_NEW_FLAPPYTHING`, fish by their
    bubbles; else by model name: `bfly*`, `*fish*`): a butterfly's wings ruffle the water up
    to 45 units under it (weaker the higher it flies) and now and then it dips (a tiny ring);
    a fish leaves a small wake near the surface and its bubbles pop up through it;
  - the frog landing on a platform splashes at the platform, hopping off one lets it bob up
    with a smaller wave, and falling in makes a big splash (`App::water_tick`), and
  - the platform and the frog dip by a couple of units on a damped spring (render-only
    `DrawCtx::dips`, interpolated between ticks).
- **Floating:** platforms, and a frog standing on one (`DrawCtx::floats` → `Actor::float_at`
  → `Rec::anchor`), ride the generated surface where their anchor is (`float_lift` in `vs`):
  made to float at the original surface, they are moved to where the generated one is (2.5
  lower) and 1.5 into it, so their bottoms are in the water, rising and falling with the waves
  and swell and tilting with their slope (gentler than the water's own). On acid only with its
  waves (half the draft); over lava, or with no water under the anchor, not at all.
- **Thickness:** the simulation's waves move by the liquid (the flow map's z): water rings out
  far; acid's are slower and short-lived, and the velocity's own Laplacian (a viscosity)
  smooths away the fine ripples, so a splash makes a dimple and a broad, soft ring.
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

## Lava (Enhanced)

`gpu_mesh.rs` (`fn lava`, the obstacle map), `water.rs` (lava's waves, the obstacle map's
mips), `app.rs` (`water_tick`: burning). Opaque lava-textured meshes (`lavafall`, `00lava04`,
`anclava`) are a `Liquid::Lava` on the GPU mesh pass; they stay emissive (their groups light
what is about them) and the waves run on them.

- **Crust:** rafts of dark cooled crust (where a slow, warped noise field is high: mostly
  large, about 35 units across; smaller, torn pieces only against and behind what stands in
  the lava), cracked into plates (a Voronoi network, 16 units, whose seams glow; faint crazing
  up close), standing a little proud: dark cooled tops lit by the lights about them, a bevel
  catching the melt's glow from below. The melt glows incandescent (orange, yellow at its
  hottest, never white) at a raft's edge, bleeding a few units out. Laid out in world space, not the texture's coordinates:
  the art's lava faces do not all meet whole texture repeats apart, and a pattern in their
  coordinates breaks along their edges. Where the lava flows, the crust is carried along it
  like the water's ripples (two copies, each carried for 4 s and put back, crossfaded keeping
  the field's contrast, so rafts melt and form rather than fade); the flow is the flow map
  smoothed over a 32-unit lattice (each face slides its own way, some 30 degrees apart: a flow
  with steps in it tears the crust). Steep faces, fast flow, stirred lava and wakes melt the
  crust away. Detail finer than a few pixels fades out.
- **Melt:** the original texture's colours, brighter in slow rolls along the texture's slide
  (its coordinates, so they flow with it), with a slow heat pulse, a glossy skin (the light
  from above in a cave, else the sun, across the bumps) and a little Fresnel; no reflections.
- **Painted crust:** some textures already are crust (`00lava04`: dark plates between glowing
  cracks; `anclava`: dark blotches). `painted_crust` measures it once per mesh (the share of
  dark texels, `Rec::slide` w): such lava gets none of our crust, its own dark plates stand
  proud of its cracks (bump-mapped from the texture) and are lit as rock.
- **Waves:** lava's simulation is thick and slow (`c² = 0.012`, damping 0.992 per step,
  velocity viscosity 0.2: a broad swell about 40 units a second that lingers a second or so).
  Where it moves the lava bulges (the normal), glows hotter and its crust tears open, on the
  surface only (not on a volcano's lava-covered cone standing in it).
- **Falling in:** a burnt frog (`DEATHBY_FIRE` / `DEATHBY_INCINERATE`: a hot tile, mostly
  lava) sinks into it for a third of a second (`App::water_tick`: a splash each tick, heaviest
  first). The game itself is unchanged; only liquid near the frog's height moves, so a hot
  tile on dry land stirs nothing. Platforms landed on splash as on water, but only if the lava
  is within the splash's radius of them.
- **What stands in it** (the obstacle map, below): a hot seam where the melt touches it, a
  collar of darker crust right under it; upstream the crust piles up in folds that creep in
  (glowing between them), downstream it is torn away in a glowing trail some 40 units long,
  and around it the crust slips by to the side.

### Obstacle map

`WaterSim::obstacle_view` (Rg16Float, the surface map's window at half its resolution, 4 mips:
3, 6, 12, 24-unit cells; `Gpu::pre_passes` after the bed map): every GPU mesh draw but the
characters (skinned meshes) and the sky, seen from above, Max-blended. r: something solid at
the surface there (from half a unit under it to 40 units over it, the highest fading out;
liquids count only where they stand 1.5 units out of the surface: a volcano's lava-covered
cone), g: how high it stands (/ 64). Where the liquid stops at what stands in it, the surface
beside it (4.5 units out) is taken. Its mips are means of what is solid (a smooth field
reaching further at each level), sampled trilinearly (`obstacle_at` in the shader): a handful
of taps say how near something is, the way to it and whether it lies upstream. The water can
use it the same way (rivers bowing around rocks).

## Tests

`crates/f2-engine/tests/headless.rs`: `enhanced_water_takes_splashes` (a pond drawn by the water
pass; a splash makes visible waves), `sliding_water_is_a_stream` (the same pond sliding is
carried along). `lighting::tests::water_names`. Debug: `F2_DRAW_EVERY=1` draws every tick of a
headless run (the waves build up as in play), `F2_TEST_SPLASH=x,y,z,radius,strength,every`. `crates/frogger/src/main.rs`:
`flowing_acid_keeps_its_surface` (Subterranean 1's acid river).
