//! `NationAllianceBehavior` (`execution/nation/NationAllianceBehavior.ts`). Owner: **P10**.
//! Snapshot `NationAllianceBehavior v1` is `{}`: no state of its own.

use crate::api::types::{Difficulty, GameMode, Relation};
use crate::exec::alliance::extension::AllianceExtensionExec;
use crate::exec::alliance::request::AllianceRequestExec;
use crate::exec::nation::NationExec;
use crate::exec::nation::emoji::{EMOJI_CONFUSED, EMOJI_HANDSHAKE, EMOJI_LOVE, EMOJI_SCARED_OF_THREAT};
use crate::exec::nation::utils::{find_juiciest_target, find_runaway_leader};
#[allow(unused_imports)]
use crate::exec::prelude::*;

/// `NationAllianceBehavior` data (none).
#[derive(StateClone, Debug, Default)]
pub struct AllianceB {}

/// CS: `{}` (`:40-42`).
impl crate::canon::Canon for AllianceB {
    fn canon(&self, _cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        crate::canon::Cs::Obj(Vec::new())
    }
}

/// `{ [Difficulty.Easy]: [60, 70], .. }` (`:373-378`).
fn troop_percent_range(d: Difficulty) -> (f64, f64) {
    match d {
        Difficulty::Easy => (60.0, 70.0),
        Difficulty::Medium => (70.0, 80.0),
        Difficulty::Hard => (75.0, 85.0),
        Difficulty::Impossible => (80.0, 90.0),
    }
}

/// `{ [Difficulty.Easy]: [70, 80], .. }` (`:379-384`).
fn tile_percent_range(d: Difficulty) -> (f64, f64) {
    match d {
        Difficulty::Easy => (70.0, 80.0),
        Difficulty::Medium => (80.0, 90.0),
        Difficulty::Hard => (85.0, 95.0),
        Difficulty::Impossible => (90.0, 100.0),
    }
}

impl NationExec {
    #[inline]
    fn alliance_difficulty(cx: &Ctx) -> Difficulty {
        cx.st.config().game_config().difficulty
    }

    /// `allianceBehavior.handleAllianceRequests()` (`:59-76`).
    pub fn alliance_handle_alliance_requests(&mut self, cx: &mut Ctx) {
        if cx.st.config().disable_alliances() {
            return;
        }

        let me = self.nation_player();
        // `incomingAllianceRequests()` is a fresh `filter` copy (PlayerImpl.ts:756-758).
        for req in cx.st.player(me).incoming_alliance_requests() {
            // Alliance Request intents created during the spawn phase are executed on
            // the first tick post-spawn phase. With the following condition we reject
            // all requests created during the spawn phase.
            if f64::from(cx.st.alliance_request(req).created_at()) <= cx.st.config().num_spawn_phase_turns() + 1.0 {
                cx.reject_alliance_request(req);
                continue;
            }
            let requestor = cx.st.alliance_request(req).requestor();
            if self.alliance_get_alliance_decision(cx, requestor, true) {
                cx.accept_alliance_request(req);
            } else {
                cx.reject_alliance_request(req);
            }
        }
    }

    /// `handleAllianceExtensionRequests()` (`:78-93`).
    pub fn alliance_handle_alliance_extension_requests(&mut self, cx: &mut Ctx) {
        if cx.st.config().disable_alliances() {
            return;
        }

        let me = self.nation_player();
        // iter-live: `alliances()` is the live array (PlayerImpl.ts:764-765); the body only draws
        // from the PRNG, reads state and queues executions (`addExecution`), so the array is never
        // reassigned or pushed to during the loop.
        let mut i = 0;
        while let Some(alliance) = cx.st.player(me).alliances().get(i) {
            i += 1;
            // Alliance expiration tracked by Events Panel, only human ally can click Request to Renew
            // Skip if no expiration yet/ ally didn't request extension yet / nation already agreed to extend
            if !cx.st.alliance(alliance).only_one_agreed_to_extend() {
                continue;
            }

            let human = cx.st.alliance(alliance).other(me);
            if !self.alliance_get_alliance_decision(cx, human, true) {
                continue;
            }

            let hid = cx.st.player(human).player_id().to_string();
            cx.st.add_execution(Exec::AllianceExtension(AllianceExtensionExec::new(me, hid)));
        }
    }

