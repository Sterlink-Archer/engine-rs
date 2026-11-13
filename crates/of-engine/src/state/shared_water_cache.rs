//! `SharedWaterCache` (`engine/src/execution/nation/SharedWaterCache.ts`). Owner: **P11**.
//!
//! A TTL cache (30 ticks) that is **state**: callers read it while it is up to 30 ticks
//! stale (its own doc, `:46-49`; ARCHITECTURE §3.8, CS W). `playerWater` is a transparent
//! per-player cache (not in the snapshot) and is cloned with the state.
//!
//! Note on order: the TS comment at `:98-99` says the lake set "is only membership-tested",
//! but `build` iterates it (`:138`, `:157`), so its insertion order (border-tile order ×
//! N/S/W/E neighbour order) decides the order of `lakePartners` and of every `byPlayer` set,
//! which is snapshot state. The port keeps every set a `JsSet`.

use crate::StateClone;
use crate::api::types::PlayerType;
use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::ctx::{Ctx, Rcx};
use crate::ids::{SmallId, SmallIdKey};
use crate::jsorder::{JsMap, JsSet, LookupMap};

/// `PlayerWater` (`:30-35`). non-snapshot.
#[derive(StateClone, Debug, Default)]
pub struct PlayerWater {
    pub tile_version: u32,
    pub water_version: u32,
    pub has_ocean: bool,
    pub lakes: JsSet<i32>,
}

/// `SharedWaterCache` data, `SharedWaterCache v1` snapshot order.
#[derive(StateClone, Debug)]
pub struct SharedWaterCache {
    /// `tick: number = -Infinity` (`:38`).
    pub tick: f64,
    /// `byPlayer: Map<Player, Set<number> | null> | null` (`:39`); sets hold component ids and
    /// the `OCEAN_SENTINEL = -1`.
    pub by_player: Option<JsMap<SmallIdKey, Option<JsSet<i32>>>>,
    /// non-snapshot: `playerWater` (`:42`).
    pub player_water: JsMap<SmallIdKey, PlayerWater>,
}

impl Default for SharedWaterCache {
    fn default() -> Self {
        SharedWaterCache { tick: f64::NEG_INFINITY, by_player: None, player_water: JsMap::default() }
    }
}

/// `TTL_TICKS` (`:25`).
pub const TTL_TICKS: i32 = 30;
/// `OCEAN_SENTINEL` (`:28`).
pub const OCEAN_SENTINEL: i32 = -1;

/// A component id as the TS number held in the sets. // int: component ids are `u8`/`u16`
/// labels (`ConnectedComponents.ts:6,96-116`), far below `i32::MAX`.
#[inline]
fn comp_i32(c: u32) -> i32 {
    #[allow(clippy::expect_used)] // rust-invariant: component labels are < 2^16
    i32::try_from(c).expect("water component id fits i32")
}

/// `waterFor(player, waterVersion)` (`:85-116`), on a `playerWater` map held outside `State`
/// for the duration of one `build` (it is a transparent cache: losing it on a throw only
/// costs a recompute). Returns a copy of the entry's `(hasOcean, lakes)`.
fn water_for(
    rcx: &mut Rcx,
    player_water: &mut JsMap<SmallIdKey, PlayerWater>,
    player: SmallId,
    water_version: u32,
) -> (bool, JsSet<i32>) {
    let st = rcx.st;
    let tile_version = st.player(player).tile_change_version();
    if let Some(cached) = player_water.get(&player)
        && cached.tile_version == tile_version
        && cached.water_version == water_version
    {
        return (cached.has_ocean, cached.lakes.clone());
    }
    let mut has_ocean = false;
    let mut lakes: JsSet<i32> = JsSet::new();
    // TS: `player.borderTiles().forEach(tile => { if (!game.isShore(tile)) return;
    // game.forEachNeighbor(tile, visit) })` (`:109-112`). Nothing here mutates the border set.
    let border: Vec<u32> = st.player(player).border_tiles().iter().collect();
    for tile in border {
        if !st.map.is_shore(tile) {
            continue;
        }
        let mut nbrs = [0u32; 4];
        let n = st.map.neighbors4(tile, &mut nbrs); // forEachNeighbor: N, S, W, E (`GameMapImpl.ts:327-335`)
        for &neighbor in &nbrs[..n] {
            // `visit` (`:100-108`)
            if !st.map.is_water(neighbor) {
                continue;
            }
            if st.map.is_ocean(neighbor) {
                has_ocean = true;
                continue;
            }
            if let Some(comp) = rcx.get_water_component(neighbor) {
                lakes.insert(comp_i32(comp));
            }
        }
    }
    let out = (has_ocean, lakes.clone());
    player_water.insert(player, PlayerWater { tile_version, water_version, has_ocean, lakes });
    out
}

