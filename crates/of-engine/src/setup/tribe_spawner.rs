//! `engine/src/execution/TribeSpawner.ts`. Owner: **P6**. PRNG `simpleHash(gameID) + 2` (`:32`).

use crate::api::types::{Cell, GameMapSize, PlayerInfo, PlayerType};
use crate::exec::spawn::SpawnExec;
use crate::ids::TileRef;
use crate::jsorder::{JsSet, LookupSet};
use crate::map::manifest::CustomTribe;
use crate::setup::tribe_names::{TribeNameData, resolve_tribe_name_data};
use crate::state::State;
use of_det::Prng;
use of_det::throw::TsExpect;

/// `TribeSpawner` (setup-time object, not state).
#[derive(Debug)]
pub struct TribeSpawner {
    pub game_id: String,
    pub random: Prng,
    pub tribe_name_data: TribeNameData,
    /// `usedCustomTribes: Set<string>` (`:22`).
    pub used_custom_tribes: JsSet<String>,
    /// `nationTiles: Set<TileRef>` (`:23`); lookup-only: only `.has` (grep `nationTiles`).
    pub nation_tiles: LookupSet<TileRef>,
}

/// `ct.name` of a manifest custom tribe (a bare string is `{ name }`).
fn ct_name(ct: &CustomTribe) -> &str {
    match ct {
        CustomTribe::Name(n) => n,
        CustomTribe::Placed { name, .. } => name,
    }
}

/// `ct.coordinates` (`undefined` for a bare name).
fn ct_coordinates(ct: &CustomTribe) -> Option<[u32; 2]> {
    match ct {
        CustomTribe::Name(_) => None,
        CustomTribe::Placed { coordinates, .. } => *coordinates,
    }
}

impl TribeSpawner {
    /// `new TribeSpawner(gs, gameID, nationCells = [])` (`:25-39`).
    pub fn new(st: &State, game_id: &str, nation_cells: &[Cell]) -> Self {
        // TS: :32 a different seed than createGameRunner (simpleHash(gameID)).
        let random = Prng::new(of_det::util::simple_hash(game_id) + 2.0);
        let tribe_name_data = resolve_tribe_name_data(Some(&st.config().game_config().game_map));
        // TS: :34-38 `new Set(nationCells.filter(isValidCoord).map(ref))`
        let mut nation_tiles = LookupSet::new();
        for c in nation_cells {
            if st.map.is_valid_coord(c.x, c.y) {
                nation_tiles.insert(st.map.ref_(c.x, c.y));
            }
        }
        TribeSpawner {
            game_id: game_id.to_owned(),
            random,
            tribe_name_data,
            used_custom_tribes: JsSet::new(),
            nation_tiles,
        }
    }

    /// `spawnTribes(numTribes, purchasedNames = [])` (`:41-89`).
    pub fn spawn_tribes(&mut self, st: &State, num_tribes: u32, purchased_names: &[String]) -> Vec<SpawnExec> {
        let num_tribes = num_tribes as usize;
        let mut tribes: Vec<SpawnExec> = Vec::new();

        // TS: :48-59 spawn positioned custom tribes first (those with coordinates)
        if let Some(custom) = self.tribe_name_data.custom_tribes.as_ref() {
            let positioned: Vec<CustomTribe> =
                custom.iter().filter(|ct| ct_coordinates(ct).is_some()).cloned().collect();
            for ct in &positioned {
                if tribes.len() >= num_tribes {
                    break;
                }
                if let Some(exec) = self.spawn_positioned_tribe(st, ct) {
                    tribes.push(exec);
                    self.used_custom_tribes.insert(ct_name(ct).to_owned());
                }
            }
        }

        // TS: :71-79 purchased names each go to one random remaining slot; guarded so games
        // without purchased names consume the PRNG exactly as before.
        let remaining = num_tribes - tribes.len(); // int: tribes.len() <= num_tribes (break above)
        // `purchasedBySlot: Map<number, string>`; lookup-only (`.get(slot)`), slots < remaining.
        let mut purchased_by_slot: Vec<Option<&String>> = vec![None; remaining];
        if !purchased_names.is_empty() && remaining > 0 {
            let used = &purchased_names[..purchased_names.len().min(remaining)];
            let all: Vec<usize> = (0..remaining).collect();
            let slots = self.random.shuffle_array(&all);
            for (i, &slot) in slots.iter().take(used.len()).enumerate() {
                purchased_by_slot[slot] = Some(&used[i]); // in-range: slot < remaining
            }
        }

        // TS: :82-87 fill remaining slots with random-spawn tribes
        let mut slot = 0usize;
        while tribes.len() < num_tribes {
            let purchased = purchased_by_slot.get(slot).copied().flatten();
            // `purchased ?? this.randomTribeName()`: the name is drawn before `spawnTribe`'s nextID.
            let name = match purchased {
                Some(n) => n.clone(),
                None => self.random_tribe_name(),
            };
            tribes.push(self.spawn_tribe(st, &name));
            slot += 1;
        }
        tribes
    }

    /// `spawnTribe(tribeName)` (`:91-96`).
    pub fn spawn_tribe(&mut self, _st: &State, tribe_name: &str) -> SpawnExec {
        let id = self.random.next_id();
        SpawnExec::new(&self.game_id, PlayerInfo::new(tribe_name, PlayerType::Bot, None, id), None, false)
    }