    /// `maybeSendAllianceRequests(borderingEnemies)` (`:95-116`).
    pub fn alliance_maybe_send_alliance_requests(&mut self, cx: &mut Ctx, bordering_enemies: &[SmallId]) {
        if cx.st.config().disable_alliances() {
            return;
        }

        let me = self.nation_player();
        // Only easy nations are allowed to send alliance requests to bots
        // Literal TS port of the upstream boolean expression; kept as written.
        #[allow(clippy::nonminimal_bool)]
        let is_acceptable_player_type = |cx: &Ctx, p: SmallId| {
            let t = cx.st.player(p).player_type();
            (t == PlayerType::Bot && Self::alliance_difficulty(cx) == Difficulty::Easy) || t != PlayerType::Bot
        };

        for &enemy in bordering_enemies {
            if self.rng.chance(30.0)
                && is_acceptable_player_type(cx, enemy)
                && cx.st.player(me).can_send_alliance_request(enemy)
                && self.alliance_get_alliance_decision(cx, enemy, false)
            {
                let eid = cx.st.player(enemy).player_id().to_string();
                cx.st.add_execution(Exec::AllianceRequest(AllianceRequestExec::new(me, eid)));
            }
        }
    }

    /// `getAllianceDecision(otherPlayer, isResponse)` (`:118-182`).
    fn alliance_get_alliance_decision(&mut self, cx: &mut Ctx, other_player: SmallId, is_response: bool) -> bool {
        let me = self.nation_player();
        let other = EmojiRecipient::Player(other_player);
        // Easy (dumb) nations sometimes get confused and accept/reject randomly (Just like dumb humans do)
        if self.alliance_is_confused(cx) {
            return self.rng.chance(2.0);
        }
        // Nearly always reject traitors
        if cx.st.player(other_player).is_traitor() && self.rng.next_int(0.0, 100.0) >= 10.0 {
            if is_response && self.rng.chance(3.0) {
                self.emoji_send_emoji(cx, other, EMOJI_CONFUSED);
            }
            return false;
        }
        // Reject if otherPlayer has allied with a lot of players (Hard and Impossible only)
        // To make sure there are enough non-friendly players in the game to stop the crown with nukes
        if self.alliance_has_too_many_alliances(cx, other_player) {
            return false;
        }
        // Don't help a runaway leader grow even further (Medium and up)
        if self.alliance_is_runaway_leader(cx, other_player) {
            return false;
        }
        // Before caring about the relation, first check if the otherPlayer is a threat
        // Easy (dumb) nations are blinded by hatred, they don't care about threats, they care about the relation
        // Impossible (smart) nations on the other hand are analyzing the facts
        if self.alliance_is_alliance_partner_threat(cx, other_player) {
            if !is_response && self.rng.chance(6.0) {
                self.emoji_send_emoji(cx, other, EMOJI_SCARED_OF_THREAT);
            }
            if is_response && self.rng.chance(6.0) {
                self.emoji_send_emoji(cx, other, EMOJI_LOVE);
            }
            return true;
        }
        // Maybe reject if we are in a team game (allying makes less sense there)
        if self.alliance_should_reject_in_team_game(cx) {
            return false;
        }
        // Reject if relation is bad
        if cx.st.player(me).relation(other_player) < Relation::Neutral {
            if is_response && self.rng.chance(3.0) {
                self.emoji_send_emoji(cx, other, EMOJI_CONFUSED);
            }
            return false;
        }
        // Maybe accept if relation is friendly
        if self.alliance_is_alliance_partner_friendly(cx, other_player) {
            if self.rng.chance(3.0) {
                self.emoji_send_emoji(cx, other, EMOJI_HANDSHAKE);
            }
            return true;
        }
        // Reject if we already have some alliances, we don't want to ally with the entire map
        if self.alliance_check_already_enough_alliances(cx, other_player) {
            return false;
        }
        // Maybe accept if we are in the earlygame
        if self.alliance_is_earlygame(cx) {
            return true;
        }
        // Accept if we are similarly strong
        self.alliance_is_alliance_partner_similarly_strong(cx, other_player)
    }

    /// `hasTooManyAlliances(otherPlayer)` (`:184-206`).
    fn alliance_has_too_many_alliances(&mut self, cx: &mut Ctx, other_player: SmallId) -> bool {
        let difficulty = Self::alliance_difficulty(cx);
        if difficulty != Difficulty::Hard && difficulty != Difficulty::Impossible {
            return false;
        }

        let me = self.nation_player();
        let total_players =
            cx.st.players().into_iter().filter(|&p| cx.st.player(p).player_type() != PlayerType::Bot).count() as f64;
        // An alliance with us is the one being renewed, not another one
        let other_player_alliances = cx.st.player(other_player).alliances().len() as f64
            - (if cx.st.player(other_player).is_allied_with(me) { 1.0 } else { 0.0 });

        if difficulty == Difficulty::Hard {
            other_player_alliances >= total_players * 0.5
        } else {
            other_player_alliances >= total_players * 0.25
        }
    }

