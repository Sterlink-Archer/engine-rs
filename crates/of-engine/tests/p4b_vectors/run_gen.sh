#!/usr/bin/env bash
# Regenerate the P4b golden vectors (from the repository root). Needs an upstream OpenFront checkout
# ($OF_UPSTREAM, default ./upstream), Node, zstd, and a TypeScript runner for the .mts generator
# ($OF_RUNNER, default `npx tsx`); the TS runner/harness is NOT part of this repository.
# Inputs: the upstream copy (identical to the pin 5b75bd3 for every
# pathfinding/Line/GameMapImpl/PseudoRandom/TileTraversalScratch source by sha256).
set -euo pipefail
U=${OF_UPSTREAM:-$PWD/upstream}
RUNNER=${OF_RUNNER:-npx tsx}
D=crates/of-engine/tests/p4b_vectors
mkdir -p $D/maps
for m in ocean_and_land half_land_half_ocean; do
  mkdir -p $D/maps/$m
  cp $U/tests/testdata/maps/$m/{manifest.json,map.bin,map4x.bin} $D/maps/$m/
done
mkdir -p $D/maps/world
cp $U/tests/testdata/maps/world/manifest.json $D/maps/world/
zstd -q -19 -f $U/tests/testdata/maps/world/map.bin -o $D/maps/world/map.bin.zst
zstd -q -19 -f $U/tests/testdata/maps/world/map4x.bin -o $D/maps/world/map4x.bin.zst
(cd $D/maps && sha256sum */manifest.json */map.bin */map4x.bin 2>/dev/null; for f in world/*.zst; do echo "$(zstd -dc $f | sha256sum | cut -d' ' -f1)  ${f%.zst} (decompressed)"; done) > $D/maps/SHA256SUMS
OF_ROOT=$U OF_ENGINE_COMMIT=5b75bd3ad8f66325a727f7bcec47680a0cc07e63 $RUNNER $D/gen_p4b_vectors.mts $PWD/$D/p4b_vectors.json.zst $U/tests/testdata/maps
