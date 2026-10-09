# Struct layouts in the retail USA `frogger2.exe`

MSVC x86 layouts (default packing) of the source structs, for the oracle
(`crates/f2fix/src/addrs.rs`), trace schema v2 (`tools/f2-trace`) and
breakpoint snapshots. Offsets were computed with
`clang --target=i686-pc-windows-msvc -Xclang -fdump-record-layouts` from the
PC branch of the source headers and then **confirmed against field accesses in
the exe** (Ghidra dump, functions named in `frogger2_usa.csv`). The globals'
addresses are in `frogger2_usa.csv`.

Types: `SVECTOR` = 3 × `short` + pad (8 bytes), `FVECTOR` = 3 × `long` 20.12
(12 bytes), `TIMER` = `{ushort time; ulong lastCount;}` (8 bytes), `fixed` = `long` 20.12.

## PLAYER (Common/frogger.h) — `player[4]` @ 0x036c1f00, sizeof 156 (0x9c)

| off | field | type | confirmed by |
|---|---|---|---|
| 0 | name[8] | char | |
| 8 | character | u8 | |
| 10 | frogon | short | |
| 12 | frogunder | short | |
| 14 | numSpawn | short | RunGameLoop `==25` |
| 16 | lives | short | HUD lives text |
| 18 | oldLives | short | |
| 20 | healthPoints | u8 | |
| 21 | deathBy | u8 | |
| 22 | inputPause | u8 | GameLoop epilogue |
| 24 | spawnTimer | short | ProcessCollectables (0x36c1f18) |
| 26 | spawnScoreLevel | short | ProcessCollectables (0x36c1f1a) |
| 28 / 29 | worldNum / levelNum | u8 | GameLoop STARTUP |
| 32 | frogState | ulong | |
| 36 | idleTime | fixed | RunGameLoop `idleTime -= gameSpeed` |
| 40 | idleEnable | u8 | |
| 41 | canJump | u8 | UpDateOnScreenInfo (0x36c1f29) |
| 42 | hasJumped | u8 | |
| 43 | hasDoubleJumped | u8 | |
| 44 | isSuperHopping | u8 | |
| 45 | isSinking | u8 | |
| 46 | extendedHopDir | u8 | |
| 48 | safe | TIMER | |
| 56 | stun | TIMER | |
| 64 | dead | TIMER | |
| 72 | isCroaking | TIMER | |
| 80 | autohop | TIMER | |
| 88 | quickhop | TIMER | |
| 96 | slowhop | TIMER | |
| 104 | jumpStartFrame | ulong | |
| 108 | jumpTime | fixed | |
| 112 | jumpSpeed | fixed | |
| 116 | jumpMultiplier | fixed | |
| 120 | jumpFwdVector | FVECTOR | |
| 132 | jumpUpVector | FVECTOR | |
| 144 | jumpOrigin | SVECTOR | |
| 152 | heightJumped | fixed | |

There is no `score` field (commented out in frogger.h); the HUD shows lives and coins (`numSpawn`).

## TRIGGER / TRIGGERLIST (Common/event.h) — `triggerList` @ 0x036c22e0

TRIGGER, sizeof 60 (0x3c): InitTriggerList allocates 0xf00 = 64 × 0x3c,
AllocTriggerBlock 0x3c8 = 8 + 16 × 0x3c.

| off | field | type |
|---|---|---|
| 0x00 | next | TRIGGER* |
| 0x04 | prev | TRIGGER* |
| 0x08 | data[4] | void* |
| 0x18 | event.func | void(*)(EVENT*) (UpdateEvents passes `t+0x18`) |
| 0x1c | event.data[4] | void* |
| 0x2c | func | int(*)(TRIGGER*) |
| 0x30 | flags | ushort |
| 0x34 | count | ulong |
| 0x38 | delay | ulong |

Flags: ONCE 1, DELAY 2, ALWAYS 4, FIRED 8, RISING 0x10, LOGIC 0x100, LOGIC2 0x200, FREE 0x8000.

TRIGGERLIST, sizeof 0x88: head 0x00 (TRIGGER), avail 0x3c (TRIGGER), base 0x78,
extra 0x7c, count 0x80 (short), alloced 0x82, blocks 0x84, garbage 0x86.
Active triggers: walk `head.next` until `&head`. AND/OR/NOT sub-triggers are
allocated (counted in `count`) but are on neither ring (`next == prev == NULL`).

## Script (Common/script.c)

