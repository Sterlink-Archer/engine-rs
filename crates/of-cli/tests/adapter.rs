//! The of-engine ↔ of-trace adapter at S0: it parses real corpus inputs and fails *cleanly*
//! (a divergence report, never a crash).

use of_canon::Cs;
use of_cli::engine::{OfEngine, cs_to_turn_json};
use of_engine::api::intents::{Intent, Turn};
use of_trace::conform::{Options, Trigger, run};
use of_trace::trace::Trace;
use std::path::PathBuf;

fn fixture(id: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").join(id)
}

#[test]
fn turn_cs_round_trips_to_typed_turn() {
    let cs = Cs::obj([
        ("turnNumber", Cs::Int(3)),
        (
            "intents",
            Cs::Arr(vec![Cs::obj([
                ("type", Cs::Str("attack".into())),
                ("clientID", Cs::Str("c1".into())),
                ("targetID", Cs::Null),
                ("troops", Cs::Num(2633.2000000000003)),
            ])]),
        ),
    ]);
    let t: Turn = serde_json::from_value(cs_to_turn_json(&cs).expect("json")).expect("Turn");
    assert_eq!(t.turn_number, 3);
    match &t.intents[0].intent {
        Intent::Attack { target_id: None, troops: Some(x) } => assert_eq!(x.to_bits(), 2633.2000000000003f64.to_bits()),
        other => panic!("{other:?}"),
    }
    assert!(cs_to_turn_json(&Cs::Big(1)).is_err());
}

#[test]
fn conform_run_reports_unimplemented_not_crash() {
    // Request P3 R6 (merge1): the S0 version pinned the message to the package whose stub was
    // reached first ("P3"), which moves as packages land. What this guards is that the adapter
    // runs a real corpus trace without crashing: it either conforms for the first ticks or
    // reports a clean divergence (`Unimplemented` names the missing piece; `K`/`Fatal` carry
    // the tick). A Rust panic escaping `run` fails the test by itself.
    let dir = fixture("RS-SMOKE-01");
    let tr = Trace::load(&dir).expect("fixture trace");
    let mut e = OfEngine::new();
    let rep = run(&tr, &mut e, &Options { max_ticks: 5, ignore_commit: false });
    if let Some(d) = rep.divergence {
        assert!(!d.message.is_empty(), "a divergence must say why (tick {}, {:?})", d.tick, d.trigger);
        if d.trigger == Trigger::Unimplemented {
            assert!(d.message.contains("not implemented") || d.message.contains("map file"), "{}", d.message);
        }
    }
}
