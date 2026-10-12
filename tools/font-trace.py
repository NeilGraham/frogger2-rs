#!/usr/bin/env python3
"""Vector ("modern") version of a Frogger 2 font strip, traced from the user's own game data.

Defaults (v8, 2026-10-11): the glyph fill's anti-aliased luminance is low-passed (ffmpeg gblur
sigma 0.5 px), upscaled 8x with centre-aligned bicubic and thresholded at 50%; potrace (-a 0)
gives the exact polygon contour, which is then fitted (F2T_FIT=1): Douglas-Peucker at 0.3 native
px (less for small rings), near-axis long edges snapped exactly horizontal/vertical, turns over
35 deg kept as crisp corners (0.15 px fillet), gentle turns and small rings (dots, counters)
blended into quadratic curves over up to 2 px, so long edges stay straight. Styling: uniform 1.0 px
outline and 0.4 px drop shadow (best match to the original's pixels). F2T_FIT=0 falls back to
plain potrace curves (F2T_A / F2T_O). Needs potrace, ffmpeg, rsvg-convert (for previews).

usage: python3 -I tools/font-trace.py STRIP.ppm OUTDIR [SIGMA] [OUTLINE_PX] [SHADOW_PX]
STRIP is the font bitmap as binary PPM (magenta key). Writes OUTDIR/fill.svg (glyph fill only)
and OUTDIR/styled.svg (outline + shadow, game look).
"""
import os
import subprocess
import sys

UP = int(os.environ.get("F2T_UP", "8"))


def read_ppm(path):
    data = open(path, "rb").read()
    parts = data.split(maxsplit=4)
    return int(parts[1]), int(parts[2]), parts[4]


def _dp(pts, tol):
    """Douglas-Peucker on an open polyline."""
    if len(pts) < 3:
        return pts
    (ax, ay), (bx, by) = pts[0], pts[-1]
    dx, dy = bx - ax, by - ay
    ln = (dx * dx + dy * dy) ** 0.5
    best, bi = -1.0, 0
    for i in range(1, len(pts) - 1):
        px, py = pts[i]
        d = abs(dy * (px - ax) - dx * (py - ay)) / ln if ln else ((px - ax) ** 2 + (py - ay) ** 2) ** 0.5
        if d > best:
            best, bi = d, i
    if best <= tol:
        return [pts[0], pts[-1]]
    return _dp(pts[: bi + 1], tol)[:-1] + _dp(pts[bi:], tol)


