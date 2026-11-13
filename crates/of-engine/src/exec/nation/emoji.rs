//! `NationEmojiBehavior` (`execution/nation/NationEmojiBehavior.ts`). Owner: **P10**.
//! The `EMOJI_*` id lists below were generated from the TS source by script (`emojiId` =
//! index into `FLATTENED_EMOJI_TABLE`).

use crate::api::types::{ALL_PLAYERS, Difficulty, GameMode, Relation};
use crate::exec::emoji::EmojiExec;
use crate::exec::nation::NationExec;
use crate::exec::nation::utils::rand_elem;
#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::state::game::Winner;
use of_det::js_sort_by;

/// `EMOJI_ASSIST_ACCEPT` (`NationEmojiBehavior.ts:28`).
pub const EMOJI_ASSIST_ACCEPT: &[u32] = &[20, 25, 41];
/// `EMOJI_ASSIST_RELATION_TOO_LOW` (`NationEmojiBehavior.ts:29`).
pub const EMOJI_ASSIST_RELATION_TOO_LOW: &[u32] = &[12, 24];
/// `EMOJI_ASSIST_TARGET_ME` (`NationEmojiBehavior.ts:32`).
pub const EMOJI_ASSIST_TARGET_ME: &[u32] = &[6, 32];
/// `EMOJI_ASSIST_TARGET_ALLY` (`NationEmojiBehavior.ts:33`).
pub const EMOJI_ASSIST_TARGET_ALLY: &[u32] = &[27, 21];
/// `EMOJI_AGGRESSIVE_ATTACK` (`NationEmojiBehavior.ts:34`).
pub const EMOJI_AGGRESSIVE_ATTACK: &[u32] = &[10];
/// `EMOJI_ATTACK` (`NationEmojiBehavior.ts:35`).
pub const EMOJI_ATTACK: &[u32] = &[9];
/// `EMOJI_WARSHIP_RETALIATION` (`NationEmojiBehavior.ts:36`).
pub const EMOJI_WARSHIP_RETALIATION: &[u32] = &[52];
/// `EMOJI_NUKE` (`NationEmojiBehavior.ts:37`).
pub const EMOJI_NUKE: &[u32] = &[33, 31];
/// `EMOJI_GOT_INSULTED` (`NationEmojiBehavior.ts:38`).
pub const EMOJI_GOT_INSULTED: &[u32] = &[14, 9, 11, 5, 7];
/// `EMOJI_LOVE` (`NationEmojiBehavior.ts:41`).
pub const EMOJI_LOVE: &[u32] = &[48, 1, 2];
/// `EMOJI_CONFUSED` (`NationEmojiBehavior.ts:42`).
pub const EMOJI_CONFUSED: &[u32] = &[57, 11];
/// `EMOJI_BRAG` (`NationEmojiBehavior.ts:43`).
pub const EMOJI_BRAG: &[u32] = &[38, 39, 19];
/// `EMOJI_CHARM_ALLIES` (`NationEmojiBehavior.ts:44`).
pub const EMOJI_CHARM_ALLIES: &[u32] = &[25, 3, 19];
/// `EMOJI_CLOWN` (`NationEmojiBehavior.ts:45`).
pub const EMOJI_CLOWN: &[u32] = &[11, 24];
/// `EMOJI_RAT` (`NationEmojiBehavior.ts:46`).
pub const EMOJI_RAT: &[u32] = &[59];
/// `EMOJI_OVERWHELMED` (`NationEmojiBehavior.ts:47`).
pub const EMOJI_OVERWHELMED: &[u32] = &[32, 26, 8, 6, 7, 5, 13, 15];
/// `EMOJI_CONGRATULATE` (`NationEmojiBehavior.ts:50`).
pub const EMOJI_CONGRATULATE: &[u32] = &[16];
/// `EMOJI_VICTORY` (`NationEmojiBehavior.ts:51`).
pub const EMOJI_VICTORY: &[u32] = &[38, 39, 4, 19, 0];
/// `EMOJI_SCARED_OF_THREAT` (`NationEmojiBehavior.ts:54`).
pub const EMOJI_SCARED_OF_THREAT: &[u32] = &[18, 6];
/// `EMOJI_BORED` (`NationEmojiBehavior.ts:55`).
pub const EMOJI_BORED: &[u32] = &[12];
/// `EMOJI_HANDSHAKE` (`NationEmojiBehavior.ts:56`).
pub const EMOJI_HANDSHAKE: &[u32] = &[25];
/// `EMOJI_DONATION_OK` (`NationEmojiBehavior.ts:57`).
pub const EMOJI_DONATION_OK: &[u32] = &[20];
/// `EMOJI_DONATION_TOO_SMALL` (`NationEmojiBehavior.ts:58`).
pub const EMOJI_DONATION_TOO_SMALL: &[u32] = &[57, 12];
/// `EMOJI_GREET` (`NationEmojiBehavior.ts:59`).
pub const EMOJI_GREET: &[u32] = &[15];

