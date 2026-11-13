#!/usr/bin/env bash
# build-release.sh [native|neoverse-v2|portable] [cargo args...]
# Max-performance build: profile.release (lto=fat, codegen-units=1, panic=abort) + target CPU.
#   native      : this machine's CPU (laptop default on x86_64). Binaries are NOT portable.
#   neoverse-v2 : GH200 / Grace (Neoverse-V2) — the default on aarch64.
#   portable    : no -C target-cpu (x86-64 baseline / generic aarch64).
# Bit-exactness does not depend on the choice: Rust never contracts a*b+c into FMA and
# never enables fast-math, so wider SIMD/FMA availability cannot change f64 results
# (verified: of-vectors + of-conform smoke pass with target-cpu=native, README §Build).
# The target dir is per CPU choice so builds do not overwrite each other.
set -euo pipefail
cd "$(dirname "$0")/.."
cpu="${1:-}"; [ $# -gt 0 ] && shift
if [ -z "$cpu" ]; then case "$(uname -m)" in aarch64|arm64) cpu=neoverse-v2 ;; *) cpu=native ;; esac; fi
flags="${RUSTFLAGS:-}"
[ "$cpu" != portable ] && flags="$flags -C target-cpu=$cpu"
export RUSTFLAGS="$flags" CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target}/cpu-$cpu"
echo "RUSTFLAGS='$RUSTFLAGS' CARGO_TARGET_DIR=$CARGO_TARGET_DIR" >&2
exec nice -n 19 cargo build --release "$@"