    /// `isRunawayLeader(otherPlayer)` (`:208-210`).
    fn alliance_is_runaway_leader(&mut self, cx: &mut Ctx, other_player: SmallId) -> bool {
        find_runaway_leader(cx.st) == Some(other_player)
    }

    /// `isConfused()` (`:212-226`).
    fn alliance_is_confused(&mut self, cx: &mut Ctx) -> bool {
        match Self::alliance_difficulty(cx) {
            Difficulty::Easy => self.rng.chance(10.0), // 10% chance to be confused on easy
            Difficulty::Medium => self.rng.chance(20.0), // 5% chance to be confused on medium
            Difficulty::Hard => self.rng.chance(40.0), // 2.5% chance to be confused on hard
            Difficulty::Impossible => false,           // No confusion on impossible
        }
    }

    /// `isEarlygame()` (`:228-259`).
    fn alliance_is_earlygame(&mut self, cx: &mut Ctx) -> bool {
        let spawn_ticks = cx.st.config().num_spawn_phase_turns();
        let ticks = f64::from(cx.st.ticks());
        match Self::alliance_difficulty(cx) {
            // On easy, accept 90% in the first 5 minutes
            Difficulty::Easy => ticks < 3000.0 + spawn_ticks && self.rng.next_int(0.0, 100.0) >= 10.0,
            // On medium, accept 70% in the first 3 minutes
            Difficulty::Medium => ticks < 1800.0 + spawn_ticks && self.rng.next_int(0.0, 100.0) >= 30.0,
            // On hard, accept 50% in the first 3 minutes
            Difficulty::Hard => ticks < 1800.0 + spawn_ticks && self.rng.next_int(0.0, 100.0) >= 50.0,
            // On impossible, accept 30% in the first minute
            Difficulty::Impossible => ticks < 600.0 + spawn_ticks && self.rng.next_int(0.0, 100.0) >= 70.0,
        }
    }

    /// `isAlliancePartnerThreat(otherPlayer)` (`:261-293`).
    fn alliance_is_alliance_partner_threat(&mut self, cx: &mut Ctx, other_player: SmallId) -> bool {
        let me = self.nation_player();
        let st = &*cx.st;
        let (o, p) = (st.player(other_player), st.player(me));
        match Self::alliance_difficulty(cx) {
            // On easy we are very dumb, we don't see anybody as a threat
            Difficulty::Easy => false,
            // On medium we just see players with much more troops as a threat
            Difficulty::Medium => o.troops() > p.troops() * 2.5,
            // On hard we are smarter, we check for maxTroops to see the actual strength
            Difficulty::Hard => {
                o.troops() > p.troops() && st.config().max_troops(&o) > st.config().max_troops(&p) * 2.0
            }
            // On impossible we check for multiple factors and try to not mess with stronger players (we want to steamroll over weaklings)
            Difficulty::Impossible => {
                let other_has_more_troops = o.troops() > p.troops() * 1.5;
                let other_has_more_max_troops =
                    o.troops() > p.troops() && st.config().max_troops(&o) > st.config().max_troops(&p) * 1.5;
                let other_has_more_tiles =
                    o.troops() > p.troops() && f64::from(o.num_tiles_owned()) > f64::from(p.num_tiles_owned()) * 1.5;
                other_has_more_troops || other_has_more_max_troops || other_has_more_tiles
            }
        }
    }

    /// `shouldRejectInTeamGame()` (`:295-313`).
    fn alliance_should_reject_in_team_game(&mut self, cx: &mut Ctx) -> bool {
        if cx.st.config().game_config().game_mode != GameMode::Team {
            return false;
        }

        match Self::alliance_difficulty(cx) {
            Difficulty::Easy => self.rng.next_int(0.0, 100.0) < 25.0, // 25% chance to reject on easy
            Difficulty::Medium => self.rng.next_int(0.0, 100.0) < 50.0, // 50% chance to reject on medium
            Difficulty::Hard => self.rng.next_int(0.0, 100.0) < 75.0, // 75% chance to reject on hard
            Difficulty::Impossible => true,                           // 100% chance to reject on impossible
        }
    }