/// `build()` (`:118-171`).
fn build(rcx: &mut Rcx, player_water: &mut JsMap<SmallIdKey, PlayerWater>) -> JsMap<SmallIdKey, Option<JsSet<i32>>> {
    let st = rcx.st;

    // Pass 1: for each non-bot player, record which water bodies they touch
    // and which lakes have them as a candidate trade partner. Bots are skipped
    // entirely — nation AI is the only caller, and bots are never candidate
    // trade partners.
    // TS `playerToWater: Map<Player, ..>` is iterated in insertion order = `players()` order
    // with no duplicates, so a Vec in push order is the same sequence.
    let mut player_to_water: Vec<(SmallId, bool, JsSet<i32>)> = Vec::new();
    // lookup-only: `lakePartners.get/set` only (`:139-142`, `:158`), never iterated.
    let mut lake_partners: LookupMap<i32, Vec<SmallId>> = LookupMap::new();

    let water_version = st.map.water_version();
    for player in st.players() {
        if st.player(player).player_type() == PlayerType::Bot {
            continue;
        }

        let (has_ocean, lakes) = water_for(rcx, player_water, player, water_version);

        // iter-live: `lakes` is a local copy; the body only touches `lake_partners`.
        for &c in lakes.iter() {
            match lake_partners.get_mut(&c) {
                Some(arr) => arr.push(player),
                None => {
                    lake_partners.insert(c, vec![player]);
                }
            }
        }
        player_to_water.push((player, has_ocean, lakes));
    }

    // Pass 2: ocean is treated as always shared (nation AI short-circuits on
    // ocean neighbors). Lake components are shared only if some *other* player
    // on that component can trade with P (i.e. no mutual embargo).
    let mut result: JsMap<SmallIdKey, Option<JsSet<i32>>> = JsMap::new();
    for (player, has_ocean, lakes) in player_to_water {
        let mut shared: JsSet<i32> = JsSet::new();

        if has_ocean {
            shared.insert(OCEAN_SENTINEL);
        }

        let me = st.player(player);
        for &c in lakes.iter() {
            let Some(partners) = lake_partners.get(&c) else { continue };
            for &other in partners {
                if other != player && me.can_trade(other) {
                    shared.insert(c);
                    break;
                }
            }
        }

        result.insert(player, if shared.is_empty() { None } else { Some(shared) });
    }
    result
}

impl Ctx<'_> {
    /// `game.sharedWaterComponents(player)` → `SharedWaterCache.get(player)` (`:76-83`):
    /// rebuilds when stale, then returns a copy of the player's set in set order (`null` →
    /// `None`). TS returns the cached `Set` object itself; no caller mutates it
    /// (`NationStructurePlacement.ts:42,73`), so a copy is equivalent.
    pub fn shared_water_get(&mut self, player: SmallId) -> Option<Vec<i32>> {
        let tick = self.st.ticks();
        let sw = &self.st.shared_water;
        if sw.by_player.is_none() || f64::from(tick) - sw.tick >= f64::from(TTL_TICKS) {
            // The transparent `playerWater` cache leaves `State` while `build` reads the rest
            // of it; `byPlayer`/`tick` are written only after `build` returns, as in TS
            // (`:79-80`), so a throw inside `build` leaves the snapshot fields untouched.
            let mut pw = std::mem::take(&mut self.st.shared_water.player_water);
            let built = {
                let mut rcx = Rcx::new(&*self.st, &mut *self.sc);
                build(&mut rcx, &mut pw)
            };
            let sw = &mut self.st.shared_water;
            sw.player_water = pw;
            sw.by_player = Some(built);
            sw.tick = f64::from(tick);
        }
        let by = self.st.shared_water.by_player.as_ref()?; // rust-invariant: set just above
        by.get(&player).and_then(|s| s.as_ref()).map(|s| s.iter().copied().collect())
    }
}

