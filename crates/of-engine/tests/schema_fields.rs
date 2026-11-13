//! S0 guard (FILE_MAP §3 step 3, rule §5.4): every state struct and every execution struct
//! declares the fields of its snapshot record **in dump order**
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). Fields whose doc comment starts with
//! `non-snapshot` are extra state and are skipped; fields that the snapshot derives from
//! another representation are listed per record in `SKIP_SNAPSHOT` with the reason.
//!
//! The check is textual (Rust has no reflection): it reads the top-level `pub` fields of the
//! named struct from its source file and compares `snake_to_camel(field)` (after the per-record
//! rename table) with the record's top-level keys. A mismatch prints both lists.
//!
//! Schema root: `$OF_SCHEMA_FILE`, else `conformance/schema/snapshot_schemas_5b75bd3.txt` in
//! the nearest ancestor that has one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn schema_path() -> PathBuf {
    match std::env::var("OF_SCHEMA_FILE") {
        Ok(p) => PathBuf::from(p),
        Err(_) => conformance_dir().join("schema/snapshot_schemas_5b75bd3.txt"),
    }
}

/// The project's `conformance/` directory: the nearest ancestor of the crate that has one
/// (`engine-rs/` sits one level below the project root, `work/rs-*` worktrees two; requests
/// P9 R2, P11 R5, P12 R4).
fn conformance_dir() -> PathBuf {
    let start = Path::new(env!("CARGO_MANIFEST_DIR"));
    start
        .ancestors()
        .map(|a| a.join("conformance"))
        .find(|c| c.join("golden").is_dir() || c.join("schema").is_dir())
        .unwrap_or_else(|| start.join("../../../conformance"))
}

/// `### Name vN [ (execution)]` → top-level keys, in order.
fn parse_schema(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut cur: Option<String> = None;
    let mut keys = Vec::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("### ") {
            if let Some(name) = cur.take() {
                out.insert(name, std::mem::take(&mut keys));
            }
            let mut parts = h.split_whitespace();
            let name = parts.next().expect("record name");
            let exec = h.contains("(execution)");
            cur = Some(if exec { format!("{name}#exec") } else { name.to_string() });
            continue;
        }
        if cur.is_some() {
            // top-level keys have exactly two spaces of indentation
            if let Some(rest) = line.strip_prefix("  ")
                && !rest.starts_with(' ')
                && let Some((k, _)) = rest.split_once(':')
            {
                keys.push(k.trim().to_string());
            }
        }
    }
    if let Some(name) = cur.take() {
        out.insert(name, keys);
    }
    out
}

fn snake_to_camel(s: &str) -> String {
    let s = s.trim_end_matches('_');
    let mut out = String::new();
    let mut up = false;
    for c in s.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Top-level `pub` fields of `struct name` in `src`, skipping `non-snapshot` ones.
fn struct_fields(src: &str, name: &str) -> Vec<String> {
    let start = src.find(&format!("pub struct {name} {{")).unwrap_or_else(|| panic!("struct {name} not found"));
    let body = &src[start..];
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut doc = String::new();
    for line in body.lines() {
        let t = line.trim();
        if depth == 1 {
            if let Some(d) = t.strip_prefix("///") {
                doc.push_str(d.trim());
                doc.push(' ');
            } else if let Some(rest) = t.strip_prefix("pub ") {
                if let Some((f, _)) = rest.split_once(':')
                    && !doc.starts_with("non-snapshot")
                {
                    out.push(f.trim().to_string());
                }
                doc.clear();
            } else if t.starts_with("//") || t.starts_with("#[") || t.is_empty() {
                // comment, attribute, blank: keep any pending doc
            } else {
                doc.clear();
            }
        }
        depth += line.matches('{').count() as i32;
        depth -= line.matches('}').count() as i32;
        if depth == 0 && line.contains('}') {
            break;
        }
    }
    out
}

struct Rec {
    record: &'static str,
    file: &'static str,
    strukt: &'static str,
    /// rust field → schema key (beyond plain snake→camel).
    renames: &'static [(&'static str, &'static str)],
    /// schema keys with no field of their own (derived at export), with the reason in `why`.
    skip_snapshot: &'static [&'static str],
    why: &'static str,
}