    /// `checkAlreadyEnoughAlliances(otherPlayer)` (`:315-347`).
    fn alliance_check_already_enough_alliances(&mut self, cx: &mut Ctx, other_player: SmallId) -> bool {
        let me = self.nation_player();
        let difficulty = Self::alliance_difficulty(cx);
        match difficulty {
            Difficulty::Easy => false, // On easy we never think we have enough alliances
            Difficulty::Medium => {
                // TS :321: the left operand is read before the draw.
                let n = cx.st.player(me).alliances().len() as f64;
                n >= self.rng.next_int(4.0, 6.0)
            }
            Difficulty::Hard | Difficulty::Impossible => {
                // On hard and impossible we try to not ally with all our neighbors (If we have 2+ neighbors)
                let bordering_players: Vec<SmallId> = cx
                    .st
                    .nearby_memo(me)
                    .into_iter()
                    .filter(|&n| n.is_player() && cx.st.player(n).player_type() != PlayerType::Bot)
                    .collect();
                let bordering_friends =
                    bordering_players.iter().filter(|&&o| cx.st.player(me).is_friendly(o, false)).count();
                if bordering_players.len() >= 2 && bordering_players.contains(&other_player) {
                    return bordering_players.len() <= bordering_friends + 1;
                }
                let n = cx.st.player(me).alliances().len() as f64;
                if difficulty == Difficulty::Hard {
                    return n >= self.rng.next_int(3.0, 5.0);
                }
                n >= self.rng.next_int(2.0, 4.0)
            }
        }
    }

    /// `isAlliancePartnerFriendly(otherPlayer)` (`:349-368`).
    fn alliance_is_alliance_partner_friendly(&mut self, cx: &mut Ctx, other_player: SmallId) -> bool {
        let me = self.nation_player();
        let friendly = cx.st.player(me).relation(other_player) == Relation::Friendly;
        match Self::alliance_difficulty(cx) {
            Difficulty::Easy | Difficulty::Medium => friendly,
            Difficulty::Hard => friendly && self.rng.next_int(0.0, 100.0) >= 17.0,
            Difficulty::Impossible => friendly && self.rng.next_int(0.0, 100.0) >= 33.0,
        }
    }

    /// `isAlliancePartnerSimilarlyStrong(otherPlayer)` (`:370-410`).
    // It would make a lot of sense to use nextFloat here, but "there's a chance floats can cause desyncs"
    fn alliance_is_alliance_partner_similarly_strong(&mut self, cx: &mut Ctx, other_player: SmallId) -> bool {
        let me = self.nation_player();
        let difficulty = Self::alliance_difficulty(cx);
        let troop_range = troop_percent_range(difficulty);
        let tile_range = tile_percent_range(difficulty);

        let outgoing = |cx: &Ctx, p: SmallId| -> f64 {
            // TS :389-394 `outgoingAttacks().reduce((sum, attack) => sum + attack.troops(), 0)`.
            let mut sum = 0.0;
            for &a in cx.st.player(p).outgoing_attacks().as_slice() {
                sum += cx.st.attack(a).troops();
            }
            sum
        };
        let player_outgoing_troops = outgoing(cx, me);
        let other_outgoing_troops = outgoing(cx, other_player);
        let player_total_troops = cx.st.player(me).troops() + player_outgoing_troops;
        let other_total_troops = cx.st.player(other_player).troops() + other_outgoing_troops;

        let troop_threshold = player_total_troops * (self.rng.next_int(troop_range.0, troop_range.1) / 100.0);
        let tile_threshold =
            f64::from(cx.st.player(me).num_tiles_owned()) * (self.rng.next_int(tile_range.0, tile_range.1) / 100.0);

        let has_comparable_troops = other_total_troops > troop_threshold;
        let has_comparable_tiles = f64::from(cx.st.player(other_player).num_tiles_owned()) > tile_threshold
            && other_total_troops > player_total_troops * 0.5;
        has_comparable_troops || has_comparable_tiles
    }

