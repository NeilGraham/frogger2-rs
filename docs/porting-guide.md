# Porting guide: C/C++ → Rust

Rules for anyone (human or agent) translating original code. The goal of Phases 1–5 is
**behavioural parity**, so faithfulness beats elegance. Refactors come after a module is `verified`.

## 1. Workflow per module

1. Pick the next `todo` row in [`source-map.md`](source-map.md) whose dependencies are ported.
2. Read the C file **and its header**, plus PSX/DC variants if behaviour is `#ifdef`'d — the PC
   build uses `PC_VERSION` + `FINAL_MASTER` + `USE_BINK_VIDEO`; delete code from other branches
   (keep a comment noting what was dropped).
3. Translate function by function. Each function gets
   `/// Port of `FuncName` (Common/file.c:123).`
4. Add tests: unit tests for leaf functions; difftests against the compiled C for pure functions
   (§5); trace tests once the module participates in a level.
5. Update `source-map.md` status; note intentional deviations in §8 of this file's log.

## 2. Types

| C | Rust |
|---|---|
| `long`, `int` (MSVC 32-bit) | `i32` |
| `unsigned long`, `DWORD` | `u32` |
| `short`/`WORD`, `unsigned short` | `i16` / `u16` |
| `char` (signed on MSVC), `UBYTE` | `i8` / `u8` |
| `fixed` | `f2_math::Fixed` (`type Fixed = i32`, 20.12; use `fmul`/`fdiv`) |
| `FVECTOR`, `SVECTOR`, `VECTOR`, `IQUATERNION` | `f2_math::{FVector, SVector, Vector, IQuaternion}` (fields `vx,vy,vz[,pad]` / `x,y,z,w`) |
| `float` (MDX) | `f32` — never upgrade to `f64` |
| `BOOL`/`TRUE` | `bool` (but keep `i32` where the value is stored in data/saves) |
| pointer into list | typed index (`EnemyId`, `TileId`) |
| `char[N]` names | `[u8; N]` in data structs; `&str`/`String` at API boundaries |

## 3. Arithmetic semantics (most common source of divergence)

- **Overflow wraps** in the original. Use `wrapping_*` ops (or `Fixed`'s operators, which wrap) —
  never rely on debug-mode panics or saturating ops.
- **Division truncates toward zero** in MSVC (same as Rust `/` on ints). Right-shift of negative
  `long` is arithmetic (Rust `>>` on `i32` is arithmetic too) — but `x >> 12` ≠ `x / 4096` for
  negatives: translate *exactly* the operator used.
- **`FMul` is not `(a*b)>>12`.** `Common/fixed.c` splits each operand into `x>>12` / `x&4095` and
  sums four 32-bit partial products (`(d*b)>>12 + d*a + c*b + (c*a)<<12`), each wrapping. Port it
  literally.
- **`FDiv` is a reciprocal approximation, not a division.** `NewFDiv` computes
  `invDiv = 2147483648L / y` — in MSVC `2147483648L` does not fit `long`, so it is `unsigned long`
  and the division is **unsigned** (negative `y` is reinterpreted as a large unsigned value) — then
  multiplies `x` by `invDiv` via partial products shifted by 19/7/7/5. Results differ from exact
  division in the low bits and for negative divisors. `FDiv2` pre-shifts until `y <= 2^24` and
  returns 0 for `y == 0`. Reproduce bit-exactly and cover with difftests.
- Implicit integer promotions: `short * short` promotes to `int`; `unsigned` mixing changes
  comparisons. Reproduce with explicit casts.
- Float→int conversion follows MSVC 6 (`f2_math::ftol`): e.g. `arccos` of an out-of-range value returns 0. C truncates; Rust `as` saturates on out-of-range/NaN. If a value could be
  out of range, document and match the original's x87 behaviour (`0x80000000` "integer indefinite").
- RNG: the game uses its own `Random(lim)` (`Common/maths.c:251`): shift-xor generator on global
  `rseed`, reset by `SeedRandom()` to **398623**. Port exactly, keep it in `GameState`, and track
  every call site — call order is part of determinism. Also grep for stray CRT `rand()` calls
  (MSVC LCG `seed*214013+2531011`) and port those separately.

## 4. Control flow & structure

- Globals → fields of `GameState` (or sub-structs). Pass `&mut GameState` (or a narrower context
  struct) explicitly. No `static mut`.
- Intrusive linked lists → `Vec`/arena + index, **preserving iteration order** and the effect of
  removal during iteration (C loops often grab `next` before freeing).
- Function pointers (event/trigger tables, enemy behaviours) → `enum` + `match`, or `fn` pointer
  tables when the original indexes them by data.
- `switch` fall-through → explicit.
- Uninitialised reads that the original relied on → document and reproduce the deterministic value
  (usually zero from BSS).
- Platform calls (`PlaySample`, `PrepareSong`, `StartVideoPlayback`, `SaveGame`) → push an event
  to `state.events`; the platform layer handles them.

## 5. Difftesting against the original C

For pure functions, compile the original C file(s) in a **test-only** build script with the `cc`
crate and stub headers, then property-test Rust vs C on random inputs:

```rust
#[test]
fn fmul_matches_c() {
    proptest!(|(a: i32, b: i32)| assert_eq!(Fixed(a) * Fixed(b), Fixed(unsafe { c::FMul(a, b) })));
}
```

Note: compile C as 32-bit semantics (`long` = 32-bit) — on 64-bit Linux `long` is 64-bit, so
stub headers must `#define long int` / use `int32_t` typedefs, or build the C for `i686`.
This is the main reason the trace oracle (retail exe) remains authoritative.

## 6. Style

- `#![forbid(unsafe_code)]` in `f2-game`, `f2-math`, `f2-formats`.
- Keep original identifiers in snake_case form (`FindNearestTile` → `find_nearest_tile`) so
  grep across C and Rust works. Keep original constant names (`TILESTATE_DEADLY`).
- Keep original comments that explain intent; drop commented-out dead code (it stays in git
  history of `frogger2-vss`).
- No behaviour "fixes" during parity porting. If you spot a bug, file it in
  `research/known-bugs.md` and add a `// BUG(original): …` comment; fixes go behind
  `Profile::Enhanced` later.

## 7. Agents

Work is executed through a task graph (adopting TPW's `work/tasks.json` + `docs/ORCHESTRATION.md`
pattern): one task = one module or one format, with an executable "done when" (tests/trace
pass). Agents must never copy game data or decompiled retail-binary pseudo-code into tracked files.

## 8. Intentional deviations log

| Module | Deviation | Reason |
|---|---|---|
| main loop | fixed 60 Hz step instead of variable `gameSpeed` | determinism; fixes FPS-dependent bugs (known-bugs #9) |