    /// `spawnPositionedTribe(ct)` (`:102-132`): `None` when the tile is invalid, water,
    /// impassable, owned, or a nation's spawn tile.
    fn spawn_positioned_tribe(&mut self, st: &State, ct: &CustomTribe) -> Option<SpawnExec> {
        let coords = ct_coordinates(ct).ts_expect("TribeSpawner.ts:103 ct.coordinates!");
        let is_compact = st.config().game_config().game_map_size == GameMapSize::Compact;
        let (c0, c1) = (f64::from(coords[0]), f64::from(coords[1]));
        let x = if is_compact { of_det::js::floor(c0 / 2.0) } else { c0 };
        let y = if is_compact { of_det::js::floor(c1 / 2.0) } else { c1 };
        if !st.map.is_valid_coord(x, y) {
            return None;
        }
        let tile = st.map.ref_(x, y);
        if !st.map.is_land(tile)
            || st.map.has_owner(tile)
            || st.map.is_impassable(tile)
            || self.nation_tiles.contains(&tile)
        {
            return None;
        }
        let id = self.random.next_id();
        Some(SpawnExec::new(
            &self.game_id,
            PlayerInfo::new(ct_name(ct), PlayerType::Bot, None, id),
            Some(f64::from(tile)),
            false,
        ))
    }

    /// `randomTribeName()` (`:134-155`).
    fn random_tribe_name(&mut self) -> String {
        if let Some(custom) = self.tribe_name_data.custom_tribes.as_ref() {
            // TS: :139-149 custom tribes first (random selection, no duplicates until exhausted)
            let available: Vec<&CustomTribe> = custom
                .iter()
                .filter(|ct| ct_coordinates(ct).is_none() && !self.used_custom_tribes.contains(&ct_name(ct).to_owned()))
                .collect();
            if !available.is_empty() {
                let index = self.random.next_int(0.0, available.len() as f64) as usize; // int: 0 <= index < len
                let chosen = ct_name(available[index]).to_owned(); // in-range: nextInt(0, len)
                self.used_custom_tribes.insert(chosen.clone());
                return chosen;
            }
        }
        // TS: :152-154 theme-based prefix + suffix (`arr[i]` of an empty theme is `undefined`)
        let d = &self.tribe_name_data;
        let prefix_index = self.random.next_int(0.0, d.prefixes.len() as f64) as usize;
        let suffix_index = self.random.next_int(0.0, d.suffixes.len() as f64) as usize;
        let p = d.prefixes.get(prefix_index).map_or("undefined", String::as_str);
        let s = d.suffixes.get(suffix_index).map_or("undefined", String::as_str);
        format!("{p} {s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_tribe_accessors() {
        let a = CustomTribe::Name("A".into());
        let b = CustomTribe::Placed { name: "B".into(), coordinates: Some([3, 4]) };
        assert_eq!((ct_name(&a), ct_coordinates(&a)), ("A", None));
        assert_eq!((ct_name(&b), ct_coordinates(&b)), ("B", Some([3, 4])));
    }

    /// The prefix/suffix fallback draws two `nextInt`s from the `simpleHash(gameID) + 2`
    /// stream, then `spawnTribe` draws `nextID` (`:91-96, :152-154`).
    #[test]
    fn random_names_and_ids_follow_the_ts_draw_order() {
        let mut sp = TribeSpawner {
            game_id: "g1".into(),
            random: Prng::new(of_det::util::simple_hash("g1") + 2.0),
            tribe_name_data: TribeNameData {
                prefixes: vec!["Red".into(), "Blue".into(), "Grey".into()],
                suffixes: vec!["Wolves".into(), "Bears".into()],
                custom_tribes: None,
            },
            used_custom_tribes: JsSet::new(),
            nation_tiles: LookupSet::new(),
        };
        let mut r = Prng::new(of_det::util::simple_hash("g1") + 2.0);
        let name = sp.random_tribe_name();
        let (pi, si) = (r.next_int(0.0, 3.0) as usize, r.next_int(0.0, 2.0) as usize);
        assert_eq!(name, format!("{} {}", ["Red", "Blue", "Grey"][pi], ["Wolves", "Bears"][si]));
        let st = State::default();
        let e = sp.spawn_tribe(&st, &name);
        assert_eq!(e.player_info.id, r.next_id());
        assert_eq!(e.player_info.player_type, PlayerType::Bot);
        assert_eq!(e.player_info.name, name);
        assert!(e.tile.is_none() && !e.from_intent);
    }

    /// Unpositioned custom tribes are drawn without repetition before the theme fallback.
    #[test]
    fn custom_names_are_used_once() {
        let mut sp = TribeSpawner {
            game_id: "g".into(),
            random: Prng::new(7.0),
            tribe_name_data: TribeNameData {
                prefixes: vec!["P".into()],
                suffixes: vec!["S".into()],
                custom_tribes: Some(vec![
                    CustomTribe::Name("One".into()),
                    CustomTribe::Placed { name: "Two".into(), coordinates: None },
                    CustomTribe::Placed { name: "Pos".into(), coordinates: Some([1, 1]) },
                ]),
            },
            used_custom_tribes: JsSet::new(),
            nation_tiles: LookupSet::new(),
        };
        let (a, b) = (sp.random_tribe_name(), sp.random_tribe_name());
        assert!((a == "One" && b == "Two") || (a == "Two" && b == "One"), "{a} {b}");
        assert_eq!(sp.random_tribe_name(), "P S");
    }
}
