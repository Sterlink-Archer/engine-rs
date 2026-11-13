# P4a water test maps

Copied from `upstream/tests/testdata/maps/` at upstream `5b75bd3ad8f66325a727f7bcec47680a0cc07e63`
(2026-11-05) so the water tests run with no upstream checkout. The `.bin.zst` files are `zstd -19` of the upstream bins; `SHA256SUMS` holds the
sha256 of the **uncompressed** upstream bins (the tests check it after decompressing).

| file | dims | used by |
|---|---|---|
| `world/map4x.bin.zst` | 1000 × 500 | oracle: CC, graph, HPA (Normal world minimap); nuke sequence main map (Compact world) |
| `world/map16x.bin.zst` | 500 × 250 | oracle: CC; nuke sequence minimap (Compact world) |
| `giantworldmap/map16x.bin.zst` | 1027 × 487 | oracle: CC |
| `half_land_half_ocean/*`, `ocean_and_land/*` | 16 × 16 / 8 × 8 | WaterNukes port, oracle CC |

## License

The map files in this directory are copied unmodified from the upstream OpenFront.io repository and are licensed Creative Commons BY-SA 4.0 (see `LICENSE-ASSETS` and `assets/LICENSE-CC-BY-SA-4.0.txt` at the repository root). Attribution: © OpenFront and Contributors; OpenFront Inc. This repository is an unofficial port and is not endorsed by OpenFront Inc.
