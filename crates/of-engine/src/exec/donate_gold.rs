//! `DonateGoldExecution` (`engine/src/execution/DonateGoldExecution.ts`). Owner: **P9**.
//!
//! Fields: the `DonateGold v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use of_det::ts_throw;

use crate::api::types::Difficulty;
use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx, big};
use crate::exec::ai_attack::p9cs;
use crate::exec::emoji::EmojiExec;
use crate::exec::nation::emoji::{EMOJI_DONATION_OK, EMOJI_DONATION_TOO_SMALL, EMOJI_LOVE};
use of_det::bigint::{div_strict, from_number_strict};
use of_det::js;
use of_det::util::to_int_strict;

/// `DonateGoldExecution` data.
#[derive(StateClone, Debug)]
pub struct DonateGoldExec {
    pub active: bool,
    pub initialized: bool,
    pub recipient: Option<SmallId>,
    pub gold: Option<Gold>,
    /// `PseudoRandom(mg.ticks())` at init (`:47`)
    pub rng: Option<Prng>,
    pub sender: SmallId,
    pub recipient_id: String,
}

impl DonateGoldExec {
    /// `new DonateGoldExecution(sender, recipientID, goldNum)` (`:37-43`; strict `BigInt` conversion).
    pub fn new(sender: SmallId, recipient_id: String, gold_num: Option<f64>) -> Self {
        // TS: :42 `goldNum !== null ? toInt(goldNum) : null`
        let gold = gold_num.map(to_int_strict);
        DonateGoldExec { active: true, initialized: false, recipient: None, gold, rng: None, sender, recipient_id }
    }
}

/// `getGoldChunkSize()` (`:102-116`).
fn gold_chunk_size(difficulty: Difficulty) -> f64 {
    match difficulty {
        Difficulty::Easy => 2_500.0,
        Difficulty::Medium => 5_000.0,
        Difficulty::Hard => 12_500.0,
        Difficulty::Impossible => 25_000.0,
    }
}

/// `calculateRelationUpdate(goldSent, ticks)` (`:118-133`) with the two config reads
/// (`getGoldChunkSize()`, `numSpawnPhaseTurns()`) passed in.
fn relation_update(gold_sent: Gold, ticks: i32, chunk_size: f64, num_spawn_phase_turns: f64) -> f64 {
    // TS: :121-122 `ticks / (3000 + numSpawnPhaseTurns())`
    let chunk_size_multiplier = f64::from(ticks) / (3000.0 + num_spawn_phase_turns);
    // TS: :123-125 `BigInt(Math.round(chunkSize + chunkSize * chunkSizeMultiplier))` (strict)
    let adjusted_chunk_size = from_number_strict(js::round(chunk_size + chunk_size * chunk_size_multiplier));
    // TS: :127 `Number(goldSent / adjustedChunkSize)` (bigint division, truncating)
    let chunks = div_strict(gold_sent, adjusted_chunk_size) as f64;
    // TS: :129-132
    let relation_update = chunks * 5.0;
    if relation_update > 100.0 {
        return 100.0;
    }
    relation_update
}

impl ExecBehavior for DonateGoldExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`DonateGoldExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:45-59`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        self.initialized = true; // TS: :46 `this.mg = mg`
        self.rng = Some(Prng::new(f64::from(cx.st.ticks()))); // TS: :47
        if !cx.st.has_player(&self.recipient_id) {
            self.active = false;
            return;
        }
        self.recipient = Some(cx.st.player_by_id(&self.recipient_id));
        // TS: :58 `this.gold ??= this.sender.gold() / 3n` (non-zero literal divisor)
        if self.gold.is_none() {
            self.gold = Some(div_strict(cx.st.player(self.sender).gold(), 3));
        }
    }
    /// TS: `:61-100`.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let Some(gold) = self.gold else {
            ts_throw!("not initialized");
        };
        let recipient = self.recipient.ts_expect("DonateGoldExecution.ts:64 recipient");
        // TS: :63-66 (short-circuit `&&`)
        if cx.st.player(self.sender).can_donate_gold(recipient)
            && cx.player_mut(self.sender).donate_gold(recipient, gold)
        {
            // TS: :68 give relation points based on how much gold was donated
            let chunk_size = gold_chunk_size(cx.st.config().game_config().difficulty);
            let relation_update = relation_update(gold, ticks, chunk_size, cx.st.config().num_spawn_phase_turns());
            if relation_update > 0.0 {
                cx.player_mut(recipient).update_relation(self.sender, relation_update);
            }
            // TS: :74-93 only AI nations auto-respond with emojis
            if cx.st.player(recipient).player_type() == PlayerType::Nation
                && cx.st.player(recipient).can_send_emoji(EmojiRecipient::Player(self.sender))
            {
                let emoji: &[u32] = if relation_update >= 50.0 {
                    EMOJI_LOVE
                } else if relation_update > 0.0 {
                    EMOJI_DONATION_OK
                } else {
                    EMOJI_DONATION_TOO_SMALL
                };
                let sender_id = cx.st.player(self.sender).player_id().to_string();
                let rng = self.rng.as_mut().ts_expect("DonateGoldExecution.ts:90 random");
                let e = match rng.rand_element(emoji) {
                    Ok(&e) => e,
                    Err(_) => ts_throw!("array must not be empty"),
                };
                cx.st.add_execution(Exec::Emoji(EmojiExec::new(recipient, sender_id, f64::from(e))));
            }
        } // else: TS :94-98 warn only
        self.active = false;
    }
}

