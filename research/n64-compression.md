# N64 asset pipeline and compression (plan phase 1C)

Measured on the USA PC data (`game-data/usa`), 2026-10-10. Everything here is produced by
`tools/f2-n64pack` from the user's own data; no derived bytes are committed. Re-run:

```sh
cargo run --release -p f2-n64pack                 # build target/n64/pack + budget table
cargo run --release -p f2-n64pack -- --bench      # codec comparison → target/n64/bench.md
cargo run --release -p f2-n64pack --example model_streams   # per-stream model diagnostics
```

## 1. Final cartridge budget (64 MiB = 67,108,864 B)

Exact output of `f2-n64pack` (DFS image measured with libdragon `mkdfs`):

| Asset class | Files | From (retail bytes) | Packed bytes | MiB | Plan target |
|---|---:|---:|---:|---:|---:|
| boot + code (reserve, `--reserve-mib`) | — | — | 2,621,440 | 2.50 | 2.0 MB |
| models (aPLib) | 71 | 18,971,136 | 2,106,693 | 2.01 | 5.0 MB |
| textures + fonts + backdrops (aPLib) | 76 | 7,353,516 | 2,753,252 | 2.63 | 6.0 MB |
| levels (F2Z + 64 KiB dictionary) | 245 | 2,946,306 | 386,176 | 0.37 | 1.5 MB |
| sound effects (all VADPCM) | 418 | 43,304,730 (unique PCM) | 11,118,388 | 10.60 | 8.0 MB |
| music (Opus 99 kbps stereo) | 11 | 268,187,284 | 18,499,810 | 17.64 | 7.5 MB |
| FMV (H.264 320×240 CRF 14 + Opus 48 kbps) | 28 | 121,241,408 | 19,310,600 | 18.42 | 25.0 MB |
| DFS overhead | | | 51,501 | 0.05 | |
| **total** (DFS image 54,226,420 B + code reserve) | | | **56,847,860** | **54.21** | 64 MiB |
| headroom | | | 10,261,004 | 9.79 | |

With `--codec models=f2z --codec textures=f2z` (F2Z smallest everywhere) the total is
53.65 MiB: 1,727,831 B models and 2,533,611 B textures (−0.6 MB) at 1.2–1.6 MB/s decode.

Every class sits at its **quality ceiling**, not at a size compromise:
- models, levels and the converted textures are **lossless** with respect to what the game
  reads (textures: 1479 of 1500 bit-exact to the PC upload; see §4.2);
- music is at audioconv64's Opus maximum (`--wav-resample` is capped at 48000 → 99 kbps
  stereo); FMV video at the N64 H.264 decoder's bitrate cap (videoconv64 maxrate
  `300·24/fps` kbps = 288 kbps at 25 fps) with the best CRF (14);
- the fill step then spent the remaining space on quality: all sound effects VADPCM (no Opus,
  +4.55 MB) and FMV audio 48 kbps mono / 96 kbps stereo (+0.97 MB).

The remaining ~10 MiB cannot buy quality with the current libdragon codecs; it is left as
margin for the code (the ELF size is not known yet; the reserve is 2.5 MiB) and saves.
Options if the runtime wants it: lossless 64×64 multiplayer portraits by splitting
triangles at TMEM tile edges (§4.2), Opus for FMV audio at 51 kbps (mono maximum).

The packer fails (`exit 1`) when DFS image + code reserve exceeds the cartridge.

## 2. Codec comparison per asset class

Each codec compresses every file separately (as stored in the pack); sizes include
per-file headers; F2Z rows with a dictionary include the (compressed) dictionary.
`raw` = the runtime bytes the game/renderer needs (levels: retail files, language text
FLA2-decompressed; models: packed F2MB banks; textures: converted NTX1 files).