/// `NationEmojiBehavior` data; snapshot `{ lastEmojiSent: [(uint, int)], gameOver }`.
#[derive(StateClone, Debug, Default)]
pub struct EmojiB {
    /// `lastEmojiSent = new Map<Player, Tick>()` (`:62`); iterated by the snapshot (insertion order).
    pub last_emoji_sent: SmallJsMap<SmallIdKey, Tick, 8>,
    pub game_over: bool,
}

/// CS: `{ lastEmojiSent: [(player, tick)] (Map order), gameOver }` (`:71-78`).
impl crate::canon::Canon for EmojiB {
    fn canon(&self, cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        use crate::canon::Cs;
        let last: Vec<Cs> = self.last_emoji_sent.iter().map(|(p, t)| Cs::Arr(vec![p.canon(cx), t.canon(cx)])).collect();
        Cs::obj([("lastEmojiSent", Cs::Arr(last)), ("gameOver", Cs::Bool(self.game_over))])
    }
}

/// `otherPlayer === AllPlayers ? AllPlayers : otherPlayer.id()` (`:305`).
fn recipient_id(st: &crate::state::State, r: EmojiRecipient) -> String {
    match r {
        EmojiRecipient::AllPlayers => ALL_PLAYERS.to_string(),
        EmojiRecipient::Player(p) => st.player(p).player_id().to_string(),
    }
}

impl NationExec {
    #[inline]
    fn emoji_state(&mut self) -> &mut EmojiB {
        &mut self.b().emoji
    }

    /// `emojiBehavior.maybeSendCasualEmoji()` (`:97-108`).
    pub fn emoji_maybe_send_casual_emoji(&mut self, cx: &mut Ctx) {
        if self.emoji_state().game_over {
            return;
        }

        self.emoji_check_overwhelmed_by_attacks(cx);
        self.emoji_check_very_small_attack(cx);
        self.emoji_congratulate_winner(cx);
        self.emoji_brag(cx);
        self.emoji_charm_allies(cx);
        self.emoji_annoy_traitors(cx);
        self.emoji_find_rat(cx);
        self.emoji_greet_nearby_players(cx);
    }

    /// `checkOverwhelmedByAttacks()` (`:110-126`).
    fn emoji_check_overwhelmed_by_attacks(&mut self, cx: &mut Ctx) {
        if !self.rng.chance(16.0) {
            return;
        }
        let me = self.nation_player();
        let incoming_attacks = cx.st.player(me).incoming_attacks();
        if incoming_attacks.is_empty() {
            return;
        }

        // TS :116-119 `reduce((sum, attack) => sum + attack.troops(), 0)`, list order.
        let mut incoming_troops = 0.0;
        for &a in &incoming_attacks {
            incoming_troops += cx.st.attack(a).troops();
        }
        let our_troops = cx.st.player(me).troops();

        // If incoming troops are at least 3x our troops, we're overwhelmed
        if incoming_troops >= our_troops * 3.0 {
            self.emoji_send_emoji(cx, EmojiRecipient::AllPlayers, EMOJI_OVERWHELMED);
        }
    }

