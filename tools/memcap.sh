#!/bin/sh
# Run a command under a memory cap so a runaway gets OOM-killed on its own
# instead of exhausting the WSL VM (which takes every distro and agent down).
# Used as the cargo runner (.cargo/config.toml), so every test/example/run
# binary is capped. Wrap other heavy commands by hand: tools/memcap.sh <cmd...>
# MEMCAP=16G raises the cap (default 8G); MEMCAP=off disables it.
cap="${MEMCAP:-8G}"
[ "$cap" = off ] && exec "$@"
# Linux only (the cap protects the shared WSL VM): macOS has no cgroups and rejects RLIMIT_AS.
# Native Windows cargo cannot run this sh script at all; see docs/BUILDING.md.
[ "$(uname -s)" = Linux ] || exec "$@"
if systemd-run --user --scope -q -p MemoryMax="$cap" -p MemorySwapMax=0 -- true 2>/dev/null; then
  exec systemd-run --user --scope -q -p MemoryMax="$cap" -p MemorySwapMax=0 -- "$@"
fi
# No user systemd: fall back to an address-space limit (looser, but still bounded).
kb=$(echo "$cap" | awk '/[Gg]$/{print int($0)*1048576; next} /[Mm]$/{print int($0)*1024; next} {print int($0/1024)}')
ulimit -v $((kb * 2)) && exec "$@"
