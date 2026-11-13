//! The typed `Intent` (`engine-api/src/Schemas.ts` `IntentSchema`), `StampedIntent` and
//! `Turn`. Owner: P1. serde is only the edge: embedders build `Intent` values directly
//! (ARCHITECTURE §10).
//!
//! Field types follow the zod schemas: `MappedID` → `String`; `zb.uint()` → `u64` and
//! `zb.int()` → `i64` (zod `.int()` = safe integers, so both hold every accepted value);
//! `zb.float()` → `f64`; `.nullable()` → `Option` written as `null`; `.optional()` →
//! `Option` omitted when absent. Range checks are the executions' business, exactly as
//! in TS (e.g. a `tile` beyond the map is rejected by the execution, not by the schema).
//!
//! The three lobby intents (`kick_player`, `update_game_config`,
//! `toggle_game_start_timer`) parse, but have no execution: `createExec` throws for them
//! outside the tick try/catch (`ExecutionManager.ts:147-148`, Q-INTENT-THROW), which the
//! runner maps to `Fatal::Intent` before any state change.

use super::types::{ALL_PLAYERS, ClientID, UnitType};
use serde::{Deserialize, Serialize};

/// `"start" | "stop"` of the embargo intents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbargoAction {
    Start,
    Stop,
}
crate::plain_copy!(EmbargoAction);

/// `IntentSchema` (discriminated by `type`), variants in schema order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Intent {
    #[serde(rename = "attack")]
    Attack {
        #[serde(rename = "targetID")]
        target_id: Option<String>,
        troops: Option<f64>,
    },
    #[serde(rename = "cancel_attack")]
    CancelAttack {
        #[serde(rename = "attackID")]
        attack_id: String,
    },
    #[serde(rename = "spawn")]
    Spawn { tile: u64 },
    #[serde(rename = "mark_disconnected")]
    MarkDisconnected {
        #[serde(rename = "isDisconnected")]
        is_disconnected: bool,
    },
    #[serde(rename = "boat")]
    Boat { troops: f64, dst: u64 },
    #[serde(rename = "cancel_boat")]
    CancelBoat {
        #[serde(rename = "unitID")]
        unit_id: u64,
    },
    #[serde(rename = "allianceRequest")]
    AllianceRequest { recipient: String },
    #[serde(rename = "allianceReject")]
    AllianceReject { requestor: String },
    #[serde(rename = "breakAlliance")]
    BreakAlliance { recipient: String },
    #[serde(rename = "targetPlayer")]
    TargetPlayer { target: String },
    /// `recipient` is a player id or [`ALL_PLAYERS`] (which itself matches the id regex,
    /// so zod's union keeps it as the plain string).
    #[serde(rename = "emoji")]
    Emoji { recipient: String, emoji: u64 },
    #[serde(rename = "donate_gold")]
    DonateGold { recipient: String, gold: Option<f64> },
    #[serde(rename = "donate_troops")]
    DonateTroops { recipient: String, troops: Option<f64> },
    #[serde(rename = "build_unit")]
    BuildUnit {
        unit: UnitType,
        tile: u64,
        #[serde(rename = "rocketDirectionUp", default, skip_serializing_if = "Option::is_none")]
        rocket_direction_up: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        amount: Option<u64>,
    },
    #[serde(rename = "upgrade_structure")]
    UpgradeStructure {
        unit: UnitType,
        #[serde(rename = "unitId")]
        unit_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        amount: Option<u64>,
    },
    #[serde(rename = "embargo")]
    Embargo {
        #[serde(rename = "targetID")]
        target_id: String,
        action: EmbargoAction,
    },
    #[serde(rename = "embargo_all")]
    EmbargoAll { action: EmbargoAction },
    #[serde(rename = "move_warship")]
    /// `unitIds` is required by the pinned schema, but record replays carry turns exactly as
    /// fed to `runner.addTurn` (SPEC §2.3), and an older client sent `unitId` instead
    /// (`R-DQWFd5um` turn 2291). TS does not re-validate: `new MoveWarshipExecution(player,
    /// undefined, tile)` iterates `new Set(undefined)` = nothing (`MoveWarshipExecution.ts:32`).
    /// So `None` = absent; unknown keys are kept in `extra` for a lossless round trip.
    MoveWarship {
        #[serde(rename = "unitIds", default, skip_serializing_if = "Option::is_none")]
        unit_ids: Option<Vec<i64>>,
        tile: u64,
        #[serde(flatten)]
        extra: serde_json::Map<String, serde_json::Value>,
    },
    #[serde(rename = "quick_chat")]
    QuickChat {
        recipient: String,
        #[serde(rename = "quickChatKey")]
        quick_chat_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    #[serde(rename = "allianceExtension")]
    AllianceExtension { recipient: String },
    #[serde(rename = "delete_unit")]
    DeleteUnit {
        #[serde(rename = "unitId")]
        unit_id: u64,
    },
    #[serde(rename = "kick_player")]
    KickPlayer {
        #[serde(rename = "targetClientID", default, skip_serializing_if = "Option::is_none")]
        target_client_id: Option<String>,
        #[serde(rename = "targetPublicID", default, skip_serializing_if = "Option::is_none")]
        target_public_id: Option<String>,
    },
    /// `paused: z.boolean().default(false)`.
    #[serde(rename = "toggle_pause")]
    TogglePause {
        #[serde(default)]
        paused: bool,
    },
    /// `config: GameConfigPatchSchema` (lobby-only; kept as raw JSON).
    #[serde(rename = "update_game_config")]
    UpdateGameConfig { config: serde_json::Value },
    #[serde(rename = "toggle_game_start_timer")]
    ToggleGameStartTimer {},
}

