#!/usr/bin/env python3
"""Writes a deterministic synthetic 1 MiB "level-like" file for the N64 DFS probes.

    python3 -I tools/n64/gen-probe-data.py <out.bin>

No game data: a mix shaped like level banks (smooth int16 vertex tables, face index
lists, keyframe deltas, ASCII names, palette-indexed texels, and ~10% noise) so the
compression ratios/throughputs measured by mkasset levels 1-3 are representative.
"""
import math
import struct
import sys

SIZE = 1 << 20


def gen():
    out = bytearray()
    seed = 0x1234567

    def rnd():
        nonlocal seed
        seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
        return seed >> 8

    block = 0
    while len(out) < SIZE:
        kind = block % 6
        block += 1
        if kind == 0:  # vertex table: smooth int16 xyz + pad
            n = 512
            for i in range(n):
                x = int(math.sin(i * 0.05 + block) * 3000)
                y = int(math.cos(i * 0.031) * 800) + (rnd() & 7)
                z = int(i * 12 - 3000)
                out += struct.pack(">hhhh", x, y, z, 0)
        elif kind == 1:  # faces: u16 indices, mostly sequential strips
            for i in range(1024):
                b = i // 2
                out += struct.pack(">HHH", b, b + 1 + (i & 1), b + 32)
        elif kind == 2:  # keyframe deltas: small signed values
            for i in range(4096):
                out += struct.pack(">b", int(math.sin(i * 0.2) * 20) + (rnd() % 3) - 1)
        elif kind == 3:  # names / script text
            words = [b"frog", b"lily", b"garden", b"spawn", b"platform", b"baby", b"swampy", b"coin", b"anim", b"tile"]
            for i in range(400):
                out += words[rnd() % len(words)] + b"_%02d\0" % (rnd() % 40)
        elif kind == 4:  # CI8 texels with runs
            v = 0
            for i in range(8192):
                if rnd() % 5 == 0:
                    v = rnd() & 0x3F
                out.append(v)
        else:  # noise (already-compressed / audio-like data)
            for i in range(2048):
                out.append(rnd() & 0xFF)
    return bytes(out[:SIZE])


if __name__ == "__main__":
    open(sys.argv[1], "wb").write(gen())
