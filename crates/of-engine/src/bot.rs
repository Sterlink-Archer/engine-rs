//! The read-only view and legality-oracle surface: a borrowed read view over a
//! [`Game`]'s [`State`] plus the legality oracles of `GameRunner` (`engine/src/GameRunner.ts:337-439`).
//! Additive only: every method forwards to an existing reader of the port (`PlayerRef`, `State`,
//! `EngineConfig`, `GameMap`); nothing here mutates state, so a caller using it cannot change a game.
//!
//! - [`BotView`]: `Game::view()`, zero-copy accessors (ticks, players in TS order, player id ↔
//!   smallID, raw relation values, unit costs, config formulas external callers read).
//! - Oracles on [`BotView`] that need a [`Scratch`] take one, or use the thread scratch
//!   (`*_ts` helpers): `can_build`, `can_attack`, `can_upgrade`, `best_transport_ship_spawn`,
//!   `nearby_units`, and the GameRunner composites [`BotView::player_actions`] /
//!   [`BotView::player_buildables`] ported literally from `GameRunner.ts:337-385`.
//! - [`nation_mirv_leak_share`]: `mirvLeakShare` (`NationMIRVBehavior.ts:53-101`) as a pure fn.
//!
//! Owner: **P1** (`GameRunner.ts` row 7 of FILE_MAP §1: `player_actions`/`player_buildables` are its
//! `playerActions`/`playerBuildables`).

use crate::Game;
use crate::api::types::{AllianceInfo, BuildableUnit, EmojiRecipient, Gold, PlayerType, Tick, UnitType};
use crate::ids::{SmallId, TileRef, UnitRef};
use crate::map::GameMap;
use crate::map::unit_grid::NearbyUnit;
use crate::scratch::{Scratch, with_thread_scratch};
use crate::state::State;
use crate::state::player::PlayerRef;

pub use crate::exec::nation::mirv::mirv_leak_share as nation_mirv_leak_share;

/// Borrowed read view (ARCHITECTURE §10 `g.view()`).
#[derive(Clone, Copy)]
pub struct BotView<'a> {
    pub st: &'a State,
}

impl Game {
    /// A read view of this game (no allocation, no mutation).
    #[inline]
    pub fn view(&self) -> BotView<'_> {
        BotView { st: &self.state }
    }
}

/// `PlayerActions.interaction` (`GameRunner.ts:368-380`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerInteraction {
    pub shared_border: bool,
    pub can_send_emoji: bool,
    pub can_target: bool,
    pub can_send_alliance_request: bool,
    pub can_break_alliance: bool,
    pub can_donate_gold: bool,
    pub can_donate_troops: bool,
    pub can_embargo: bool,
    pub alliance_info: Option<AllianceInfo>,
}

/// `PlayerActions` (`GameRunner.ts:357-363`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerActions {
    pub can_attack: bool,
    pub buildable_units: Vec<BuildableUnit>,
    pub can_send_emoji_all_players: bool,
    pub can_embargo_all: bool,
    pub interaction: Option<PlayerInteraction>,
}

