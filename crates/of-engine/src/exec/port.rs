//! `PortExecution` (`engine/src/execution/PortExecution.ts`). Owner: **P7**.
//!
//! Fields: the `Port v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::city::add_train_station_exec;
use crate::exec::prelude::*;
use crate::exec::trade_ship::{TradeShipExec, prng_canon};

/// The `init` block of the Port snapshot.
#[derive(StateClone, Debug)]
pub struct PortInit {
    /// `PseudoRandom(mg.ticks())` (`:33`).
    pub rng: Prng,
    pub check_offset: i32,
}

/// `PortExecution` data.
#[derive(StateClone, Debug)]
pub struct PortExec {
    pub active: bool,
    pub port: UnitRef,
    pub init: Option<PortInit>,
    pub trade_ship_spawn_rejections: i32,
}

impl PortExec {
    /// `new PortExecution(port)`.
    pub fn new(port: UnitRef) -> Self {
        // TS: :20,25 `active = true`, `tradeShipSpawnRejections = 0`
        PortExec { active: true, port, init: None, trade_ship_spawn_rejections: 0 }
    }

    /// `shouldSpawnTradeShip()` (`:84-102`).
    pub fn should_spawn_trade_ship(&mut self, cx: &mut Ctx) -> bool {
        // TS: :85-86
        let num_trade_ships = cx.st.unit_count_memo(UnitType::TradeShip);
        let port_owner = cx.st.unit(self.port).owner();
        let port_levels = cx.st.player_unit_count_memo(port_owner, UnitType::Port);
        // TS: :87 `this.port!.level()`: nothing in the loop changes the port's level.
        let level = cx.st.unit(self.port).level();
        let cfg = cx.st.config();
        let rng = &mut self.init.as_mut().ts_expect("PortExecution.ts:95 this.random").rng;
        should_spawn_loop(level, &mut self.trade_ship_spawn_rejections, rng, |rejections| {
            // TS: :88-94 `tradeShipSpawnRate(rejections, numTradeShips, portLevels)`
            cfg.trade_ship_spawn_rate(rejections, f64::from(num_trade_ships), f64::from(port_levels))
        })
    }

    /// `createStation()` (`:104-113`).
    pub fn create_station(&mut self, cx: &mut Ctx) {
        // TS: :105-109 `hasUnitNearby(port.tile()!, trainStationMaxRange(), Factory)`
        let tile = cx.st.unit(self.port).tile();
        let range = cx.st.config().train_station_max_range();
        let nearby_factory = cx.st.has_unit_nearby(tile, range, UnitType::Factory, None, false);
        if nearby_factory {
            // TS: :111 `addExecution(new TrainStationExecution(this.port))`
            add_train_station_exec(cx, self.port);
        }
    }

    /// `tradingPorts()` (`:115-161`; rules-v36, upstream #6332 8a286b802: v36 `:118-157`): one `{ port, weight }`
    /// per destination (`WeightedPort`), no longer an expanded probability list.
    pub fn trading_ports(&self, cx: &mut Ctx) -> Vec<(UnitRef, f64)> {
        let port_tile = cx.st.unit(self.port).tile();
        let port_owner = cx.st.unit(self.port).owner();

        // TS: :118-123 `sourceComponents = new Set<number>()` filled from the water neighbours.
        // A local set, only iterated after it is complete: insertion order + dedup.
        let mut source_components: Vec<u32> = Vec::new();
        for neighbor in cx.st.map.neighbors(port_tile) {
            if !cx.st.map.is_water(neighbor) {
                continue;
            }
            let comp = cx.rcx().get_water_component(neighbor);
            if let Some(comp) = comp
                && !source_components.contains(&comp)
            {
                source_components.push(comp);
            }
        }

        // TS: :124-127 `mg.players().filter(p => p !== owner && p.canTrade(owner))
        //   .flatMap(p => p.units(UnitType.Port))`
        let mut ports: Vec<UnitRef> = Vec::new();
        for p in cx.st.players() {
            if p != port_owner && cx.st.player(p).can_trade(port_owner) {
                ports.extend(cx.st.player_units_of_type_memo(p, UnitType::Port));
            }
        }
        // TS: :128-133 `.filter(p => sourceComponents.some(comp => hasWaterComponent(p.tile(), comp)))`
        let mut kept: Vec<UnitRef> = Vec::with_capacity(ports.len());
        for p in ports {
            let ptile = cx.st.unit(p).tile();
            let mut any = false;
            for &comp in &source_components {
                if cx.rcx().has_water_component(ptile, comp) {
                    any = true;
                    break;
                }
            }
            if any {
                kept.push(p);
            }
        }
        let mut ports = kept;
        // TS: :134-139 `.sort((p1, p2) => manhattan(port, p1) - manhattan(port, p2))`
        {
            let st = &*cx.st;
            of_det::js_sort_by(&mut ports, |&p1, &p2| {
                f64::from(st.map.manhattan_dist(port_tile, st.unit(p1).tile()))
                    - f64::from(st.map.manhattan_dist(port_tile, st.unit(p2).tile()))
            });
        }

        // TS: :141-160 (the `isFriendly` / config reads are pure, so they are gathered first)
        let short_range_debuff = cx.st.config().trade_ship_short_range_debuff();
        #[allow(clippy::cast_precision_loss)] // int: a list length (< 2^53)
        let proximity_bonus_nb = cx.st.config().proximity_bonus_ports_nb(ports.len() as f64);
        let cands: Vec<PortCand> = ports
            .iter()
            .map(|&other_port| {
                let o = cx.st.unit(other_port);
                PortCand {
                    unit: other_port,
                    level: o.level(),
                    dist: f64::from(cx.st.map.manhattan_dist(port_tile, o.tile())),
                    friendly: cx.st.player(port_owner).is_friendly(o.owner(), false),
                }
            })
            .collect();
        weight_ports(&cands, short_range_debuff, proximity_bonus_nb)
    }
}

