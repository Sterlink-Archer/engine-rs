//! T1 for P3 (FILE_MAP §4): the whole `config` golden-vector suite
//! (`conformance/golden/vectors/config.json.zst`, VECTORS §`config`, 79,584 rows) against the
//! real [`EngineConfig`], on the suite's duck-typed player stubs. Owner: **P3**.
//!
//! `of-vectors` checks only the five config-independent formulas in `of_det::config`
//! (the other 17 functions are `unported` there, ledger t094); this module checks all 22.
//! Every row is compared bit-exactly (f64 by `to_bits`, NaN matching any NaN; bigints exactly;
//! throws by message). Rows that cannot be represented are counted and must be zero.

#![allow(clippy::disallowed_methods, clippy::unwrap_used, clippy::expect_used)] // test code

use super::config::NukeMagnitude;
use super::engine_config::{
    AttackLogicAttacker, AttackLogicDefender, AttackLogicInput, CostInputs, EngineConfig, SamUpgrade, TrainRelation,
};
use crate::api::game_config::GameConfig;
use crate::api::types::{PlayerInfo, PlayerType, TerrainType, UnitInfo, UnitType};
use crate::api::views::{PlayerLike, UnitLike};
use crate::ids::{SmallId, TileRef};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

fn vectors_path() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("OF_VECTORS_DIR") {
        return Path::new(&d).join("config.json.zst");
    }
    // The nearest ancestor holding `conformance/` (engine-rs/ and work/rs-P3/ sit at different depths).
    let mut d = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        let c = d.join("conformance/golden/vectors/config.json.zst");
        if c.exists() {
            return c;
        }
        d = d.parent().expect("no conformance/golden/vectors above CARGO_MANIFEST_DIR (set OF_VECTORS_DIR)");
    }
}

/// `num`: JSON int, or `"<16 hex IEEE-754 bits>:<decimal>"` (hex is normative).
fn dnum(v: &Value) -> f64 {
    match v {
        Value::Number(n) => {
            n.as_i64().map(|i| i as f64).or_else(|| n.as_u64().map(|u| u as f64)).unwrap_or_else(|| n.as_f64().unwrap())
        }
        Value::String(s) => f64::from_bits(u64::from_str_radix(s.split(':').next().unwrap(), 16).unwrap()),
        _ => panic!("not a num: {v}"),
    }
}
fn same(got: f64, want: f64) -> bool {
    if want.is_nan() { got.is_nan() } else { got.to_bits() == want.to_bits() }
}
fn dbig(v: &Value) -> i128 {
    v.as_str().unwrap().strip_suffix('n').unwrap().parse().unwrap()
}
fn is_hex_num(s: &str) -> bool {
    s.len() > 17 && s.as_bytes()[16] == b':' && s[..16].bytes().all(|b| b.is_ascii_hexdigit())
}
/// Replace every encoded `num` string by a JSON number (for serde of the GameConfig variants).
fn decode_nums(v: &Value) -> Value {
    match v {
        Value::String(s) if is_hex_num(s) => serde_json::Number::from_f64(dnum(v)).map(Value::Number).unwrap(),
        Value::Array(a) => Value::Array(a.iter().map(decode_nums).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), decode_nums(x))).collect()),
        _ => v.clone(),
    }
}
fn throws_msg(v: &Value) -> Option<&str> {
    v.get("throws").and_then(Value::as_str)
}
/// Run `f`; `Err("Error: <msg>")` (the vector's `Name: msg` form) when it raised a `TsThrow`.
fn catch<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| {
        let t = of_det::throw::as_ts_throw(&*p).expect("a TsThrow, not a Rust panic");
        format!("{:?}: {}", t.kind, t.msg)
    })
}
fn ptype(s: &str) -> PlayerType {
    PlayerType::from_ts(s).unwrap()
}
fn utype(s: &str) -> UnitType {
    UnitType::from_ts(s).unwrap()
}