| Codec | Levels (2,990,453) | Models (8,771,888) | Textures (5,860,320) |
|---|---:|---:|---:|
| mkasset -c 1, LZ4 (256 KiB window) | 823,175 (0.275) | 2,804,231 (0.320) | 3,212,312 (0.548) |
| mkasset -c 2, aPLib | 566,210 (0.189) | 2,106,693 (0.240) | 2,753,252 (0.470) |
| mkasset -c 3, Shrinkler | 428,079 (0.143)¹ | 1,746,440 (0.199) | 2,458,967 (0.420) |
| zstd -19 --ultra (host reference) | 470,858 (0.157) | 1,917,807 (0.219) | 2,643,403 (0.451) |
| zstd -19 + trained 64 KiB dict (incl. dict) | 509,803 (0.170) | 1,860,049 (0.212) | 2,665,151 (0.455) |
| xz -9e (.lzma) (host reference) | 394,410 (0.132) | 1,642,080 (0.187) | 2,392,543 (0.408) |
| brotli -q 11 -w 24 (host reference) | 439,281 (0.147) | 1,732,844 (0.198) | 2,465,930 (0.421) |
| xz -9e on the F2Z-transformed bytes | 349,443 (0.117) | 1,635,928 (0.186) | 2,382,219 (0.406) |
| **F2Z** plain (no transform, no dictionary) | 509,738 (0.170) | 1,943,197 (0.222) | 2,603,102 (0.444) |
| **F2Z** + structure transforms | 427,767 (0.143) | 1,915,521 (0.218) | 2,577,501 (0.440) |
| **F2Z** + transforms + 32 KiB class dictionary | 389,935 (0.130) | 1,820,571 (0.208) | 2,540,659 (0.434) |
| **F2Z** + transforms + 64 KiB class dictionary | **388,316 (0.130)** | 1,781,263 (0.203) | 2,538,404 (0.433) |
| **F2Z** + transforms + 128 KiB class dictionary | 387,907 (0.130) | **1,727,831 (0.197)** | **2,533,611 (0.432)** |

¹ Shrinkler aborted ("internal error") on one level file, counted at raw size.

Host F2Z decode throughput (one x86-64 core, Rust release build): 320–420 MB/s on levels and
models, 215–230 MB/s on textures. VR4300 numbers: §5.

On ratio, F2Z (smallest setting) beats Shrinkler, the best libdragon ratio, on levels (−9 %)
and models (−1 %) and is within 3 % on textures; it is 31 % / 18 % / 8 % smaller than aPLib.
The remaining distance to xz (adaptive binary range coding, far too slow for the VR4300) is
2–6 %. **Decode speed on the VR4300 decides the per-class winner (§5):**

| Class | Winner | Why |
|---|---|---|
| models | **aPLib** (mkasset -c 2) | best ratio among codecs decoding ≥ ~3.5 MB/s: 24.7 % at 5.6 MB/s incl. ROM read; F2Z-small 21.5 % at 1.55 MB/s, F2Z-fast 31 % at 5.0 MB/s |
| textures | **aPLib** | 59.6 % at 3.5 MB/s; F2Z-small 54 % at 0.9 MB/s, F2Z-fast 68 % at 5.7 MB/s |
| levels | **F2Z, smallest** (64 KiB dictionary) | 13 % vs aPLib 19 % / Shrinkler 14 %; a level's files are ~70 KB decoded → ~40 ms even at 1.8 MB/s |

The packer stores each file with its class codec (`--codec CLASS=f2z|lz4|aplib|shrinkler|none`
overrides); the loader dispatches on the magic (`F2Z1` / libdragon `DCA`).
Dictionary sizes: models/textures 128 KiB, levels 64 KiB.

## 3. F2Z design

Crate `crates/f2-n64z` (format spec in `lib.rs`, decoder `no_std`, allocation-free, 32-bit
integer ops only, bytewise reads, no unaligned access; encoder + trainer behind feature
`encoder`).