    /// `checkVerySmallAttack()` (`:128-149`).
    fn emoji_check_very_small_attack(&mut self, cx: &mut Ctx) {
        if !self.rng.chance(8.0) {
            return;
        }
        let me = self.nation_player();
        let incoming_attacks = cx.st.player(me).incoming_attacks();
        if incoming_attacks.is_empty() {
            return;
        }

        let our_troops = cx.st.player(me).troops();
        if our_troops <= 0.0 {
            return;
        }

        // Find attacks from humans that are very small (less than 10% of our troops).
        // `incomingAttacks()` is a fresh `filter` copy (PlayerImpl.ts:1903-1904): iterate the copy.
        for &attack in &incoming_attacks {
            let attacker = cx.st.attack(attack).attacker();
            if cx.st.player(attacker).player_type() != PlayerType::Human {
                continue;
            }

            if cx.st.attack(attack).troops() < our_troops * 0.1 {
                // TS :143-146: the `chance(2)` argument is evaluated before `maybeSendEmoji` runs.
                let list = if self.rng.chance(2.0) { EMOJI_CONFUSED } else { EMOJI_BORED };
                self.emoji_maybe_send_emoji(cx, EmojiRecipient::Player(attacker), list);
            }
        }
    }

    /// `congratulateWinner()` (`:152-185`).
    fn emoji_congratulate_winner(&mut self, cx: &mut Ctx) {
        let winner = cx.st.get_winner().clone();
        if matches!(winner, Winner::None) {
            return;
        }

        self.emoji_state().game_over = true;

        let me = self.nation_player();
        let is_me_or_my_team = match &winner {
            Winner::None => false,
            Winner::Player(p) => *p == me,
            Winner::Team(t) => cx.st.player(me).team() == Some(t),
        };
        if is_me_or_my_team {
            // Half of the winning nations celebrate
            if self.rng.chance(2.0) {
                self.emoji_send_emoji(cx, EmojiRecipient::AllPlayers, EMOJI_VICTORY);
            }
            return;
        }

        let is_team_game = cx.st.config().game_config().game_mode == GameMode::Team;

        if is_team_game {
            // Team game: all nations congratulate if another team won
            self.emoji_send_emoji(cx, EmojiRecipient::AllPlayers, EMOJI_CONGRATULATE);
        } else {
            // FFA game: The largest nation congratulates if a human player won
            let winner = match winner {
                Winner::Player(p) => p,
                _ => return, // It's a team, not a player (TS :174)
            };

            // Only the largest nation sends the congratulation.
            // TS :177-180 `players().filter(Nation).sort((a, b) => b.numTilesOwned() - a.numTilesOwned())[0]`.
            let mut nations: Vec<SmallId> =
                cx.st.players().into_iter().filter(|&p| cx.st.player(p).player_type() == PlayerType::Nation).collect();
            js_sort_by(&mut nations, |a, b| {
                f64::from(cx.st.player(*b).num_tiles_owned()) - f64::from(cx.st.player(*a).num_tiles_owned())
            });
            if nations.first().copied() != Some(me) {
                return;
            }

            self.emoji_send_emoji(cx, EmojiRecipient::Player(winner), EMOJI_CONGRATULATE);
        }
    }

    /// `brag()` (`:188-198`).
    fn emoji_brag(&mut self, cx: &mut Ctx) {
        if !self.rng.chance(300.0) {
            return;
        }

        // TS :191-193: `players()` is a fresh `filter` copy (GameImpl.ts:775-777); sort it.
        let mut sorted = cx.st.players();
        js_sort_by(&mut sorted, |a, b| {
            f64::from(cx.st.player(*b).num_tiles_owned()) - f64::from(cx.st.player(*a).num_tiles_owned())
        });

        if sorted.is_empty() || sorted[0] != self.nation_player() {
            return;
        }

        self.emoji_send_emoji(cx, EmojiRecipient::AllPlayers, EMOJI_BRAG);
    }

