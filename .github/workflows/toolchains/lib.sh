# shellcheck shell=bash
# Helpers for the toolchain set-up scripts (sourced). Everything goes under $HOME/opt, the
# defaults the build scripts (tools/ps4, tools/ps5, tools/n64) already use; run with another
# HOME to try them out locally without touching a real install.
set -euo pipefail
OPT="$HOME/opt"
DL="$OPT/dl"
mkdir -p "$OPT" "$DL"

# fetch URL SHA256 FILE: download once, verified (a cached file with the right hash is kept).
fetch() {
    if [ -f "$3" ] && echo "$2  $3" | sha256sum -c --quiet >/dev/null 2>&1; then return 0; fi
    curl -fsSL --retry 3 -o "$3.part" "$1"
    if ! echo "$2  $3.part" | sha256sum -c --quiet; then
        echo "error: $1 changed upstream (sha256 mismatch); update the pin in $0" >&2
        exit 1
    fi
    mv "$3.part" "$3"
}

# checkout URL COMMIT DIR: a shallow clone of exactly that commit.
checkout() {
    if [ "$(git -C "$3" rev-parse HEAD 2>/dev/null)" = "$2" ]; then return 0; fi
    rm -rf "$3"
    git init -q "$3"
    git -C "$3" remote add origin "$1"
    git -C "$3" fetch -q --depth 1 origin "$2"
    git -C "$3" -c advice.detachedHead=false checkout -q FETCH_HEAD
}

# setenv NAME VALUE: for this script, later CI steps ($GITHUB_ENV) and the log.
setenv() {
    export "$1=$2"
    [ -n "${GITHUB_ENV:-}" ] && echo "$1=$2" >> "$GITHUB_ENV"
    echo "$1=$2"
}
