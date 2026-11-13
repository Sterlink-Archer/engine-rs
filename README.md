# engine-rs

© OpenFront and Contributors. An **unofficial** Rust port of the OpenFront.io game engine, licensed AGPL-3.0.
Not affiliated with, endorsed by, or sponsored by OpenFront.

## What this is

OpenFront is built by its authors and contributors, and every rule in this project comes from their work. `engine-rs` re-implements the
game's simulation in Rust so that it plays out exactly the same way as the original TypeScript engine, tick for tick. Give it the same game
setup and the same player actions and you get the same game. It only covers the simulation: no graphics, no server, no accounts.

It grew out of a game-AI research project that needed to simulate a very large number of OpenFront games quickly. We are offering the
engine back to the OpenFront team and community as a possible contribution.

## Why it might be useful

- **Replays:** re-run a recorded game and check that every moment matches.
- **Fast simulation:** play out many games quickly, for example for research or balance experiments.
- **Testing:** a second, independent implementation is a handy cross-check for the original engine when rules change.
- **Research:** copying a whole game state is cheap, so you can try "what if" branches from any point.

## How fast is it?

A live game runs at 10 ticks per second.

| | Original engine (TypeScript) | engine-rs |
|---|---|---|
| Playing a full World game, one CPU core | about 156 ticks per second | about 747 ticks per second (roughly 4.8x) |
| Copying a whole mid-game state | about 0.4 seconds | about 1 to 2 milliseconds (roughly 200 to 400x) |

Rounded from our own measurements on a big World free-for-all game, on one CPU core (747 ticks per second is about 75 times live speed). The playing
numbers were measured on a slightly earlier engine version and not re-measured since. The copying numbers compare different methods (the original
saves and restores a game; engine-rs copies it in memory) on different machines, so treat that gap as a rough guide.
The latest engine update adds a small extra gain of about 2 to 6 percent depending on the game. Details and the full tables are in [docs/PERFORMANCE.md](docs/PERFORMANCE.md).

## Try it

You need [Rust](https://rustup.rs) (the repository pins the version it uses). Then, from the repository folder:

```
cargo build --release -p of-cli --features stats-full
target/release/engine-rs conform --scenario tests/fixtures/RS-SMOKE-01
target/release/engine-rs conform --scenario tests/fixtures/RS-SMOKE-02
target/release/engine-rs bench --scenario tests/fixtures/RS-SMOKE-02
cargo build --release -p of-trace
cargo test --workspace
```

The two `conform` commands replay small recorded games and should print `PASS`. `bench` prints a line of numbers; `ticksPerSecond` is your machine's speed. The last two lines build the trace checker and run the test suite.

## How we know it is exact

We ran test games (many maps and game modes) through the official TypeScript engine and recorded them, including a fingerprint of the key game
state at every tick plus full snapshots at checkpoints. `engine-rs` replays the same player actions and compares its own fingerprints tick by tick.
Across 222 recorded games it matched with no differences. If a single tick ever differs, the tool reports the first tick that disagrees and, where a
snapshot exists, the exact field. The test suite also checks hundreds of thousands of small rule calculations against values produced by the original engine.
The numbers, versions and limits are in [docs/CONFORMANCE.md](docs/CONFORMANCE.md).

It is pinned to OpenFront v0.36.0 and only claims to match what the recorded games cover.

## Credit and licence

This is a derivative of OpenFront.io's engine and all the game design belongs to OpenFront and its contributors. The code is AGPL-3.0, with the upstream additional terms (see [LICENSE](LICENSE) and
[NOTICE](NOTICE)). The bundled map data is CC BY-SA 4.0 ([LICENSE-ASSETS](LICENSE-ASSETS)). OpenFront's proprietary assets are not included.
Please use it on your own copy of the game or for offline research. The hosted service has its own terms.

## Contributing and contact

Fixes and questions are welcome through GitHub issues. One rule matters most: a change must never alter a single game result. See [CONTRIBUTING.md](CONTRIBUTING.md).
More documentation: [architecture](docs/ARCHITECTURE.md), [porting guide](docs/PORTING_GUIDE.md), [file map](docs/FILE_MAP.md).
