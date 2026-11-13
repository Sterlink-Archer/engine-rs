//! Section T rebuild (SPEC §1.3 T, §2.5): dumps omit T; it is derived from
//! P[*].tiles (owner smallID, bits 0-11), M.<map>.fallout (bit 13) and M.<map>.defense
//! (bit 14) — `GameMapImpl.ts:37-41` state bits. The rebuilt T must re-digest to the
//! stored `d_T` (checked by `of-conform smoke` on every dump of the fixture).

use crate::value::{Cs, Typed};

#[derive(Debug, Clone, PartialEq)]
pub enum RebuildError {
    Missing(&'static str),
    TileOutOfRange { which: &'static str, tile: u32 },
    OwnerTooLarge(i64),
}

const PLAYER_ID_MASK: u16 = 0x0fff;
const FALLOUT_BIT: u16 = 1 << 13;
const DEFENSE_BIT: u16 = 1 << 14;

fn tiles_of<'a>(v: Option<&'a Cs>, what: &'static str) -> Result<&'a [u32], RebuildError> {
    match v {
        Some(Cs::Typed(Typed::U32(t))) => Ok(t),
        _ => Err(RebuildError::Missing(what)),
    }
}

/// One map's raw state array. `owners`: (smallID, tiles) per player (only the main map
/// carries ownership; the minimap's state is written only through fallout/defense).
fn build(m: &Cs, owners: &[(i64, &[u32])], which: &'static str) -> Result<Vec<u16>, RebuildError> {
    let w = m.get("width").and_then(Cs::as_i64).ok_or(RebuildError::Missing("width"))?;
    let h = m.get("height").and_then(Cs::as_i64).ok_or(RebuildError::Missing("height"))?;
    let n = (w * h) as usize;
    let mut s = vec![0u16; n];
    for &(id, tiles) in owners {
        if !(0..=PLAYER_ID_MASK as i64).contains(&id) {
            return Err(RebuildError::OwnerTooLarge(id));
        }
        for &t in tiles {
            let cell = s.get_mut(t as usize).ok_or(RebuildError::TileOutOfRange { which, tile: t })?;
            *cell = (*cell & !PLAYER_ID_MASK) | id as u16;
        }
    }
    for &t in tiles_of(m.get("fallout"), "fallout")? {
        *s.get_mut(t as usize).ok_or(RebuildError::TileOutOfRange { which, tile: t })? |= FALLOUT_BIT;
    }
    for &t in tiles_of(m.get("defense"), "defense")? {
        *s.get_mut(t as usize).ok_or(RebuildError::TileOutOfRange { which, tile: t })? |= DEFENSE_BIT;
    }
    Ok(s)
}

/// Rebuild `T = {map, miniMap}` from sections M and P.
pub fn rebuild_t(m: &Cs, p: &Cs) -> Result<Cs, RebuildError> {
    let players = p.as_arr().ok_or(RebuildError::Missing("P"))?;
    let mut owners = Vec::with_capacity(players.len());
    for row in players {
        let id = row.get("smallID").and_then(Cs::as_i64).ok_or(RebuildError::Missing("smallID"))?;
        owners.push((id, tiles_of(row.get("tiles"), "P.tiles")?));
    }
    let map = build(m.get("map").ok_or(RebuildError::Missing("M.map"))?, &owners, "map")?;
    let mini = build(m.get("miniMap").ok_or(RebuildError::Missing("M.miniMap"))?, &[], "miniMap")?;
    Ok(Cs::Obj(vec![("map".into(), Cs::Typed(Typed::U16(map))), ("miniMap".into(), Cs::Typed(Typed::U16(mini)))]))
}
