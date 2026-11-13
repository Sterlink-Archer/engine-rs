//! `DonateTroopExecution` (`engine/src/execution/DonateTroopExecution.ts`). Owner: **P9**.
//!
//! Fields: the `DonateTroops v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use of_det::ts_throw;

use crate::api::types::Difficulty;
use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;
use crate::exec::emoji::EmojiExec;
use crate::exec::nation::emoji::{EMOJI_DONATION_TOO_SMALL, EMOJI_LOVE};
use of_det::js;

/// `DonateTroopsExecution` data.
#[derive(StateClone, Debug)]
pub struct DonateTroopsExec {
    pub active: bool,
    pub initialized: bool,
    pub recipient: Option<SmallId>,
    /// `PseudoRandom(mg.ticks())` at init (`:43`)
    pub rng: Option<Prng>,
    pub sender: SmallId,
    pub recipient_id: String,
    pub troops: Option<f64>,
}

impl DonateTroopsExec {
    /// `new DonateTroopsExecution(sender, recipientID, troops)` (`:35-39`).
    pub fn new(sender: SmallId, recipient_id: String, troops: Option<f64>) -> Self {
        DonateTroopsExec { active: true, initialized: false, recipient: None, rng: None, sender, recipient_id, troops }
    }
}

/// `getMinTroopsForRelationUpdate()` (`:101-133`): one `nextInt` draw.
fn min_troops_for_relation_update(rng: &mut Prng, difficulty: Difficulty, recipient_max_troops: f64) -> f64 {
    match difficulty {
        // ~7.7k - ~9.1k troops (for 100k troops)
        Difficulty::Easy => rng.next_int(recipient_max_troops / 13.0, recipient_max_troops / 11.0),
        // ~9.1k - ~11.1k troops
        Difficulty::Medium => rng.next_int(recipient_max_troops / 11.0, recipient_max_troops / 9.0),
        // ~11.1k - ~14.3k troops
        Difficulty::Hard => rng.next_int(recipient_max_troops / 9.0, recipient_max_troops / 7.0),
        // ~14.3k - ~20k troops
        Difficulty::Impossible => rng.next_int(recipient_max_troops / 7.0, recipient_max_troops / 5.0),
    }
}

impl ExecBehavior for DonateTroopsExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`DonateTroopExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:41-62`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        self.initialized = true; // TS: :42 `this.mg = mg`
        self.rng = Some(Prng::new(f64::from(cx.st.ticks()))); // TS: :43
        if !cx.st.has_player(&self.recipient_id) {
            self.active = false;
            return;
        }
        let recipient = cx.st.player_by_id(&self.recipient_id);
        self.recipient = Some(recipient);
        // TS: :54 `this.troops ??= mg.config().defaultDonationAmount(this.sender)`
        let troops = match self.troops {
            Some(t) => t,
            None => cx.st.config().default_donation_amount(&cx.st.player(self.sender)),
        };
        // TS: :55-57
        let max_donation = cx.st.config().max_troops(&cx.st.player(recipient)) - cx.st.player(recipient).troops();
        let troops = js::min(troops, max_donation);
        self.troops = Some(troops);
        // TS: :59-61 (NaN <= 0 is false)
        if troops <= 0.0 {
            self.active = false;
        }
    }
    /// TS: `:64-99`.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        let Some(troops) = self.troops else {
            ts_throw!("not initialized");
        };
        let recipient = self.recipient.ts_expect("DonateTroopExecution.ts:67 recipient");
        // TS: :67 drawn before the donation check
        let difficulty = cx.st.config().game_config().difficulty;
        let recipient_max_troops = cx.st.config().max_troops(&cx.st.player(recipient));
        let rng = self.rng.as_mut().ts_expect("DonateTroopExecution.ts:108 random");
        let min_troops = min_troops_for_relation_update(rng, difficulty, recipient_max_troops);
        // TS: :69-72 (short-circuit `&&`)
        if cx.st.player(self.sender).can_donate_troops(recipient)
            && cx.player_mut(self.sender).donate_troops(recipient, troops)
        {
            if troops >= min_troops {
                cx.player_mut(recipient).update_relation(self.sender, 50.0); // TS: :74-76
            }
            // TS: :79-92 only AI nations auto-respond with emojis
            if cx.st.player(recipient).player_type() == PlayerType::Nation
                && cx.st.player(recipient).can_send_emoji(EmojiRecipient::Player(self.sender))
            {
                let sender_id = cx.st.player(self.sender).player_id().to_string();
                let list: &[u32] = if troops >= min_troops { EMOJI_LOVE } else { EMOJI_DONATION_TOO_SMALL };
                let rng = self.rng.as_mut().ts_expect("DonateTroopExecution.ts:87 random");
                let e = match rng.rand_element(list) {
                    Ok(&e) => e,
                    Err(_) => ts_throw!("array must not be empty"),
                };
                cx.st.add_execution(Exec::Emoji(EmojiExec::new(recipient, sender_id, f64::from(e))));
            }
        } // else: TS :93-97 warn only
        self.active = false;
    }
}

/// `DonateTroopsExecutionSnapshot`, `DonateTroops v1 (execution)` field order.
impl Canon for DonateTroopsExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("active".into(), self.active.canon(cx)),
            ("initialized".into(), self.initialized.canon(cx)),
            ("recipient".into(), self.recipient.canon(cx)),
            ("random".into(), self.rng.as_ref().map_or(Cs::Null, p9cs::prng)),
            ("sender".into(), self.sender.canon(cx)),
            ("recipientID".into(), self.recipient_id.canon(cx)),
            ("troops".into(), self.troops.canon(cx)),
        ])
    }
}

impl Uncanon for DonateTroopsExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let f = |k| p9cs::field(v, k);
        let rnd = f("random")?;
        let t = f("troops")?;
        Ok(DonateTroopsExec {
            active: p9cs::bool_(f("active")?)?,
            initialized: p9cs::bool_(f("initialized")?)?,
            recipient: p9cs::opt_small_id(f("recipient")?)?,
            rng: if rnd.is_null() { None } else { Some(p9cs::prng_from(rnd)?) },
            sender: p9cs::small_id(f("sender")?)?,
            recipient_id: p9cs::str_(f("recipientID")?)?,
            troops: if t.is_null() { None } else { Some(p9cs::num(t)?) },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `nextInt(max / a, max / b)` draw per difficulty, floors applied by `nextInt`.
    #[test]
    fn min_troops_draw() {
        for (d, a, b) in [
            (Difficulty::Easy, 13.0, 11.0),
            (Difficulty::Medium, 11.0, 9.0),
            (Difficulty::Hard, 9.0, 7.0),
            (Difficulty::Impossible, 7.0, 5.0),
        ] {
            let mut r1 = Prng::new(1234.0);
            let mut r2 = Prng::new(1234.0);
            let got = min_troops_for_relation_update(&mut r1, d, 100_000.0);
            assert_eq!(got, r2.next_int(100_000.0 / a, 100_000.0 / b));
            assert_eq!(r1.get_state(), r2.get_state());
            assert!(got >= (100_000.0f64 / a).floor() && got < (100_000.0f64 / b).floor());
        }
    }
}