    /// `maybeBetray(otherPlayer, juiciestAlly, borderingFriends, borderingEnemies)` (`:414-471`).
    /// `juiciestAlly` comes from `findJuiciestAlly(borderingFriends)`: callers looping over
    /// `borderingFriends` compute it once.
    pub fn alliance_maybe_betray(
        &mut self,
        cx: &mut Ctx,
        other_player: SmallId,
        juiciest_ally: Option<SmallId>,
        bordering_friends: &[SmallId],
        bordering_enemies: &[SmallId],
    ) -> bool {
        let me = self.nation_player();
        if !cx.st.player(me).is_allied_with(other_player) {
            return false;
        }

        let difficulty = Self::alliance_difficulty(cx);

        // Betray our juiciest ally (e.g. a MIRVed one), if it's safe to do so (everybody around us is weak)
        if (difficulty == Difficulty::Hard || difficulty == Difficulty::Impossible)
            && juiciest_ally == Some(other_player)
            && self.alliance_is_safe_to_betray(cx, other_player, bordering_friends, bordering_enemies)
        {
            self.alliance_betray(cx, other_player);
            return true;
        }

        // Betray very weak players (similar check as above but for the easier difficulties)
        // This doesn't check for maxTroops and isn't really smart. It makes nations vulnerable, but that's intended.
        // On easy, don't betray humans
        if (difficulty == Difficulty::Easy || difficulty == Difficulty::Medium)
            && !(difficulty == Difficulty::Easy && cx.st.player(other_player).player_type() == PlayerType::Human)
            && cx.st.player(me).troops() >= cx.st.player(other_player).troops() * 10.0
        {
            self.alliance_betray(cx, other_player);
            return true;
        }

        // Betray traitors who aren't significantly stronger than us
        if difficulty != Difficulty::Easy
            && cx.st.player(other_player).is_traitor()
            && cx.st.player(other_player).troops() < cx.st.player(me).troops() * 1.2
        {
            self.alliance_betray(cx, other_player);
            return true;
        }

        // Betray our only bordering player if we are much stronger than them
        if difficulty != Difficulty::Easy
            && bordering_friends.len() + bordering_enemies.len() == 1
            && cx.st.player(other_player).troops() * 3.0 < cx.st.player(me).troops()
        {
            self.alliance_betray(cx, other_player);
            return true;
        }

        false
    }

    /// `findJuiciestAlly(borderingFriends)` (`:473-479`): juiciest ally regardless of strength;
    /// `isSafeToBetray()` rejects the too-strong ones.
    pub fn alliance_find_juiciest_ally(&mut self, cx: &mut Ctx, bordering_friends: &[SmallId]) -> Option<SmallId> {
        let me = self.nation_player();
        let candidates: Vec<SmallId> =
            bordering_friends.iter().copied().filter(|&f| cx.st.player(me).is_allied_with(f)).collect();
        find_juiciest_target(cx.st, &candidates)
    }

    /// `isSafeToBetray(target, borderingFriends, borderingEnemies)` (`:481-501`): safe if target +
    /// non-allied neighbors + (unless target's already a traitor, since betraying them wouldn't
    /// make us one) our other allies stay under a third of our troops.
    fn alliance_is_safe_to_betray(
        &mut self,
        cx: &mut Ctx,
        target: SmallId,
        bordering_friends: &[SmallId],
        bordering_enemies: &[SmallId],
    ) -> bool {
        let me = self.nation_player();
        let st = &*cx.st;
        let other_allies: Vec<SmallId> = if st.player(target).is_traitor() {
            Vec::new()
        } else {
            bordering_friends.iter().copied().filter(|&f| f != target && st.player(me).is_allied_with(f)).collect()
        };
        // TS :493 `[target, ...borderingEnemies, ...otherAllies]`, then a left fold from 0.
        let mut nearby_threat_troops = 0.0;
        for &threat in std::iter::once(&target).chain(bordering_enemies.iter()).chain(other_allies.iter()) {
            let mut outgoing = 0.0;
            for &a in st.player(threat).outgoing_attacks().as_slice() {
                outgoing += st.attack(a).troops();
            }
            nearby_threat_troops = nearby_threat_troops + st.player(threat).troops() + outgoing;
        }
        nearby_threat_troops < st.player(me).troops() * 0.33
    }

    /// `betray(target)` (`:503-507`).
    fn alliance_betray(&mut self, cx: &mut Ctx, target: SmallId) {
        let me = self.nation_player();
        let alliance = cx.st.player(me).alliance_with(target);
        let Some(alliance) = alliance else { return };
        cx.player_mut(me).break_alliance(alliance);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_ranges_match_ts_tables() {
        assert_eq!(troop_percent_range(Difficulty::Easy), (60.0, 70.0));
        assert_eq!(troop_percent_range(Difficulty::Impossible), (80.0, 90.0));
        assert_eq!(tile_percent_range(Difficulty::Hard), (85.0, 95.0));
        assert_eq!(tile_percent_range(Difficulty::Impossible), (90.0, 100.0));
    }
}