    /// `charmAllies()` (`:200-211`).
    fn emoji_charm_allies(&mut self, cx: &mut Ctx) {
        if !self.rng.chance(250.0) {
            return;
        }

        let me = self.nation_player();
        let human_allies: Vec<SmallId> = cx
            .st
            .player(me)
            .allies()
            .into_iter()
            .filter(|&p| cx.st.player(p).player_type() == PlayerType::Human)
            .collect();
        if human_allies.is_empty() {
            return;
        }

        let ally = rand_elem(&mut self.rng, &human_allies);
        let emoji_list = if self.rng.chance(3.0) { EMOJI_LOVE } else { EMOJI_CHARM_ALLIES };
        self.emoji_send_emoji(cx, EmojiRecipient::Player(ally), emoji_list);
    }

    /// `annoyTraitors()` (`:213-222`).
    fn emoji_annoy_traitors(&mut self, cx: &mut Ctx) {
        if !self.rng.chance(3.0) {
            return;
        }

        let me = self.nation_player();
        // `players()` is a copy (GameImpl.ts:775-777).
        for traitor in cx.st.players() {
            if cx.st.player(traitor).player_type() != PlayerType::Human || !cx.st.player(traitor).is_traitor() {
                continue;
            }
            // Only one nation mocks each traitor, otherwise every nation on the map piles on
            if traitor_mocker(cx.st, traitor) != Some(me) {
                continue;
            }
            self.emoji_send_emoji(cx, EmojiRecipient::Player(traitor), EMOJI_CLOWN);
        }
    }

    /// `findRat()` (`:236-256`).
    fn emoji_find_rat(&mut self, cx: &mut Ctx) {
        if cx.st.ticks() < 6000 {
            return; // Ignore first 10 minutes (everybody is small in the early game)
        }
        if !self.rng.chance(10000.0) {
            return;
        }

        let total_land = f64::from(cx.st.map.num_land_tiles());
        let threshold = total_land * 0.01; // 1% of land

        let small_players: Vec<SmallId> = cx
            .st
            .players()
            .into_iter()
            .filter(|&p| {
                let pr = cx.st.player(p);
                pr.player_type() == PlayerType::Human
                    && f64::from(pr.num_tiles_owned()) < threshold
                    && pr.num_tiles_owned() > 0
            })
            .collect();

        if small_players.is_empty() {
            return;
        }

        let small_player = rand_elem(&mut self.rng, &small_players);
        self.emoji_send_emoji(cx, EmojiRecipient::Player(small_player), EMOJI_RAT);
    }

    /// `greetNearbyPlayers()` (`:258-272`).
    fn emoji_greet_nearby_players(&mut self, cx: &mut Ctx) {
        if cx.st.ticks() > 600 {
            return; // Only in the first minute
        }
        if !self.rng.chance(250.0) {
            return;
        }

        let me = self.nation_player();
        let nearby_humans: Vec<SmallId> = cx
            .st
            .nearby_memo(me)
            .into_iter()
            .filter(|&p| p.is_player() && cx.st.player(p).player_type() == PlayerType::Human)
            .collect();

        if nearby_humans.is_empty() {
            return;
        }

        let neighbor = rand_elem(&mut self.rng, &nearby_humans);
        self.emoji_send_emoji(cx, EmojiRecipient::Player(neighbor), EMOJI_GREET);
    }

    /// `maybeSendEmoji(otherPlayer, emojisList)` (`:274-281`).
    pub fn emoji_maybe_send_emoji(&mut self, cx: &mut Ctx, other_player: EmojiRecipient, emojis_list: &[u32]) {
        if !self.emoji_should_send_emoji(cx, other_player, true) {
            return;
        }

        self.emoji_send_emoji(cx, other_player, emojis_list)
    }

