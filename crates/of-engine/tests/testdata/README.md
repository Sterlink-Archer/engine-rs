# of-engine test data

Copied verbatim from `upstream/tests/testdata/maps/` at upstream commit
`5b75bd3ad8f66325a727f7bcec47680a0cc07e63` on 2026-11-05, so tests run with no upstream checkout. Checksums are in
each map's `SHA256SUMS`; they must equal the upstream files' (`sha256sum` both).

| dir | source | used by |
|---|---|---|
| `maps/plains` | `upstream/tests/testdata/maps/plains` (100x100, all land) | `tests/empty_world.rs` |

## License

The map files in this directory are copied unmodified from the upstream OpenFront.io repository and are licensed Creative Commons BY-SA 4.0 (see `LICENSE-ASSETS` and `assets/LICENSE-CC-BY-SA-4.0.txt` at the repository root). Attribution: © OpenFront and Contributors; OpenFront Inc. This repository is an unofficial port and is not endorsed by OpenFront Inc.
