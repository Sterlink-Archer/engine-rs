//! `Stats` (`engine/src/game/Stats.ts`, `StatsImpl.ts`). Owner: **P2**. ARCHITECTURE §3.10.
//!
//! - **Always on** (the lite build): per clientID, `attacks_sent` (the one stats value the sim
//!   reads, `GameImpl.conquerPlayer`, `GameImpl.ts:1777-1781`). Every entry point keeps the
//!   clientID gate and the strict `BigInt(Math.floor(x))` conversions of `_bigint`
//!   (`StatsImpl.ts:58-65, 116-130`), discarding the results, so fatal throws happen at the
//!   same tick in every build.
//! - `feature = "stats-full"`: the whole `StatsImpl` (CS S), including `_sampleCache`.
//!
//! TS stats methods take `Player` objects and read their `clientID()`; the port reaches the
//! players through [`StatsCx`] (`cx.st.stats_cx()`), which splits the borrow of `State`.
//! Method names and argument order are the `Stats` interface's (`Stats.ts`).
//!
//! Statement order inside every `_addX`/`_maxX` helper is TS's: the record is created
//! (`_makePlayerStats`), the array is created/extended (`??=`, `push(0n)`), and only then is
//! the value converted with `_bigint` (which may throw). A `TsThrow` therefore leaves the
//! record and the zero-extended array behind, exactly like TS.
//!
//! **Key order.** `data` is a plain object keyed by clientID; CSE1 sorts object keys
//! (SPEC Q-OBJKEYS), so the insertion order kept by the `JsMap`s is not observable in CS.

pub mod schema;

use crate::StateClone;
use crate::api::types::{ClientID, PlayerInfo, PlayerType, UnitType};
use crate::ids::{Owner, SmallId, TileRef};
use crate::jsorder::JsMap;
use crate::state::State;
use crate::state::player::Player;
use of_det::bigint::from_floor_strict;
use schema::*;

/// `number | bigint` stats arguments (`_bigint`: a number is `BigInt(Math.floor(v))`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StatNum {
    Num(f64),
    Big(i64),
}

impl From<f64> for StatNum {
    fn from(v: f64) -> Self {
        StatNum::Num(v)
    }
}
impl From<i64> for StatNum {
    fn from(v: i64) -> Self {
        StatNum::Big(v)
    }
}

impl StatNum {
    /// `_bigint(value)` (`StatsImpl.ts:59-66`): a bigint passes through; a number is
    /// `BigInt(Math.floor(value))`, which throws (RangeError → `TsThrow`) on NaN/±Inf.
    #[inline]
    #[track_caller]
    pub fn bigint(self) -> i64 {
        match self {
            StatNum::Big(b) => b,
            StatNum::Num(v) => from_floor_strict(v),
        }
    }
    /// TS unary `-troops` on a `BigIntLike` (`StatsImpl.ts:279,281`): a number negates as a
    /// number (so `_bigint(-x)` floors `-x`, **not** `-floor(x)`), a bigint as a bigint.
    #[inline]
    fn neg(self) -> StatNum {
        match self {
            StatNum::Num(v) => StatNum::Num(-v),
            StatNum::Big(b) => StatNum::Big(-b),
        }
    }
}

/// `allianceEnded` counter (`Stats.ts`): `"brokenByOther" | "expired" | null`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllianceEndCounter {
    BrokenByOther,
    Expired,
}

/// The always-on per-client record (lite build).
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct PlayerStatsLite {
    /// `attacks[ATTACK_INDEX_SENT]` (bigint; `0n` when the array is absent).
    pub attacks_sent: i64,
}

/// `StatsImpl` data (`Stats v1` = `{ data: record<str, unknown> }`, keyed by clientID).
#[derive(StateClone, Debug, Default)]
pub struct Stats {
    /// Per clientID, in first-touch order (`data` object key order).
    pub lite: JsMap<ClientID, PlayerStatsLite>,
    /// non-snapshot as a separate key: with `stats-full`, the full `AllPlayersStats` record
    /// (P2 defines its shape); CS `data` is exported from `lite` + `full`.
    #[cfg(feature = "stats-full")]
    pub full: StatsFull,
}

/// `AllPlayersStats` (`stats-full`): one [`PlayerStatsRec`] per clientID, created at the
/// same moments as the lite records (they are always in step).
#[cfg(feature = "stats-full")]
#[derive(StateClone, Debug, Default)]
pub struct StatsFull {
    pub data: JsMap<ClientID, PlayerStatsRec>,
}

/// `{ victim, tick }` (`StatsSchemas.ts:180-182`).
#[cfg(feature = "stats-full")]
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct KillRec {
    pub victim: ClientID,
    pub tick: i64,
}

/// `_sampleCache` entry (`StatsImpl.ts:74-82`), keyed in TS by the `PlayerStats` object in a
/// `WeakMap`; here it lives inside that record (ARCHITECTURE §3.11). non-snapshot (a TS
/// restore drops it; `from_canon` sets `None`).
#[cfg(feature = "stats-full")]
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct SampleCache {
    pub peak: f64,
    pub dd_peak: f64,
    pub dd_trough: f64,
    pub peak_troops: f64,
    pub peak_alliance: f64,
}