- **LZ77, optimal parse.** Hash-chain match finder (3-byte hash, 256 candidates, nice length
  273); forward dynamic programming over every position with exact symbol prices
  (1/16 bit) for literals, literal-run codes, match-length codes and offset codes; three
  repeat offsets (LZMA-style move-to-front); minimum match 3 (2 for repeats); a match longer
  than 273 is taken immediately. Three passes per block: each re-prices with the previous
  parse's statistics.
- **Entropy coding: static rANS** (32-bit state, byte renormalisation, 12-bit
  probabilities): 4 streams per block (literals, LL codes, ML codes, OF codes) plus a raw
  bit stream for the extra bits of the log-bucket codes (16 direct LL/ML codes, 4 direct
  distance codes, then 2 codes per power of two). Per stream and block the encoder picks the
  cheapest of: dictionary default table, explicit table (LEB128 frequencies with zero runs),
  single symbol, unused.
- **Literal modelling** (decided per block by exact cost):
  - *context modes* — the literal table is selected by a class of the previous byte
    and/or the output position mod 2/4 (10 modes, ≤ 8 tables, `litctx.rs`); both are known
    to the decoder independently of the parse, so the optimal parser prices them exactly;
  - *matched literals* — the first literal after a match is coded as `lit XOR byte[rep0]`
    with its own table (a static-table version of LZMA's matched literal; the parser tracks
    the state).
- **Structure transforms** (size-preserving, inverted after LZ): byte-plane transposition of
  record arrays, per-field delta + zigzag (1/2/4-byte words, either endianness). The packers
  describe their files (collision tiles 68 B, camera planes, scenic records 44 B, demo keys
  12 B, sfx-anim tables, NTX descriptor/entry/TLUT sections, model streams) and the encoder
  keeps the smallest of {described, plain}.
- **Shared dictionaries per class.** Content trained COVER-style (d-mers scored by the
  number of *other* files containing them, best 256-byte segment per epoch, covered d-mers
  zeroed, highest score last); default entropy tables = per-file-averaged distributions of
  the class compressed with that content. Matches may reach into the dictionary, which
  logically precedes every file; dictionary files are themselves F2Z-compressed in the pack.

What each step bought (bytes, F2Z with 128 KiB dictionary unless noted):

| Step | Levels | Models | Textures |
|---|---:|---:|---:|
| LZ + order-0 rANS, no transform, no dict | 523,024 | 1,980,788 | 2,679,513 |
| + structure transforms | 424,497 | 1,978,428 | 2,660,291 |
| + class dictionary (64 KiB levels) | 382,733 | 1,957,863 | 2,620,869 |
| + literal context modes | 389,484² | 1,953,482 | 2,569,264 |
| + matched literals | 387,356² | 1,948,903 | 2,549,113 |
| + 4-byte-aligned model streams, per-stream transform choice | 388,316² | **1,727,831** | **2,533,611** |

² Levels lose ~5 KB from the larger default literal tables in the dictionary; the
per-file streams themselves got smaller. The model step is large because the position
contexts only work when 2/4-byte fields sit at aligned offsets, and because the earlier
default (delta/planes on vertex/UV/key streams) was measured to *hurt* (vertex order is not
spatially coherent): per-stream diagnostics below.

Per model stream (all 71 banks concatenated per stream, before the alignment change):

| stream | raw | F2Z | F2Z planes | F2Z delta | xz |
|---|---:|---:|---:|---:|---:|
| VERT (i16) | 703,404 | 246,041 | 276,197 | 327,566 | 208,867 |
| FACE (u16 zigzag delta) | 882,900 | 192,951 | 177,824 | 209,458 | 174,215 |
| UV (i16) | 1,765,800 | 106,022 | 118,351 | 167,521 | 80,605 |
| NORMAL (i8) | 181,989 | 82,977 | — | — | 79,757 |
| COLOR (u8) | 1,765,800 | 189,049 | — | — | 151,277 |
| DECV (i32 move/scale keys) | 842,532 | 116,013 | 140,102 | 168,652 | 100,710 |
| DECQ (i32 rotation keys) | 1,093,984 | 131,172 | 149,604 | 144,851 | 113,183 |
| DECM (i32 misc floats) | 513,564 | 127,892 | 237,596 | 248,786 | 100,930 |
| TIME (i32 key time deltas) | 554,340 | 17,535 | 12,581 | 14,868 | 12,983 |
| META (varints) | 60,224 | 10,871 | — | — | 9,975 |

