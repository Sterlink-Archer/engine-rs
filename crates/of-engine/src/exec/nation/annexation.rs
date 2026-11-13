//! `execution/nation/BotAnnexation.ts`. Owner: **P10**. `BotScanner` is a per-call local
//! (`nbuf`, `attackableBots`), not state.

use crate::api::types::unit_groups;
#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::jsorder::{LookupMap, LookupSet};
use crate::state::State;
use of_det::js_sort_by;

/// `interface BotSurroundings` (`:6-13`).
#[derive(Clone, Debug)]
struct BotSurroundings {
    bot: SmallId,
    borders_us: bool,
    /// Touches water, the map edge, free land or anyone but us and bots we may attack.
    open: bool,
    /// `bots: Set<Player>`: iterated (`:44, :70`) in insertion order, never deleted from, so an
    /// insert-if-absent `Vec` has exactly the `Set` iteration order.
    bots: Vec<SmallId>,
}

impl BotSurroundings {
    fn add_bot(&mut self, b: SmallId) {
        if !self.bots.contains(&b) {
            self.bots.push(b);
        }
    }
}

/// `walledInBots(game, player, bots)` (`:16-28`).
pub fn walled_in_bots(st: &State, player: SmallId, bots: &[SmallId]) -> Vec<SmallId> {
    let mut scanner = BotScanner::new(st, player);
    let attacking = attacked_by(st, player);
    // TS :24-27: `bots.filter(bot => !attacking.has(bot) && scanner.scan(bot, true, NO_BOTS)?.bordersUs === true)`.
    let mut out = Vec::new();
    for &bot in bots {
        if attacking.contains(&bot) {
            continue;
        }
        // `NO_BOTS` = an empty `within` set (`:95`).
        if scanner.scan(bot, true, Some(&[])).is_some_and(|s| s.borders_us) {
            out.push(bot);
        }
    }
    out
}

/// `planBotAttacks(game, player, bots)` (`:32-93`; `js_sort_by` at `:62, :87, :90`).
pub fn plan_bot_attacks(st: &State, player: SmallId, bots: &[SmallId]) -> Vec<SmallId> {
    let mut scanner = BotScanner::new(st, player);
    // `region: Map<Player, BotSurroundings>`: iterated by `[...region.values()]` (`:54`) → JsMap.
    let mut region: JsMap<SmallIdKey, BotSurroundings> = JsMap::new();
    for &bot in bots {
        // TS :39 `scanner.scan(bot, false)!`: never null with closedOnly = false and no `within`.
        let s = scanner.scan(bot, false, None).ts_expect("BotAnnexation.ts:39 scan(bot, false)!");
        region.insert(bot, s);
    }
    let front: Vec<SmallId> = bots
        .iter()
        .copied()
        .filter(|b| region.get(b).ts_expect("BotAnnexation.ts:40 region.get(bot)!").borders_us)
        .collect();
    // Open bots behind the front are never kept, so only the closed ones are needed.
    // lookup-only: `behind` is only read by `has` (`:45`).
    let mut behind: LookupSet<SmallId> = LookupSet::new();
    for &bot in &front {
        // iter-mut: the loop body adds new keys to `region` but never touches `bot`'s own `bots`
        // set object, so iterating a copy of it equals TS's live `for..of` (`:44`).
        let others = region.get(&bot).ts_expect("BotAnnexation.ts:44 region.get(bot)!").bots.clone();
        for other in others {
            if region.contains_key(&other) || behind.contains(&other) {
                continue;
            }
            behind.insert(other);
            let s = scanner.scan(other, true, None);
            if let Some(s) = s {
                region.insert(other, s);
            }
        }
    }
    let attacking = attacked_by(st, player);

    // Largest first, bots that don't border each other; each neighbor earns a share of the land it walls in.
    let mut candidates: Vec<BotSurroundings> = region
        .values()
        .filter(|s| !s.open && !s.bots.is_empty() && !attacking.contains(&s.bot) && !owns_structures(st, s.bot))
        .cloned()
        .collect();
    // TS :62 `.sort((a, b) => b.bot.numTilesOwned() - a.bot.numTilesOwned())`.
    js_sort_by(&mut candidates, |a, b| {
        f64::from(st.player(b.bot).num_tiles_owned()) - f64::from(st.player(a.bot).num_tiles_owned())
    });
    // lookup-only: `kept`/`nextToKept` are only read by `has`, `walledIn` only by `get`/`set`
    // (`:63-74, :77-85`).
    let mut kept: LookupSet<SmallId> = LookupSet::new();
    let mut next_to_kept: LookupSet<SmallId> = LookupSet::new();
    let mut walled_in: LookupMap<SmallId, f64> = LookupMap::new();
    for s in &candidates {
        if next_to_kept.contains(&s.bot) {
            continue;
        }
        kept.insert(s.bot);
        let share = f64::from(st.player(s.bot).num_tiles_owned()) / (s.bots.len() as f64);
        for &other in &s.bots {
            next_to_kept.insert(other);
            let prev = walled_in.get(&other).copied().unwrap_or(0.0);
            walled_in.insert(other, prev + share);
        }
    }

    // Most land per bot troop, counting the land it walls in.
    // TS :77-79 `(bot.numTilesOwned() + (walledIn.get(bot) ?? 0)) / Math.max(1, bot.troops())`.
    let value = |bot: SmallId| -> f64 {
        (f64::from(st.player(bot).num_tiles_owned()) + walled_in.get(&bot).copied().unwrap_or(0.0))
            / of_det::js::max(1.0, st.player(bot).troops())
    };
    let by_value = |a: &SmallId, b: &SmallId| value(*b) - value(*a);
    let mut targets: Vec<SmallId> = bots
        .iter()
        .copied()
        .filter(|bot| {
            let s = region.get(bot).ts_expect("BotAnnexation.ts:83 region.get(bot)!");
            let walled = s.borders_us && !s.open && s.bots.is_empty();
            !kept.contains(bot) && !walled
        })
        .collect();
    js_sort_by(&mut targets, by_value);
    // Behind water or the map edge, kept bots can be all we border: then they're the way on.
    if !targets.iter().any(|bot| region.get(bot).ts_expect("BotAnnexation.ts:89 region.get(bot)!").borders_us) {
        let mut tail: Vec<SmallId> = front.iter().copied().filter(|bot| kept.contains(bot)).collect();
        js_sort_by(&mut tail, by_value);
        targets.extend(tail);
    }
    targets
}

