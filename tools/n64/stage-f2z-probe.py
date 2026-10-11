#!/usr/bin/env python3
"""Stages the F2Z decode probe's input into the N64 DFS root (game-derived: stays in target/).

    python3 -I tools/n64/stage-f2z-probe.py <pack dir> [<rel path> ...]

Copies the chosen pack files (F2Z streams, keeping their pack paths) and the dictionaries
they need to target/n64/filesystem/f2z/, decodes each one on the host
(`cargo run --release -p f2-n64z --features encoder --example f2z -- d ... --dict ...`),
re-compresses the decoded bytes with `mkasset -c 0..3 -w 256` into
target/n64/filesystem/mka<level>/<rel path> (the libdragon codecs on identical bytes) and
writes target/n64/filesystem/f2z/list.txt, one line per file:

    <rel path> <dictionary rel path or -> <raw size> <FNV-1a 32 of the decoded bytes, hex>
        <mkasset c0 size> <c1 size> <c2 size> <c3 size>   (same line)

The pack must store the chosen files as F2Z (build it with
`f2-n64pack --codec models=f2z --codec textures=f2z [--size-weight W]`; the default pack stores
models/textures with aPLib). Dictionaries come first (dictionary column `-`). The ROM probe (probes::f2z_probes) reads
the list, so nothing game-derived is compiled into the source. Then run tools/n64/build.sh.
"""
import os
import shutil
import subprocess
import sys

DEFAULT = [
    "models/garden1.mdl",
    "models/gardenmaster.mdl",
    "models/gamegeneric.mdl",
    "textures/garden.ntx",
    "textures/backdrop/artwork00.ntx",
    "collision/garden1.col",
    "scenics/garden1.sce",
    "maps/entity-0-0.dat",
]
DICTS = {1: "dict/models.f2zd", 2: "dict/textures.f2zd", 3: "dict/levels.f2zd"}


def fnv1a(b):
    h = 0x811C9DC5
    for x in b:
        h = ((h ^ x) * 0x01000193) & 0xFFFFFFFF
    return h


def main():
    root = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
    pack = os.path.abspath(sys.argv[1])
    files = sys.argv[2:] or DEFAULT
    n64inst = os.environ.get("N64_INST", os.path.expanduser("~/opt/n64root/opt/libdragon"))
    out = os.path.join(root, "target", "n64")
    fs = os.path.join(out, "filesystem")
    raw_dir = os.path.join(out, "f2z-raw")
    for d in ["f2z", "mka0", "mka1", "mka2", "mka3"]:
        shutil.rmtree(os.path.join(fs, d), ignore_errors=True)
    shutil.rmtree(raw_dir, ignore_errors=True)
    subprocess.run(["cargo", "build", "--release", "-p", "f2-n64z", "--features", "encoder", "--example", "f2z"],
                   cwd=root, check=True)
    f2z = os.path.join(root, "target", "release", "examples", "f2z")

    def dict_id(rel):
        with open(os.path.join(pack, rel), "rb") as f:
            h = f.read(12)
        assert h[:4] == b"F2Z1", rel
        return h[8]

    need = sorted({dict_id(r) for r in files} - {0})
    entries = [(DICTS[i], "-") for i in need] + [(r, DICTS[dict_id(r)] if dict_id(r) else "-") for r in files]
    lines = []
    for rel, dct in entries:
        src = os.path.join(pack, rel)
        dst = os.path.join(fs, "f2z", rel)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copyfile(src, dst)
        raw = os.path.join(raw_dir, rel)
        os.makedirs(os.path.dirname(raw), exist_ok=True)
        cmd = [f2z, "d", src, raw] + (["--dict", os.path.join(pack, dct)] if dct != "-" else [])
        subprocess.run(cmd, check=True)
        with open(raw, "rb") as f:
            data = f.read()
        for lvl in range(4):
            tmp = os.path.join(out, "mkasset-f2z")
            shutil.rmtree(tmp, ignore_errors=True)
            os.makedirs(tmp)
            subprocess.run([os.path.join(n64inst, "bin", "mkasset"), "-c", str(lvl), "-w", "256", "-o", tmp, raw],
                           check=True)
            d = os.path.join(fs, f"mka{lvl}", rel)
            os.makedirs(os.path.dirname(d), exist_ok=True)
            shutil.move(os.path.join(tmp, os.path.basename(raw)), d)
            shutil.rmtree(tmp)
        sizes = " ".join(str(os.path.getsize(os.path.join(fs, f"mka{l}", rel))) for l in range(4))
        lines.append(f"{rel} {dct} {len(data)} {fnv1a(data):08x} {sizes}")
        print(f"{rel}: f2z {os.path.getsize(src)} raw {len(data)} mkasset c0..3 {sizes} fnv {fnv1a(data):08x}")
    with open(os.path.join(fs, "f2z", "list.txt"), "w") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