def _fit_ring(ring, tol, corner_deg, reach, radius):
    """Closed ring -> SVG subpath: simplified polygon whose sharp vertices keep a small fillet,
    gentle vertices become curves, and the middles of long edges stay straight."""
    import math
    pts = [tuple(p) for p in ring]
    if pts[0] == pts[-1]:
        pts = pts[:-1]
    if len(pts) < 3:
        return ""
    # split the ring at its two mutually farthest points so DP sees two open chains
    i0 = 0
    i1 = max(range(len(pts)), key=lambda i: (pts[i][0] - pts[0][0]) ** 2 + (pts[i][1] - pts[0][1]) ** 2)
    i0 = max(range(len(pts)), key=lambda i: (pts[i][0] - pts[i1][0]) ** 2 + (pts[i][1] - pts[i1][1]) ** 2)
    a, b = sorted((i0, i1))
    # small rings (counters, dots) keep proportionally more detail
    xs, ys = [p[0] for p in pts], [p[1] for p in pts]
    tol = min(tol, 0.08 * max(max(xs) - min(xs), max(ys) - min(ys)))
    c1 = _dp(pts[a: b + 1], tol)
    c2 = _dp(pts[b:] + pts[: a + 1], tol)
    v = c1[:-1] + c2[:-1]
    n = len(v)
    if n < 3:
        return ""
    # snap near-horizontal/vertical edges exactly onto the axis (the pixel font is built on them)
    snap = math.tan(math.radians(float(os.environ.get("F2T_SNAP", "8"))))
    snap_len = float(os.environ.get("F2T_SNAP_LEN", "2.0")) * UP
    v = [list(q) for q in v]
    for i in range(n):
        p, q = v[i], v[(i + 1) % n]
        dx, dy = q[0] - p[0], q[1] - p[1]
        if dx * dx + dy * dy < snap_len * snap_len:
            continue  # short edges belong to curves
        if abs(dy) <= snap * abs(dx):
            p[1] = q[1] = (p[1] + q[1]) / 2
        elif abs(dx) <= snap * abs(dy):
            p[0] = q[0] = (p[0] + q[0]) / 2
    v = [tuple(q) for q in v]
    v = [q for i, q in enumerate(v) if q != v[i - 1]]
    n = len(v)
    if n < 3:
        return ""
    cos_c = math.cos(math.radians(corner_deg))
    cos_s = math.cos(math.radians(float(os.environ.get("F2T_CORNER_SHORT", "75"))))
    short_len = float(os.environ.get("F2T_SHORT", "1.5")) * UP
    if max(max(xs) - min(xs), max(ys) - min(ys)) < float(os.environ.get("F2T_SMALL", "9")) * UP:
        short_len = 1e9  # dots and counters: all curve
    cut = []
    for i in range(n):
        p, q, r = v[i - 1], v[i], v[(i + 1) % n]
        e1 = (p[0] - q[0], p[1] - q[1])
        e2 = (r[0] - q[0], r[1] - q[1])
        l1, l2 = math.hypot(*e1), math.hypot(*e2)
        u1, u2 = (e1[0] / l1, e1[1] / l1), (e2[0] / l2, e2[1] / l2)
        turn = -(u1[0] * u2[0] + u1[1] * u2[1])  # cos of the turning angle
        short = min(l1, l2) < short_len  # a turn between short edges is part of a curve
        if turn < (cos_s if short else cos_c):  # sharp corner: small fillet
            half = math.acos(max(-1.0, min(1.0, u1[0] * u2[0] + u1[1] * u2[1]))) / 2
            d = radius / math.tan(half) if half > 1e-6 else 0.0
        else:  # gentle: blend over `reach`
            d = reach
        d = min(d, 0.5 * l1, 0.5 * l2)
        cut.append(((q[0] + u1[0] * d, q[1] + u1[1] * d), q, (q[0] + u2[0] * d, q[1] + u2[1] * d)))
    f = lambda pt: f"{pt[0]:.2f} {pt[1]:.2f}"
    out = [f"M{f(cut[0][2])}"]
    for i in range(1, n + 1):
        a_, q, b_ = cut[i % n]
        out.append(f"L{f(a_)}Q{f(q)} {f(b_)}")
    return "".join(out) + "Z"


def fit_paths(pgm, height):
    """potrace polygon of the upscaled bitmap -> fitted SVG path data in upscaled pixels (y down)."""
    import json
    tol = float(os.environ.get("F2T_TOL", "0.3")) * UP
    corner = float(os.environ.get("F2T_CORNER", "35"))
    reach = float(os.environ.get("F2T_REACH", "2.0")) * UP
    radius = float(os.environ.get("F2T_RADIUS", "0.15")) * UP
    js = subprocess.run(["potrace", "-b", "geojson", "-a", "0", "-t", os.environ.get("F2T_T", "150"), "-o", "-", pgm],
                        check=True, capture_output=True).stdout
    d = []
    for feat in json.loads(js)["features"]:
        for ring in feat["geometry"]["coordinates"]:
            d.append(_fit_ring([(x, height - y) for x, y in ring], tol, corner, reach, radius))
    return f'<path fill-rule="evenodd" d="{"".join(d)}"/>'


