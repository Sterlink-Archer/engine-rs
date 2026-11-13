# Contributing

Thanks for looking. This is an unofficial Rust port of the OpenFront.io engine (see NOTICE). The one
rule that overrides everything else: **the port must stay bit-exact with the TypeScript engine it
tracks.** A change that is faster, cleaner or "more correct" but alters a single result is a bug here.

## Ground rules

- Behaviour changes must follow upstream. If the TypeScript engine changes, port the change; cite the
  upstream `file:line` in the doc comment of the Rust item, as the existing code does.
- Anything that can change a result needs a conformance argument: a golden vector, a differential
  test against the straightforward port, or a TS-generated fixture. "It looks equivalent" is not one.
- Determinism lints are enforced (`clippy.toml`, `docs/PORTING_GUIDE.md`): no std hash-order
  iteration, no `f64::round/max/min/powf/ln`, no `sort_by` on JS-sorted data, no FMA. Use the `of-det`
  equivalents.
- `unsafe` is forbidden in the engine crates.
- Gate before sending a change: `tools/ci.sh` (fmt, clippy `-D warnings`, `cargo test --workspace`,
  golden vectors).

## Licensing of contributions

Code is contributed under AGPL-3.0-only (see LICENSE); map/asset data under CC BY-SA 4.0. Do not
add anything from the upstream `proprietary/` directory or from OpenFront's servers. Keep the
copyright notice "© OpenFront and Contributors" (NOTICE) intact.

## Scope

This repository is the game engine simulator only: rules, determinism, conformance tooling, CLI and
benchmarks. Player strategy / decision code beyond the engine's own built-in nation and tribe AI is
out of scope.

## Using it responsibly

Do not point automation at the live openfront.io service. Run it on your own copy of the game. If
you want to do research that involves the official service, contact OpenFront Inc. first.
