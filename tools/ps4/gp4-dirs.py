"""Rewrites a create-gp4 project's <rootdir> to match the staged directory tree.

create-gp4 writes a fixed tree (sce_sys, sce_module, assets/...), so files in other
directories (the bundled data/) make PkgTool fail with "Sequence contains no elements".
Usage: python3 -I gp4-dirs.py STAGE_DIR PKG.gp4
"""
import os
import re
import sys
from xml.sax.saxutils import quoteattr

stage, gp4 = sys.argv[1], sys.argv[2]


def tree(path, indent):
    out = []
    for name in sorted(os.listdir(path)):
        sub = os.path.join(path, name)
        if not os.path.isdir(sub):
            continue
        inner = tree(sub, indent + "\t")
        if inner:
            out.append(f"{indent}<dir targ_name={quoteattr(name)}>\n{inner}{indent}</dir>\n")
        else:
            out.append(f"{indent}<dir targ_name={quoteattr(name)} />\n")
    return "".join(out)


text = open(gp4, encoding="utf-8").read()
root = "<rootdir>\n" + tree(stage, "\t\t") + "\t</rootdir>"
text, n = re.subn(r"<rootdir>.*?</rootdir>", lambda _: root, text, flags=re.S)
if n != 1:
    sys.exit("gp4-dirs: no <rootdir> in " + gp4)
open(gp4, "w", encoding="utf-8").write(text)