### Decoder resources
- `Decoder`: 24 rANS tables of 1.3 KiB (8 literal contexts + matched literal + 3 code
  streams, for the block and for the dictionary defaults) ≈ 31 KB, reusable across files.
- Output buffer `raw_size`; a second `raw_size` scratch buffer only for files with
  transforms (`Header::needs_scratch`). Dictionary content (≤ 128 KiB) + its file.
- Per literal: one table load for the context, one rANS step (slot lookup, one 32×32
  multiply, renormalisation every ~1–2 symbols); matches are 8-byte chunk copies. Token
  blocks: byte reads, 16-byte literal wild copies.

## 4. Formats chosen per class

### 4.1 Models — `models/<bank>.mdl` ("F2MB", lossless)
`tools/f2-n64pack/src/models.rs` (spec in the module docs, encoder + reference decoder).
A columnar serialisation of `f2_formats::objbank::ObjectBank`: 18 homogeneous streams
(varints, names, ids, i16 vertices, zigzag-delta face indices, UVs, i8 normals, u8 colours,
face flags, per-mesh texture palette indices, decimal keys, raw floats, time deltas, skin
indices, sprites, animations), each 4-byte aligned.

Finding: **no quantisation is needed** — the bank tool printed its C source with
`printf("%f")`, so every float is a 6-decimal number. Verified on all 71 banks: every
vertex and bounding-box value is an integer (±6610), every colour is exactly
`round6(b/255)` for a byte `b`, 99.7 % of normal components are `round6(i/255)`, all keys
and matrices are `n / 10^k` with `n` fitting i32. Each float array is stored in the smallest
exact integer form (checked value by value, raw IEEE bits as fallback), and decoding
reproduces every `f32` **bit-for-bit** (test `real_banks_round_trip` checks all 71 banks).
Gameplay-relevant data (vertices → `FindActorCentre`, keyframes, bounding boxes, sprite
placement, animation table incl. the past-the-end entry) is therefore exact by construction.
19.0 MB of DLLs → 8.77 MB packed → 1.73 MB F2Z.

The payload is a layer under the codec: the RSP renderer's resident fixed-point mesh format
(`crates/f2-n64fmt`, phase 2R) can replace or complement F2MB as the payload without
touching F2Z — `RawAsset` carries the bytes plus optional transform hints.

### 4.2 Textures — `textures/<bank>.ntx`, `textures/shared/s<hash>.ntx`, `textures/backdrop/*.ntx` ("NTX1")
`tools/f2-n64pack/src/textures/` (spec in `mod.rs`). 1500 bank textures → 1269 unique
images (231 duplicates in 39 shared pools, one per exact set of banks using them);
CI8 1049, CI4 127, RGBA16 83, I4 21, I8 12; 20.4 MB of 16-bit pixels → 5.86 MB.
1479 bit-exact to what the PC uploads (incl. the transparent-texel colour bleed); 16 lossy
(PSNR min 27.6 dB / mean 34.9 dB: 64×64 multiplayer portraits, book pages, swamp troll that
only fit TMEM as CI4) + 2 downscaled (`page256a/b`, `00wate04`). Every UV-mapped texture
fits one 4 KB TMEM load (CI: 2 KB data + TLUT); 126 2D-only images are split into
TMEM tiles. Backdrops 640×480 → 320×240 with Floyd–Steinberg dither, 17 RGBA16 and 6
per-tile-palette CI8 (40.8–43 dB). Fonts carry a precomputed `InitFont` glyph table.