/// One sorted trading port with the values `tradingPorts` reads about it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PortCand {
    pub unit: UnitRef,
    /// `otherPort.level()`.
    pub level: i32,
    /// `manhattanDist(this.port.tile(), otherPort.tile())`.
    pub dist: f64,
    /// `this.port.owner().isFriendly(otherPort.owner())` (treatAFKFriendly = false).
    pub friendly: bool,
}

/// The weighting map of `tradingPorts` (rules-v36, #6332; v36 `:142-157`) over the distance-sorted ports:
/// `weight = otherPort.level() * multiplier`, the multiplier 1 + (close but not too close) + (friendly, not too close).
/// For integer levels >= 0 this is exactly the pin's copy count per port, in the same order.
pub(crate) fn weight_ports(
    ports: &[PortCand],
    short_range_debuff: f64,
    proximity_bonus_nb: f64,
) -> Vec<(UnitRef, f64)> {
    // TS: :142 `ports.map((otherPort, i) => ..)`
    ports
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut multiplier = 1.0;
            // TS: :144-146
            let too_close = c.dist < short_range_debuff;
            // TS: :147-148 `i < proximityBonusPortsNb(ports.length)`
            #[allow(clippy::cast_precision_loss)] // int: an index (< 2^53)
            let close_bonus = (i as f64) < proximity_bonus_nb;
            // TS: :149-151
            if !too_close && close_bonus {
                multiplier += 1.0;
            }
            // TS: :152-154
            if !too_close && c.friendly {
                multiplier += 1.0;
            }
            // TS: :155 `{ port: otherPort, weight: otherPort.level() * multiplier }`
            (c.unit, f64::from(c.level) * multiplier)
        })
        .collect()
}

/// `pickTradingPort(ports)` (rules-v36, upstream #6332; v36 `:160-171`): one `nextInt(0, total)` roll walked over the
/// weights. The same single draw as the pin's `randElement` over the expanded list, mapped to the same port.
pub(crate) fn pick_trading_port(rng: &mut Prng, ports: &[(UnitRef, f64)]) -> Option<UnitRef> {
    // TS: :161-162
    let mut total = 0.0;
    for &(_, weight) in ports {
        total += weight;
    }
    // TS: :163-165
    if total == 0.0 {
        return None;
    }
    // TS: :166 `let roll = this.random.nextInt(0, total)`
    let mut roll = rng.next_int(0.0, total);
    // TS: :167-170
    for &(port, weight) in ports {
        if roll < weight {
            return Some(port);
        }
        roll -= weight;
    }
    // TS: :171
    of_det::ts_throw!("unreachable: roll exceeded total weight");
}