def main():
    src, out = sys.argv[1], sys.argv[2]
    sigma = float(sys.argv[3]) if len(sys.argv) > 3 else 0.0
    outline = float(sys.argv[4]) if len(sys.argv) > 4 else 1.0
    shadow = float(sys.argv[5]) if len(sys.argv) > 5 else 0.4
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
    holes_set = set()
    for i in holes:
        x, y = i % w, i // w
        holes_set.add(i)
        for j in ((i - 1) if x > 0 else -1, (i + 1) if x < w - 1 else -1, (i - w) if y > 0 else -1, (i + w) if y < h - 1 else -1):
            if j >= 0:
                holes_set.add(j)
    for i in holes:
        x, y = i % w, i // w
        for j in ((i - 1) if x > 0 else -1, (i + 1) if x < w - 1 else -1, (i - w) if y > 0 else -1, (i + w) if y < h - 1 else -1):
            if j >= 0:
                g[j] = 255
    with open(f"{out}/fill.pgm", "wb") as f:
        f.write(b"P5\n%d %d\n255\n" % (w, h))
        f.write(bytes(g))
    if os.environ.get("F2T_MODE", "gray") == "gray":
        # Anti-aliasing-aware: the fill's luminance as grey levels (white fill -> black for
        # potrace, black outline / key -> white), low-passed, cubic-upscaled 8x (ffmpeg) and
        # thresholded at 50%, so edges land between pixels where the grey pixels put them.
        gg = bytearray(w * h)
        for i in range(w * h):
            r, g2, b = px[3 * i], px[3 * i + 1], px[3 * i + 2]
            key = r > 180 and b > 180 and g2 < 100
            lum = 0 if key else (r * 299 + g2 * 587 + b * 114) // 1000
            gg[i] = 255 if g[i] == 255 and not key and holes_set and i in holes_set else 255 - lum
        with open(f"{out}/gray.pgm", "wb") as f:
            f.write(b"P5\n%d %d\n255\n" % (w, h))
            f.write(bytes(gg))
        # Low-pass at native size, then a centre-aligned cubic 8x upscale (ffmpeg) and a 50%
        # threshold. (mkbitmap -s scales corner-aligned: everything moved ~0.44 px down/left.)
        blur = float(os.environ.get("F2T_BLUR", "1.0"))
        pre = f"gblur=sigma={blur * 0.5}," if blur > 0 else ""
        thr = int(float(os.environ.get("F2T_THR", "0.5")) * 255)
        vf2 = f"{pre}scale=iw*{UP}:ih*{UP}:flags=bicubic+accurate_rnd+full_chroma_int,lut=y='if(lt(val\\,{thr})\\,0\\,255)'"
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", f"{out}/gray.pgm", "-vf", vf2, "-pix_fmt", "gray", f"{out}/fill_up.pgm"], check=True)
        UPX = 8
    vf = f"scale=iw*{UP}:ih*{UP}:flags=neighbor" + (f",gblur=sigma={sigma},lut=y='if(lt(val\\,128)\\,0\\,255)'" if sigma > 0 else "")
    if os.environ.get("F2T_MODE", "gray") != "gray":
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", f"{out}/fill.pgm", "-vf", vf, "-pix_fmt", "gray", f"{out}/fill_up.pgm"], check=True)
    subprocess.run(["potrace", "-s", "-a", os.environ.get("F2T_A", "1.0"), "-O", os.environ.get("F2T_O", "2.5"), "-t", os.environ.get("F2T_T", "150"), "-o", f"{out}/fill.svg", f"{out}/fill_up.pgm"], check=True)
    s = open(f"{out}/fill.svg").read()
    head = s[: s.index("<g")]
    g_open = s[s.index("<g"): s.index(">", s.index("<g")) + 1]
    paths = s[s.index(">", s.index("<g")) + 1: s.rindex("</g>")]
    # potrace's group scales path units by 0.1: stroke widths are in path units.
    unit = 10.0
    if os.environ.get("F2T_FIT", "1") == "1":
        hgt = float(head.split('viewBox="')[1].split()[3].rstrip('"'))
        paths = fit_paths(f"{out}/fill_up.pgm", hgt)
        g_open, unit = "<g>", 1.0
        s = head + g_open + paths + "</g></svg>\n"
        open(f"{out}/fill.svg", "w").write(s)
    sw = outline * UP * unit * 2  # stroke is centred: twice the visible outside width
    off = shadow * UP
    bevel = float(os.environ.get("F2T_BEVEL", "0")) * UP  # inner lower-right bevel width
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
