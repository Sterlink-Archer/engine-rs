| suite | function | rows | x86_64 portable | x86_64 native | aarch64 portable | aarch64 neoverse-v2 |
|---|---|---:|---|---|---|---|
| detmath | exp | 36314 | pass | pass | pass | pass |
| detmath | log | 25774 | pass | pass | pass | pass |
| detmath | pow2 | 2516 | pass | pass | pass | pass |
| detmath | pow | 45638 | pass | pass | pass | pass |
| detmath | atan2 | 49720 | pass | pass | pass | pass |
| detmath | nuke_ring_lookup | 31416 | pass | pass | pass | pass |
| prng | stream_next_1000 | 195 | pass | pass | pass | pass |
| prng | fromState | 40 | pass | pass | pass | pass |
| prng | mixed_scripts | 48 | pass | pass | pass | pass |
| prng | nextInt_0_100 | 80 | pass | pass | pass | pass |
| prng | nextInt_m7p5_3p25 | 80 | pass | pass | pass | pass |
| prng | nextFloat_m1_1 | 80 | pass | pass | pass | pass |
| prng | chance | 10 | pass | pass | pass | pass |
| prng | nextID | 40 | pass | pass | pass | pass |
| prng | shuffleArray | 320 | pass | pass | pass | pass |
| util | simpleHash | 1816 | pass | pass | pass | pass |
| util | toInt | 2057 | pass (4 ood) | pass (4 ood) | pass (4 ood) | pass (4 ood) |
| util | minInt_maxInt_withinInt | 400 | pass | pass | pass | pass |
| util | within | 2424 | pass | pass | pass | pass |
| util | sigmoid | 4812 | pass | pass | pass | pass |
| util | manhattanDistWrapped | 1000 | pass | pass | pass | pass |
| util | boundingBoxCenter_inscribed | 600 | pass | pass | pass | pass |
| util | findMinimumBy | 400 | pass | pass | pass | pass |
| util | getMode | 300 | pass | pass | pass | pass |
| util | closestTwoTiles | 400 | pass | pass | pass | pass |
| util | doomsdayClockRequiredTiles | 13620 | pass | pass | pass | pass |
| util | doomsdayClockTroopFloor | 1500 | pass | pass | pass | pass |
| util | doomsdayClockDrain | 3000 | pass | pass | pass | pass |
| util | doomsdayClockRotQuota | 600 | pass | pass | pass | pass |
| util | rotNoise | 2000 | pass | pass | pass | pass |
| util | maxHealthWithVeterancy | 140 | pass | pass | pass | pass |
| config | attackLogic | 12327 | unported | unported | unported | unported |
| config | conquerGoldAmount | 18 | unported | unported | unported | unported |
| config | dynamicSamRange | 1500 | unported | unported | unported | unported |
| config | goldAdditionRate | 108 | unported | unported | unported | unported |
| config | maxTroops | 2785 | unported | unported | unported | unported |
| config | misc_formulas | 302 | unported | unported | unported | unported |
| config | nukeDeathFactor | 3000 | unported | unported | unported | unported |
| config | nukeMagnitudes_nukeSpeed | 288 | unported | unported | unported | unported |
| config | percentageTilesOwnedToWin | 5676 | unported | unported | unported | unported |
| config | samRange | 1020 | pass | pass | pass | pass |
| config | saturation | 3501 | pass | pass | pass | pass |
| config | smallEconomySpawnBoost | 50 | pass | pass | pass | pass |
| config | start_manpower_gold | 108 | unported | unported | unported | unported |
| config | tradeShipGold | 21400 | unported | unported | unported | unported |
| config | tradeShipSpawnRate | 4480 | pass | pass | pass | pass |
| config | trainGold | 9072 | unported | unported | unported | unported |
| config | trainSpawnRate | 2610 | pass | pass | pass | pass |
| config | troopIncreaseRate | 7250 | unported | unported | unported | unported |
| config | troop_amounts | 1500 | unported | unported | unported | unported |
| config | unitCost | 825 | unported | unported | unported | unported |
| config | unitInfo | 288 | unported | unported | unported | unported |
| config | zero_arg_methods | 1476 | unported | unported | unported | unported |
| jssem | Math.round | 8664 | pass | pass | pass | pass |
| jssem | Math.fround | 15018 | pass | pass | pass | pass |
| jssem | Math.floor_ceil_trunc | 8664 | pass | pass | pass | pass |
| jssem | Math.sign_abs_sqrt | 8664 | pass | pass | pass | pass |
| jssem | ToInt32_ToUint32 | 3010 | pass | pass | pass | pass |
| jssem | Math.imul | 3529 | pass | pass | pass | pass |
| jssem | shifts | 2500 | pass | pass | pass | pass |
| jssem | bitwise_and_wrapping_add | 1500 | pass | pass | pass | pass |
| jssem | mod_div | 3400 | pass | pass | pass | pass |
| jssem | Math.min_max | 65 | pass | pass | pass | pass |
| jssem | bigint_div_mod_mul | 2916 | pass (48 ood) | pass (48 ood) | pass (48 ood) | pass (48 ood) |
| jssem | Number(bigint) | 1528 | pass (12 ood) | pass (12 ood) | pass (12 ood) | pass (12 ood) |
| jssem | BigInt(number) | 648 | pass (8 ood) | pass (8 ood) | pass (8 ood) | pass (8 ood) |
| jssem | toString36 | 1000 | pass | pass | pass | pass |
| order | array_sort | 5121 | pass | pass | pass | pass |
| order | FlatBinaryHeap | 120 | pass | pass | pass | pass |
| order | map_insertion_order | 200 | pass | pass | pass | pass |
| order | set_iteration_with_mutation | 200 | pass | pass | pass | pass |
| order | object_key_order | 100 | pass | pass | pass | pass |

- x86_64 portable: {"cases": 373771, "fail": 0, "ood": 72, "pass": 305776, "unported": 67923}
- x86_64 native: {"cases": 373771, "fail": 0, "ood": 72, "pass": 305776, "unported": 67923}
- aarch64 portable: {"cases": 373771, "fail": 0, "ood": 72, "pass": 305776, "unported": 67923}
- aarch64 neoverse-v2: {"cases": 373771, "fail": 0, "ood": 72, "pass": 305776, "unported": 67923}
