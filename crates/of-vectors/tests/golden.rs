//! `cargo test` gate: every golden-vector row for the ported functions passes.
//! One test per suite so failures name the suite.

fn check(suite: &str) {
    let dir = of_vectors::default_dir();
    let reps = of_vectors::run_suite(&dir, suite).unwrap_or_else(|e| panic!("{suite}: {e:#}"));
    let bad: Vec<_> = reps.iter().filter(|r| r.fail > 0).collect();
    if !bad.is_empty() {
        panic!("{suite} failures:\n{}", of_vectors::table(&reps));
    }
    let checked: usize = reps.iter().map(|r| r.pass).sum();
    assert!(suite == "config" || checked > 0, "{suite}: nothing checked");
}

#[test]
fn detmath() {
    check("detmath");
}
#[test]
fn prng() {
    check("prng");
}
#[test]
fn util() {
    check("util");
}
#[test]
fn jssem() {
    check("jssem");
}
#[test]
fn order() {
    check("order");
}
#[test]
fn config_listed() {
    check("config");
}