/// CS W `sharedWaterCache` (`snapshot`, `:50-61`): `{ tick, byPlayer: [(player, [int]|null)] |
/// null }`, entries in Map order, sets in Set order.
impl Canon for SharedWaterCache {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let by_player = match &self.by_player {
            None => Cs::Null,
            Some(m) => Cs::Arr(
                m.iter()
                    .map(|(p, set)| {
                        let s = match set {
                            None => Cs::Null,
                            Some(s) => Cs::Arr(s.iter().map(|&c| Cs::Int(i64::from(c))).collect()),
                        };
                        Cs::Arr(vec![p.canon(cx), s])
                    })
                    .collect(),
            ),
        };
        Cs::Obj(vec![("tick".to_string(), Cs::Num(self.tick)), ("byPlayer".to_string(), by_player)])
    }
}

/// `restoreSnapshot` (`:63-74`): `tick`, `byPlayer` replaced; `playerWater` starts empty (a
/// fresh `SharedWaterCache` on restore has an empty cache, `:42`).
impl Uncanon for SharedWaterCache {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let Cs::Obj(fields) = v else {
            return Err(CanonError::new(format!("SharedWaterCache: expected obj, got {v:?}")));
        };
        let get = |k: &str| {
            fields
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, x)| x)
                .ok_or_else(|| CanonError::new(format!("SharedWaterCache: missing {k}")))
        };
        let tick = f64::uncanon(get("tick")?, cx)?;
        let bp = get("byPlayer")?;
        let by_player = if bp.is_null() {
            None
        } else {
            let arr = bp.as_arr().ok_or_else(|| CanonError::new("SharedWaterCache.byPlayer: expected array"))?;
            let mut m: JsMap<SmallIdKey, Option<JsSet<i32>>> = JsMap::new();
            for e in arr {
                let pair = e.as_arr().filter(|a| a.len() == 2).ok_or_else(|| CanonError::new("byPlayer entry"))?;
                let p = SmallId::uncanon(&pair[0], cx)?;
                let set = if pair[1].is_null() {
                    None
                } else {
                    let xs = Vec::<i32>::uncanon(&pair[1], cx)?;
                    Some(xs.into_iter().collect::<JsSet<i32>>())
                };
                m.insert(p, set);
            }
            Some(m)
        };
        Ok(SharedWaterCache { tick, by_player, player_water: JsMap::default() })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::exec::ExecStore;
    use crate::state::State;

    /// The cached build (`byPlayer`) for `player`, without rebuilding.
    fn peek(st: &State, player: SmallId) -> Option<Option<Vec<i32>>> {
        st.shared_water
            .by_player
            .as_ref()
            .map(|m| m.get(&player).and_then(|s| s.as_ref()).map(|s| s.iter().copied().collect()))
    }

    fn cache_with(entries: &[(u16, Option<&[i32]>)]) -> SharedWaterCache {
        let mut m: JsMap<SmallIdKey, Option<JsSet<i32>>> = JsMap::new();
        for &(p, s) in entries {
            m.insert(SmallId(p), s.map(|xs| xs.iter().copied().collect()));
        }
        SharedWaterCache { tick: 120.0, by_player: Some(m), player_water: JsMap::default() }
    }

    #[test]
    fn default_is_unbuilt_minus_infinity() {
        let c = SharedWaterCache::default();
        assert_eq!(c.tick, f64::NEG_INFINITY);
        assert!(c.by_player.is_none());
    }

    /// CS W keeps Map order and Set order (not sorted): `[(3,[-1,7,2]), (1,null)]`.
    #[test]
    fn canon_keeps_map_and_set_order() {
        let st = State::default();
        let ex = ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        let c = cache_with(&[(3, Some(&[OCEAN_SENTINEL, 7, 2])), (1, None)]);
        let want = Cs::Obj(vec![
            ("tick".into(), Cs::Num(120.0)),
            (
                "byPlayer".into(),
                Cs::Arr(vec![
                    Cs::Arr(vec![Cs::Int(3), Cs::Arr(vec![Cs::Int(-1), Cs::Int(7), Cs::Int(2)])]),
                    Cs::Arr(vec![Cs::Int(1), Cs::Null]),
                ]),
            ),
        ]);
        assert_eq!(c.canon(&mut cx), want);
        // Unbuilt: `{tick: -Infinity, byPlayer: null}`.
        assert_eq!(
            SharedWaterCache::default().canon(&mut cx),
            Cs::Obj(vec![("tick".into(), Cs::Num(f64::NEG_INFINITY)), ("byPlayer".into(), Cs::Null)])
        );
    }

    #[test]
    fn uncanon_round_trips_and_drops_player_water() {
        let mut c = cache_with(&[(2, Some(&[5, OCEAN_SENTINEL])), (4, None)]);
        c.player_water.insert(SmallId(2), PlayerWater::default());
        let st0 = State::default();
        let ex = ExecStore::default();
        let cs = c.canon(&mut CanonCx::new(&st0, &ex));
        let mut st = State::default();
        let back = SharedWaterCache::uncanon(
            &cs,
            &mut UncanonCx { st: &mut st, keys: &crate::canon::LoadKeys { n_players: 8, ..Default::default() } },
        )
        .unwrap();
        assert_eq!(back.tick, 120.0);
        let m = back.by_player.as_ref().unwrap();
        let keys: Vec<u16> = m.keys().map(|k| k.0).collect();
        assert_eq!(keys, [2, 4]);
        let s2: Vec<i32> = m.get(&SmallId(2)).unwrap().as_ref().unwrap().iter().copied().collect();
        assert_eq!(s2, [5, -1]);
        assert!(m.get(&SmallId(4)).unwrap().is_none());
        assert!(back.player_water.is_empty());
        assert_eq!(back.canon(&mut CanonCx::new(&st0, &ex)), cs);
    }

    /// The TTL is state: within 30 ticks of the last build the stale `byPlayer` is served
    /// without touching any player (a rebuild would hit P2/P3 stubs at this tag), and the
    /// returned copy keeps set order.
    #[test]
    fn stale_reads_inside_ttl_do_not_rebuild() {
        let mut st = State::default();
        st.shared_water = cache_with(&[(1, Some(&[9, OCEAN_SENTINEL, 4])), (2, None)]);
        let mut sc = crate::scratch::Scratch::new();
        for t in [120, 135, 149] {
            st.ticks = t;
            let mut cx = Ctx::new(&mut st, &mut sc, None);
            assert_eq!(cx.shared_water_get(SmallId(1)), Some(vec![9, -1, 4]), "tick {t}");
            assert_eq!(cx.shared_water_get(SmallId(2)), None);
            assert_eq!(cx.shared_water_get(SmallId(7)), None, "absent player → null");
        }
        assert_eq!(st.shared_water.tick, 120.0);
        assert_eq!(peek(&st, SmallId(1)), Some(Some(vec![9, -1, 4])));
    }

    /// `tick - this.tick >= TTL_TICKS` rebuilds at exactly tick+30 (an empty game: no players,
    /// so `build` touches nothing but `players()`).
    #[test]
    fn rebuild_at_exactly_ttl() {
        let mut st = State::default();
        st.shared_water = cache_with(&[(1, Some(&[3]))]);
        st.ticks = 150;
        let mut sc = crate::scratch::Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        assert_eq!(cx.shared_water_get(SmallId(1)), None);
        assert_eq!(st.shared_water.tick, 150.0);
        assert_eq!(st.shared_water.by_player.as_ref().map(|m| m.len()), Some(0));
    }
}
