# P4b vector test maps

Copied verbatim from `upstream/tests/testdata/maps/` at upstream commit
`5b75bd3ad8f66325a727f7bcec47680a0cc07e63` (2026-11-05) so the P4b vector tests run with no upstream
checkout. `SHA256SUMS` holds the sha256 of the **uncompressed** upstream files (checked after decompressing the `.bin.zst` files).

| dir | used by |
|---|---|
| `half_land_half_ocean`, `ocean_and_land` | P4b vector tests (`../p4b_vectors.json.zst`) |
| `world` (`map.bin.zst`, `map4x.bin.zst`) | P4b vector tests (`../p4b_vectors.json.zst`) |

## License

The map files in this directory are copied unmodified from the upstream OpenFront.io repository and are licensed Creative Commons BY-SA 4.0 (see `LICENSE-ASSETS` and `assets/LICENSE-CC-BY-SA-4.0.txt` at the repository root). Attribution: © OpenFront and Contributors; OpenFront Inc. This repository is an unofficial port and is not endorsed by OpenFront Inc.