const RECS: &[Rec] = &[
    Rec {
        record: "Alliance",
        file: "state/alliance.rs",
        strukt: "Alliance",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "AllianceRequest",
        file: "state/alliance_request.rs",
        strukt: "AllianceRequest",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
    Rec { record: "Attack", file: "state/attack.rs", strukt: "Attack", renames: &[], skip_snapshot: &[], why: "" },
    Rec { record: "Cluster", file: "rail/station.rs", strukt: "Cluster", renames: &[], skip_snapshot: &[], why: "" },
    Rec {
        record: "Game",
        file: "state/game.rs",
        strukt: "State",
        renames: &[
            ("uninit", "unInitExecs"),
            ("next_player_id", "nextPlayerID"),
            ("next_unit_id", "nextUnitID"),
            ("next_alliance_id", "nextAllianceID"),
            ("shared_water", "sharedWaterCache"),
        ],
        skip_snapshot: &["execs"],
        why: "execs = Game.execs.live (outside State while an exec runs, ARCHITECTURE §4.2)",
    },
    Rec {
        record: "GameMap",
        file: "map/game_map.rs",
        strukt: "GameMap",
        renames: &[("w", "width"), ("h", "height"), ("pristine_edits", "editTiles"), ("terrain", "editCurrent")],
        skip_snapshot: &["pristineHash", "editOriginal", "fallout", "defense"],
        why: "pristineHash dropped (SPEC N2); editOriginal is the pristine_edits values; fallout/defense lists derive from the private state buffer",
    },
    Rec {
        record: "Player",
        file: "state/player.rs",
        strukt: "Player",
        renames: &[
            ("small_id", "smallID"),
            ("rng", "random"),
            ("my_units_version", "unitsVersion"),
            ("largest_cluster_bbox", "largestClusterBoundingBox"),
        ],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "RailNetwork",
        file: "rail/network.rs",
        strukt: "RailNetwork",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
    Rec { record: "Railroad", file: "rail/railroad.rs", strukt: "Railroad", renames: &[], skip_snapshot: &[], why: "" },
    Rec {
        record: "SharedWaterCache",
        file: "state/shared_water_cache.rs",
        strukt: "SharedWaterCache",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "Stats",
        file: "state/stats/mod.rs",
        strukt: "Stats",
        renames: &[("lite", "data")],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "TrainStation",
        file: "rail/station.rs",
        strukt: "TrainStation",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "Unit",
        file: "state/unit.rs",
        strukt: "Unit",
        renames: &[("unit_type", "type"), ("active", "active")],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "WaterManager",
        file: "water/manager.rs",
        strukt: "WaterManager",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "AiAttackBehavior",
        file: "exec/nation/attack.rs",
        strukt: "NationAttackB",
        renames: &[("base", "triggerRatio")],
        skip_snapshot: &["reserveRatio", "expandRatio"],
        why: "the three ratios live in `base: AiAttackBase` (exported in record order by P10/P13)",
    },
    Rec {
        record: "NationEmojiBehavior",
        file: "exec/nation/emoji.rs",
        strukt: "EmojiB",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "NationNukeBehavior",
        file: "exec/nation/nuke.rs",
        strukt: "NukeB",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "NationStructureBehavior",
        file: "exec/nation/structure.rs",
        strukt: "StructureB",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "NationWarshipBehavior",
        file: "exec/nation/warship.rs",
        strukt: "WarshipB",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "NationAllianceBehavior",
        file: "exec/nation/alliance.rs",
        strukt: "AllianceB",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
    Rec {
        record: "NationMIRVBehavior",
        file: "exec/nation/mirv.rs",
        strukt: "MirvB",
        renames: &[],
        skip_snapshot: &[],
        why: "",
    },
];

/// One execution row: `(record, file, struct, renames, skip, why)`.
type ExecRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
    &'static str,
);

/// Executions.
const EXECS: &[ExecRow] = &[
    ("AllianceExtension", "exec/alliance/extension.rs", "AllianceExtensionExec", &[("to_id", "toID")], &[], ""),
    ("AllianceReject", "exec/alliance/reject.rs", "AllianceRejectExec", &[("requestor_id", "requestorID")], &[], ""),
    ("AllianceRequest", "exec/alliance/request.rs", "AllianceRequestExec", &[("recipient_id", "recipientID")], &[], ""),
    ("Attack", "exec/attack.rs", "AttackExec", &[("rng", "random"), ("target_id", "targetID")], &[], ""),
    ("BoatRetreat", "exec/boat_retreat.rs", "BoatRetreatExec", &[("unit_id", "unitID")], &[], ""),
    ("BreakAlliance", "exec/alliance/break_.rs", "BreakAllianceExec", &[("recipient_id", "recipientID")], &[], ""),
    ("City", "exec/city.rs", "CityExec", &[], &[], ""),
    ("Construction", "exec/construction.rs", "ConstructionExec", &[], &[], ""),
    ("DefensePost", "exec/defense_post.rs", "DefensePostExec", &[], &[], ""),
    ("DeleteUnit", "exec/delete_unit.rs", "DeleteUnitExec", &[], &[], ""),
    (
        "DonateGold",
        "exec/donate_gold.rs",
        "DonateGoldExec",
        &[("rng", "random"), ("recipient_id", "recipientID")],
        &[],
        "",
    ),
    (
        "DonateTroops",
        "exec/donate_troops.rs",
        "DonateTroopsExec",
        &[("rng", "random"), ("recipient_id", "recipientID")],
        &[],
        "",
    ),
    ("DoomsdayClock", "exec/doomsday.rs", "DoomsdayClockExec", &[], &[], ""),
    ("Embargo", "exec/embargo.rs", "EmbargoExec", &[("target_id", "targetID")], &[], ""),
    ("EmbargoAll", "exec/embargo_all.rs", "EmbargoAllExec", &[], &[], ""),
    ("Emoji", "exec/emoji.rs", "EmojiExec", &[("rng", "random"), ("recipient_id", "recipientID")], &[], ""),
    ("Factory", "exec/factory.rs", "FactoryExec", &[], &[], ""),
    ("MarkDisconnected", "exec/mark_disconnected.rs", "MarkDisconnectedExec", &[], &[], ""),
    ("MIRV", "exec/mirv.rs", "MirvExec", &[], &[], ""),
    ("MissileSilo", "exec/missile_silo.rs", "MissileSiloExec", &[], &[], ""),
    ("MoveWarship", "exec/move_warship.rs", "MoveWarshipExec", &[], &[], ""),
    (
        "Nation",
        "exec/nation/mod.rs",
        "NationExec",
        &[("game_id", "gameID"), ("rng", "random")],
        &["nationIndex"],
        "nationIndex/nation are one TS field (`nation`), exported as an index when it is one of game.nations()",
    ),
    ("NoOp", "exec/noop.rs", "NoOpExec", &[], &[], ""),
    ("Nuke", "exec/nuke.rs", "NukeExec", &[], &[], ""),
    ("Pause", "exec/pause.rs", "PauseExec", &[], &[], ""),
    ("Player", "exec/player.rs", "PlayerExec", &[], &[], ""),
    ("Port", "exec/port.rs", "PortExec", &[], &[], ""),
    ("QuickChat", "exec/quick_chat.rs", "QuickChatExec", &[("recipient_id", "recipientID")], &[], ""),
    ("RecomputeRailCluster", "exec/rail_cluster.rs", "RecomputeRailClusterExec", &[], &[], ""),
    ("Retreat", "exec/retreat.rs", "RetreatExec", &[("attack_id", "attackID")], &[], ""),
    ("SAMLauncher", "exec/sam_launcher.rs", "SamLauncherExec", &[], &[], ""),
    ("SAMMissile", "exec/sam_missile.rs", "SamMissileExec", &[], &[], ""),
    ("Shell", "exec/shell.rs", "ShellExec", &[], &[], ""),
    ("Spawn", "exec/spawn.rs", "SpawnExec", &[("rng", "random")], &[], ""),
    ("SpawnTimer", "exec/spawn_timer.rs", "SpawnTimerExec", &[], &[], ""),
    ("TargetPlayer", "exec/target_player.rs", "TargetPlayerExec", &[("target_id", "targetID")], &[], ""),
    ("TradeShip", "exec/trade_ship.rs", "TradeShipExec", &[], &[], ""),
    ("Train", "exec/train.rs", "TrainExec", &[], &[], ""),
    ("TrainStation", "exec/train_station.rs", "TrainStationExec", &[("rng", "random")], &[], ""),
    ("TransportShip", "exec/transport_ship.rs", "TransportShipExec", &[], &[], ""),
    ("Tribe", "exec/tribe/mod.rs", "TribeExec", &[("rng", "random")], &[], ""),
    ("UpgradeStructure", "exec/upgrade_structure.rs", "UpgradeStructureExec", &[], &[], ""),
    ("Warship", "exec/warship.rs", "WarshipExec", &[("rng", "random")], &[], ""),
    ("WinCheck", "exec/win_check.rs", "WinCheckExec", &[], &[], ""),
];

fn check(
    schema: &BTreeMap<String, Vec<String>>,
    key: &str,
    file: &str,
    strukt: &str,
    renames: &[(&str, &str)],
    skip: &[&str],
    bad: &mut Vec<String>,
) {
    let want: Vec<String> = schema
        .get(key)
        .unwrap_or_else(|| panic!("no schema record {key}"))
        .iter()
        .filter(|k| !skip.contains(&k.as_str()))
        .cloned()
        .collect();
    let src_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file);
    let src = std::fs::read_to_string(&src_path).unwrap_or_else(|e| panic!("{}: {e}", src_path.display()));
    let got: Vec<String> = struct_fields(&src, strukt)
        .into_iter()
        .map(|f| {
            renames.iter().find(|(r, _)| *r == f).map(|(_, s)| s.to_string()).unwrap_or_else(|| snake_to_camel(&f))
        })
        .collect();
    if got != want {
        bad.push(format!("{key} ({file}::{strukt})\n  schema: {want:?}\n  rust:   {got:?}"));
    }
}

#[test]
#[ignore = "needs the full conformance corpus (not shipped): set OF_CORPUS_DIR"]
fn every_snapshot_record_is_declared_in_dump_order() {
    let path = schema_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("schema dump {}: {e}", path.display()));
    let schema = parse_schema(&text);
    assert_eq!(schema.len(), 65, "65 record types (ARCHITECTURE header)");
    let mut bad = Vec::new();
    let mut covered = 0;
    for r in RECS {
        let _ = r.why;
        check(&schema, r.record, r.file, r.strukt, r.renames, r.skip_snapshot, &mut bad);
        covered += 1;
    }
    for (rec, file, strukt, renames, skip, _why) in EXECS {
        check(&schema, &format!("{rec}#exec"), file, strukt, renames, skip, &mut bad);
        covered += 1;
    }
    assert!(bad.is_empty(), "{} record(s) out of order or incomplete:\n{}", bad.len(), bad.join("\n"));
    assert_eq!(covered, 65, "every record is checked");
}
