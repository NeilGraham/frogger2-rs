#!/usr/bin/env python3
"""Writes an ares settings override (settings.bml) for automated N64 test runs.

    python3 -I tools/n64/ares-settings.py <ares settings.bml> <out settings.bml>

Starting from ares' own settings.bml (created next to ares.exe on first run), it sets:
  - Nintendo64/ExpansionPak: true (8 MB RDRAM)
  - General/HomebrewMode: true (ISViewer -> emulator stdout, so libdragon debugf is captured;
    plus ares' homebrew-developer checks)
  - General/ShowStatusBar: false (screenshots are the bare 4:3 picture)
  - Input/Defocus: Block (keep running unfocused, but ignore the host keyboard then)
  - Video/WindowWidth/Height 640x480 (2x)
  - Audio/Blocking: false, Audio/Mute: true (unthrottled; ARES_REALTIME=1 keeps real time)
  - VirtualPad1 bound to keyboard keys (ares/ruby keyboard device id 0x1, button group 0,
    rawinput key index), so the harness can press N64 buttons with keybd_event:
        arrows = D-pad, X = A, Z = B, Return = Start, PageUp/PageDown = L/R, Home = Z
        (ares maps N64 Z to the virtual pad's R-Trigger),
        I/K/J/L = analog stick.
"""
import os
import sys

KEYS = {  # ruby rawinput keyboard button indices (zero-based)
    "Up": 86, "Down": 87, "Left": 88, "Right": 89, "Return": 91, "Home": 31,
    "PageUp": 33, "PageDown": 34, "X": 58, "Z": 60, "I": 43, "K": 45, "J": 44, "L": 46,
}
PAD = {
    "Pad.Up": "Up", "Pad.Down": "Down", "Pad.Left": "Left", "Pad.Right": "Right",
    "Start": "Return", "A..South": "X", "X..West": "Z", "L-Bumper": "PageUp", "R-Bumper": "PageDown",
    "R-Trigger": "Home", "L-Up": "I", "L-Down": "K", "L-Left": "J", "L-Right": "L",
}
SET = {
    ("Nintendo64", "ExpansionPak"): "true",
    ("General", "HomebrewMode"): "true",
    ("General", "ShowStatusBar"): "false",
    ("General", "NoFilePrompt"): "true",
    ("Input", "Defocus"): "Block",
    ("Video", "WindowWidth"): "640",
    ("Video", "WindowHeight"): "480",
    # Unthrottled: ares runs as fast as the host emulates (audio sync paces it to real time
    # otherwise). The ROM sees only emulated time, so runs are identical at any speed.
    # ARES_REALTIME=1 keeps real-time pacing (watching a run, listening to it).
    **({} if os.environ.get("ARES_REALTIME") else {("Audio", "Blocking"): "false", ("Audio", "Mute"): "true"}),
}


def main(src, dst):
    out = []
    section = None
    for line in open(src, encoding="utf-8").read().splitlines():
        if line and not line.startswith(" "):
            section = line.strip()
        else:
            key = line.strip().split(":")[0]
            ind = line[: len(line) - len(line.lstrip())]
            if len(ind) == 2 and (section, key) in SET:
                line = f"{ind}{key}: {SET[(section, key)]}"
            elif section == "VirtualPad1" and len(ind) == 2 and key in PAD:
                line = f"{ind}{key}: 0x1/0/{KEYS[PAD[key]]};;"
        out.append(line)
    open(dst, "w", encoding="utf-8", newline="\n").write("\n".join(out) + "\n")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
