# Handoff — console ports in flight (2026-10-11)

Resume point for a new session if work was cut off (e.g. by a rate limit). Read this, then
`docs/N64-PLAN.md`, `tools/ps4/README.md`, `tools/ps5/README.md`, `tools/n64/README.md`.

## Branches
| Branch | State |
|---|---|
| `dev` | base; nothing from this work merged yet |
| `feature/ps5-pkg` | PS4 fPKG + native PS5 + GPU paths (`ccdb5bd` = merge-prepped: fmt/clippy/CI fixed). Not merged to `dev` — owner decides. |
| `feature/n64` | branched from `feature/ps5-pkg`; N64 plan + phases 1A, 1B, 2G merged (head `7087378`) |

Agent worktree branches (`.claude/worktrees/agent-*`, branch `worktree-agent-<id>`); each
agent was told to commit WIP and keep a "Status / next steps" section in its docs:

| Branch suffix | Task | Status at handoff | Next |
|---|---|---|---|
| `adc96df12c4758e62` | N64 1C: asset packer `tools/f2-n64pack`, codec `crates/f2-n64z` (F2Z), `research/n64-compression.md` | **done, merged** (`6d4b562`): whole game packs to 54.21 MiB (aPLib models/textures, F2Z levels, VADPCM sfx, Opus music 99 kbps, H.264 FMV 320x240) | N64 loader must dispatch on magic (`F2Z1` / `DCA` / raw), no_std model-format reader or conversion to 2R mesh format, FMV via `fmv_play` |
| `adf53493d6484db51` | N64 3I: 2R merged; the game on r3d from the 1C pack (F2NM meshes via the mesh sink, NTX1 textures, F2Z levels, F2SX sfx, ULC music, FMV, SRAM saves) | **done, NOT merged** (see `research/n64-performance.md` §3I): ROM 64,323,584 B; all worlds boot in ares at 3.6–9.7 fps (sim-bound), loads 0.5–0.8 s | merge into `feature/n64` (2R comes with it); then: rspq ring size / frame block (CPU waits on the RSP), `xform_actor` caching, texture arena against fragmentation, hardware test |
| `a7fca4ab703e18cb9` | N64 2R: RSP renderer on libdragon magma + custom MDX vertex shader (`csrc/rsp_f2mdx.S`), F2NM mesh format (`crates/f2-n64fmt`, `docs/n64-model-format.md`), API `frogger_n64::r3d` | **done, NOT merged** (head `a7cceb6`, based on `bb36afb`): ~4100 tris/frame at 60 fps in ares; Garden 1 game camera 60 fps | merge into `feature/n64` (expect conflicts: its default ROM runs the renderer test app, 2G's runs the game — keep the game as default, renderer test behind a feature); then wire `r3d` into the 2G front end in place of `render_cpu.rs`, and have `tools/f2-n64pack` emit F2NM meshes |
| `acca0b3ce23c50ba2` | N64 2G: playable game loop + water/story-book fixes | **done, merged** (`efe8ea4`) | arcade-machine screen copy not drawn (needs retained RGBA like the book); FMV, saves |
| `a90c2347f335f8b21` | N64 3S: f2-game speed on the VR4300 (sim = 10–14 ms/tick, need ≤ ~5) with bit-identical parity | in progress | exact rsin/rcos tables, no per-tick list copies, cached per-level data; parity: 3 demo traces + `f2-parity check --corpus all` identical |
| `a0d5f7cb14e199305` | PS4/PS5 real-hardware fixes + hardware test ladder | in progress | see below |

## PS4 / PS5 real-hardware failures (owner's PS5: FW 11.40, ShadowMountPlus 1.7beta5-fix1, kstuff-fpkg-1.13-dr-test6)
- PS4 fPKG: installs, launches, then **CE-108255-1** (app crash); no log found.
- Native PS5 folder on /mnt/ext1 via ShadowMountPlus: **CE-107694-7** (can't start).
- Diagnosis: (1) our PS5 build uses native static TLS (patched converter) while every working
  PS5 title uses **emulated TLS** (`-femulated-tls`, `__emutls_get_address`) → rebuild std with a
  custom target `tls-model: emulated` (nightly `-Zbuild-std`), drop the TLS converter patch;
  (2) PS5 titles need **`sce_sys/icon0.dds`** (+ pic DDS) to launch; (3) owner's kstuff supports the
  new **native PS5 fPKG** format → build `.pkg` with drakmor/LibProsperoPkg (GPL-3, .NET; used by
  knutwurst/ps5-ultrapack, verified FW 11.60 + kstuff-lite 1.13); (4) PS4: make the CPU renderer
  the default (Piglet opt-in), revert to conservative param.sfo ATTRIBUTE / default authinfo
  (enhanced variant separate), emulated TLS too, klog-first logging.
- Deliverable: `C:\Users\<you>\Desktop\Frogger2-HW-Test\` test ladder (PS5 C hello →
  PS5 Rust smoke → PS5 game .pkg/folder/.ffpkg/.ffpfsc; PS4 smoke → PS4 game conservative → PS4
  game enhanced), README with klog capture steps. Reference project: Nazky/UnleashedRecomp-Prospero
  (clone at `~/opt/ref/UnleashedRecomp-Prospero`).

## N64 facts so far
- Toolchain: libdragon preview `39d0d60` at `~/opt/libdragon`, installed `N64_INST=~/opt/n64root/opt/libdragon`;
  Rust targets n32 and objects are relabelled to o64 (`tools/n64/n32-to-o64.py`); emulator ares v148
  (`C:\Users\<you>\Emulation\emulators\ares-v148`), harness `tools/n64/run-ares.sh`, scratch `C:\f2-n64test\`.
- Playable in ares (frontend → Garden 1, sfx, music) at 3.5–4 fps; sim 35–45 % real time.
- Faithful CPU render path ≈ 6100 cycles/tri (≈256 tris/frame at 60); game needs 450–1500 → RSP renderer (2R).
- RAM: peak 4.2 MiB of 6.1 MiB free heap. Level loads 5–7 s.
- Not done (as of 2G): FMV, saves, 64 MB pack integration (1C), all-levels pass, perf to 30/60 fps.
  Phase 3I (branch `worktree-agent-adf53493d6484db51`) did the first four; perf is sim-bound.

## Tool locations outside the repo
`~/opt/openorbis` (PS4 toolchain), `~/opt/libssl11` (PkgTool's OpenSSL 1.1), `~/opt/ps5-payload-sdk`
(v0.43), `~/opt/PS5_Vulkan` (+ `.deps`), `~/opt/zlib`, `~/opt/ps5-native-tool`, `~/opt/n64root`,
`~/opt/libdragon`, `~/opt/ref/*`. Emulator scratch dirs: `C:\f2-ps4test`, `C:\f2-ps5test`,
`C:\f2-ps5vk`, `C:\f2-ps4gl`, `C:\f2-n64test`. User artwork: `game-data/ps4-art/boxart.png`
(`F2_ICON_CROP_X=85`).

## Phase 3 (started 2026-10-11 after the rate-limit reset)
| Branch suffix | Task | Status |
|---|---|---|
| `adf53493d6484db51` | N64 3I: merge 2R renderer, wire `r3d` + 1C pack into the game, full ≤64 MiB ROM, FMV, saves, per-world ares evidence | **done, merged** (`673125c`): ROM 61.34 MiB, all worlds boot, 3.6–9.7 fps (CPU draw 51–87 ms/frame, sim 10–25 ms/tick); music switched Opus→ULC 128 kbps (CPU cost) |
| `a90c2347f335f8b21` | N64 3S: sim speed (resumed after the limit cut it off mid-edit) | in progress |
| `a0d5f7cb14e199305` | PS4/PS5 hardware fixes + test ladder (resumed) | in progress |
| `aaa520b4d4f6d5752` | N64 3P: draw-side CPU performance (RSP queue/overlap, matrix caching, sprite fast paths) | started from `673125c` |
| `adfb9e878e97eca51` | N64 3T: retail variable timestep (`f2_game::timing`: exact `UpdateTiming`/`LoopFunc`, `Game::tick_frames`), N64 pacing (`frogger-n64/src/pacing.rs`: one k-frame tick when behind, cap 4, `F2N64_TIMESTEP=fixed`) | **done, NOT merged** (see `research/n64-performance.md` §3T): `tick` + `tick_frames(1)` trace-identical (demos + 102 corpus); ares fps 1.6–1.8× (Garden 1 7.4 → 12.9), real time 50–100 % at cap 4, 75–100 % at cap 6 |