/// `attackedBy(player)` (`:97-104`): the players our outgoing attacks target.
/// lookup-only: the result is only read by `has` (`:23, :59`).
fn attacked_by(st: &State, player: SmallId) -> LookupSet<SmallId> {
    let mut targets = LookupSet::new();
    for &a in st.player(player).outgoing_attacks().as_slice() {
        let target = st.attack(a).target();
        if target.is_player() {
            targets.insert(target);
        }
    }
    targets
}

/// `ownsStructures(player)` (`:106-108`).
fn owns_structures(st: &State, player: SmallId) -> bool {
    st.player(player)
        .units()
        .as_slice()
        .iter()
        .any(|&u| unit_groups::has(unit_groups::STRUCTURES, st.unit(u).unit_type()))
}

/// `class BotScanner` (`:111-182`). Mirrors PlayerExecution's surrounded check: every land tile
/// next to the bot's border counts.
struct BotScanner<'a> {
    st: &'a State,
    player: SmallId,
    nbuf: [TileRef; 4],
    /// `attackableBots: Map<number, Player | null>`: smallID → a bot we may attack, or `None` for
    /// anyone else. lookup-only: `get`/`set` only (`:169-180`).
    attackable_bots: LookupMap<u16, Option<SmallId>>,
}

impl<'a> BotScanner<'a> {
    fn new(st: &'a State, player: SmallId) -> Self {
        BotScanner { st, player, nbuf: [0; 4], attackable_bots: LookupMap::new() }
    }

    /// `scan(bot, closedOnly, within = null)` (`:121-166`): gives up (`None`) once the bot turns
    /// out open if `closed_only`, or borders a bot outside `within`.
    fn scan(&mut self, bot: SmallId, closed_only: bool, within: Option<&[SmallId]>) -> Option<BotSurroundings> {
        let st = self.st;
        let map = &st.map;
        let us = self.player.0;
        let self_id = bot.0;
        let mut s = BotSurroundings { bot, borders_us: false, open: false, bots: Vec::new() };
        let mut gave_up = false;
        // TS :138 `bot.borderTiles().forEach(...)`; once `gaveUp`, every later callback returns at
        // once (`:139`) and has no effect, so stopping the walk is equivalent.
        for tile in st.player(bot).border_tiles().iter() {
            if gave_up {
                break;
            }
            if map.is_ocean_shore(tile) || map.is_on_edge_of_map(tile) {
                s.open = true;
                gave_up = closed_only;
                continue;
            }
            let n = map.neighbors4(tile, &mut self.nbuf);
            for i in 0..n {
                let id = map.owner_id(self.nbuf[i]);
                if id == self_id {
                    continue;
                }
                if id == us {
                    s.borders_us = true;
                    continue;
                }
                let other = if id == 0 { None } else { self.attackable_bot(id) };
                match other {
                    None => {
                        s.open = true;
                        gave_up = closed_only;
                    }
                    Some(o) if within.is_some_and(|w| !w.contains(&o)) => {
                        gave_up = true;
                    }
                    Some(o) => s.add_bot(o),
                }
                if gave_up {
                    break;
                }
            }
        }
        if gave_up { None } else { Some(s) }
    }

    /// `attackableBot(smallID)` (`:168-181`).
    fn attackable_bot(&mut self, small_id: u16) -> Option<SmallId> {
        if let Some(&b) = self.attackable_bots.get(&small_id) {
            return b;
        }
        let st = self.st;
        let owner = st.player_by_small_id(small_id);
        let bot = if owner.is_player()
            && st.player(owner).player_type() == PlayerType::Bot
            && !st.player(self.player).is_friendly(owner, false)
        {
            Some(owner)
        } else {
            None
        };
        self.attackable_bots.insert(small_id, bot);
        bot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surroundings_bots_keep_set_order() {
        let mut s = BotSurroundings { bot: SmallId(1), borders_us: false, open: false, bots: Vec::new() };
        for b in [5, 3, 5, 9, 3, 1] {
            s.add_bot(SmallId(b));
        }
        // JS `Set` iteration = first-insertion order, duplicates ignored.
        assert_eq!(s.bots, vec![SmallId(5), SmallId(3), SmallId(9), SmallId(1)]);
    }
}