impl<'a> BotView<'a> {
    #[inline]
    pub fn new(st: &'a State) -> Self {
        BotView { st }
    }
    /// `game.ticks()`.
    #[inline]
    pub fn ticks(&self) -> Tick {
        self.st.ticks()
    }
    /// `game.inSpawnPhase()`.
    #[inline]
    pub fn in_spawn_phase(&self) -> bool {
        self.st.in_spawn_phase()
    }
    /// `game.map()`.
    #[inline]
    pub fn map(&self) -> &'a GameMap {
        &self.st.map
    }
    /// A player view (`playerBySmallID(sid)` for a player id).
    #[inline]
    pub fn player(&self, sid: SmallId) -> PlayerRef<'a> {
        self.st.player(sid)
    }
    /// Number of player slots (`_playersBySmallID.length`); smallIDs are `1..=n`.
    #[inline]
    pub fn num_player_slots(&self) -> usize {
        self.st.players.len()
    }
    /// `game.players()` (alive, smallID order) without allocating.
    pub fn alive_players(&self) -> impl Iterator<Item = SmallId> + 'a {
        self.st.players.iter().filter(|p| p.tiles.size() > 0).map(|p| p.small_id)
    }
    /// `game.allPlayers()` (smallID order) without allocating.
    pub fn all_players(&self) -> impl Iterator<Item = SmallId> + 'a {
        self.st.players.iter().map(|p| p.small_id)
    }
    /// `game.playerByClientID(id)`.
    #[inline]
    pub fn player_by_client_id(&self, id: &str) -> Option<SmallId> {
        self.st.player_by_client_id(id)
    }
    /// `player.id()` (the `PlayerID` intents name players by).
    #[inline]
    pub fn player_id(&self, sid: SmallId) -> &'a str {
        self.st.player(sid).player_id()
    }
    /// PlayerID → smallID (`game.player(id).smallID()`, `None` instead of the TS throw).
    #[inline]
    pub fn small_id_of(&self, id: &str) -> Option<SmallId> {
        self.st.find_player(id)
    }
    /// `player.type()`.
    #[inline]
    pub fn player_type(&self, sid: SmallId) -> PlayerType {
        self.st.player(sid).player_type()
    }
    /// The raw relation number `of` holds towards `towards`: TS `of.relations.get(towards) ?? 0`
    /// (`PlayerImpl.ts:188`; external callers read `o.relations?.get?.(me) ?? 0`).
    pub fn relation_value(&self, of: SmallId, towards: SmallId) -> f64 {
        self.st.player_data(of).relations.get(&towards).copied().unwrap_or(0.0)
    }
    /// `config().unitInfo(t).cost(game, player)` (`DefaultConfig`, extraUnits = 0).
    #[inline]
    pub fn unit_cost(&self, t: UnitType, sid: SmallId) -> Gold {
        self.st.cfg.unit_cost(t, self.st, sid, 0.0)
    }
    /// `config().maxTroops(player)`.
    #[inline]
    pub fn max_troops(&self, sid: SmallId) -> f64 {
        self.st.cfg.max_troops(&self.st.player(sid))
    }
    /// `config().percentageTilesOwnedToWinTenths(game.elapsedGameSeconds())`.
    #[inline]
    pub fn win_bar_tenths(&self) -> f64 {
        self.st.cfg.percentage_tiles_owned_to_win_tenths(self.st.elapsed_game_seconds())
    }
    /// `game.numLandTiles()`.
    #[inline]
    pub fn num_land_tiles(&self) -> u32 {
        self.st.map.num_land_tiles()
    }
    /// `game.numTilesWithFallout()`.
    #[inline]
    pub fn num_tiles_with_fallout(&self) -> u32 {
        self.st.num_tiles_with_fallout()
    }
    /// `game.nearbyUnits(tile, range, types)` with the TS defaults (no predicate,
    /// `includeUnderConstruction = false`).
    #[inline]
    pub fn nearby_units(&self, tile: TileRef, range: f64, types: &[UnitType]) -> Vec<NearbyUnit> {
        self.st.nearby_units(tile, range, types, |_, _, _| true, false)
    }
    /// `nationMirvLeakShare(attacker, target)` (`mirvLeakShare`, NationMIRVBehavior.ts:53-101).
    #[inline]
    pub fn mirv_leak_share(&self, attacker: SmallId, target: SmallId) -> f64 {
        nation_mirv_leak_share(self.st, attacker, target)
    }

    // ---- oracles (same code paths as TS; they only read) ----
    /// `player.canBuild(type, tile)`: `Some(spawn tile)` or `None` (= TS `false`).
    #[inline]
    pub fn can_build(&self, sc: &mut Scratch, sid: SmallId, t: UnitType, tile: TileRef) -> Option<TileRef> {
        self.st.player(sid).can_build(sc, t, tile, None)
    }
    /// [`Self::can_build`] on the thread scratch.
    pub fn can_build_ts(&self, sid: SmallId, t: UnitType, tile: TileRef) -> Option<TileRef> {
        with_thread_scratch(|sc| self.can_build(sc, sid, t, tile))
    }
    /// `player.canAttack(tile)`.
    #[inline]
    pub fn can_attack(&self, sc: &mut Scratch, sid: SmallId, tile: TileRef) -> bool {
        self.st.player(sid).can_attack(sc, tile)
    }
    /// `player.canUpgradeUnit(unit)`.
    #[inline]
    pub fn can_upgrade(&self, sid: SmallId, u: UnitRef) -> bool {
        self.st.player(sid).can_upgrade_unit(u)
    }
    /// `GameRunner.bestTransportShipSpawn(playerID, targetTile)` (`GameRunner.ts:425-438`).
    #[inline]
    pub fn best_transport_ship_spawn(&self, sc: &mut Scratch, sid: SmallId, tile: TileRef) -> Option<TileRef> {
        self.st.player(sid).best_transport_ship_spawn(sc, tile)
    }
    /// `GameRunner.playerBuildables(playerID, x, y, units, includeBuildPreview)` (`:337-348`).
    pub fn player_buildables(
        &self,
        sc: &mut Scratch,
        sid: SmallId,
        xy: Option<(u32, u32)>,
        units: Option<&[UnitType]>,
        include_build_preview: bool,
    ) -> Vec<BuildableUnit> {
        let tile = xy.map(|(x, y)| self.st.map.ref_(f64::from(x), f64::from(y)));
        self.st.player(sid).buildable_units(sc, tile, units, include_build_preview)
    }
    /// `GameRunner.playerActions(playerID, x, y, units)` (`:350-385`). `units = Some(None)` is the
    /// TS default (`undefined` → all buildable types), `None` is TS `null` (no buildables).
    pub fn player_actions(
        &self,
        sc: &mut Scratch,
        sid: SmallId,
        xy: Option<(u32, u32)>,
        units: Option<Option<&[UnitType]>>,
    ) -> PlayerActions {
        let p = self.st.player(sid);
        let tile = xy.map(|(x, y)| self.st.map.ref_(f64::from(x), f64::from(y)));
        let can_attack = match tile {
            Some(t) => p.can_attack(sc, t),
            None => false,
        };
        let buildable_units = match units {
            None => Vec::new(),
            Some(u) => p.buildable_units(sc, tile, u, false),
        };
        let mut actions = PlayerActions {
            can_attack,
            buildable_units,
            can_send_emoji_all_players: p.can_send_emoji(EmojiRecipient::AllPlayers),
            can_embargo_all: p.can_embargo_all(),
            interaction: None,
        };
        if let Some(t) = tile
            && self.st.map.has_owner(t)
        {
            let other = self.st.owner(t);
            actions.interaction = Some(PlayerInteraction {
                shared_border: p.shares_border_with(other),
                can_send_emoji: p.can_send_emoji(EmojiRecipient::Player(other)),
                can_target: p.can_target(other),
                can_send_alliance_request: p.can_send_alliance_request(other),
                can_break_alliance: p.is_allied_with(other),
                can_donate_gold: p.can_donate_gold(other),
                can_donate_troops: p.can_donate_troops(other),
                can_embargo: !p.has_embargo_against(other),
                alliance_info: p.alliance_info(other),
            });
        }
        actions
    }
}