impl Intent {
    /// The TS `type` discriminator.
    pub fn type_name(&self) -> &'static str {
        match self {
            Intent::Attack { .. } => "attack",
            Intent::CancelAttack { .. } => "cancel_attack",
            Intent::Spawn { .. } => "spawn",
            Intent::MarkDisconnected { .. } => "mark_disconnected",
            Intent::Boat { .. } => "boat",
            Intent::CancelBoat { .. } => "cancel_boat",
            Intent::AllianceRequest { .. } => "allianceRequest",
            Intent::AllianceReject { .. } => "allianceReject",
            Intent::BreakAlliance { .. } => "breakAlliance",
            Intent::TargetPlayer { .. } => "targetPlayer",
            Intent::Emoji { .. } => "emoji",
            Intent::DonateGold { .. } => "donate_gold",
            Intent::DonateTroops { .. } => "donate_troops",
            Intent::BuildUnit { .. } => "build_unit",
            Intent::UpgradeStructure { .. } => "upgrade_structure",
            Intent::Embargo { .. } => "embargo",
            Intent::EmbargoAll { .. } => "embargo_all",
            Intent::MoveWarship { .. } => "move_warship",
            Intent::QuickChat { .. } => "quick_chat",
            Intent::AllianceExtension { .. } => "allianceExtension",
            Intent::DeleteUnit { .. } => "delete_unit",
            Intent::KickPlayer { .. } => "kick_player",
            Intent::TogglePause { .. } => "toggle_pause",
            Intent::UpdateGameConfig { .. } => "update_game_config",
            Intent::ToggleGameStartTimer {} => "toggle_game_start_timer",
        }
    }

    /// The lobby intents that `createExec` throws for (`ExecutionManager.ts:147-148`).
    pub fn is_lobby_only(&self) -> bool {
        matches!(self, Intent::KickPlayer { .. } | Intent::UpdateGameConfig { .. } | Intent::ToggleGameStartTimer {})
    }
}

/// `recipient === AllPlayers` for an emoji intent.
pub fn is_all_players(recipient: &str) -> bool {
    recipient == ALL_PLAYERS
}

/// `StampedIntent` = `Intent & { clientID }` (the server stamps the sender).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StampedIntent {
    #[serde(rename = "clientID")]
    pub client_id: ClientID,
    #[serde(flatten)]
    pub intent: Intent,
}

/// `TurnSchema`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub turn_number: u64,
    pub intents: Vec<StampedIntent>,
    /// `hash: zb.float().nullable().optional()`.
    #[serde(default, deserialize_with = "super::game_config::double_option", skip_serializing_if = "Option::is_none")]
    pub hash: Option<Option<f64>>,
}

impl Turn {
    /// An empty turn (no intents, no hash).
    pub fn empty(turn_number: u64) -> Self {
        Turn { turn_number, intents: Vec::new(), hash: None }
    }
}