/// `PlayerStats` (`StatsSchemas.ts:167-200`), every key optional. The partial records
/// `boats`/`bombs`/`units` are arrays indexed by the unit key (`BOAT_KEYS`, `BOMB_KEYS`,
/// `OTHER_KEYS`); their key order is unobservable (CSE1 sorts object keys).
#[cfg(feature = "stats-full")]
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct PlayerStatsRec {
    pub attacks: Option<Vec<i64>>,
    pub betrayals: Option<i64>,
    pub killed_at: Option<i64>,
    /// `killedBy?: string | null` (absent = unstamped).
    pub killed_by: crate::jsval::Tri<ClientID>,
    /// `deathPosition?: number`.
    pub death_position: Option<f64>,
    pub final_tiles: Option<i64>,
    pub kills: Option<Vec<KillRec>>,
    pub conquests: Option<Vec<i64>>,
    /// `boats: { trade?, trans? }`.
    pub boats: Option<[Option<Vec<i64>>; 2]>,
    /// `bombs: { abomb?, hbomb?, mirv?, mirvw? }`.
    pub bombs: Option<[Option<Vec<i64>>; 4]>,
    pub gold: Option<Vec<i64>>,
    /// `units: { city?, defp?, port?, wshp?, silo?, saml?, fact? }`.
    pub units: Option<[Option<Vec<i64>>; 7]>,
    pub tiles: Option<Vec<i64>>,
    pub alliances: Option<Vec<i64>>,
    pub peak_troops: Option<i64>,
    pub donations: Option<Vec<i64>>,
    pub spawn_tile: Option<TileRef>,
    /// non-snapshot: `_sampleCache.get(p)`.
    pub sample_cache: Option<SampleCache>,
}

/// Index order of [`PlayerStatsRec::boats`]/`bombs`/`units`: the schema key lists.
#[cfg(feature = "stats-full")]
use schema::{BOAT_UNITS as BOAT_KEYS, BOMB_UNITS as BOMB_KEYS, OTHER_UNITS as OTHER_KEYS};

#[cfg(feature = "stats-full")]
fn key_index(keys: &[&str], k: &str) -> usize {
    #[allow(clippy::expect_used)] // rust-invariant: keys come from the schema maps
    keys.iter().position(|x| *x == k).expect("stats key in table")
}

/// `while (arr.length <= index) arr.push(0n)` after `arr ??= init` (every `_addX` helper).
#[cfg(feature = "stats-full")]
fn ensure_arr<'a>(slot: &'a mut Option<Vec<i64>>, init: &[i64], index: usize) -> &'a mut Vec<i64> {
    let arr = slot.get_or_insert_with(|| init.to_vec());
    while arr.len() <= index {
        arr.push(0);
    }
    arr
}

/// `p.boats ??= { [type]: [0n] }; p.boats[type] ??= [0n]; while (..) push(0n)` (`:158-160`).
#[cfg(feature = "stats-full")]
fn ensure_keyed<const N: usize>(slot: &mut Option<[Option<Vec<i64>>; N]>, k: usize, index: usize) -> &mut Vec<i64> {
    let rec = slot.get_or_insert_with(|| {
        let mut r: [Option<Vec<i64>>; N] = std::array::from_fn(|_| None);
        r[k] = Some(vec![0]); // in-range: k < N by construction
        r
    });
    ensure_arr(&mut rec[k], &[0], index) // in-range: k < N by construction
}

impl Stats {
    /// `getPlayerStats(player)` (`StatsImpl.ts:96-100`): `None` for a player without a clientID
    /// (`client_id == None`) or with no record yet. The lite build only holds `attacks_sent`.
    pub fn get_player_stats(&self, client_id: Option<&str>) -> Option<&PlayerStatsLite> {
        let cid = client_id?;
        self.lite.get(&cid.to_owned())
    }
    /// `getPlayerStats(player)?.attacks?.[ATTACK_INDEX_SENT] ?? 0n` for a clientID (the
    /// `conquerPlayer` read). `None` client → `0`.
    pub fn attacks_sent(&self, client_id: Option<&str>) -> i64 {
        // `attacks` absent → `?.` gives undefined → `?? 0n`; lite keeps 0 for that case, and
        // `attacks[0]` is never undefined once the array exists (`??= [0n]`).
        self.get_player_stats(client_id).map_or(0, |p| p.attacks_sent)
    }

    /// `_makePlayerStats` for clientID `cid` (`:114-124`): the existing record or a new `{}`.
    fn make(&mut self, cid: &str) {
        let key: ClientID = cid.to_owned();
        if !self.lite.contains_key(&key) {
            #[cfg(feature = "stats-full")]
            self.full.data.insert(key.clone(), PlayerStatsRec::default());
            self.lite.insert(key, PlayerStatsLite::default());
        }
    }

    #[cfg(feature = "stats-full")]
    fn rec(&mut self, cid: &str) -> &mut PlayerStatsRec {
        #[allow(clippy::expect_used)] // rust-invariant: `make` ran first
        self.full.data.get_mut(&cid.to_owned()).expect("stats record made")
    }

    /// The full `AllPlayersStats` record for `cid` (`stats()[cid]`), if any.
    #[cfg(feature = "stats-full")]
    pub fn full_record(&self, cid: &str) -> Option<&PlayerStatsRec> {
        self.full.data.get(&cid.to_owned())
    }
}

/// `Stats` with access to the players (for `player.clientID()`): `st.stats_cx()`.
pub struct StatsCx<'a> {
    pub stats: &'a mut Stats,
    pub players: &'a [Player],
    pub infos: &'a [PlayerInfo],
}

