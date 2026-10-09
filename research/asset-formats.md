# On-disk formats read by the PC game

Derived from the loader code in `frogger2-vss/teamSpirit/` (paths relative to it; `FP/` =
`FroggerProject/`, `MDX/` = `MDX Library/`) and **checked against real USA retail data** where
marked ✅. All integers little-endian, MSVC x86 natural alignment unless stated "packed".

| Status | Meaning |
|---|---|
| ✅ verified | Prototype parser in `research/prototypes/` consumed every file of that kind exactly |
| 🟡 partial | Header/structure confirmed on real data, some fields unvalidated |
| ⬜ code-only | Derived from source, not yet checked against data |

## 0. Paths, registry, generic loader

- Install dir from `HKLM\Software\Hasbro Interactive\Frogger2\InstallDir` (+ `\`) → `baseDirectory`
  (`FP/PC/Main.cpp:153-214`). Other values: `Resolution` (DWORD `x<<16|y`), `Fullscreen`,
  `VideoDevice`, `Language` ("English"/"Français"/"Deutsch"/"Italiano"/"US"), `Control 1..4`,
  `Keymap` (REG_BINARY 168 B = 56 × {player u8, buttonBit u8, DIK u8}, `FP/PC/controll.cpp:291-395`).
- CD found by volume label `FROGGER2` (`Main.cpp:112-143`); only `Video\` and `Gallery\` are read
  from CD.
- Dirs: `collision\ scenics\ objects\ textures\ maps\ savedata\ sfx\ DEMOS\` (+ `Design\` debug).
  Windows is case-insensitive; the port's VFS must be too.
- `fileLoad` → `mdxFileLoad` (`MDX/mdxFile.cpp:106-131`): reads whole file; **any file may be
  FLA2-compressed transparently**.

## 1. FLA2 compression — ✅

```
u32 magic 'FLA2' (0x32414C46)   u32 decompressedSize   u8 stream[] @8
```
LZSS (`MDX/mdxFile.cpp:62-103`): 4096-byte zeroed ring, write pos 0. Read `tag` byte; 8 items
MSB-first. Bit 0 → literal byte. Bit 1 → `c = byte`; `c == 0` ends stream; else
`off = 4096 - (((c & 0xF0) * 16) + nextByte)`, `len = (c & 0x0F) + 2`, copy `len` bytes from
`ring[(pos + off) & 4095]` (each output byte is pushed to the ring).
Verified on all 38 `textures/*.fla` + `maps/language.fla`: output size == header size.

## 2. Texture banks `textures/<Bank>.fla` — ✅

Banks: Ancient, City, Editor, Font, Garden, Halloween, Lab, Portraits, ProcData, Space, Subter,
Super, System, Titles. Loader `LoadTexBankFile` (`MDX/mdxTexture.cpp:680-755`), builder
`Frogger2 Tools/texturebanker`.
```
u32 numTextures
numTextures × {
  TEXTURE_HEADER (24 B): u32 flags (0 normal, 1 "ai_" alpha-from-blue, 2 "prc" procedural, 4 "ac_")
                         u32 crc   // CRC of lowercase "name.bmp"
                         i16 width, i16 height
                         char name[12]   // truncated, e.g. "00wate04.bm"
  u16 pixels[w*h]                 // X1R5G5B5, top-down; 0x7C1F (magenta) = transparent
}
u32 numAnims
numAnims × { u32 numFrames; u32 baseTexCrc; numFrames × { u32 frameCrc; i32 time } }  // time in frames, -1 = every frame
```
Verified: every bank consumed exactly (e.g. Garden 134 tex/4 anims, Halloween 195/9).
Pixel conversion rules: `MDX/mdxTexture.cpp:193-339`, `MDX/mdxD3D.cpp:631-734` (flag 1 → white
with A = blue5; flag 4 → ARGB4444 with A = avg of channels; non-power-of-2 width → plain surface).

## 3. Backdrops `textures/{title,loading,artwork00-20}.fla` — ✅

One `TEXTURE_HEADER` (24 B, width 640, height 480) + 640×480 X1R5G5B5 pixels = 614,424 B
decompressed. (`mdxLoadBackdrop` body is missing from the source; format confirmed from data.)
`artworkNN` = extras art gallery.

## 4. Fonts `textures/font/*.bmp`, `Font.fla` — 🟡

`InitFont(bmp)` (`MDX/mdxFont.cpp:85-300`): BMP strip, top-left pixel = transparent colour,
glyphs separated by empty columns, glyph order = `alphabet[]` (`mdxFont.cpp:66-68`: A-Z a-z 0-9
punctuation, Latin-1 accents). `Font.fla` contains `bigfont.bmp` 3212×34, `frogfont5.b…` 2615×12
etc. PC call site is in the missing `GameStartup()` — verify which file is used.

## 5. Object banks `objects/<bank>.dll` — 🟡

**Win32 PE DLLs containing only data** (generated C from `BankManager/drawlists.cpp`). Each
has an identical 13 KB CRT stub `.text`, one export `GetbankAddress`, whose code is
`55 8B EC B8 <imm32> 5D C3` (`push ebp; mov ebp,esp; mov eax,imm32; pop ebp; ret`). The imm32 is
the VA of an `MDX_OBJECT_DESCRIPTOR[]` array terminated by a null controller. Image base
`0x10000000`; all pointers are absolute VAs → translate `VA - ImageBase` via the section table
(or apply `.reloc`). **Never load/execute these DLLs.**

Loader: `LoadObjBank` (`MDX/mdxObject.cpp:795-847`), pointer fix-ups `RestoreObjectPointers`
(`:614-760`). Layouts (`MDX/mdxObject.h`; `MDX_VECTOR` = f32×3, `MDX_QUATERNION` = f32×4,
`MDX_MATRIX` = f32[4][4]):

- `MDX_OBJECT_DESCRIPTOR` { i32 objectId = CRC("<name>.obe"); MDX_OBJECT_CONTROLLER* }
- `MDX_OBJECT_CONTROLLER` { i32 objectId; MDX_MATRIX*; i16 objectSize; MDX_OBJECT* object;
  f32 radius; void* planes; MDX_ACTOR_ANIMATION*; void* drawList; void* vtx[2];
  MDX_MESH* LOCMesh; i16 isSkinned; i16 numVtx }
- `MDX_OBJECT` { u32 objId; u8 name[16]; i16 pad; u8 xlu; u8 numSprites; MDX_OBJECTSPRITE*;
  MDX_MESH*; void* drawList; f32* postMatrix; MDX_SKINVTX* effectedVerts; actor*; i16 numVerts;
  MDX_VKEYFRAME* scaleKeys, moveKeys; MDX_QKEYFRAME* rotateKeys; i16 numScale/Move/RotateKeys;
  MDX_VECTOR* bBox; tex* phong (CRC); i16 flags; i16 lastRKey; MDX_OBJECT* children, next;
  i16 lastMKey, lastSKey; MDX_MATRIX objMatrix }   (name `gl_*` → glow)
- `MDX_MESH` { i16 numFaces, numVertices; MDX_VECTOR* vertices; i16[3]* faceIndex;
  D3DTLVERTEX* (runtime); i16[2]* faceTC (3/face, UV = v × 0.000975); MDX_VECTOR* vertexNormals;
  MDX_QUATERNION* gouraudColors (RGBA f32, 3/face); char* faceFlags; tex** textureIDs (CRC/face) }
- Keyframes `MDX_VKEYFRAME {vec; i32 time}`, `MDX_QKEYFRAME {quat; i32 time}`;
  `MDX_ACTOR_ANIMATION` → `MDX_ANIMATION {i16 start, end; f32 speed; i32 loop}`;
  `MDX_OBJECTSPRITE {tex* (CRC); sprite*; i16 x,y,z,sx,sy,ox,oy,flags}`.

Verified on data: descriptor walk works on `frogger.dll` (6 objects, `frog` 242 faces/132
verts), `garden1.dll` (20 objects incl. `world`, `jetty`, `wat_*`), `gamegeneric.dll` (9);
descriptor IDs equal `CRC("frogger.obe")`, `CRC("world.obe")`, `CRC("jetty.obe")`. Some mesh
counts look implausible (e.g. `wwheel` 14,291 faces) → field offsets after `name` must be
validated in Phase 1 (alignment of `u8 xlu/numSprites` vs pointer).

Special objects: `world.obe` = level geometry, `backdrop.obe` = sky (`FP/Common/layout.c:598`,
`FP/PC/banks.cpp:517-551`). `.obe`↔`.psi` names interchanged by `stringChange`
(`FP/Common/general.c:91-131`).

## 6. CRC — ✅

`UpdateCRC` (`MDX/mdxCRC.cpp`): CRC-32, poly `0x04C11DB7`, MSB-first, init 0, no reflection,
no final XOR. Special case: string starting with byte 0xFF returns global `objectCRC`. Used for
textures (`"name.bmp"`), objects (`"name.obe"`), samples (`"name"` without `.wav`).

## 7. Scenics `scenics/<LEVEL>.sce` — 🟡

`LoadScenics` (`FP/Common/map.c:34-353`).
```
i32 count
SCENIC[] (44 B): i32 next (byte offset from first, -1 end); char name[16] ("world.psi"…);
                 SVECTOR pos (i16 x,y,z,pad; ×SCALE 10 at load); IQUATERNION rot (i32×4)
```
Header confirmed on `garden1.sce` (count 18, first `world.psi`, next = 44).
Prefixes `wat_`, `slu_`, `lea_`, `xxa_` and 5th char `f` change render flags (`FP/PC/banks.cpp:533-585`).

## 8. Collision `collision/<LEVEL>.col` — 🟡

`LoadCollision` (`FP/Common/map.c:688-768`); `SetupCollisionTiles` `:410-490`, `SetupCameraBoxes` `:492-528`.
```
i32 tileCount; i32 playerStartTile[4]; i32 numBabies; i32 babyStartTile[numBabies]
GAMETILE[tileCount] (68 B): i32 adj[4] (N,S,E,W or -1); u32 state (TILESTATE_*, conveyors 0x20-0x2F);
                            SVECTOR centre (×10), SVECTOR normal, SVECTOR dirVector[4]
optional: i32 numPlanes; CAM_PLANE[] {i32 nx,ny,nz (fixed); i32 k (×10); u32 status} (20 B)
          i32 numBoxes;  CAM_BOX[]  {i32 numPlanes; i32 firstPlaneIndex} (8 B)
```
Header confirmed on `garden1.col` (726 tiles, start 0x231, 5 babies).

## 9. Entities `maps/entity-<world>-<level>.dat` — 🟡

`LoadLevelEntities` (`map.c:355-397`) → `MemLoadEntities` (`FP/Common/memload.c:107-485`).
**Packed byte stream.** `WORD`=i16, `INT`=i32, `FIXED`=i32>>4.
```
u8 version (=16; |0x40 = "fast": positions are tile indices)     ← garden1: 0x50 ✓
i16 totalPathNodes; i16 numPaths
 path: i32 numNodes; node: (fast ? i32 tile : i32 x,y,z (>>16, ×10 → nearest tile));
       FIXED offset(×10), offset2(×10), speed; i32 waitTime; i32 sampleUID
i16 totalEnemies, totalPlatforms, totalGaribs, totalCameraCases, totalPlaceholders; i16 itemCount
 item: u8 type (0 enemy,1 platform,2 garib,3 camera case,4 placeholder)
  0/1/4: u8 len; char model[len]; i32 flags; i32 uid; FIXED scale; FIXED radius(×10); i32 animSpeed(>>8);
         FIXED value1; u8 facing; i32 objFlags; u8 depthShift; u8 dffClipping; u8 pad[2]; i32 effects;
         i32 startNode; i16 pathIndex
  2:     u8 garibType; (fast ? i32 tile : i32 x,y,z); FIXED heightOffset
  3:     i32 flags (hi16 dir, lo8 flags); i32 x,y,z; FIXED fov; i32 speed(>>16); i32 target (3 s8 dir <<4);
         i32 numNodes; nodes (tile | xyz)
```
World/level numbers = `WORLDID_*`/`LEVELID_*`; mapping to banks in `worldVisualData` (`layout.c:329+`).

## 10. Event scripts `maps/script-<world>-<level>.fev` — ⬜

`LoadLevelScript`/`InitLevelScript` (`FP/Common/script.c:1269-1385`); compiler `Etc/`.
`u8 version (=3); i32 outerBlockSize; block`. Block = `i16 numCommands` + commands. Command = `u8`
opcode (`FP/Common/codes.h`): `EV_ON`(64) `u8 flags, i32 size, block`; `EV_IF`(65) trigger,
`i32 size, block`; 0–50 = actions with fixed operands (`ExecuteCommand` `script.c:645-1260`).
Triggers (`LoadTrigger` `:502-640`): `TR_AND/OR`(64/65) two sub-triggers, `TR_NOT`(66) one,
`TR_WAIT` FIXED seconds, `TR_FROGONTILE` u8 frog + i16 tile, …
`maps/sscript*` — purpose TBD (likely shared/global scripts).

## 11. Sound banks `sfx/*.wus` — ✅

`LoadSfx`/`LoadSfxSet` (`FP/PC/pcaudio.c:156-243`), builder `Frogger2 Tools/soundbanker`.
```
u32 numSamples          // NB: loader reads only (signed char)byte0
numSamples × { u32 pcmLength; u32 uid = CRC(lowercase name w/o ".wav"); u32 loop ("lp_" prefix);
               WAVEFORMATEX (18 B packed; cbSize is junk); u8 pcm[pcmLength] }
```
Verified: all 19 banks consumed exactly; all PCM 16-bit mono at 22050 / 11025 / 11050 Hz.
Banks: generic, multi, per-world (garden, ancients, space, city, sub, lab, halloween, superretro,
frontend) and per-character voice banks (frogger, lillie, tad, treeby, wart, tank, roobie, swampy).

Animation → SFX map `maps/sfxanim<world><levelhex>.sam` ⬜ (`LoadSfxMapping`,
`FP/Common/general.c:330-500`): header-less records `i32 uid; i32 type`; type 4 with uid <
tileCount → one `i32 sampleUID` (ambient at tile); else N × `i32 sampleUID`, N = 43 (frog), 17
(multi frog), 4 (enemy), 1 (scenic).

## 12. Language `maps/language.fla` — ✅ (container) / 🟡 (semantics)

FLA2-compressed **tab-separated text**, CRLF rows. Header row:
`LANG_NAME \t Comments \t English \t Français \t Deutsch \t Italiano \t US`. Each row: string ID,
comment, then 5 language columns (Latin-1). Parser: `gameTextInit` (`FP/PC/isllocal.c:32-84`).
EU and USA files differ — diff them in Phase 1. String IDs: `FP/Common/lang.h`.

## 13. Music — ✅

Red Book tracks 2–12 via MCI (`pcaudio.c:128-145, 940-1050`). See `retail-media.md` §2.

## 14. Video — ✅

`<CD>\Video\<name>.bik` (`FP/PC/video.cpp:170-190`), names from `FP/Common/story.c:28-44`:
`HASBROS`, `BLITZS`, `01S`…`12S` (frame counts in table). Bink 1 `BIKi`.

## 15. Save game `savedata/savegame.dat` — 🟡

`LoadGame`/`SaveGame` (`FP/PC/banks.cpp:441-497`), struct `FP/Common/savegame.h:39-61`, logic
`savegame.c:36-180`. 2112 B:
- header 64 B: u8 version(=1), versioncheck, story_world, story_level; char name[8]; u8 language,
  controller, musicVol, sfxVol, char_unlock, lives; u8 res[2]; u8 flags[32]; u8 res[12]
- 64 × LEVELINFO (32 B): u8 flags (1 open, 2 completed, 4 training done, 8 beaten, 16 hard,
  32 easy), u8 garibs, u8 res[2], char winner[8], u32 bestTime, u8 res[16]

## 16. Demo recordings `Demos/record-<w>-<l>.key` — 🟡

`FP/Common/levplay.c:52-60`, writer `FP/PC/controll.cpp:250-280`. Records of 3 × u32
`{frame, buttonMask, player}`. Retail ships `record-0-0`, `record-1-0`, `record-6-1` — **these
are our first parity test vectors.**

## 17. Misc

- `frogkeys.map`: 56 ints, legacy.
- Cheat probe: existence of `m\u\l\l\a\r\d\is_my_daddy.txt` (`layout.c:968`).
- `STAKFILES\*.STK` packs (`FP/Common/flatpack.c`) are **PSX-only**.

## 18. Prototype scripts

`research/prototypes/` holds the throwaway Python used to verify the above (no game data). The
Rust parsers in `f2-formats` supersede them.
