# Controllers: the PlayStation layout on modern pads

Frogger 2 was a PlayStation game first. Both the PC and PS1 builds read the same 16 PSX pad bits
(`PSX/islpad.h:13-28`, `PC/islpad.h:8-23`), so the game logic in `f2-game` already speaks "PS1
pad". The port maps modern gamepads onto that pad by physical position. Source paths below are
relative to `frogger2-vss/teamSpirit/FroggerProject/`.

## What the PS1 version does

### Pad reading (`PSX/islpad.c`)
- Only digital buttons are used. `padHandlePort` finds a DualShock and then calls
  `PadSetMainMode(port,0,3)` (`islpad.c:258`), which forces the pad into digital mode and locks
  it there. The stick values are copied into `padData.analogX/Y` (`:310-313`), but no game code
  reads them. `GameProcessController` declares `stickX/stickY` and never uses them
  (`Common/game.c:118,509`).
- Debounce marks the press frame only: `(digital ^ prev) & digital` (`islpad.c:395-399`). The
  port computes it the same way (`InputMapper::process`).
- **No vibration.** The DualShock macros (`SHOCK_SET`, `SHOCK_SLIDE`, `SHOCK_BUZZTIME`, ...,
  `islpad.h:61-80`) and the per-frame motor manager `padHandleShock` (`islpad.c:411-457`) are
  never called anywhere: not in the PSX tree, the PSX demo, or the Dreamcast tree. `shockEnable`
  is never set either. The "Vibration" option strings (`PSX/gametext.h:62-64`) are in a header
  nothing includes.
- Players: player N reads `padData[N]`. Slots 0-3 are port 1 (multitap A-D) and 4-7 are port 2.
  Without a multitap, port 2 is copied into slot 1 (`temp_psx.c:1270-1279`). With pad 0
  unplugged, the game auto-pauses (`frontend.c:197-202`).

### In-game (`Common/game.c` `GameProcessController`)

| PS1 button | Action | Condition (source) |
|---|---|---|
| D-pad | Hop, relative to the camera | press, or held with the auto-hop power-up (`:199,225,251,277`) |
| ✕ Cross | Super hop (in the direction held, else the last hop) | press (`:304`) |
| ✕ then ✕ | Double jump (an early press is queued) | press while super hopping (`:335-372`) |
| ✕ held | Float after a double jump | held (`:186`) |
| ○ Circle | Tongue (single player) | press (`:446`) |
| □ Square | Croak (shows where the baby frogs are) | press (`:457`) |
| START | Pause | press (`:149-164`) |
| L1/R1/L2/R2, Select, △ | none in retail. L2/R2 camera rotation exists only with `debugKeys` (`:476-506`) | |
| Left stick | none (pad locked to digital) | |

In the hub world every hop is a super hop. In multiplayer battle the d-pad only turns the frog
(`multi.c:922-947`).

### Menus

| Screen | Confirm | Back | Other | Source |
|---|---|---|---|---|
| Options, book, arcade, level/character select, FMV list, multiplayer setup | ✕ | △ | D-pad moves. Some screens also go back on a direction (main menu ↓, book ↓) | `options.c:1590-1658`, table `:499-512` |
| Pause | ✕ or START | △ (cancels the yes/no) | START always resumes. □ held plus a sequence enters cheats | `menus.c:513-630` |
| Level complete | ✕ | | ↑/↓ choose, ✕ skips the tallies | `frontend.c:352-389,1623-1729` |
| Best-time name entry | ✕ (next letter / finish) | ← | ↑/↓ cycle A-Z and space, START finishes | `temp_psx.c:575-655` |
| Game over | any button | | | `frontend.c:409` |
| FMV | ✕/START skip | | START skips all | `temp_psx.c:1281-1287` |
| Art viewer | | △ or START | any button skips | `options.c:3156,3181` |

Prompts use `@X`/`@T` ("Press @T to go back", "@X - Select"). The PS1 has no
controller-configuration screen: `CONTROLLERVIEW_MODE` is commented out (`frontend.c:256`).

## The port's defaults