impl State {
    /// Split borrow: stats + the read-only player tables.
    pub fn stats_cx(&mut self) -> StatsCx<'_> {
        StatsCx { stats: &mut self.stats, players: &self.players, infos: &self.infos }
    }
}

/// `player.clientID()` (`PlayerImpl.ts:438-440`) from the split tables.
fn client_of<'b>(players: &[Player], infos: &'b [PlayerInfo], p: SmallId) -> Option<&'b str> {
    let pl = &players[p.player_index()]; // in-range: smallIDs come from add_player
    infos[pl.info.0 as usize].client_id.as_deref() // in-range: indexes come from add_player
}

fn player_type_of(players: &[Player], infos: &[PlayerInfo], p: SmallId) -> PlayerType {
    let pl = &players[p.player_index()]; // in-range: smallIDs come from add_player
    infos[pl.info.0 as usize].player_type // in-range: indexes come from add_player
}

/// Index helpers for the three kinds of array the helpers write.
#[derive(Clone, Copy)]
enum ArrKind {
    Attacks,
    Gold,
    Donations,
    Conquests,
}

impl<'a> StatsCx<'a> {
    /// `_makePlayerStats(player)`: `Some(clientID)` with the record ensured, `None` for a
    /// player without a clientID (`:114-124`).
    fn make(&mut self, p: SmallId) -> Option<&'a str> {
        let cid = client_of(self.players, self.infos, p)?;
        self.stats.make(cid);
        Some(cid)
    }

    /// `_addAttack`/`_addGold`/`_addDonation`/`_addConquest` (`:126-132,179-193,210-216`):
    /// `arr ??= [0n]; while (arr.length <= index) arr.push(0n); arr[index] += _bigint(value)`.
    fn add_arr(&mut self, p: SmallId, kind: ArrKind, index: usize, value: StatNum) {
        let Some(cid) = self.make(p) else { return };
        #[cfg(feature = "stats-full")]
        let v = {
            let rec = self.stats.rec(cid);
            let slot = match kind {
                ArrKind::Attacks => &mut rec.attacks,
                ArrKind::Gold => &mut rec.gold,
                ArrKind::Donations => &mut rec.donations,
                ArrKind::Conquests => &mut rec.conquests,
            };
            let arr = ensure_arr(slot, &[0], index);
            let v = value.bigint(); // TS: `_bigint(value)` after the array exists
            arr[index] += v; // in-range: ensure_arr extended it
            v
        };
        #[cfg(not(feature = "stats-full"))]
        let v = value.bigint(); // TS: `_bigint(value)` (strict even in the lite build, §3.10)
        if matches!(kind, ArrKind::Attacks) && index == ATTACK_INDEX_SENT {
            #[allow(clippy::expect_used)] // rust-invariant: `make` ran first
            let lite = self.stats.lite.get_mut(&cid.to_owned()).expect("lite record made");
            lite.attacks_sent += v;
        }
    }

    /// `_maxAttack` (`:134-141`).
    fn max_attack(&mut self, p: SmallId, index: usize, value: StatNum) {
        let Some(cid) = self.make(p) else { return };
        #[cfg(feature = "stats-full")]
        {
            let arr = ensure_arr(&mut self.stats.rec(cid).attacks, &[0], index);
            let v = value.bigint();
            if v > arr[index] {
                arr[index] = v; // in-range: ensure_arr extended it
            }
        }
        #[cfg(not(feature = "stats-full"))]
        {
            let _ = (cid, index);
            let _ = value.bigint();
        }
        // ATTACK_INDEX_SENT is never maxed (only MAX_RECV is), so lite stays as is.
        debug_assert_ne!(index, ATTACK_INDEX_SENT);
    }

    /// `_addBoat(player, type, index, value)` (`:150-162`).
    fn add_boat(&mut self, p: SmallId, key: &str, index: usize, value: StatNum) {
        let Some(cid) = self.make(p) else { return };
        #[cfg(feature = "stats-full")]
        {
            let k = key_index(&BOAT_KEYS, key);
            let arr = ensure_keyed(&mut self.stats.rec(cid).boats, k, index);
            let v = value.bigint();
            arr[index] += v; // in-range: ensure_keyed extended it
        }
        #[cfg(not(feature = "stats-full"))]
        {
            let _ = (cid, key, index);
            let _ = value.bigint();
        }
    }

    /// `_addBomb(player, nukeType, index, value)` (`:164-177`): `unitTypeToBombUnit[nukeType]`
    /// is read before `_makePlayerStats`.
    fn add_bomb(&mut self, p: SmallId, t: UnitType, index: usize, value: StatNum) {
        let key = bomb_unit(t);
        let Some(cid) = self.make(p) else { return };
        // TS: `p.bombs[undefined]` for a non-nuke type would create an "undefined" key; every
        // caller passes a NukeType (`StatsSchemas.ts:8-12`).
        #[allow(clippy::expect_used)] // rust-invariant: callers pass a NukeType
        let key = key.expect("bombLaunch/bombLand/bombIntercept take a NukeType");
        #[cfg(feature = "stats-full")]
        {
            let k = key_index(&BOMB_KEYS, key);
            let arr = ensure_keyed(&mut self.stats.rec(cid).bombs, k, index);
            let v = value.bigint();
            arr[index] += v; // in-range: ensure_keyed extended it
        }
        #[cfg(not(feature = "stats-full"))]
        {
            let _ = (cid, key, index);
            let _ = value.bigint();
        }
    }

    /// `_addOtherUnit(player, otherUnitType, index, value)` (`:195-208`).
    fn add_other_unit(&mut self, p: SmallId, t: UnitType, index: usize, value: StatNum) {
        let key = other_unit(t);
        let Some(cid) = self.make(p) else { return };
        #[allow(clippy::expect_used)] // rust-invariant: callers pass an OtherUnitType
        let key = key.expect("unitBuild/Capture/Upgrade/Destroy/Lose take an OtherUnitType");
        #[cfg(feature = "stats-full")]
        {
            let k = key_index(&OTHER_KEYS, key);
            let arr = ensure_keyed(&mut self.stats.rec(cid).units, k, index);
            let v = value.bigint();
            arr[index] += v; // in-range: ensure_keyed extended it
        }
        #[cfg(not(feature = "stats-full"))]
        {
            let _ = (cid, key, index);
            let _ = value.bigint();
        }
    }

    /// `_allianceArray` + `_addAlliance` / `_maxAlliance` / `_setAlliance` (`:224-252`).
    fn alliance_op(&mut self, p: SmallId, index: usize, value: StatNum, op: AllianceOp) {
        let Some(cid) = self.make(p) else { return };
        #[cfg(feature = "stats-full")]
        {
            let arr = ensure_arr(&mut self.stats.rec(cid).alliances, &[0], index);
            let v = value.bigint();
            match op {
                AllianceOp::Add => arr[index] += v, // in-range: ensure_arr extended it
                AllianceOp::Max => {
                    if v > arr[index] {
                        arr[index] = v;
                    }
                }
                AllianceOp::Set => arr[index] = v,
            }
        }
        #[cfg(not(feature = "stats-full"))]
        {
            let _ = (cid, index, op);
            let _ = value.bigint();
        }
    }

    pub fn migrate_client_id(&mut self, old_client_id: &str, new_client_id: &str) {
        // TS `:106-112`.
        if old_client_id == new_client_id {
            return;
        }
        let old: ClientID = old_client_id.to_owned();
        if let Some(rec) = self.stats.lite.remove(&old) {
            // TS: `data[new] = data[old]` (an existing `new` key keeps its position), then
            // `delete data[old]`. Removing first is equivalent: `old != new`.
            self.stats.lite.insert(new_client_id.to_owned(), rec);
            #[cfg(feature = "stats-full")]
            if let Some(full) = self.stats.full.data.remove(&old) {
                self.stats.full.data.insert(new_client_id.to_owned(), full);
            }
        }
    }
    pub fn attack(&mut self, player: SmallId, target: Owner, troops: StatNum) {
        self.add_arr(player, ArrKind::Attacks, ATTACK_INDEX_SENT, troops);
        if target.is_player() {
            self.add_arr(target, ArrKind::Attacks, ATTACK_INDEX_RECV, troops);
        }
    }
    pub fn attack_max_incoming(&mut self, target: Owner, troops: StatNum) {
        if !target.is_player() {
            return;
        }
        self.max_attack(target, ATTACK_INDEX_MAX_RECV, troops);
    }
    /// `attackCancel` (floor asymmetry `floor(x) + floor(-x) = -1`, ARCHITECTURE §3.10).
    pub fn attack_cancel(&mut self, player: SmallId, target: Owner, troops: StatNum) {
        self.add_arr(player, ArrKind::Attacks, ATTACK_INDEX_CANCEL, troops);
        self.add_arr(player, ArrKind::Attacks, ATTACK_INDEX_SENT, troops.neg()); // TS: `-troops` (`:279`)
        if target.is_player() {
            self.add_arr(target, ArrKind::Attacks, ATTACK_INDEX_RECV, troops.neg());
        }
    }
    pub fn betray(&mut self, player: SmallId) {
        // `_addBetrayal(player, 1)` (`:143-148`).
        let Some(cid) = self.make(player) else { return };
        #[cfg(feature = "stats-full")]
        {
            let rec = self.stats.rec(cid);
            let b = rec.betrayals.get_or_insert(0);
            *b += 1;
        }
        #[cfg(not(feature = "stats-full"))]
        let _ = cid;
    }
    pub fn alliance_formed(&mut self, player: SmallId) {
        self.alliance_op(player, ALLIANCE_INDEX_FORMED, StatNum::Num(1.0), AllianceOp::Add);
    }
    pub fn alliance_ended(&mut self, player: SmallId, duration_ticks: StatNum, counter: Option<AllianceEndCounter>) {
        match counter {
            Some(AllianceEndCounter::BrokenByOther) => {
                self.alliance_op(player, ALLIANCE_INDEX_BROKEN_BY_OTHER, StatNum::Num(1.0), AllianceOp::Add)
            }
            Some(AllianceEndCounter::Expired) => {
                self.alliance_op(player, ALLIANCE_INDEX_EXPIRED, StatNum::Num(1.0), AllianceOp::Add)
            }
            None => {}
        }
        self.alliance_op(player, ALLIANCE_INDEX_LONGEST_HELD, duration_ticks, AllianceOp::Max);
    }
    /// `lobbyFillTime` is a no-op in `StatsImpl` (`:597`).
    pub fn lobby_fill_time(&mut self, fill_time_ms: f64) {
        let _ = fill_time_ms;
    }
    pub fn boat_send_trade(&mut self, player: SmallId, target: SmallId) {
        let _ = target;
        self.add_boat(player, "trade", BOAT_INDEX_SENT, StatNum::Num(1.0));
    }
    pub fn boat_arrive_trade(&mut self, player: SmallId, target: SmallId, gold: StatNum) {
        self.add_boat(player, "trade", BOAT_INDEX_ARRIVE, StatNum::Num(1.0));
        self.add_arr(player, ArrKind::Gold, GOLD_INDEX_TRADE, gold);
        self.add_arr(target, ArrKind::Gold, GOLD_INDEX_TRADE, gold);
    }
    pub fn boat_captured_trade(&mut self, player: SmallId, target: SmallId, gold: StatNum) {
        let _ = target;
        self.add_boat(player, "trade", BOAT_INDEX_CAPTURE, StatNum::Num(1.0));
        self.add_arr(player, ArrKind::Gold, GOLD_INDEX_STEAL, gold);
    }
    pub fn boat_destroy_trade(&mut self, player: SmallId, target: SmallId) {
        let _ = target;
        self.add_boat(player, "trade", BOAT_INDEX_DESTROY, StatNum::Num(1.0));
    }
    /// `troops` is not read (`:325-331`).
    pub fn boat_send_troops(&mut self, player: SmallId, target: Owner, troops: StatNum) {
        let _ = (target, troops);
        self.add_boat(player, "trans", BOAT_INDEX_SENT, StatNum::Num(1.0));
    }
    pub fn boat_arrive_troops(&mut self, player: SmallId, target: Owner, troops: StatNum) {
        let _ = (target, troops);
        self.add_boat(player, "trans", BOAT_INDEX_ARRIVE, StatNum::Num(1.0));
    }
    pub fn boat_destroy_troops(&mut self, player: SmallId, target: SmallId, troops: StatNum) {
        let _ = (target, troops);
        self.add_boat(player, "trans", BOAT_INDEX_DESTROY, StatNum::Num(1.0));
    }
    pub fn boat_captured_troops(&mut self, player: SmallId, target: SmallId) {
        let _ = target;
        self.add_boat(player, "trans", BOAT_INDEX_CAPTURE, StatNum::Num(1.0));
    }
    /// `boatLose(player, type)` (`type`: TradeShip | TransportShip).
    pub fn boat_lose(&mut self, player: SmallId, t: UnitType) {
        #[allow(clippy::expect_used)] // rust-invariant: callers pass a BoatUnitType
        let key = boat_unit(t).expect("boatLose takes a BoatUnitType");
        self.add_boat(player, key, BOAT_INDEX_LOST, StatNum::Num(1.0));
    }
    pub fn bomb_launch(&mut self, player: SmallId, target: Owner, t: UnitType) {
        let _ = target;
        self.add_bomb(player, t, BOMB_INDEX_LAUNCH, StatNum::Num(1.0));
    }
    pub fn bomb_land(&mut self, player: SmallId, target: Owner, t: UnitType) {
        let _ = target;
        self.add_bomb(player, t, BOMB_INDEX_LAND, StatNum::Num(1.0));
    }
    pub fn bomb_intercept(&mut self, player: SmallId, t: UnitType, count: StatNum) {
        self.add_bomb(player, t, BOMB_INDEX_INTERCEPT, count);
    }
    /// `goldWar(player, captured, gold)` (`:389-395`).
    pub fn gold_war(&mut self, player: SmallId, captured: SmallId, gold: StatNum) {
        self.add_arr(player, ArrKind::Gold, GOLD_INDEX_WAR, gold);
        // `conquest_by_type[captured.type()]` is defined for all three player types.
        let conquest_type = match player_type_of(self.players, self.infos, captured) {
            PlayerType::Human => PLAYER_INDEX_HUMAN,
            PlayerType::Nation => PLAYER_INDEX_NATION,
            PlayerType::Bot => PLAYER_INDEX_BOT,
        };
        // `_addConquest(player, index)`: `+= _bigint(1)`.
        self.add_arr(player, ArrKind::Conquests, conquest_type, StatNum::Num(1.0));
    }
    pub fn gold_work(&mut self, player: SmallId, gold: StatNum) {
        self.add_arr(player, ArrKind::Gold, GOLD_INDEX_WORK, gold);
    }
    /// `goldDonationReceived(player, gold, goldBefore)` (`:377-387`). The `_bigint(goldBefore)`
    /// comparison runs **outside** the clientID gate.
    pub fn gold_donation_received(&mut self, player: SmallId, gold: StatNum, gold_before: StatNum) {
        self.add_arr(player, ArrKind::Gold, GOLD_INDEX_DONATE_RECV, gold);
        self.add_arr(player, ArrKind::Donations, DONATION_INDEX_GOLD_RECV, StatNum::Num(1.0));
        if gold_before.bigint() < DONATION_BROKE_GOLD_THRESHOLD {
            self.add_arr(player, ArrKind::Donations, DONATION_INDEX_GOLD_RECV_BROKE, StatNum::Num(1.0));
        }
    }
    /// `unitBuild(player, type)` (`OtherUnitType`).
    pub fn unit_build(&mut self, player: SmallId, t: UnitType) {
        self.add_other_unit(player, t, OTHER_INDEX_BUILT, StatNum::Num(1.0));
    }
    pub fn unit_capture(&mut self, player: SmallId, t: UnitType) {
        self.add_other_unit(player, t, OTHER_INDEX_CAPTURE, StatNum::Num(1.0));
    }
    pub fn unit_upgrade(&mut self, player: SmallId, t: UnitType) {
        self.add_other_unit(player, t, OTHER_INDEX_UPGRADE, StatNum::Num(1.0));
    }
    pub fn unit_destroy(&mut self, player: SmallId, t: UnitType) {
        self.add_other_unit(player, t, OTHER_INDEX_DESTROY, StatNum::Num(1.0));
    }
    pub fn unit_lose(&mut self, player: SmallId, t: UnitType) {
        self.add_other_unit(player, t, OTHER_INDEX_LOST, StatNum::Num(1.0));
    }
    /// `playerKilled` → `_addPlayerKilled(player, tick)`: `killedAt = _bigint(tick)` (`:218-222`).
    pub fn player_killed(&mut self, player: SmallId, tick: i32) {
        let Some(cid) = self.make(player) else { return };
        let v = StatNum::Num(f64::from(tick)).bigint();
        #[cfg(feature = "stats-full")]
        {
            self.stats.rec(cid).killed_at = Some(v);
        }
        #[cfg(not(feature = "stats-full"))]
        let _ = (cid, v);
    }
    /// `recordKilledBy` (`:565-571`): first write wins; `null` is a recorded value.
    pub fn record_killed_by(&mut self, victim: SmallId, killer_client_id: Option<&str>) {
        let Some(cid) = self.make(victim) else { return };
        #[cfg(feature = "stats-full")]
        {
            let rec = self.stats.rec(cid);
            if rec.killed_by.is_absent() {
                rec.killed_by = match killer_client_id {
                    Some(k) => crate::jsval::Tri::Some(k.to_owned()),
                    None => crate::jsval::Tri::Null,
                };
            }
        }
        #[cfg(not(feature = "stats-full"))]
        let _ = (cid, killer_client_id);
    }
    /// `recordDeathPosition` (`:573-577`): `p.deathPosition ??= position`.
    pub fn record_death_position(&mut self, victim: SmallId, position: i32) {
        let Some(cid) = self.make(victim) else { return };
        #[cfg(feature = "stats-full")]
        {
            let rec = self.stats.rec(cid);
            if rec.death_position.is_none() {
                rec.death_position = Some(f64::from(position));
            }
        }
        #[cfg(not(feature = "stats-full"))]
        let _ = (cid, position);
    }
    /// `recordFinalTiles` (`:421-425`).
    pub fn record_final_tiles(&mut self, player: SmallId, tiles: StatNum) {
        let Some(cid) = self.make(player) else { return };
        let v = tiles.bigint();
        #[cfg(feature = "stats-full")]
        {
            self.stats.rec(cid).final_tiles = Some(v);
        }
        #[cfg(not(feature = "stats-full"))]
        let _ = (cid, v);
    }
    /// `recordSpawnTile` (`:427-431`): last write wins.
    pub fn record_spawn_tile(&mut self, player: SmallId, tile: TileRef) {
        let Some(cid) = self.make(player) else { return };
        #[cfg(feature = "stats-full")]
        {
            self.stats.rec(cid).spawn_tile = Some(tile);
        }
        #[cfg(not(feature = "stats-full"))]
        let _ = (cid, tile);
    }
    /// `recordAlliancesAtEnd` (`:433-448`).
    pub fn record_alliances_at_end(&mut self, player: SmallId, still_standing: i32, longest_standing_ticks: StatNum) {
        self.alliance_op(player, ALLIANCE_INDEX_HELD_TO_END, StatNum::Num(f64::from(still_standing)), AllianceOp::Set);
        self.alliance_op(player, ALLIANCE_INDEX_LONGEST_HELD, longest_standing_ticks, AllianceOp::Max);
    }
    /// `recordTickSample` (`stats-full` only; compiles to nothing otherwise). Its inputs are
    /// `numTilesOwned()`, `troops()` (a `Number(bigint)`) and `alliances().length`, all
    /// integral, so the lite build loses no throw by skipping it.
    pub fn record_tick_sample(&mut self, player: SmallId, tiles: StatNum, troops: StatNum, alliance_count: i32) {
        #[cfg(feature = "stats-full")]
        self.record_tick_sample_full(player, tiles, troops, alliance_count);
        #[cfg(not(feature = "stats-full"))]
        let _ = (player, tiles, troops, alliance_count);
    }
    /// `recordKill(player, victim, tick)` (`:579-587`).
    pub fn record_kill(&mut self, player: SmallId, victim: SmallId, tick: StatNum) {
        if player_type_of(self.players, self.infos, victim) != PlayerType::Human {
            return;
        }
        let Some(victim_id) = client_of(self.players, self.infos, victim) else { return };
        let Some(cid) = self.make(player) else { return };
        #[cfg(feature = "stats-full")]
        {
            let victim_id = victim_id.to_owned();
            let rec = self.stats.rec(cid);
            let kills = rec.kills.get_or_insert_with(Vec::new);
            let t = tick.bigint(); // TS: `{ victim, tick: _bigint(tick) }` before the push
            kills.push(KillRec { victim: victim_id, tick: t });
        }
        #[cfg(not(feature = "stats-full"))]
        {
            let _ = (cid, victim_id);
            let _ = tick.bigint();
        }
    }
    pub fn train_self_trade(&mut self, player: SmallId, gold: StatNum) {
        self.add_arr(player, ArrKind::Gold, GOLD_INDEX_TRAIN_SELF, gold);
    }
    pub fn train_external_trade(&mut self, player: SmallId, gold_player: StatNum) {
        self.add_arr(player, ArrKind::Gold, GOLD_INDEX_TRAIN_OTHER, gold_player);
    }
}

