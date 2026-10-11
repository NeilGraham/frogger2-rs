#!/usr/bin/env python3
"""Maps the N64 sampling profiler's output (`PROF ...` lines in the debugf log of a ROM built
with F2N64_FEATURES=profile) to functions.

    python3 -I tools/n64/prof.py /mnt/c/f2-n64test/run.txt target/n64/build/frogger2.elf [window]

Uses the last window by default (or the given 0-based window index). `pc` = where the CPU
was (self time, 32-byte buckets), `ra` = return address register at the sample (attributes
leaf functions such as libm calls to their callers)."""
import collections
import os
import re
import subprocess
import sys

log, elf = sys.argv[1], sys.argv[2]
addr2line = os.path.join(os.environ.get("N64_INST", os.path.expanduser("~/opt/n64root/opt/libdragon")), "bin", "mips64-elf-addr2line")
lines = open(log, encoding="utf-8", errors="replace").read().replace("\r", "").splitlines()
starts = [i for i, l in enumerate(lines) if l.startswith("PROF samples")]
if not starts:
    sys.exit("no PROF lines in the log")
idx = starts[int(sys.argv[3])] if len(sys.argv) > 3 else starts[-1]
total = int(lines[idx].split()[2])
hist = {"pc": [], "ra": []}
for l in lines[idx + 1:]:
    m = re.match(r"PROF (pc|ra) ([0-9a-f]+) (\d+)", l)
    if not m:
        break
    hist[m.group(1)].append((m.group(2), int(m.group(3))))


def symbols(addrs):
    """(innermost inlined function, outermost real function) per address."""
    out = subprocess.run([addr2line, "-a", "-f", "-C", "-i", "-e", elf] + ["0x" + a for a in addrs], capture_output=True, text=True).stdout.splitlines()
    res, cur = [], None
    for l in out:
        if re.match(r"^0x[0-9a-f]+$", l):
            if cur is not None:
                res.append(cur)
            cur = []
        elif cur is not None and not re.search(r":(\d+|\?)( \(discriminator \d+\))?$", l):
            cur.append(l)
    if cur is not None:
        res.append(cur)
    return [(f[0] if f else "??", f[-1] if f else "??") for f in res]


def area(fn):
    for key, label in (("f2_game", "simulation (f2_game)"), ("f2_math", "simulation (f2_math)"), ("f2_engine", "render: f2_engine transform/build"),
                       ("f2_draw", "render: f2_draw draw loop"), ("render_cpu", "render: N64 submit"), ("rdpq", "render: libdragon rdpq"),
                       ("rspq", "render: libdragon rspq"), ("wav64", "audio"), ("mixer", "audio"), ("huffv", "audio"), ("samplebuffer", "audio"),
                       ("dma", "PI DMA"), ("f2_formats", "loading/parsing"), ("postcard", "loading/parsing"), ("serde", "loading/parsing")):
        if key in fn:
            return label
    return "other: " + fn[:60]


for name, lst in hist.items():
    inner, outer, areas = collections.Counter(), collections.Counter(), collections.Counter()
    for (a, n), (i, o) in zip(lst, symbols([a for a, _ in lst])):
        inner[i[:120]] += n
        outer[o[:120]] += n
        if name == "pc":
            areas[area(o)] += n
    if name == "pc":
        print(f"== pc by area (% of {total} samples; {sum(n for _, n in lst) * 100 / total:.0f}% listed)")
        for s, n in areas.most_common(20):
            print(f"{n * 100 / total:5.1f}% {s}")
    for what, agg in (("enclosing function", outer), ("innermost inlined", inner)):
        print(f"== {name} by {what} (top buckets; % of {total} samples)")
        for s, n in agg.most_common(45):
            print(f"{n * 100 / total:5.1f}% {s}")