| PS1 | PlayStation pad | Xbox pad | Nintendo pad (positional, default) | Keyboard P1 (retail) |
|---|---|---|---|---|
| ✕ Cross (super hop, confirm) | ✕ | A | B | Return |
| ○ Circle (tongue) | ○ | B | A | Right Shift |
| □ Square (croak) | □ | X | Y | Right Ctrl |
| △ Triangle (back) | △ | Y | X | Escape |
| START (pause) | Options | Menu | + | Escape |
| SELECT | Create/Share | View | - | Numpad 0 |
| L1 / R1 / L2 / R2 | L1/R1/L2/R2 | LB/RB/LT/RT | L/R/ZL/ZR | Numpad 1 / R / Numpad 4 / Numpad 6 |
| D-pad | D-pad + left stick | D-pad + left stick | D-pad + left stick | arrows |

- **Position, not label.** Bottom = ✕, right = ○, left = □, top = △ on every family. On a Nintendo
  pad this makes B confirm. `[input] face_layout = "nintendolabels"` swaps South/East and
  West/North on Nintendo pads only, so A confirms and B goes back
  (`controller::FaceLayout`).
- **Left stick.** The PS1 has no analog control, so the stick acts as a d-pad.
  `[input] stick_mode = "fourway"` (the default) uses `controller::StickDpad`:
  - It engages once the radial deflection reaches `deadzone` (default 0.5, the retail
    `DEAD_ZONE`). The nearest axis wins; an exact diagonal goes to up/down.
  - It holds while the deflection stays at or above `deadzone - 0.15` and within 60° (45° + 15°)
    of the held axis.
  - So a stick resting near a diagonal or near the dead-zone edge does not release and re-press.
    Each re-press would be a new hop.
  - `stick_mode = "raw"` is the PC retail behaviour: each axis is tested on its own, so
    diagonals press two directions.
- **Rebinding.** Rebinding works through `Bindings` (`[[input.bindings]]`). Multiple inputs per
  action are allowed. Face-layout and stick filtering happen before bindings, in
  `Gamepads::poll_raw`.

## Vibration

The PS1 game never vibrates, and `f2-game` emits no `Effect::Rumble`, so authentic play has no
rumble. The plumbing is ready for an optional enhancement:

- `Effect::Rumble { player, strength, frames }` goes to `Gamepads::rumble`.
- The translation follows the DualShock model of the unused `padHandleShock`, ported as
  `controller::ShockMotors`:
  - The large, low-frequency motor is 0-255 and maps to gilrs `Strong`.
  - The small, high-frequency motor is on/off and maps to gilrs `Weak` at full magnitude.
  - `strength` sets the large motor. A strength of 192 or more also buzzes the small motor.
  - The duration is `frames * 1000 / 60` ms.
- `[input] rumble` / `rumble_scale` scale or disable it.

## Players and devices

- Pads become players 1-4 in connection order (`controller::reassign`).
- When a pad is unplugged, the other players keep their own pads even though the slots shift.
- Each player's device is remembered in `[input] pad_devices` (the SDL-style GUID of the pad) and
  returns to that player on replug or the next start. Identical pads are matched one each.
- `[input] controllers` is still written for compatibility.
- The keyboard always works for every player through the keymap.
- Device families (Xbox, PlayStation, Nintendo, generic) come from the name and USB vendor.
  - `PadButton::label` gives prompt text ("Cross", "A", ...).
  - `controller::glyph` gives ✕ ○ □ △ and arrows for fonts that have them.
  - `controller::menu_buttons` gives the physical confirm/back buttons.

## Backends

gilrs 0.11 with default features:

- **Windows:** Windows.Gaming.Input. Xbox/XInput pads work, as do HID pads such as the
  DualShock 4, DualSense and Switch Pro through the SDL mapping database. Force feedback works
  on Xbox pads; HID pads may not rumble.
- **Linux:** evdev. Rumble works where the kernel driver supports FF (xpad, hid-sony,
  hid-playstation, hid-nintendo).

## Testing

- `cargo test -p f2-platform` covers the pure logic: mapping, stick hysteresis, rumble
  translation and assignment.
- `cargo test -p f2-game --lib name_entry` covers pad name entry.
- Manually: run the game, plug a pad in and out, navigate the frontend/book/pause with
  ✕/△/d-pad/stick, and beat a level's par time to try name entry (↑/↓ letter, ✕ next, START
  done).