#[derive(Clone, Copy, Debug)]
enum AllianceOp {
    Add,
    Max,
    Set,
}

#[cfg(feature = "stats-full")]
impl StatsCx<'_> {
    /// `recordTickSample` (`StatsImpl.ts:450-563`), literally, with `_sampleCache` inside the
    /// record.
    fn record_tick_sample_full(&mut self, player: SmallId, tiles: StatNum, troops: StatNum, alliance_count: i32) {
        // `:456-463`: the clientID gate and the inline `_makePlayerStats`.
        let Some(cid) = self.make(player) else { return };
        let p = self.stats.rec(cid);
        if p.sample_cache.is_none() {
            // `:466-480`.
            ensure_arr(&mut p.tiles, &[0, 0, 0], TILE_INDEX_DRAWDOWN_TROUGH);
            ensure_arr(&mut p.alliances, &[0, 0], ALLIANCE_INDEX_PEAK_CONCURRENT);
            let t = p.tiles.as_deref().unwrap_or(&[]);
            let a = p.alliances.as_deref().unwrap_or(&[]);
            p.sample_cache = Some(SampleCache {
                peak: t[TILE_INDEX_PEAK] as f64, // in-range: ensured above; Number(bigint)
                dd_peak: t[TILE_INDEX_DRAWDOWN_PEAK] as f64,
                dd_trough: t[TILE_INDEX_DRAWDOWN_TROUGH] as f64,
                peak_troops: match p.peak_troops {
                    None => -1.0,
                    Some(x) => x as f64,
                },
                peak_alliance: a[ALLIANCE_INDEX_PEAK_CONCURRENT] as f64,
            });
        }
        let ac = f64::from(alliance_count);
        let is_int = |x: f64| x.is_finite() && x.trunc() == x; // Number.isInteger
        let fast = match (tiles, troops) {
            (StatNum::Num(ti), StatNum::Num(tr)) => {
                is_int(ti)
                    && is_int(tr)
                    && is_int(ac)
                    && ti >= 0.0
                    && tr >= 0.0
                    && ti < 90_000_000.0
                    && tr < 90_000_000.0
            }
            _ => false,
        };
        #[allow(clippy::expect_used)] // rust-invariant: set just above
        let cache = p.sample_cache.as_mut().expect("sample cache");
        #[allow(clippy::expect_used)] // rust-invariant: ensured when the cache was built
        let pt = p.tiles.as_mut().expect("tiles");
        if fast {
            let (StatNum::Num(tiles), StatNum::Num(troops)) = (tiles, troops) else { unreachable!() };
            // `BigInt(x)` of a checked integer below 9e7 is exact.
            let big = |x: f64| x as i64;
            if tiles > cache.peak {
                cache.peak = tiles;
                let t = big(tiles);
                pt[TILE_INDEX_PEAK] = t;
                if cache.dd_peak == 0.0 {
                    cache.dd_peak = tiles;
                    cache.dd_trough = tiles;
                    pt[TILE_INDEX_DRAWDOWN_PEAK] = t;
                    pt[TILE_INDEX_DRAWDOWN_TROUGH] = t;
                }
            } else if cache.dd_peak == 0.0 {
                cache.dd_peak = cache.peak;
                cache.dd_trough = tiles;
                pt[TILE_INDEX_DRAWDOWN_PEAK] = pt[TILE_INDEX_PEAK];
                pt[TILE_INDEX_DRAWDOWN_TROUGH] = big(tiles);
            } else if tiles < cache.peak {
                let decline = (cache.peak - tiles) * cache.dd_peak;
                let worst_decline = (cache.dd_peak - cache.dd_trough) * cache.peak;
                if decline > worst_decline {
                    cache.dd_peak = cache.peak;
                    cache.dd_trough = tiles;
                    pt[TILE_INDEX_DRAWDOWN_PEAK] = pt[TILE_INDEX_PEAK];
                    pt[TILE_INDEX_DRAWDOWN_TROUGH] = big(tiles);
                }
            }
            if troops > cache.peak_troops {
                cache.peak_troops = troops;
                p.peak_troops = Some(big(troops));
            }
            if ac > cache.peak_alliance {
                cache.peak_alliance = ac;
                #[allow(clippy::expect_used)] // rust-invariant: ensured when the cache was built
                let pa = p.alliances.as_mut().expect("alliances");
                pa[ALLIANCE_INDEX_PEAK_CONCURRENT] = i64::from(alliance_count);
            }
        } else {
            // Fallback path (`:530-561`), on i128 like unbounded BigInt.
            let t = tiles.bigint();
            while pt.len() <= TILE_INDEX_DRAWDOWN_TROUGH {
                pt.push(0);
            }
            if t > pt[TILE_INDEX_PEAK] {
                pt[TILE_INDEX_PEAK] = t;
            }
            let peak = i128::from(pt[TILE_INDEX_PEAK]);
            let dd_peak = i128::from(pt[TILE_INDEX_DRAWDOWN_PEAK]);
            let dd_trough = i128::from(pt[TILE_INDEX_DRAWDOWN_TROUGH]);
            let t128 = i128::from(t);
            if dd_peak == 0 || (peak - t128) * dd_peak > (dd_peak - dd_trough) * peak {
                pt[TILE_INDEX_DRAWDOWN_PEAK] = pt[TILE_INDEX_PEAK];
                pt[TILE_INDEX_DRAWDOWN_TROUGH] = t;
            }
            let tr = troops.bigint();
            if p.peak_troops.is_none_or(|x| tr > x) {
                p.peak_troops = Some(tr);
            }
            let pa = ensure_arr(&mut p.alliances, &[0], ALLIANCE_INDEX_PEAK_CONCURRENT);
            let v = i64::from(alliance_count); // `_bigint(allianceCount)` of an integer
            if v > pa[ALLIANCE_INDEX_PEAK_CONCURRENT] {
                pa[ALLIANCE_INDEX_PEAK_CONCURRENT] = v;
            }
            // Sync cache (`:556-561`).
            cache.peak = pt[TILE_INDEX_PEAK] as f64;
            cache.dd_peak = pt[TILE_INDEX_DRAWDOWN_PEAK] as f64;
            cache.dd_trough = pt[TILE_INDEX_DRAWDOWN_TROUGH] as f64;
            #[allow(clippy::expect_used)] // rust-invariant: set just above
            let ptr = p.peak_troops.expect("peak troops");
            cache.peak_troops = ptr as f64;
            cache.peak_alliance = pa[ALLIANCE_INDEX_PEAK_CONCURRENT] as f64;
        }
    }
}