### 4.3 Levels — retail bytes under retail paths
`maps/*.dat|*.fev|*.sam`, `collision/*.col`, `scenics/*.sce`, `demos/*.key`, and
`maps/language.fla` FLA2-decompressed (the game's `fileLoad` accepts both). 2.95 MB →
388 KB.

### 4.4 Sound effects — `sfx/NNNN.wav64` + `sfx/index.bin` ("F2SX")
`tools/f2-n64pack/src/audio.rs`, `audio/index.rs`. 19 banks, 443 entries → 417 unique
samples (PCM 46.57 → 43.30 MB, −7 %); 53 uids map to different audio in different banks
(kept per bank by the index). All VADPCM (huffman) in the final pack: 11.1 MB. The codec rule
(Opus 32 kbps for mono samples ≥ 2 s when ≤ 0.6 × VADPCM: 6.56 MB total) is used only when
space is short. Loop flags kept (whole-sample loops). Source rates unchanged.

### 4.5 Music — `music/trackNN.wav64`
Opus stereo via audioconv64 (bitrate = 3000 + `--wav-resample`·channels). Sizes for the
11 tracks (1520.3 s): 32 kbps 6.23 MB, 48 kbps 9.37 MB, 64 kbps 12.27 MB, 96 kbps 17.90 MB,
99 kbps (maximum) 18.50 MB; prediction within 0.05 % of real encodes.

### 4.6 FMV — `video/NAME.h264` + `video/NAME.wav64`
H.264 (libdragon's decoder: baseline + weighted prediction, CAVLC) chosen over MPEG-1:
2.5–4× smaller at equal SSIM (`05s`: H.264 348 kB SSIM 0.952 vs MPEG-1 1349 kB 0.928;
`12s`: 1689 kB 0.938 vs 5539 kB needed for 0.942). 320×240, 25 fps; quality ladder CRF
36 → 14 searched by bisection (max-quality total 15.96 MB video). Audio Opus 48 kbps mono
(blitzs 96 kbps stereo) after the fill step.

## 5. Decode speed on the VR4300

Baselines measured by the platform work (`research/n64-performance.md` §(d), `asset_load`
of a 1 MiB synthetic level-like file from ROM, ares v148): none 5.06 MB/s, LZ4 6.17 MB/s,
aPLib 3.95 MB/s, Shrinkler 0.76 MB/s.

All numbers below: ares v148, release + LTO ROM, Garden 1 working set staged by
`tools/n64/stage-f2z-probe.py` (3 model banks, garden + backdrop textures, collision, scenics,
entities: 652,647 B decoded), each file also re-compressed with `mkasset -c 0..3 -w 256` so
every codec decodes identical bytes in the same run; every output checked against the host
FNV-1a (all match). Probe code: `crates/frogger-n64/src/f2z_probe.rs`; method and the
per-file table: `research/n64-performance.md` (e). MB/s = decoded bytes per second.

### 5.1 libdragon codecs on the same bytes (`asset_load` = ROM read + decompress)

| | stored | MB/s |
|---|---:|---:|
| stored (c0) | 100 % | 5.05 |
| LZ4 (c1, MIPS assembly) | 45.7 % | **7.77** |
| aPLib (c2, MIPS assembly) | 36.9 % | **4.59** |
| Shrinkler (c3) | 32.0 % | 0.79 |

### 5.2 F2Z along its space–speed curve (decode alone; ROM read adds ~19 cycles per stored byte)

| F2Z setting | stored | decode MB/s | read + decode MB/s | models | textures | collision | maps |
|---|---:|---:|---:|---|---|---|---|
| smallest (`--size-weight inf`), rANS blocks, original decoder | 32.6 % | 0.79 | — | | | | |
| smallest, optimised decoder (bit-exact) | 32.6 % | 1.23 | 1.13 | 21.5 %, 1.55 | 54.4 %, 0.89 | 8.5 %, 1.84 | 14.6 %, 1.77 |
| weight 60 (mixed token/rANS blocks) | ≈ 39 % | ≈ 4.1 | ≈ 3.1 | 28 %, 4.05 | 64 %, 4.04 | 20 %, 4.27 | 19 %, 4.43 |
| weight 19 = minimum load time (mostly token blocks) | 44.2 % | **5.23** | 3.55 | 31 %, 5.02 | 68 %, 5.67 | 22 %, 5.03 | 20 %, 4.74 |

(class cells: stored %, decode MB/s.) Dictionaries are decoded once at boot (smallest:
332 KB in 310 ms; weight 19: 77 ms) and stay resident while their class is being loaded.

### 5.3 What limits F2Z on the VR4300, and what was done
- ares cost model (calibrated in the probe ROM): ~1 cycle per simple instruction, **~45 cycles
  per D-cache miss** (8 KiB direct-mapped, 16-byte lines). A static-rANS literal costs
  ~125–155 cycles and a sequence (three rANS symbols + extra bits) ~300–430 cycles, almost all
  of it table misses. That caps any rANS-literal codec at ~1–2 MB/s on this CPU, whatever the
  ratio.
- Decoder work (stream format unchanged): 1.3 KiB bucket tables instead of 5 KiB slot tables
  (Decoder 123 KB → 31 KB), inlined hot paths, 8-byte match copies, tiled inverse transforms:
  0.79 → 1.23 MB/s.
- **Space–speed coding** (format extension, `TOKEN_BLOCK`): a block can instead be
  byte-aligned tokens (LZ4-style nibble lengths, raw literals, 16-byte wild copies) keeping
  F2Z's repeat offsets, LEB128 distances into the shared dictionary and the structure
  transforms. The optimal parser minimises `bits + decode cycles / w` with a per-block-type
  cycle model, and each block keeps the type with the lower `cycles + w × bytes`; transform
  candidates are charged their measured 21 cycles/byte. `w ≈ 19` (cycles to read one byte
  from ROM) minimises total load time.
- Result: F2Z spans 1.2–5.2 MB/s, but at each point **libdragon's hand-written MIPS LZ4/aPLib
  decoders are on or beyond the frontier** (aPLib 36.9 % at 4.59 MB/s incl. read vs F2Z 39 %
  at ~3.1; LZ4 45.7 % at 7.77 vs F2Z 44 % at 3.55). F2Z's win is ratio where speed is
  irrelevant (small, per-level data) and the Shrinkler niche (same ratio, 1.5× faster).
- Next steps if F2Z should own the fast end: an assembly token decoder (the Rust one is
  ~17 cycles/byte; LZ4's assembly ~12 including the read), overlapping decode with PI DMA as
  `asset_load` does, and fewer/smaller rANS tables (≤ 2 literal contexts, 10–11-bit
  probabilities) for the rANS blocks.

## 6. What the N64 runtime must implement
0. **Loader dispatch** on the first bytes: `F2Z1` → F2Z, `DCA` → libdragon `asset_load`
   (models and textures by default), anything else = stored raw.
1. **F2Z loader** (levels by default): `f2_n64z::header` → allocate `raw_size` (+ scratch if
   `needs_scratch`) → `Decoder::decode(src, Some(&dict), out, tmp)`. Keep one `Decoder`
   (≈31 KB, heap). Load `dict/<class>.f2zd` (F2Z-compressed without a dictionary) when a
   level load starts, `Dict::parse` it, free it afterwards (or keep the 64 KiB levels one).
   Wrong/missing dictionary → `Error::WrongDict(id)`; corrupt input never panics.
2. **Models**: decode F2MB to `ObjectBank` (reference decoder `models::decode`, std-only
   today; port to `no_std + alloc` next to the phase-2R mesh format), or convert at pack time
   to the RSP mesh format once `f2-n64fmt` exists.
3. **Textures**: NTX1 reader + rdpq upload rules (combiner per texture kind, TLUT ranges,
   tile grids for 2D images, UV scale for downscaled textures, glyph tables) per
   `textures/mod.rs`.
4. **Audio**: `F2SX` index → `wav64_open("rom:/sfx/NNNN.wav64")`, mixer channels with
   volume/pan from `f2-audio`; music streamed (`wav64` Opus), loop per `PlayMusic`.
5. **FMV**: `fmv_play("rom:/video/NAME.h264")` (picks up `NAME.wav64`); ~1 MB decode buffers.
6. **Paths**: levels keep retail relative paths (`maps/…`, `collision/…`), so the existing
   `DataSource` lookups map 1:1 onto `rom:/…` + F2Z decode.

## 7. Open issues
- F2Z / aPLib speeds on real hardware (ares is cycle-approximate; a flashcart run should
  confirm the ordering).
- The token-block cycle model (`CycleModel::tok_*`) is a rough calibration from the
  aggregate ares runs; a per-file regression would sharpen the w trade-off.
- Textures: F2Z is 3 % behind Shrinkler; a 2D-aware literal context (byte one row above)
  is the obvious next step for CI index data.
- Sound effects are 417 small files; a single blob + offsets in the index would cut DFS
  overhead and open/seek time.
- Opus loop seams and FMV audio were not listened to on hardware.
- The ~10 MiB headroom: decide with the code size known.

## 8. Status / next steps (for resuming this work)
Branch `worktree-agent-adc96df12c4758e62` (phase 1C). Done: `tools/f2-n64pack` (levels,
lossless models, textures (NTX1), sfx/music/FMV, budget + fill step, `--bench`), `crates/f2-n64z`
(F2Z with rANS + token blocks, dictionaries, transforms, space–speed parse), ares probe
(`crates/frogger-n64/src/f2z_probe.rs`, `tools/n64/stage-f2z-probe.py`). Default codecs:
models/textures aPLib, levels F2Z. Gates: `cargo test --workspace`, clippy `-D warnings`, fmt.
Next: (1) a per-file regression to recalibrate `CycleModel::tok_*` from the probe log;
(2) optional MIPS-assembly token decoder / DMA-overlapped decode to push F2Z-fast past aPLib;
(3) 2D literal context for textures; (4) no_std port of `models::decode` once the phase-2R
mesh format lands; (5) single-blob sfx file.

## 9. Phase 3I changes (the runtime uses the pack)
- **models**: `rmodels.rs` replaces F2MB in the default pack: `objects/<bank>.dll.pcd` (the
  `ObjectBank`, postcard, mesh geometry reduced to the vertices — all the simulation reads) +
  `objects/<bank>.dll.f2nm` (F2NM render meshes, `docs/n64-model-format.md`), both aPLib:
  9.4 MB → 2.98 MB. F2MB stays for the codec bench.
- **textures**: NTX1 unchanged (loaded zero-copy by `crates/frogger-n64/src/ntxload.rs`); the
  pack adds the three font BMPs and `textures/<bank>.prc` (postcard `TexBank` of the
  procedural sources the simulation reads).
- **sound effects**: all VADPCM from the start and without the Huffman stage (`vadpcm,huffman=false`:
  RSP-only decode; libdragon's Opus wav64 keeps one decoder state per file and cannot serve
  several voices of the same sample).
- **music**: ULC stereo ABR, the highest of 128/112/96/… kbps that fits (128: 24.3 MB). Opus
  99 kbps measured ~14 ms of VR4300 time per 60 Hz tick for one track in ares; ULC ~2 ms.
- Full ROM: 64,323,584 bytes (`research/n64-performance.md` §3I).
