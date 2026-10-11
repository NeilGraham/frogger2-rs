#!/usr/bin/env python3
"""Vector ("modern") version of a Frogger 2 font strip, traced from the user's own game data.

Defaults (owner-reviewed v4, 2026-10-11): potrace on the original pixels, corner threshold
0.75 (straight strokes, notches and slots kept), original counters, 0.9 px outline, 1 px drop
shadow and a light-grey inner bevel on the lower-right edges like the original pixel art.

usage: python3 -I trace2.py STRIP.ppm OUTDIR [SIGMA] [OUTLINE_PX] [SHADOW_PX]
The glyph FILL (white pixels) is upscaled 8x (bicubic), blurred (SIGMA, upscaled pixels) and
thresholded with ffmpeg, then traced with potrace into smooth curves. The black outline is a
constant-width stroke around the traced fill and the shadow an offset copy, instead of the
uneven pixel outline. Writes OUTDIR/styled.svg.
"""
import os
import subprocess
import sys

UP = int(os.environ.get("F2T_UP", "1"))


def read_ppm(path):
    data = open(path, "rb").read()
    parts = data.split(maxsplit=4)
    return int(parts[1]), int(parts[2]), parts[4]


def main():
    src, out = sys.argv[1], sys.argv[2]
    sigma = float(sys.argv[3]) if len(sys.argv) > 3 else 0.0
    outline = float(sys.argv[4]) if len(sys.argv) > 4 else 0.9
    shadow = float(sys.argv[5]) if len(sys.argv) > 5 else 1.0
    os.makedirs(out, exist_ok=True)
    w, h, px = read_ppm(src)
    # Fill mask as a grey image: glyph fill black (potrace traces black), the rest white.
    g = bytearray(w * h)
    for i in range(w * h):
        r, gg, b = px[3 * i], px[3 * i + 1], px[3 * i + 2]
        key = r > 180 and b > 180 and gg < 100
        g[i] = 0 if (not key and (r + gg + b) / 3 > 128) else 255
    # Open up enclosed counters (holes in A, B, D, O, P, Q, R, a, b, d, e, o, p, q...): background
    # pixels the outside can't reach, grown by one pixel into the glyph, so they survive the
    # smoothing and stay legible at small sizes.
    seen = bytearray(w * h)
    stack = [i for i in range(w * h) if (i % w in (0, w - 1) or i // w in (0, h - 1)) and g[i] == 255]
    for i in stack:
        seen[i] = 1
    while stack:
        i = stack.pop()
        x, y = i % w, i // w
        for j in ((i - 1) if x > 0 else -1, (i + 1) if x < w - 1 else -1, (i - w) if y > 0 else -1, (i + w) if y < h - 1 else -1):
            if j >= 0 and not seen[j] and g[j] == 255:
                seen[j] = 1
                stack.append(j)
    holes = [i for i in range(w * h) if g[i] == 255 and not seen[i]] if os.environ.get("F2T_OPEN", "0") == "1" else []
    for i in holes:
        x, y = i % w, i // w
        for j in ((i - 1) if x > 0 else -1, (i + 1) if x < w - 1 else -1, (i - w) if y > 0 else -1, (i + w) if y < h - 1 else -1):
            if j >= 0:
                g[j] = 255
    with open(f"{out}/fill.pgm", "wb") as f:
        f.write(b"P5\n%d %d\n255\n" % (w, h))
        f.write(bytes(g))
    vf = f"scale=iw*{UP}:ih*{UP}:flags=neighbor" + (f",gblur=sigma={sigma},lut=y='if(lt(val\\,128)\\,0\\,255)'" if sigma > 0 else "")
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", f"{out}/fill.pgm", "-vf", vf, "-pix_fmt", "gray", f"{out}/fill_up.pgm"], check=True)
    subprocess.run(["potrace", "-s", "-a", os.environ.get("F2T_A", "0.75"), "-O", os.environ.get("F2T_O", "0.3"), "-t", os.environ.get("F2T_T", "0"), "-o", f"{out}/fill.svg", f"{out}/fill_up.pgm"], check=True)
    s = open(f"{out}/fill.svg").read()
    head = s[: s.index("<g")]
    g_open = s[s.index("<g"): s.index(">", s.index("<g")) + 1]
    paths = s[s.index(">", s.index("<g")) + 1: s.rindex("</g>")]
    # potrace's group scales path units by 0.1: stroke widths are in path units.
    unit = 10.0
    sw = outline * UP * unit * 2  # stroke is centred: twice the visible outside width
    off = shadow * UP
    bevel = float(os.environ.get("F2T_BEVEL", "1.0")) * UP  # inner lower-right bevel width
    bevel_col = os.environ.get("F2T_BEVEL_COL", "#c4c4c4")
    # potrace's transform (translate + scale 0.1,-0.1) baked into one reusable group.
    g_t = g_open.replace('fill="#000000"', "").replace('stroke="none"', "")
    body = (
        f'<defs><g id="g">{g_t}{paths}</g></g>'
        f'<mask id="inside" maskUnits="userSpaceOnUse"><use href="#g" fill="#ffffff"/></mask></defs>'
        # drop shadow
        f'<use href="#g" transform="translate({off},{off})" fill="#101010" stroke="#101010" stroke-width="{sw}" stroke-linejoin="miter" stroke-miterlimit="3"/>'
        # outline + bevel-grey face
        f'<use href="#g" fill="{bevel_col}" stroke="#101010" stroke-width="{sw}" stroke-linejoin="miter" stroke-miterlimit="3" paint-order="stroke"/>'
        # white face shifted up-left, clipped to the glyph: a grey band remains on the lower-right inside edge
        f'<g mask="url(#inside)"><use href="#g" transform="translate({-bevel},{-bevel})" fill="#ffffff"/></g>'
    )
    open(f"{out}/styled.svg", "w").write(head + body + "</svg>\n")


main()
