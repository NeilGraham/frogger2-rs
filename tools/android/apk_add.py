"""Adds files to an APK (zip): python3 -I apk_add.py APK BASE PATH...  Each PATH (a file or
directory, relative to BASE) is stored under its relative name; native libraries (.so) are
stored uncompressed so they can be mapped from the APK (extractNativeLibs=false), and
zipalign later page-aligns them."""
import os
import sys
import zipfile


def main():
    apk, base, paths = sys.argv[1], sys.argv[2], sys.argv[3:]
    with zipfile.ZipFile(apk, "a") as z:
        for p in paths:
            full = os.path.join(base, p)
            files = [full] if os.path.isfile(full) else [os.path.join(r, f) for r, _, fs in os.walk(full) for f in fs]
            for f in sorted(files):
                name = os.path.relpath(f, base).replace(os.sep, "/")
                kind = zipfile.ZIP_STORED if f.endswith(".so") else zipfile.ZIP_DEFLATED
                z.write(f, name, compress_type=kind)


if __name__ == "__main__":
    main()