/// `flattenedEmojiTable` = `emojiTable.flat()` (`Schemas.ts:364-377,380`), extracted from the TS
/// source by script (code points escaped; variation selectors and ZWJ sequences kept).
/// `EmojiSchema` is `uint <= 59` (`:384`).
pub const FLATTENED_EMOJI_TABLE: [&str; 60] = [
    "\u{1f600}",
    "\u{1f60a}",
    "\u{1f970}",
    "\u{1f607}",
    "\u{1f60e}",
    "\u{1f61e}",
    "\u{1f97a}",
    "\u{1f62d}",
    "\u{1f631}",
    "\u{1f621}",
    "\u{1f608}",
    "\u{1f921}",
    "\u{1f971}",
    "\u{1fae1}",
    "\u{1f595}",
    "\u{1f44b}",
    "\u{1f44f}",
    "\u{270b}",
    "\u{1f64f}",
    "\u{1f4aa}",
    "\u{1f44d}",
    "\u{1f44e}",
    "\u{1faf4}",
    "\u{1f90c}",
    "\u{1f926}\u{200d}\u{2642}\u{fe0f}",
    "\u{1f91d}",
    "\u{1f198}",
    "\u{1f54a}\u{fe0f}",
    "\u{1f3f3}\u{fe0f}",
    "\u{23f3}",
    "\u{1f525}",
    "\u{1f4a5}",
    "\u{1f480}",
    "\u{2622}\u{fe0f}",
    "\u{26a0}\u{fe0f}",
    "\u{2196}\u{fe0f}",
    "\u{2b06}\u{fe0f}",
    "\u{2197}\u{fe0f}",
    "\u{1f451}",
    "\u{1f947}",
    "\u{2b05}\u{fe0f}",
    "\u{1f3af}",
    "\u{27a1}\u{fe0f}",
    "\u{1f948}",
    "\u{1f949}",
    "\u{2199}\u{fe0f}",
    "\u{2b07}\u{fe0f}",
    "\u{2198}\u{fe0f}",
    "\u{2764}\u{fe0f}",
    "\u{1f494}",
    "\u{1f4b0}",
    "\u{2693}",
    "\u{26f5}",
    "\u{1f3e1}",
    "\u{1f6e1}\u{fe0f}",
    "\u{1f3ed}",
    "\u{1f682}",
    "\u{2753}",
    "\u{1f414}",
    "\u{1f400}",
];

/// `flattenedEmojiTable.indexOf(e)` (the `emojiId` helper, `NationEmojiBehavior.ts:26-27`).
pub fn emoji_id(e: &str) -> Option<u32> {
    FLATTENED_EMOJI_TABLE.iter().position(|x| *x == e).map(|i| i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stamped_intent_json_shapes() {
        let j = r#"{"type":"attack","clientID":"SCRIPTAA","targetID":null,"troops":1234.5}"#;
        let s: StampedIntent = serde_json::from_str(j).expect("attack");
        assert_eq!(s.client_id, "SCRIPTAA");
        assert_eq!(s.intent, Intent::Attack { target_id: None, troops: Some(1234.5) });
        let back = serde_json::to_value(&s).expect("ser");
        assert_eq!(back, serde_json::from_str::<serde_json::Value>(j).expect("json"));
        let b: Intent = serde_json::from_str(r#"{"type":"build_unit","unit":"Atom Bomb","tile":77}"#).expect("build");
        assert_eq!(
            b,
            Intent::BuildUnit { unit: UnitType::AtomBomb, tile: 77, rocket_direction_up: None, amount: None }
        );
        let p: Intent = serde_json::from_str(r#"{"type":"toggle_pause"}"#).expect("pause");
        assert_eq!(p, Intent::TogglePause { paused: false });
        let t: Intent = serde_json::from_str(r#"{"type":"toggle_game_start_timer"}"#).expect("timer");
        assert!(t.is_lobby_only() && t.type_name() == "toggle_game_start_timer");
        assert!(serde_json::from_str::<Intent>(r#"{"type":"nope"}"#).is_err());
        let turn: Turn = serde_json::from_str(r#"{"turnNumber":3,"intents":[],"hash":null}"#).expect("turn");
        assert_eq!(turn.hash, Some(None));
    }
}