// ---- duck-typed stubs (`conformance/vectors/eng.ts`) ----
#[derive(Clone)]
struct StubPlayer {
    t: PlayerType,
    tiles: f64,
    troops: f64,
    gold: i64,
    creator: bool,
    cities: Vec<StubCity>,
}
#[derive(Clone, Copy)]
struct StubCity {
    level: f64,
    under_construction: bool,
}
impl PlayerLike for StubPlayer {
    fn id(&self) -> &str {
        "stub"
    }
    fn small_id(&self) -> SmallId {
        SmallId(1)
    }
    fn is_player(&self) -> bool {
        true
    }
    fn player_type(&self) -> PlayerType {
        self.t
    }
    fn is_lobby_creator(&self) -> bool {
        self.creator
    }
    fn num_tiles_owned(&self) -> f64 {
        self.tiles
    }
    fn troops(&self) -> f64 {
        self.troops
    }
    fn gold(&self) -> i64 {
        self.gold
    }
    fn units(&self, t: UnitType) -> impl ExactSizeIterator<Item = impl UnitLike + '_> + '_ {
        let v: &[StubCity] = if t == UnitType::City { &self.cities } else { &[] };
        v.iter().copied()
    }
}
impl UnitLike for StubCity {
    fn unit_type(&self) -> UnitType {
        UnitType::City
    }
    fn owner(&self) -> impl PlayerLike + '_ {
        StubPlayer { t: PlayerType::Human, tiles: 0.0, troops: 0.0, gold: 0, creator: false, cities: vec![] }
    }
    fn tile(&self) -> TileRef {
        0
    }
    fn last_tile(&self) -> TileRef {
        0
    }
    fn is_active(&self) -> bool {
        true
    }
    fn is_under_construction(&self) -> bool {
        self.under_construction
    }
    fn level(&self) -> f64 {
        self.level
    }
}
/// `player = [type, numTilesOwned, troops, gold, isLobbyCreator, cities:[level, underConstruction][]]`.
fn stub_player(v: &Value) -> StubPlayer {
    let a = v.as_array().unwrap();
    StubPlayer {
        t: ptype(a[0].as_str().unwrap()),
        tiles: dnum(&a[1]),
        troops: dnum(&a[2]),
        gold: i64::try_from(dbig(&a[3])).unwrap(),
        creator: a[4].as_bool().unwrap(),
        cities: a[5]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| StubCity { level: dnum(&c[0]), under_construction: c[1].as_bool().unwrap() })
            .collect(),
    }
}
fn info(t: PlayerType, creator: bool) -> PlayerInfo {
    let mut i = PlayerInfo::new("stub", t, None, "stubid");
    i.is_lobby_creator = creator;
    i
}