`scriptFlags[8]` @ 0x0048a238 (SCRIPT_FLAGS_BYTES 8 = 64 flags, `[f>>3] |= 1<<(f&7)`),
`short scriptCounters[16]` @ 0x0048a240 (the retail exe keeps the `c < 0x11`
off-by-one check), `scriptBuffer` @ 0x036bba60 (5-byte header, version byte 3).

## SPECFX / SPECFXLIST (Common/specfx.h) — `sfxList` @ 0x036bb9c0

SPECFX, sizeof 136 (0x88) (InitSpecFXList `maxSpecfx * 0x88`). PC branch: there
is no `flags`/`deadCount`; `lifetime` is an absolute `actFrameCount`.

| off | field | type |
|---|---|---|
| 0x00 / 0x04 | next / prev | SPECFX* |
| 0x08 | normal | FVECTOR |
| 0x14 | vel | FVECTOR |
| 0x20 | scale | FVECTOR |
| 0x2c | origin | SVECTOR |
| 0x34 | rebound | PLANE2* |
| 0x38 | particles | PARTICLE* |
| 0x3c | sprites | SPRITE* |
| 0x40 | tex | TextureType* |
| 0x44 | follow | ACTOR* |
| 0x48 | act | ACTOR2** |
| 0x4c | type | short (FXTYPE_*) |
| 0x4e / 0x50 / 0x52 / 0x54 | start / end / angle / spin | short |
| 0x58 / 0x5c / 0x60 / 0x64 | speed / accn / tilt / gravity | fixed |
| 0x68 | lifetime | long |
| 0x6c | numP | long |
| 0x70 / 0x74 | fade / fadeAlpha | long |
| 0x78..0x7b | r, g, b, a | u8 |
| 0x7c | updateType | u8 |
| 0x80 / 0x84 | Update / Draw | fn* |

SPECFXLIST, sizeof 156: array 0, count 4 (short), stack 8, stackPtr 0xc (short),
lastAdded 0x10 (SPECFX**), head 0x14 (SPECFX). `p2List` (PLANE2LIST, PLANE2 = 0x1c) @ 0x036bb9b0.

## SPRITE / SPRITELIST (Common/sprite.h) — `sprList` @ 0x036c1ea0

SPRITE, sizeof 52 (0x34): next 0, prev 4, texture 8, pos 0x0c (SVECTOR),
scaleX 0x14, scaleY 0x16, flags 0x18 (short), r/g/b/a 0x1a..0x1d, offsetX 0x1e,
offsetY 0x1f, draw 0x20 (u8), angle 0x22, angleInc 0x24, arrayIndex 0x26 (short),
sc 0x28 (MDX_VECTOR, PC only).

SPRITELIST, sizeof 68: array 0, count 4 (short), stack 8, stackPtr 0xc (short),
head 0x10 (SPRITE). `maxSprites` @ 0x0047afec (512 on PC).
`partList` (PARTICLELIST, same shape, PARTICLE = 0x34, 512 entries) @ 0x036c2280.

## GARIB / GARIBLIST (Common/collect.h) — `garibList` @ 0x036c2460

GARIB, sizeof 32: prev 0, next 4, active 8 (char), type 9 (u8), sprite 0xc
(SPRITE*), fx 0x10 (SPECFX*), pos 0x14 (SVECTOR), dropSpeed 0x1c (short).

GARIBLIST, sizeof 44: array 0, head 4 (GARIB), blocks 0x24, count 0x26,
total 0x28, maxCoins 0x2a (short). `pauseFlag` @ 0x036c2440, `pauseTimer` (TIMER) @ 0x036c2448.

## BABY (Common/babyfrog.h) — `babyList[5]` @ 0x036c24e0

sizeof 24: isSaved 0 (int), fxColour[4] 4 (u8), idle 8, collect 0xc (int),
baby 0x10 (ACTOR2*), enemy 0x14 (ENEMY*). NUM_BABIES = 5; `numBabies` (map.c) is
the per-level count.

## TONGUE (Common/tongue.h) — `tongue[4]` @ 0x036bb880

sizeof 76 (0x4c) (InitTongues stride 0x13 dwords, 4 entries up to 0x036bb9b0):
source 0, target 8, pos 0x10 (SVECTOR), fwd 0x18, interp 0x24 (FVECTOR),
flags 0x30 (ulong), radius 0x34 (fixed, init 0x708000), progress 0x38 (fixed),
type 0x3c (u8), canTongue 0x3d (u8), tex 0x40, segment 0x44 (FVECTOR*, 12 entries),
thing 0x48 (void*).

## TRANSCAMERA / TRANSCAMERALIST (Common/Cam.h) — `tcList` @ 0x036c24c0