/// `DonateGoldExecutionSnapshot`, `DonateGold v1 (execution)` field order.
impl Canon for DonateGoldExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("active".into(), self.active.canon(cx)),
            ("initialized".into(), self.initialized.canon(cx)),
            ("recipient".into(), self.recipient.canon(cx)),
            ("gold".into(), self.gold.map_or(Cs::Null, big)),
            ("random".into(), self.rng.as_ref().map_or(Cs::Null, p9cs::prng)),
            ("sender".into(), self.sender.canon(cx)),
            ("recipientID".into(), self.recipient_id.canon(cx)),
        ])
    }
}

impl Uncanon for DonateGoldExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let f = |k| p9cs::field(v, k);
        let g = f("gold")?;
        let gold = match g {
            Cs::Null => None,
            Cs::Big(b) => Some(i64::try_from(*b).map_err(|_| CanonError::new("gold outside i64"))?),
            _ => return Err(CanonError::new(format!("expected bigint gold, got {g:?}"))),
        };
        let rnd = f("random")?;
        Ok(DonateGoldExec {
            active: p9cs::bool_(f("active")?)?,
            initialized: p9cs::bool_(f("initialized")?)?,
            recipient: p9cs::opt_small_id(f("recipient")?)?,
            gold,
            rng: if rnd.is_null() { None } else { Some(p9cs::prng_from(rnd)?) },
            sender: p9cs::small_id(f("sender")?)?,
            recipient_id: p9cs::str_(f("recipientID")?)?,
        })
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests may catch panics to assert them
mod tests {
    use super::*;

    /// `calculateRelationUpdate` (`:118-133`): JS `Math.round` half-up on the chunk size,
    /// truncating bigint division, the 100 cap, and negative gold.
    #[test]
    fn relation_update_formula() {
        // Medium, tick 0: chunk 5000 → 5 points per 5000 gold.
        assert_eq!(relation_update(0, 0, 5_000.0, 300.0), 0.0);
        assert_eq!(relation_update(4_999, 0, 5_000.0, 300.0), 0.0);
        assert_eq!(relation_update(5_000, 0, 5_000.0, 300.0), 5.0);
        assert_eq!(relation_update(99_999, 0, 5_000.0, 300.0), 95.0);
        assert_eq!(relation_update(1_000_000, 0, 5_000.0, 300.0), 100.0);
        // ticks = 1, 3000 + 300 spawn turns: 2500 + 2500 / 3300 = 2500.757.. rounds to 2501.
        assert_eq!(relation_update(2_501, 1, 2_500.0, 300.0), 5.0);
        assert_eq!(relation_update(2_500, 1, 2_500.0, 300.0), 0.0);
        // An exact half rounds up: 1 + 1 * (1650 / 3300) = 1.5 → 2.
        assert_eq!(relation_update(2, 1650, 1.0, 300.0), 5.0);
        assert_eq!(relation_update(1, 1650, 1.0, 300.0), 0.0);
        // Negative gold truncates toward zero (bigint division).
        assert_eq!(relation_update(-5_001, 0, 5_000.0, 300.0), -5.0);
        assert_eq!(relation_update(-4_999, 0, 5_000.0, 300.0), 0.0);
    }

    /// `BigInt(Math.round(..))` is strict: an infinite chunk size throws a `TsThrow`.
    #[test]
    fn relation_update_infinite_chunk_throws() {
        let r = std::panic::catch_unwind(|| relation_update(1, 1, 5_000.0, -3000.0));
        match r {
            Ok(v) => panic!("BigInt(Infinity) must throw, got {v}"),
            Err(p) => assert!(of_det::throw::is_ts_throw(&*p)),
        }
    }

    #[test]
    fn constructor_to_int() {
        assert_eq!(DonateGoldExec::new(SmallId(1), "x".into(), Some(12.9)).gold, Some(12));
        assert_eq!(DonateGoldExec::new(SmallId(1), "x".into(), Some(-0.5)).gold, Some(-1));
        assert_eq!(DonateGoldExec::new(SmallId(1), "x".into(), Some(f64::INFINITY)).gold, Some(9_007_199_254_740_991));
        assert_eq!(DonateGoldExec::new(SmallId(1), "x".into(), None).gold, None);
        assert_eq!(gold_chunk_size(Difficulty::Impossible), 25_000.0);
    }
}