/// A typed result, compared against an encoded vector value.
enum Out {
    Num(f64),
    Bool(bool),
    Str(&'static str),
    Obj(Vec<(&'static str, Out)>),
}
fn out_eq(o: &Out, v: &Value) -> bool {
    match o {
        Out::Num(x) => matches!(v, Value::Number(_) | Value::String(_)) && same(*x, dnum(v)),
        Out::Bool(b) => v.as_bool() == Some(*b),
        Out::Str(s) => v.as_str() == Some(*s),
        Out::Obj(fields) => {
            let Some(m) = v.as_object() else { return false };
            m.len() == fields.len() && fields.iter().all(|(k, x)| m.get(*k).is_some_and(|y| out_eq(x, y)))
        }
    }
}
fn speed_name(s: of_det::util::DoomsdaySpeed) -> &'static str {
    use of_det::util::DoomsdaySpeed::*;
    match s {
        Slow => "slow",
        Normal => "normal",
        Fast => "fast",
        VeryFast => "veryfast",
    }
}
/// Every zero-argument method on the TS prototype chain, by TS name.
fn zero_arg(c: &EngineConfig, m: &str) -> Out {
    use Out::*;
    let i = |t: i32| Num(f64::from(t));
    match m {
        "SAMCooldown" => Num(c.sam_cooldown()),
        "SiloCooldown" => Num(c.silo_cooldown()),
        "allianceDuration" => i(c.alliance_duration()),
        "allianceExtensionPromptOffset" => Num(c.alliance_extension_prompt_offset()),
        "allianceRequestCooldown" => i(c.alliance_request_cooldown()),
        "allianceRequestDuration" => i(c.alliance_request_duration()),
        "armyLimitWarningThreshold" => Num(c.army_limit_warning_threshold()),
        "boatMaxNumber" => Num(c.boat_max_number()),
        "bots" => Num(c.bots()),
        "cityTroopIncrease" => Num(c.city_troop_increase()),
        "defaultNukeTargetableRange" => Num(c.default_nuke_targetable_range()),
        "defaultSamMissileSpeed" => Num(c.default_sam_missile_speed()),
        "defaultSamRange" => Num(c.default_sam_range()),
        "defensePostDefenseBonus" => Num(c.defense_post_defense_bonus()),
        "defensePostRange" => Num(c.defense_post_range()),
        "defensePostShellAttackRate" => Num(c.defense_post_shell_attack_rate()),
        "defensePostSpeedBonus" => Num(c.defense_post_speed_bonus()),
        "defensePostTargettingRange" => Num(c.defense_post_targetting_range()),
        "deleteUnitCooldown" => i(c.delete_unit_cooldown()),
        "deletionMarkDuration" => i(c.deletion_mark_duration()),
        "disableAlliances" => Bool(c.disable_alliances()),
        "disableNavMesh" => Bool(c.disable_nav_mesh()),
        "donateCooldown" => i(c.donate_cooldown()),
        "donateGold" => Bool(c.donate_gold()),
        "donateTroops" => Bool(c.donate_troops()),
        "doomsdayClockConfig" => {
            let d = c.doomsday_clock_config();
            Obj(vec![
                ("enabled", Bool(d.enabled)),
                ("speed", Str(speed_name(d.speed))),
                ("warnSeconds", Num(d.warn_seconds)),
                ("drainStartPercent", Num(d.drain_start_percent)),
                ("drainMaxPercent", Num(d.drain_max_percent)),
                ("drainRampSeconds", Num(d.drain_ramp_seconds)),
                ("drainFloorPercent", Num(d.drain_floor_percent)),
                ("floorStartPercent", Num(d.floor_start_percent)),
                ("floorDecaySeconds", Num(d.floor_decay_seconds)),
                ("rotDeathSeconds", Num(d.rot_death_seconds)),
                ("rotGrainSeconds", Num(d.rot_grain_seconds)),
                ("rotSpecklePercent", Num(d.rot_speckle_percent)),
                ("warshipDrainStartPercent", Num(d.warship_drain_start_percent)),
                ("warshipDrainMaxPercent", Num(d.warship_drain_max_percent)),
                ("warshipDrainCurveExponent", Num(d.warship_drain_curve_exponent)),
            ])
        }
        "embargoAllCooldown" => i(c.embargo_all_cooldown()),
        "emojiMessageDuration" => i(c.emoji_message_duration()),
        "emojiMessageLimit" => Num(c.emoji_message_limit()),
        "emojiMessageWindow" => i(c.emoji_message_window()),
        "goldMultiplier" => Num(c.gold_multiplier()),
        "hasExtendedSpawnImmunity" => Bool(c.has_extended_spawn_immunity()),
        "infiniteGold" => Bool(c.infinite_gold()),
        "infiniteTroops" => Bool(c.infinite_troops()),
        "instantBuild" => Bool(c.instant_build()),
        "isIntentionalSpectator" => Bool(c.is_intentional_spectator()),
        "isRandomSpawn" => Bool(c.is_random_spawn()),
        "isReplay" => Bool(c.is_replay()),
        "maxSamRange" => Num(c.max_sam_range()),
        "minDistanceBetweenPlayers" => Num(c.min_distance_between_players()),
        "mirvNormalizeTargetTicks" => Num(c.mirv_normalize_target_ticks()),
        "msPerTick" => Num(c.ms_per_tick()),
        "nationSpawnImmunityDuration" => i(c.nation_spawn_immunity_duration()),
        "nukeAllianceBreakThreshold" => Num(c.nuke_alliance_break_threshold()),
        "numBots" => Num(c.num_bots()),
        "numSpawnPhaseTurns" => Num(c.num_spawn_phase_turns()),
        "overtimeConfig" => {
            let o = c.overtime_config();
            Obj(vec![
                ("enabled", Bool(o.enabled)),
                ("startMinutes", Num(o.start_minutes)),
                ("dropPercentPerMinute", Num(o.drop_percent_per_minute)),
            ])
        }
        "playerTeams" => match c.player_teams() {
            crate::api::game_config::TeamCountConfig::Count(n) => Num(n as f64),
            other => Str(other.preset_str().unwrap()),
        },
        "quickChatCooldown" => i(c.quick_chat_cooldown()),
        "railroadMaxSize" => Num(c.railroad_max_size()),
        "safeFromPiratesCooldownMax" => Num(c.safe_from_pirates_cooldown_max()),
        "samUpgradeDuration" => i(c.sam_upgrade_duration()),
        "shellLifetime" => Num(c.shell_lifetime()),
        "spawnImmunityDuration" => i(c.spawn_immunity_duration()),
        "spawnNations" => Bool(c.spawn_nations()),
        "structureMinDist" => Num(c.structure_min_dist()),
        "targetCooldown" => i(c.target_cooldown()),
        "targetDuration" => i(c.target_duration()),
        "teamLandShareWinThresholdTenths" => Num(c.team_land_share_win_threshold_tenths()),
        "temporaryEmbargoDuration" => i(c.temporary_embargo_duration()),
        "tradeShipShortRangeDebuff" => Num(c.trade_ship_short_range_debuff()),
        "trainStationMaxRange" => Num(c.train_station_max_range()),
        "trainStationMinRange" => Num(c.train_station_min_range()),
        "traitorDefenseDebuff" => Num(c.traitor_defense_debuff()),
        "traitorDuration" => i(c.traitor_duration()),
        "traitorSpeedDebuff" => Num(c.traitor_speed_debuff()),
        "warshipDockingRange" => Num(c.warship_docking_range()),
        "warshipMaxVeterancy" => Num(c.warship_max_veterancy()),
        "warshipPassiveHealing" => Num(c.warship_passive_healing()),
        "warshipPassiveHealingRange" => Num(c.warship_passive_healing_range()),
        "warshipPatrolRange" => Num(c.warship_patrol_range()),
        "warshipPortHealingBonusPerLevel" => Num(c.warship_port_healing_bonus_per_level()),
        "warshipPortSwitchThreshold" => Num(c.warship_port_switch_threshold()),
        "warshipRetreatHealthPercent" => Num(c.warship_retreat_health_percent()),
        "warshipShellAttackRate" => Num(c.warship_shell_attack_rate()),
        "warshipShellLifetime" => Num(c.warship_shell_lifetime()),
        "warshipTargettingRange" => Num(c.warship_targetting_range()),
        "warshipVeterancyHealthBonus" => Num(c.warship_veterancy_health_bonus()),
        "warshipVeterancyShellDamageBonus" => Num(c.warship_veterancy_shell_damage_bonus()),
        "warshipVeterancyTradeCaptures" => Num(c.warship_veterancy_trade_captures()),
        "warshipVeterancyTransportKills" => Num(c.warship_veterancy_transport_kills()),
        "waterNukes" => Bool(c.water_nukes()),
        _ => panic!("zero_arg_methods: no Rust method mapped for {m}"),
    }
}
fn unit_info_eq(u: UnitInfo, v: &Value) -> bool {
    let m = v.as_object().unwrap();
    // `cost` is the closure (encoded as null); every other key must match exactly.
    let mut want = 1; // cost
    let mut ok = m.get("cost") == Some(&Value::Null);
    let mut num = |k: &str, x: Option<f64>| match x {
        Some(x) => {
            want += 1;
            ok &= m.get(k).is_some_and(|y| same(x, dnum(y)));
        }
        None => ok &= !m.contains_key(k),
    };
    num("maxHealth", u.max_health);
    num("damage", u.damage);
    num("constructionDuration", u.construction_duration);
    match u.upgradable {
        Some(b) => {
            want += 1;
            ok &= m.get("upgradable").and_then(Value::as_bool) == Some(b);
        }
        None => ok &= !m.contains_key("upgradable"),
    }
    ok && m.len() == want
}

#[derive(Default)]
struct Tally {
    rows: BTreeMap<String, (usize, usize, Option<String>)>,
}
impl Tally {
    fn row(&mut self, f: &str, ok: bool, ctx: impl FnOnce() -> String) {
        let e = self.rows.entry(f.to_owned()).or_default();
        e.0 += 1;
        if !ok {
            e.1 += 1;
            if e.2.is_none() {
                e.2 = Some(ctx());
            }
        }
    }
}

#[test]
fn config_vector_suite_all_functions() {
    let p = vectors_path();
    let raw = of_trace::trace::zstd_read(&p).unwrap_or_else(|e| panic!("reading {}: {e:#}", p.display()));
    let v: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(v["schema"], "openfront-conformance-vectors/1");
    let mut cfgs: BTreeMap<String, EngineConfig> = BTreeMap::new();
    for (name, gc) in v["context"]["configs"].as_object().unwrap() {
        let gc: GameConfig = serde_json::from_value(decode_nums(gc)).unwrap_or_else(|e| panic!("config {name}: {e}"));
        cfgs.insert(name.clone(), EngineConfig::new(gc, false, false, false));
    }
    let unit_types: Vec<UnitType> =
        v["context"]["unit_types"].as_array().unwrap().iter().map(|s| utype(s.as_str().unwrap())).collect();
    let cfg = |r: &Value| &cfgs[r.as_str().unwrap()];
    let medium = &cfgs["medium"];
    let mut t = Tally::default();
    let funcs = v["functions"].as_object().unwrap();
    for (fname, f) in funcs {
        let cases = f["cases"].as_array().unwrap();
        for row in cases {
            let r = row.as_array().unwrap();
            let fail = || format!("{fname} {row}");
            match fname.as_str() {
                "zero_arg_methods" => {
                    let got = zero_arg(cfg(&r[0]), r[1].as_str().unwrap());
                    t.row(fname, out_eq(&got, &r[2]), fail);
                }
                "maxTroops" => {
                    let (c, p) = (cfg(&r[0]), stub_player(&r[1]));
                    let ok = same(c.max_troops(&p), dnum(&r[2])) && same(c.max_troops_without_cities(&p), dnum(&r[3]));
                    t.row(fname, ok, fail);
                }
                "troopIncreaseRate" => {
                    let (c, p) = (cfg(&r[0]), stub_player(&r[1]));
                    t.row(fname, same(c.troop_increase_rate(&p), dnum(&r[2])), fail);
                }
                "goldAdditionRate" => {
                    let g = cfg(&r[0]).gold_addition_rate_with(ptype(r[1].as_str().unwrap()), r[2].as_bool().unwrap());
                    t.row(fname, i128::from(g) == dbig(&r[3]), fail);
                }
                "trainGold" => {
                    let rel = match r[3].as_str().unwrap() {
                        "self" => TrainRelation::SelfOwned,
                        "team" => TrainRelation::Team,
                        "ally" => TrainRelation::Ally,
                        "other" => TrainRelation::Other,
                        x => panic!("rel {x}"),
                    };
                    // playerType does not enter trainGold (only isLobbyCreator does).
                    let g = cfg(&r[0]).train_gold_with(rel, dnum(&r[4]), r[2].as_bool().unwrap());
                    t.row(fname, i128::from(g) == dbig(&r[5]), fail);
                }
                "tradeShipGold" => {
                    let g = cfg(&r[0]).trade_ship_gold_with(dnum(&r[2]), r[1].as_bool().unwrap());
                    t.row(fname, i128::from(g) == dbig(&r[3]), fail);
                }
                "conquerGoldAmount" => {
                    let g = medium
                        .conquer_gold_amount_with(ptype(r[0].as_str().unwrap()), i64::try_from(dbig(&r[1])).unwrap());
                    t.row(fname, i128::from(g) == dbig(&r[2]), fail);
                }
                "saturation" => {
                    let x = dnum(&r[0]);
                    let ok = same(medium.train_saturation(x), dnum(&r[1]))
                        && same(medium.trade_ship_saturation(x), dnum(&r[2]));
                    t.row(fname, ok, fail);
                }
                "smallEconomySpawnBoost" => {
                    t.row(fname, same(medium.small_economy_spawn_boost(dnum(&r[0])), dnum(&r[1])), fail);
                }
                "tradeShipSpawnRate" => {
                    let g = medium.trade_ship_spawn_rate(dnum(&r[0]), dnum(&r[1]), dnum(&r[2]));
                    t.row(fname, same(g, dnum(&r[3])), fail);
                }
                "trainSpawnRate" => {
                    t.row(fname, same(medium.train_spawn_rate(dnum(&r[0]), dnum(&r[1])), dnum(&r[2])), fail);
                }
                "unitInfo" => {
                    let u = cfg(&r[0]).unit_info(utype(r[1].as_str().unwrap()));
                    t.row(fname, unit_info_eq(u, &r[2]), fail);
                }
                "unitCost" => {
                    let c = cfg(&r[0]);
                    let counts: Vec<(f64, f64)> =
                        r[3].as_array().unwrap().iter().map(|x| (dnum(&x[0]), dnum(&x[1]))).collect();
                    let inp = CostInputs {
                        player_type: ptype(r[1].as_str().unwrap()),
                        is_lobby_creator: r[2].as_bool().unwrap(),
                        counts: |ty: UnitType| counts[unit_types.iter().position(|&u| u == ty).unwrap()],
                        extra_units: dnum(&r[4]),
                        mirvs_launched: dnum(&r[5]),
                    };
                    let want = r[6].as_array().unwrap();
                    let ok = unit_types
                        .iter()
                        .enumerate()
                        .all(|(i, &ty)| i128::from(c.unit_cost_with(ty, &inp)) == dbig(&want[i]));
                    t.row(fname, ok, fail);
                }
                "attackLogic" => {
                    let def = (!r[4].is_null()).then(|| AttackLogicDefender {
                        player_type: ptype(r[4][0].as_str().unwrap()),
                        num_tiles: dnum(&r[4][1]),
                        troops: dnum(&r[4][2]),
                        is_traitor: r[4][3].as_bool().unwrap(),
                        is_disconnected_teammate: r[4][4].as_bool().unwrap(),
                    });
                    let code = u8::try_from(r[0].as_u64().unwrap()).unwrap();
                    let input = AttackLogicInput {
                        terrain: TerrainType::Plains, // ignored: the numeric code below is used
                        attack_troops: dnum(&r[1]),
                        attacker: AttackLogicAttacker {
                            player_type: ptype(r[2].as_str().unwrap()),
                            num_tiles: dnum(&r[3]),
                        },
                        defender: def,
                        defender_has_defense_post: r[5].as_bool().unwrap(),
                        fallout_ratio: (!r[6].is_null()).then(|| dnum(&r[6])),
                        border_size: dnum(&r[7]),
                    };
                    let got = catch(|| medium.attack_logic_code(&input, code));
                    let ok = match (got, throws_msg(&r[8])) {
                        (Err(m), Some(w)) => m == w,
                        (Ok(res), None) => {
                            let w = r[8].as_array().unwrap();
                            same(res.attacker_troop_loss, dnum(&w[0]))
                                && same(res.defender_troop_loss, dnum(&w[1]))
                                && same(res.tick_fraction, dnum(&w[2]))
                        }
                        _ => false,
                    };
                    t.row(fname, ok, fail);
                }
                "troop_amounts" => {
                    let p = StubPlayer {
                        t: ptype(r[0].as_str().unwrap()),
                        tiles: 0.0,
                        troops: dnum(&r[1]),
                        gold: 0,
                        creator: false,
                        cities: vec![],
                    };
                    let none: Option<&StubPlayer> = None;
                    let ok = same(medium.attack_amount(&p, none), dnum(&r[2]))
                        && same(medium.boat_attack_amount(&p, none), dnum(&r[3]))
                        && same(medium.default_donation_amount(&p), dnum(&r[4]));
                    t.row(fname, ok, fail);
                }
                "start_manpower_gold" => {
                    let c = cfg(&r[0]);
                    let i = info(ptype(r[1].as_str().unwrap()), r[2].as_bool().unwrap());
                    let ok = same(c.start_manpower(&i), dnum(&r[3])) && i128::from(c.starting_gold(&i)) == dbig(&r[4]);
                    t.row(fname, ok, fail);
                }
                "nukeMagnitudes_nukeSpeed" => {
                    let (c, ty) = (cfg(&r[0]), utype(r[1].as_str().unwrap()));
                    let ok_m = match (catch(|| c.nuke_magnitudes(ty)), throws_msg(&r[2])) {
                        (Err(m), Some(w)) => m == w,
                        (Ok(NukeMagnitude { inner, outer }), None) => {
                            same(inner, dnum(&r[2]["inner"])) && same(outer, dnum(&r[2]["outer"]))
                        }
                        _ => false,
                    };
                    let ok_s = match (catch(|| c.nuke_speed(ty)), throws_msg(&r[3])) {
                        (Err(m), Some(w)) => m == w,
                        (Ok(s), None) => same(s, dnum(&r[3])),
                        _ => false,
                    };
                    t.row(fname, ok_m && ok_s, fail);
                }
                "samRange" => t.row(fname, same(medium.sam_range(dnum(&r[0])), dnum(&r[1])), fail),
                "dynamicSamRange" => {
                    let st = (!r[1].is_null()).then(|| {
                        let o = &r[1];
                        let opt = |k: &str| o.get(k).filter(|x| !x.is_null()).map(dnum);
                        SamUpgrade {
                            upgrade_start_tick: opt("upgradeStartTick"),
                            target_level: dnum(&o["targetLevel"]),
                            start_range: dnum(&o["startRange"]),
                            duration: opt("duration"),
                        }
                    });
                    let g = medium.dynamic_sam_range_with(dnum(&r[0]), st, dnum(&r[2]));
                    t.row(fname, same(g, dnum(&r[3])), fail);
                }
                "nukeDeathFactor" => {
                    let g =
                        medium.nuke_death_factor(utype(r[0].as_str().unwrap()), dnum(&r[1]), dnum(&r[2]), dnum(&r[3]));
                    t.row(fname, same(g, dnum(&r[4])), fail);
                }
                "misc_formulas" => {
                    let x = dnum(&r[1]);
                    let g = match r[0].as_str().unwrap() {
                        "falloutDefenseModifier" => medium.fallout_defense_modifier(x),
                        "proximityBonusPortsNb" => medium.proximity_bonus_ports_nb(x),
                        m => panic!("misc {m}"),
                    };
                    t.row(fname, same(g, dnum(&r[2])), fail);
                }
                "percentageTilesOwnedToWin" => {
                    let (c, e) = (cfg(&r[0]), dnum(&r[1]));
                    let ok = same(c.percentage_tiles_owned_to_win_tenths(e), dnum(&r[2]))
                        && same(c.percentage_tiles_owned_to_win(e), dnum(&r[3]));
                    t.row(fname, ok, fail);
                }
                other => panic!("config suite: function {other} not handled (new vectors?)"),
            }
        }
    }
    let total: usize = t.rows.values().map(|e| e.0).sum();
    let failed: Vec<String> = t
        .rows
        .iter()
        .filter(|(_, e)| e.1 > 0)
        .map(|(k, e)| format!("{k}: {}/{} fail, first {}", e.1, e.0, e.2.as_deref().unwrap_or("")))
        .collect();
    eprintln!("config vectors: {} functions, {total} rows, {} failing functions", t.rows.len(), failed.len());
    for (k, e) in &t.rows {
        eprintln!("  {k}: {} rows, {} fail", e.0, e.1);
    }
    assert_eq!(t.rows.len(), 22, "all 22 config functions covered");
    assert_eq!(total, 79_584, "VECTORS §1 row count");
    assert!(failed.is_empty(), "config vectors failing:\n{}", failed.join("\n"));
}
