#!/usr/bin/env python3
"""Writes the Rust target spec the PS4 and PS5 console builds use.

    rustc +nightly -Z unstable-options --print target-spec-json --target x86_64-unknown-freebsd \
        | python3 -I tools/ps4/console-target.py CPU OUT.json

It is the stock `x86_64-unknown-freebsd` spec (so `target_os = "freebsd"` and std's and libc's
FreeBSD code paths apply unchanged) with:

- `tls-model: emulated`: every thread local goes through `__emutls_get_address`
  (crates/console-host/csrc/emutls.c, on pthread keys) and the executable has no TLS
  segment. Native static TLS depends on how the console's loader lays out the TLS block of
  the main executable, which homebrew titles never use: the native PS5 titles that run on
  hardware (PS5_Vulkan, UnleashedRecomp-Prospero) are built with `-femulated-tls`;
- `cpu`: btver2 (PS4 Jaguar) or znver2 (PS5 Zen 2);
- `panic-strategy: abort` (no unwinding across the system libraries);
- no `metadata` (informational only).

std itself must then be built from source with this spec (`cargo +nightly -Zbuild-std`),
which tools/ps4/build-pkg.sh and tools/ps5/build.sh do.
"""
import json
import sys

cpu, out = sys.argv[1], sys.argv[2]
spec = json.load(sys.stdin)
if spec.get("os") != "freebsd" or spec.get("arch") != "x86_64":
    raise SystemExit("expected the x86_64-unknown-freebsd spec on stdin")
spec.pop("metadata", None)
spec["tls-model"] = "emulated"
spec["has-thread-local"] = True
spec["cpu"] = cpu
spec["panic-strategy"] = "abort"
with open(out, "w", encoding="utf-8") as f:
    json.dump(spec, f, indent=2, sort_keys=True)
    f.write("\n")
