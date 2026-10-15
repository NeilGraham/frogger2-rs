#!/usr/bin/env bash
# The console builds call `cargo +nightly` (std rebuilt with -Zbuild-std). Install the pinned
# nightly with rust-src and make `nightly` resolve to it, so a release does not depend on
# whatever nightly is current. Bump F2_NIGHTLY together with the console crates.
set -euo pipefail
N="${F2_NIGHTLY:-nightly-2026-10-11}"   # rustc 1.101.0-nightly (6f5da406b 2026-10-10)
host="$(rustc -vV | sed -n 's/^host: //p')"
rustup toolchain install "$N" --profile minimal --component rust-src --no-self-update
tc="${RUSTUP_HOME:-$HOME/.rustup}/toolchains"
rm -rf "${tc:?}/nightly-$host"
ln -sfn "$N-$host" "$tc/nightly-$host"
rustc +nightly -V
