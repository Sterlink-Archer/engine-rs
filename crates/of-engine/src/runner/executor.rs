//! `Executor` (`engine/src/execution/ExecutionManager.ts`): intent → execution. Owner: **P1**.
//! Its PRNG (`simpleHash(gameID)+1`, `:49`) is never drawn and not ported. The constructors
//! called here belong to the executions' owners (FILE_MAP §2.1).
//!
//! Numeric intent fields arrive as zod-checked integers (`u64`/`i64`). TS hands the JS number
//! to the constructor unchanged; where a Rust constructor takes a narrower type, a value
//! outside it is mapped to the type's maximum, which is, like the TS value, a tile or unit id
//! that does not exist (every real tile ref and unit id is < 2^32, `ids.rs`).

use crate::api::intents::{Intent, StampedIntent, Turn};
use crate::exec::Exec;
use crate::exec::alliance::{
    break_::BreakAllianceExec, extension::AllianceExtensionExec, reject::AllianceRejectExec,
    request::AllianceRequestExec,
};
use crate::exec::{
    attack::AttackExec, boat_retreat::BoatRetreatExec, construction::ConstructionExec, delete_unit::DeleteUnitExec,
    donate_gold::DonateGoldExec, donate_troops::DonateTroopsExec, embargo::EmbargoExec, embargo_all::EmbargoAllExec,
    emoji::EmojiExec, mark_disconnected::MarkDisconnectedExec, move_warship::MoveWarshipExec, noop::NoOpExec,
    pause::PauseExec, quick_chat::QuickChatExec, retreat::RetreatExec, spawn::SpawnExec,
    target_player::TargetPlayerExec, transport_ship::TransportShipExec, upgrade_structure::UpgradeStructureExec,
};
use crate::runner::Fatal;
use crate::state::State;

/// A JS number id/tile as `u32` (out of range → `u32::MAX`, which matches nothing; module docs).
#[inline]
fn id32(x: u64) -> u32 {
    u32::try_from(x).unwrap_or(u32::MAX)
}

/// `createExecs(turn)` (`:58-60`): one execution per intent, in array order. All are created
/// before any is added (`addExecution(...createExecs(turn))`, `GameRunner.ts:222-224`).
pub fn create_execs(st: &State, game_id: &str, turn: &Turn) -> Result<Vec<Exec>, Fatal> {
    turn.intents.iter().map(|i| create_exec(st, game_id, i)).collect()
}

/// `createExec(intent)` (`:62-150`). The three lobby intents return `Fatal::Intent` (TS throws
/// `intent type ${intent} not found` outside the tick try/catch, `:147-148`; `${intent}` of an
/// object is `[object Object]`).
pub fn create_exec(st: &State, game_id: &str, intent: &StampedIntent) -> Result<Exec, Fatal> {
    // TS: :63-67 — an unknown sender becomes a NoOp (console.warn dropped).
    let Some(player) = st.player_by_client_id(&intent.client_id) else {
        return Ok(Exec::NoOp(NoOpExec::new()));
    };
    Ok(match &intent.intent {
        Intent::Attack { target_id, troops } => {
            // TS: :71-77 `new AttackExecution(intent.troops, player, intent.targetID, null)`
            // (`removeTroops` defaults to true, AttackExecution.ts:62).
            Exec::Attack(AttackExec::new(*troops, player, target_id.clone(), None, true))
        }
        Intent::CancelAttack { attack_id } => Exec::Retreat(RetreatExec::new(player, attack_id.clone())),
        Intent::CancelBoat { unit_id } => Exec::BoatRetreat(BoatRetreatExec::new(player, id32(*unit_id))),
        Intent::MoveWarship { unit_ids, tile, .. } => Exec::MoveWarship(MoveWarshipExec::new(
            player,
            unit_ids.as_ref().map(|v| v.iter().map(|&x| x as f64).collect()),
            *tile as f64,
        )),
        // TS: :83-90 `new SpawnExecution(gameID, player.info(), intent.tile, true)`
        Intent::Spawn { tile } => {
            let info = st.info(st.player_data(player).info).clone();
            Exec::Spawn(SpawnExec::new(game_id, info, Some(*tile as f64), true))
        }
        Intent::Boat { troops, dst } => Exec::TransportShip(TransportShipExec::new(player, id32(*dst), *troops)),
        Intent::AllianceRequest { recipient } => {
            Exec::AllianceRequest(AllianceRequestExec::new(player, recipient.clone()))
        }
        // TS: :97 `new AllianceRejectExecution(intent.requestor, player)`
        Intent::AllianceReject { requestor } => {
            Exec::AllianceReject(AllianceRejectExec::new(requestor.clone(), player))
        }
        Intent::BreakAlliance { recipient } => Exec::BreakAlliance(BreakAllianceExec::new(player, recipient.clone())),
        Intent::TargetPlayer { target } => Exec::TargetPlayer(TargetPlayerExec::new(player, target.clone())),
        Intent::Emoji { recipient, emoji } => Exec::Emoji(EmojiExec::new(player, recipient.clone(), *emoji as f64)),
        Intent::DonateTroops { recipient, troops } => {
            Exec::DonateTroops(DonateTroopsExec::new(player, recipient.clone(), *troops))
        }
        Intent::DonateGold { recipient, gold } => {
            Exec::DonateGold(DonateGoldExec::new(player, recipient.clone(), *gold))
        }
        Intent::Embargo { target_id, action } => Exec::Embargo(EmbargoExec::new(player, target_id.clone(), *action)),
        Intent::EmbargoAll { action } => Exec::EmbargoAll(EmbargoAllExec::new(player, *action)),
        // TS: :122-129
        Intent::BuildUnit { unit, tile, rocket_direction_up, amount } => Exec::Construction(ConstructionExec::new(
            player,
            *unit,
            id32(*tile),
            *rocket_direction_up,
            amount.map(id32),
        )),
        Intent::AllianceExtension { recipient } => {
            Exec::AllianceExtension(AllianceExtensionExec::new(player, recipient.clone()))
        }
        // TS: :134-139; `amount` undefined → the constructor default 1
        // (UpgradeStructureExecution.ts:22).
        Intent::UpgradeStructure { unit_id, amount, .. } => {
            Exec::UpgradeStructure(UpgradeStructureExec::new(player, *unit_id as f64, amount.map_or(1.0, |a| a as f64)))
        }
        Intent::DeleteUnit { unit_id } => Exec::DeleteUnit(DeleteUnitExec::new(player, id32(*unit_id))),
        Intent::QuickChat { recipient, quick_chat_key, target } => {
            Exec::QuickChat(QuickChatExec::new(player, recipient.clone(), quick_chat_key.clone(), target.clone()))
        }
        Intent::MarkDisconnected { is_disconnected } => {
            Exec::MarkDisconnected(MarkDisconnectedExec::new(player, *is_disconnected))
        }
        Intent::TogglePause { paused } => Exec::Pause(PauseExec::new(player, *paused)),
        // TS: :146-148 `default: throw new Error(`intent type ${intent} not found`)`
        Intent::KickPlayer { .. } | Intent::UpdateGameConfig { .. } | Intent::ToggleGameStartTimer {} => {
            return Err(Fatal::Intent { msg: "intent type [object Object] not found".to_owned() });
        }
    })
}