/// The loop of `shouldSpawnTradeShip` (`:87-101`): per port level, the spawn rate is
/// recomputed with the current rejection count, then `random.chance(rate)` is drawn.
pub(crate) fn should_spawn_loop(
    level: i32,
    trade_ship_spawn_rejections: &mut i32,
    rng: &mut Prng,
    mut spawn_rate: impl FnMut(f64) -> f64,
) -> bool {
    for _ in 0..level {
        let rate = spawn_rate(f64::from(*trade_ship_spawn_rejections));
        // TS: :95-98
        if rng.chance(rate) {
            *trade_ship_spawn_rejections = 0;
            return true;
        }
        // TS: :99
        *trade_ship_spawn_rejections += 1;
    }
    false
}

impl ExecBehavior for PortExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`PortExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :32-34
        let ticks = cx.st.ticks();
        // int: ticks are non-negative, so JS `%` equals Rust `%`.
        self.init = Some(PortInit { rng: Prng::new(f64::from(ticks)), check_offset: ticks % 10 });
    }
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :38-40 `mg === null || random === null || checkOffset === null`: these fields are
        // `undefined` (not null) before init, so the guard never throws; `tick` only runs after
        // `init` (GameImpl.ts:599-627).
        let check_offset = self.init.as_ref().ts_expect("PortExecution.ts:56 this.checkOffset").check_offset;

        // TS: :42-45
        if !cx.st.unit(self.port).is_active() {
            self.active = false;
            return;
        }
        // TS: :47-49
        if cx.st.unit(self.port).is_under_construction() {
            return;
        }
        // TS: :51-53
        if !cx.st.unit(self.port).has_train_station() {
            self.create_station(cx);
        }
        // TS: :55-58 (every 10 ticks)
        if (cx.st.ticks() + check_offset) % 10 != 0 {
            return;
        }
        // TS: :60-62
        if !self.should_spawn_trade_ship(cx) {
            return;
        }
        // TS: v36 :69-72 `const port = this.pickTradingPort(this.tradingPorts()); if (port === null) return;`
        // (rules-v36, upstream #6332)
        let ports = self.trading_ports(cx);
        let rng = &mut self.init.as_mut().ts_expect("PortExecution.ts:69 this.random").rng;
        let Some(port) = pick_trading_port(rng, &ports) else {
            return;
        };
        // TS: :71-73 `new TradeShipExecution(this.port.owner(), this.port, port)`
        let owner = cx.st.unit(self.port).owner();
        cx.st.add_execution(Exec::TradeShip(TradeShipExec::new(owner, self.port, port)));
    }
}

/// `Port v1`: `{ active, port, init: {random, checkOffset}|null, tradeShipSpawnRejections }`.
impl Canon for PortExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let init = match &self.init {
            None => Cs::Null,
            Some(i) => Cs::Obj(vec![
                ("random".into(), prng_canon(&i.rng)),
                ("checkOffset".into(), Cs::Int(i64::from(i.check_offset))),
            ]),
        };
        Cs::Obj(vec![
            ("active".into(), Cs::Bool(self.active)),
            ("port".into(), self.port.canon(cx)),
            ("init".into(), init),
            ("tradeShipSpawnRejections".into(), Cs::Int(i64::from(self.trade_ship_spawn_rejections))),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(slot: u32, level: i32, dist: f64, friendly: bool) -> PortCand {
        PortCand { unit: UnitRef { slot, generation: 0 }, level, dist, friendly }
    }

    fn total(w: &[(UnitRef, f64)]) -> f64 {
        w.iter().map(|&(_, x)| x).sum()
    }

    /// `PortExecution.test.ts` "Destination ports chances scale with level": one level-3 port,
    /// `proximityBonusPortsNb = 0`, `tradeShipShortRangeDebuff = 0` → total weight 3 (v36 test: `totalWeight`).
    #[test]
    fn destination_chances_scale_with_level() {
        // (7,10) → (0,0) on half_land_half_ocean: manhattan 17
        let got = weight_ports(&[cand(1, 3, 17.0, false)], 0.0, 0.0);
        assert_eq!(total(&got), 3.0);
    }

    /// "Trade ship proximity bonus": bonus 10, debuff 0, one level-1 port → weight 2.
    #[test]
    fn proximity_bonus_doubles() {
        assert_eq!(total(&weight_ports(&[cand(1, 1, 17.0, false)], 0.0, 10.0)), 2.0);
    }

    /// "Trade ship short range debuff": debuff 100 > distance 17 cancels the bonus → weight 1.
    #[test]
    fn short_range_debuff_cancels_bonus() {
        assert_eq!(total(&weight_ports(&[cand(1, 1, 17.0, false)], 100.0, 10.0)), 1.0);
    }

    /// Friendly owners add a third share (`:152-154`), only outside the short range, and the
    /// bonus applies to the first `proximityBonusPortsNb` sorted ports only (`i < nb`).
    #[test]
    fn friendly_and_bonus_index_cutoff() {
        let ports = [cand(1, 1, 50.0, true), cand(2, 2, 60.0, false), cand(3, 1, 5.0, true)];
        let got: Vec<(u32, f64)> = weight_ports(&ports, 10.0, 1.0).iter().map(|(u, w)| (u.slot, *w)).collect();
        // port 1: base + bonus (i=0 < 1) + friendly; port 2: level 2, no bonus at i=1;
        // port 3: too close (5 < 10): base only, no bonus, no friendly share
        assert_eq!(got, [(1, 3.0), (2, 2.0), (3, 1.0)]);
    }

    /// v36 test "Very high level ports are weighted without expanding a list": level 1,000,000 x 2 (proximity bonus).
    #[test]
    fn very_high_level_ports_are_weighted() {
        let w = weight_ports(&[cand(7, 1_000_000, 17.0, false)], 0.0, 10.0);
        assert_eq!(w, [(UnitRef { slot: 7, generation: 0 }, 2_000_000.0)]);
        let mut rng = Prng::new(0.0);
        assert_eq!(pick_trading_port(&mut rng, &w), Some(UnitRef { slot: 7, generation: 0 }));
    }

    /// v36 test "pickTradingPort honours weights and skips empty lists".
    #[test]
    fn pick_honours_weights_and_skips_empty() {
        let mut rng = Prng::new(0.0);
        assert_eq!(pick_trading_port(&mut rng, &[]), None);
        let (a, b) = (UnitRef { slot: 1, generation: 0 }, UnitRef { slot: 2, generation: 0 });
        for _ in 0..50 {
            assert_eq!(pick_trading_port(&mut rng, &[(a, 0.0), (b, 5.0)]), Some(b));
        }
    }

    /// State identity with the pin (t713): for integer levels the weighted pick makes the same single draw as
    /// `randElement` over the pin's expanded list and maps it to the same port, so PRNG stream and choice agree.
    #[test]
    fn weighted_pick_equals_pin_expanded_rand_element() {
        let ports = [cand(1, 3, 50.0, true), cand(2, 2, 60.0, false), cand(3, 1, 5.0, true), cand(4, 5, 70.0, true)];
        for &(debuff, nb) in &[(10.0, 1.0), (0.0, 10.0), (100.0, 0.0), (0.0, 0.0)] {
            let w = weight_ports(&ports, debuff, nb);
            // the pin's list: per port `level` copies, again for the bonus, again for friendly, in port order
            let mut expanded: Vec<UnitRef> = Vec::new();
            for &(u, weight) in &w {
                expanded.extend(std::iter::repeat_n(u, weight as usize));
            }
            for seed in 0..300 {
                let (mut r1, mut r2) = (Prng::new(f64::from(seed)), Prng::new(f64::from(seed)));
                let old = *r1.rand_element(&expanded).unwrap_or_else(|_| panic!("non-empty"));
                assert_eq!(pick_trading_port(&mut r2, &w), Some(old), "seed {seed}");
                assert_eq!(r1.next(), r2.next(), "PRNG streams diverge after the pick (seed {seed})");
            }
        }
    }

    /// "shouldSpawnTradeShip recomputes spawn rate per level with updated rejection count":
    /// a level-2 port, PRNG seeded with `mg.ticks()` = 0 at init.
    #[test]
    fn should_spawn_recomputes_rate_per_level() {
        let mut rng = Prng::new(0.0);
        let mut rejections_count = 0;
        let mut seen: Vec<f64> = Vec::new();
        let r1 = should_spawn_loop(2, &mut rejections_count, &mut rng, |r| {
            seen.push(r);
            1_000_000.0
        });
        assert!(!r1);
        assert_eq!(seen, [0.0, 1.0]);
        let r2 = should_spawn_loop(2, &mut rejections_count, &mut rng, |r| {
            seen.push(r);
            1.0
        });
        assert!(r2);
        assert_eq!(seen, [0.0, 1.0, 2.0]);
        let r3 = should_spawn_loop(2, &mut rejections_count, &mut rng, |r| {
            seen.push(r);
            1_000_000.0
        });
        assert!(!r3);
        assert_eq!(seen, [0.0, 1.0, 2.0, 0.0, 1.0]);
    }
}