    /// `maybeSendAttackEmoji(otherPlayer)` (`:283-296`).
    pub fn emoji_maybe_send_attack_emoji(&mut self, cx: &mut Ctx, other_player: SmallId) {
        let other = EmojiRecipient::Player(other_player);
        if !self.emoji_should_send_emoji(cx, other, true) {
            return;
        }

        // If we have a good relation to the other player, we are probably attacking first (aggressive)
        if cx.st.player(self.nation_player()).relation(other_player) >= Relation::Neutral {
            if !self.rng.chance(2.0) {
                return;
            }
            self.emoji_send_emoji(cx, other, EMOJI_AGGRESSIVE_ATTACK);
            return;
        }

        // We are probably retaliating
        if !self.rng.chance(4.0) {
            return;
        }
        self.emoji_send_emoji(cx, other, EMOJI_ATTACK);
    }

    /// `sendEmoji(otherPlayer, emojisList)` (`:298-309`).
    pub fn emoji_send_emoji(&mut self, cx: &mut Ctx, other_player: EmojiRecipient, emojis_list: &[u32]) {
        if !self.emoji_should_send_emoji(cx, other_player, false) {
            return;
        }
        let me = self.nation_player();
        if !cx.st.player(me).can_send_emoji(other_player) {
            return;
        }

        // TS :302-308: the constructor arguments are evaluated in order: recipient id, then the
        // `randElement` draw.
        let rid = recipient_id(cx.st, other_player);
        let emoji = rand_elem(&mut self.rng, emojis_list);
        cx.st.add_execution(Exec::Emoji(EmojiExec::new(me, rid, f64::from(emoji))));
    }

    /// `shouldSendEmoji(otherPlayer, limitEmojisByTime = true)` (`:311-326`).
    fn emoji_should_send_emoji(
        &mut self,
        cx: &mut Ctx,
        other_player: EmojiRecipient,
        limit_emojis_by_time: bool,
    ) -> bool {
        let other = match other_player {
            EmojiRecipient::AllPlayers => return true,
            EmojiRecipient::Player(p) => p,
        };
        let me = self.nation_player();
        if cx.st.player(me).player_type() == PlayerType::Bot {
            return false;
        }
        if cx.st.player(other).player_type() != PlayerType::Human {
            return false;
        }

        if limit_emojis_by_time {
            let ticks = cx.st.ticks();
            let last_sent = self.emoji_state().last_emoji_sent.get(&other).copied().unwrap_or(-300);
            // int: ticks and `lastSent` are small integers; i32 subtraction is exact.
            if ticks - last_sent <= 300 {
                return false;
            }
            self.emoji_state().last_emoji_sent.insert(other, ticks);
        }

        true
    }
}

/// `traitorMocker(traitor)` (`:225-234`): the most hostile nation (usually the betrayed one), first
/// in player order on ties.
fn traitor_mocker(st: &crate::state::State, traitor: SmallId) -> Option<SmallId> {
    let mut mocker: Option<SmallId> = None;
    for p in st.players() {
        if st.player(p).player_type() != PlayerType::Nation || st.player(p).is_friendly(traitor, false) {
            continue;
        }
        let better = match mocker {
            None => true,
            Some(m) => st.player(p).relation(traitor) < st.player(m).relation(traitor),
        };
        if better {
            mocker = Some(p);
        }
    }
    mocker
}

/// `"🖕"` (`NationEmojiBehavior.ts:341`).
const EMOJI_STR_MIDDLE_FINGER: &str = "\u{1f595}";
/// `"🤡"` (`:352`).
const EMOJI_STR_CLOWN: &str = "\u{1f921}";
/// `["🕊️", "🏳️", "❤️", "🥰", "👏"]` (`:363`), code points copied from the TS source bytes.
const EMOJI_STR_FRIENDLY: [&str; 5] =
    ["\u{1f54a}\u{fe0f}", "\u{1f3f3}\u{fe0f}", "\u{2764}\u{fe0f}", "\u{1f970}", "\u{1f44f}"];

