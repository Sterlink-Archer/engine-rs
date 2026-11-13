//! `EmojiExecution` (`engine/src/execution/EmojiExecution.ts`). Owner: **P9**.
//!
//! Fields: the `Emoji v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::api::intents::FLATTENED_EMOJI_TABLE;
use crate::api::types::ALL_PLAYERS;
use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;
use crate::exec::nation::emoji::respond_to_emoji;

/// `EmojiExecution` data.
#[derive(StateClone, Debug)]
pub struct EmojiExec {
    pub active: bool,
    pub initialized: bool,
    pub recipient: Option<EmojiRecipient>,
    /// `PseudoRandom(mg.ticks())` at init (`:35`)
    pub rng: Option<Prng>,
    pub requestor: SmallId,
    /// `PlayerID | AllPlayers`
    pub recipient_id: String,
    pub emoji: f64,
}

impl EmojiExec {
    /// `new EmojiExecution(requestor, recipientID, emoji)` (`:27-31`).
    pub fn new(requestor: SmallId, recipient_id: String, emoji: f64) -> Self {
        EmojiExec { active: true, initialized: false, recipient: None, rng: None, requestor, recipient_id, emoji }
    }
}

/// `flattenedEmojiTable[emoji]` with JS property-lookup semantics: only an integral index
/// in range hits (`-0` reads index 0, like `String(-0) === "0"`); anything else is
/// `undefined`.
fn emoji_string(emoji: f64) -> Option<&'static str> {
    if emoji.is_finite() && emoji.trunc() == emoji && emoji >= 0.0 && emoji < FLATTENED_EMOJI_TABLE.len() as f64 {
        Some(FLATTENED_EMOJI_TABLE[emoji as usize]) // in-range: checked above
    } else {
        None
    }
}

impl ExecBehavior for EmojiExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`EmojiExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:33-47`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        self.initialized = true; // TS: :34 `this.mg = mg`
        self.rng = Some(Prng::new(f64::from(cx.st.ticks()))); // TS: :35
        let all = self.recipient_id == ALL_PLAYERS;
        if !all && !cx.st.has_player(&self.recipient_id) {
            self.active = false; // TS: :37-41
            return;
        }
        // TS: :43-46
        self.recipient = Some(if all {
            EmojiRecipient::AllPlayers
        } else {
            EmojiRecipient::Player(cx.st.player_by_id(&self.recipient_id))
        });
    }
    /// TS: `:49-70`.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        match emoji_string(self.emoji) {
            None => {} // TS: :51-54 warn only
            Some(emoji_string) => {
                let recipient = self.recipient.ts_expect("EmojiExecution.ts:55 recipient");
                if cx.st.player(self.requestor).can_send_emoji(recipient) {
                    cx.player_mut(self.requestor).send_emoji(recipient, emoji_string); // TS: :56
                    // TS: :57-63 with this execution's own PRNG
                    let rng = self.rng.as_mut().ts_expect("EmojiExecution.ts:59 random");
                    respond_to_emoji(cx, rng, self.requestor, recipient, emoji_string);
                }
            }
        }
        self.active = false;
    }
}

/// `EmojiExecutionSnapshot`, `Emoji v1 (execution)` field order; `recipient` is
/// `"AllPlayers" | smallID | null`.
impl Canon for EmojiExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let recipient = match self.recipient {
            None => Cs::Null,
            Some(EmojiRecipient::AllPlayers) => Cs::Str(ALL_PLAYERS.to_string()),
            Some(EmojiRecipient::Player(p)) => p.canon(cx),
        };
        Cs::Obj(vec![
            ("active".into(), self.active.canon(cx)),
            ("initialized".into(), self.initialized.canon(cx)),
            ("recipient".into(), recipient),
            ("random".into(), self.rng.as_ref().map_or(Cs::Null, p9cs::prng)),
            ("requestor".into(), self.requestor.canon(cx)),
            ("recipientID".into(), self.recipient_id.canon(cx)),
            ("emoji".into(), self.emoji.canon(cx)),
        ])
    }
}

impl Uncanon for EmojiExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let f = |k| p9cs::field(v, k);
        let r = f("recipient")?;
        let recipient = if r.is_null() {
            None
        } else if r.as_str() == Some(ALL_PLAYERS) {
            Some(EmojiRecipient::AllPlayers)
        } else {
            Some(EmojiRecipient::Player(p9cs::small_id(r)?))
        };
        let rnd = f("random")?;
        Ok(EmojiExec {
            active: p9cs::bool_(f("active")?)?,
            initialized: p9cs::bool_(f("initialized")?)?,
            recipient,
            rng: if rnd.is_null() { None } else { Some(p9cs::prng_from(rnd)?) },
            requestor: p9cs::small_id(f("requestor")?)?,
            recipient_id: p9cs::str_(f("recipientID")?)?,
            emoji: p9cs::num(f("emoji")?)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JS `arr[x]` semantics for the emoji index (`:50`).
    #[test]
    fn emoji_lookup_js_semantics() {
        assert_eq!(emoji_string(0.0), Some(FLATTENED_EMOJI_TABLE[0]));
        assert_eq!(emoji_string(-0.0), Some(FLATTENED_EMOJI_TABLE[0]));
        assert_eq!(emoji_string(59.0), Some(FLATTENED_EMOJI_TABLE[59]));
        for bad in [60.0, -1.0, 1.5, f64::NAN, f64::INFINITY] {
            assert_eq!(emoji_string(bad), None, "{bad}");
        }
    }
}
