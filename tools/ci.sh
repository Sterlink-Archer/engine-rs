#!/usr/bin/env bash
# CI gate: fmt, clippy -D warnings, tests, golden vectors.
# Run from anywhere. By default it runs niced, on cores 4-15 and with a 6 GB cap (when systemd-run
# exists); CI_NO_LIMITS=1 disables that. CI_JOBS sets the cargo job count; CI_OFFLINE=1 adds --offline.
#   tools/ci.sh            full gate
#   tools/ci.sh --quick    skip the of-vectors release CLI run
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${CI_NO_LIMITS:-0}" != "1" ]] && command -v systemd-run >/dev/null; then
  RUN=(systemd-run --user --scope -q -p MemoryMax=6G nice -n 19 taskset -c 4-15)
else
  RUN=()
fi
J=(-j "${CI_JOBS:-2}")
OFF=(); [[ "${CI_OFFLINE:-0}" == "1" ]] && OFF=(--offline)

# Crates formatted by rustfmt.toml. The conformance crates (of-canon, of-trace,
# of-vectors) are owned by the conformance workflow and keep their own layout.
FMT_PKGS=(-p of-engine-derive -p of-engine -p of-cli)
# Crates held to clippy -D warnings (of-det: our t088 contributions live there).
# --no-deps: the other conformance crates are path dependencies and keep their own lint
# state (of-canon has 3 pre-existing clippy findings).
CLIPPY_PKGS=(-p of-det -p of-engine-derive -p of-engine -p of-cli)

step() { printf '\n== %s\n' "$*"; }

step "cargo fmt --check (${FMT_PKGS[*]})"
"${RUN[@]}" cargo fmt --check "${FMT_PKGS[@]}"

step "cargo clippy -D warnings (${CLIPPY_PKGS[*]})"
"${RUN[@]}" cargo clippy "${OFF[@]}" "${J[@]}" --no-deps "${CLIPPY_PKGS[@]}" --all-targets -- -D warnings
"${RUN[@]}" cargo clippy "${OFF[@]}" "${J[@]}" --no-deps -p of-det -p of-engine --all-targets --features of-engine/prng-trace -- -D warnings
"${RUN[@]}" cargo clippy "${OFF[@]}" "${J[@]}" --no-deps -p of-engine --all-targets --features stats-full,verify-memos -- -D warnings

step "cargo test --workspace"
"${RUN[@]}" cargo test "${OFF[@]}" "${J[@]}" --workspace
"${RUN[@]}" cargo test "${OFF[@]}" "${J[@]}" -p of-det --features prng-trace

if [[ "${1:-}" != "--quick" ]]; then
  step "of-vectors gate (release CLI, exits 1 on any fail)"
  "${RUN[@]}" cargo run "${OFF[@]}" "${J[@]}" --release -q -p of-vectors | tail -1
fi

step "CI PASS"