/// `respondToEmoji(game, random, sender, recipient, emojiString)` (`:329-377`); called from
/// `EmojiExecution` with its own PRNG.
pub fn respond_to_emoji(
    cx: &mut Ctx,
    random: &mut Prng,
    sender: SmallId,
    recipient: EmojiRecipient,
    emoji_string: &str,
) {
    let recipient = match recipient {
        EmojiRecipient::AllPlayers => return,
        EmojiRecipient::Player(r) => r,
    };
    if cx.st.player(recipient).player_type() != PlayerType::Nation {
        return;
    }
    if !cx.st.player(recipient).can_send_emoji(EmojiRecipient::Player(sender)) {
        return;
    }

    if emoji_string == EMOJI_STR_MIDDLE_FINGER {
        cx.player_mut(recipient).update_relation(sender, -100.0);
        let sid = cx.st.player(sender).player_id().to_string();
        let e = rand_elem(random, EMOJI_GOT_INSULTED);
        cx.st.add_execution(Exec::Emoji(EmojiExec::new(recipient, sid, f64::from(e))));
    }

    if emoji_string == EMOJI_STR_CLOWN {
        cx.player_mut(recipient).update_relation(sender, -10.0);
        let sid = cx.st.player(sender).player_id().to_string();
        let e = rand_elem(random, EMOJI_CONFUSED);
        cx.st.add_execution(Exec::Emoji(EmojiExec::new(recipient, sid, f64::from(e))));
    }

    if EMOJI_STR_FRIENDLY.contains(&emoji_string) {
        if cx.st.config().game_config().difficulty == Difficulty::Easy {
            cx.player_mut(recipient).update_relation(sender, 15.0);
        }
        let sid = cx.st.player(sender).player_id().to_string();
        let e = if cx.st.player(sender).relation(recipient) >= Relation::Neutral {
            rand_elem(random, EMOJI_LOVE)
        } else {
            rand_elem(random, EMOJI_CONFUSED)
        };
        cx.st.add_execution(Exec::Emoji(EmojiExec::new(recipient, sid, f64::from(e))));
    }
}

/// `respondToMIRV(game, random, mirvTarget)` (`:379-394`).
pub fn respond_to_mirv(cx: &mut Ctx, random: &mut Prng, mirv_target: SmallId) {
    if !random.chance(8.0) {
        return;
    }
    if !cx.st.player(mirv_target).can_send_emoji(EmojiRecipient::AllPlayers) {
        return;
    }

    let e = rand_elem(random, EMOJI_OVERWHELMED);
    cx.st.add_execution(Exec::Emoji(EmojiExec::new(mirv_target, ALL_PLAYERS.to_string(), f64::from(e))));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_emoji_strings_are_the_ts_code_points() {
        // Code points taken from NationEmojiBehavior.ts:341,352,363 (python dump of the source).
        assert_eq!(EMOJI_STR_MIDDLE_FINGER, "🖕");
        assert_eq!(EMOJI_STR_CLOWN, "🤡");
        assert_eq!(EMOJI_STR_FRIENDLY, ["🕊️", "🏳️", "❤️", "🥰", "👏"]);
        // the bare dove / flag / heart without VS16 must NOT match (JS `includes` is ===)
        assert!(!EMOJI_STR_FRIENDLY.contains(&"\u{1f54a}"));
        assert!(!EMOJI_STR_FRIENDLY.contains(&"\u{2764}"));
    }

    #[test]
    fn emoji_id_tables() {
        // Verified against flattenedEmojiTable (engine-api/src/Schemas.ts:364-380) by script:
        // all 24 EMOJI_* lists match `table.indexOf(e)`.
        assert_eq!(EMOJI_OVERWHELMED.len(), 8);
        assert_eq!(EMOJI_GOT_INSULTED, &[14, 9, 11, 5, 7]);
        assert_eq!(EMOJI_LOVE, &[48, 1, 2]);
        assert_eq!(EMOJI_CONFUSED, &[57, 11]);
    }
}