The source's `transCameraList` is `TRANSCAMERALIST tcList` in this build:
`{TRANSCAMERA *array; short count, blocks;}` (count at +4, blocks at +6).
TRANSCAMERA, sizeof 36 (0x24) (AllocCamBlock `num*0x24`):

| off | field | type |
|---|---|---|
| 0x00 | tile | GAMETILE* |
| 0x04 | dirCamMustFace | u8 |
| 0x05 | flags | u8 (0 STATIC_CAMERA, 1 FIXED_SOURCE, 2 LOOK_AT_FROG, 3 LOOK_AT_FROG_NOTILT, 4 LOOK_AT_TILE, 5 LOOK_IN_DIR) |
| 0x06 | camOffset | SVECTOR |
| 0x10 | camLookAt | FVECTOR |
| 0x1c | FOV | fixed (default 0x2d000) |
| 0x20 | speed | fixed (default 0x1000) |

CheckForDynamicCameraChange walks `i = count .. 1` (`cur = &array[i-1]`) and
applies entries whose `tile` is the frog's new tile. There is no "current
transition" global; trace v2 `camera.trans` is the highest index whose tile is
`currTile[0]`, i.e. the entry that function applies.

## Camera globals (Common/cam.c)

Initialised data sits in source order at 0x00471150.. (`cameraUpVect`,
`currCamSource` 0x471160, `currCamDist` 0x471170, `camSource` 0x471180,
`camOffset` 0x471190, `yFOVNew` 0x4711a0, `camLookOfs` 0x4711b0, `camDist`
0x4711b8 (SVECTOR, `.data` {0,6800,1920}), `camSpeed`..`camSpeed3`,
`transCamSpeedMult` 0x4711d0, `scaleV` 0x4711d4, `cam_edge_spacing`,
`cam_shake_falloff`); zero-initialised ones at 0x004891a8.. (`camControlMode`,
`currCamTarget` 0x4891b0, `camFacing[4]` 0x4891bc, `startCamFacing`,
`controlCamera` 0x4891d0, `fixedDir`/`fixedPos`/`firstPerson`/`fixedUp`
0x4891d1..d4 (char), `camSideOfs`, `lastTile[4]` 0x4891dc, `prevCamFacing[4]`,
`cam_shakiness` 0x4891fc, `playerFocus`); tentative definitions in the common
block at 0x036c24xx (`currCamBox`, `cameraBoxes`, `camTarget` 0x36c24a0,
`currCamOffset` 0x36c24b0, `tcList` 0x36c24c0, `camVect` 0x36c24d0).
Per-frog facing: `frogFacing[4]` @ 0x004896f0, `oldFrogFacing[4]`, `nextFrogFacing[4]` (frogmove.c).

## FOG (Common/sprite.h) — `fog` @ 0x00471b50

sizeof 8: r 0, g 1, b 2, mode 3 (u8), min 4, max 6 (short). `.data` = {120,120,160,FOG_OFF,995,5000}.

## Timers and HUD

`TIMER`s (`.time` u16 at 0, `.lastCount` u32 at 4): `scoreTimer` 0x036c1c40
(set to 90 by InitLevel; never read in retail), `modeTimer` 0x036c1c60,
`endLevelTimer` 0x036c1c68, `goTimer` 0x036c1908 (hud.c race start),
`demoTimeout` 0x036bc680, collect.c `pauseTimer` 0x036c2448. hud.c:
`handAngle` 0x036c1ab0 (`+= gameSpeed`, wraps at 0x3c000), `storeFrameCount` 0x0048994c.

## gameSpeed (0x00474070) users that matter per tick

RunGameLoop (`idleTime -= gameSpeed`), UpDateOnScreenInfo (HUD fades,
`handAngle`), UpdateFroggerPos, SlurpCamPosition (camera interpolation
`(gameSpeed<<12)/FMul(camSpeedN, transCamSpeedMult)`), the
`cam_shakiness` falloff, UpdateFrogTongue, RunBabyCollect,
ExecuteCommand (one `FMul(x>>4, gameSpeed)`), and unnamed 0x00411310, 0x004121f0,
0x00424dd0 (68 refs), 0x0041ebc0, 0x0042f410, 0x0042f4d0. With the oracle's fixed
timestep gameSpeed is always 4096.

## Not found / not present

- `levelTrophy` (game.h `extern ACTOR2 *levelTrophy`) has no definition in the source and no match in the exe.
- `PLAYER.score` does not exist (see PLAYER).
- `specFXList` is called `sfxList` in the source; `transCameraList` is `tcList`.
- `actualCamSource`/`actualCamTarget`, `camrotval*`, `controlcamspeed`, `camturnspeed` (cam.c) were not placed.