impl crate::ctx::Ctx<'_> {
    /// `stats().recordTickSample(player, numTilesOwned, troops, alliances.length)`
    /// (`GameImpl.ts:635-643`, `StatsImpl.ts`): the per-tick peak samples of `stats-full`.
    #[cfg(feature = "stats-full")]
    pub fn stats_record_tick_sample(&mut self, player: SmallId) {
        let p = self.st.player(player);
        let tiles = f64::from(p.num_tiles_owned());
        let troops = p.troops();
        #[allow(clippy::expect_used)] // rust-invariant: alliance counts are far below 2^31
        let n = i32::try_from(p.alliances().len()).expect("alliance count fits i32");
        self.st.stats_cx().record_tick_sample(player, StatNum::Num(tiles), StatNum::Num(troops), n);
    }
}

// ---------------------------------------------------------------------------------------
// CS S export (`stats-full`): `StatsSnapshot = { data }` (`StatsImpl.ts:87-89`).
// ---------------------------------------------------------------------------------------

#[cfg(feature = "stats-full")]
impl PlayerStatsRec {
    /// The record as a CS object (present keys only; CSE1 sorts keys).
    pub fn to_cs(&self) -> crate::canon::Cs {
        use crate::canon::{Cs, big};
        fn arr(v: &[i64]) -> Cs {
            Cs::Arr(v.iter().map(|x| big(*x)).collect())
        }
        fn keyed<const N: usize>(r: &[Option<Vec<i64>>; N], keys: &[&str; N]) -> Cs {
            Cs::Obj(
                r.iter().zip(keys.iter()).filter_map(|(v, k)| v.as_ref().map(|v| ((*k).to_owned(), arr(v)))).collect(),
            )
        }
        let mut o: Vec<(String, Cs)> = Vec::new();
        let mut put = |k: &str, v: Cs| o.push((k.to_owned(), v));
        if let Some(v) = &self.attacks {
            put("attacks", arr(v));
        }
        if let Some(v) = self.betrayals {
            put("betrayals", big(v));
        }
        if let Some(v) = self.killed_at {
            put("killedAt", big(v));
        }
        match &self.killed_by {
            crate::jsval::Tri::Absent => {}
            crate::jsval::Tri::Null => put("killedBy", Cs::Null),
            crate::jsval::Tri::Some(s) => put("killedBy", Cs::Str(s.clone())),
        }
        if let Some(v) = self.death_position {
            put("deathPosition", Cs::Num(v));
        }
        if let Some(v) = self.final_tiles {
            put("finalTiles", big(v));
        }
        if let Some(v) = &self.kills {
            put(
                "kills",
                Cs::Arr(
                    v.iter()
                        .map(|k| {
                            Cs::Obj(vec![
                                ("victim".to_owned(), Cs::Str(k.victim.clone())),
                                ("tick".to_owned(), big(k.tick)),
                            ])
                        })
                        .collect(),
                ),
            );
        }
        if let Some(v) = &self.conquests {
            put("conquests", arr(v));
        }
        if let Some(v) = &self.boats {
            put("boats", keyed(v, &BOAT_KEYS));
        }
        if let Some(v) = &self.bombs {
            put("bombs", keyed(v, &BOMB_KEYS));
        }
        if let Some(v) = &self.gold {
            put("gold", arr(v));
        }
        if let Some(v) = &self.units {
            put("units", keyed(v, &OTHER_KEYS));
        }
        if let Some(v) = &self.tiles {
            put("tiles", arr(v));
        }
        if let Some(v) = &self.alliances {
            put("alliances", arr(v));
        }
        if let Some(v) = self.peak_troops {
            put("peakTroops", big(v));
        }
        if let Some(v) = &self.donations {
            put("donations", arr(v));
        }
        if let Some(v) = self.spawn_tile {
            put("spawnTile", Cs::Int(i64::from(v)));
        }
        Cs::Obj(o)
    }
}

/// CS S = `{ data: { <clientID>: PlayerStats } }` (SPEC §1.3; class X, `stats-full` only).
#[cfg(feature = "stats-full")]
impl crate::canon::Canon for Stats {
    fn canon(&self, _cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        use crate::canon::Cs;
        let data = self.full.data.iter().map(|(k, v)| (k.clone(), v.to_cs())).collect();
        Cs::Obj(vec![("data".to_owned(), Cs::Obj(data))])
    }
}

#[cfg(test)]
pub(crate) mod tests;
