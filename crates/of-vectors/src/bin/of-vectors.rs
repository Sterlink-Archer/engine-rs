//! `of-vectors [--dir DIR] [--json OUT.json] [--suite NAME]...`
//! Prints a per-function pass/fail table for of-det against the golden vectors.
//! Exit 0 iff no function has a failing row.
use anyhow::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = of_vectors::default_dir();
    let mut json: Option<PathBuf> = None;
    let mut suites: Vec<String> = vec![];
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                dir = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--json" => {
                json = Some(PathBuf::from(&args[i + 1]));
                i += 1;
            }
            "--suite" => {
                suites.push(args[i + 1].clone());
                i += 1;
            }
            "-h" | "--help" => {
                println!("of-vectors [--dir DIR] [--json OUT] [--suite NAME]...");
                return Ok(());
            }
            a => anyhow::bail!("unknown arg {a}"),
        }
        i += 1;
    }
    let t0 = std::time::Instant::now();
    let reps = if suites.is_empty() {
        of_vectors::run_all(&dir)?
    } else {
        let mut v = vec![];
        for s in &suites {
            v.extend(of_vectors::run_suite(&dir, s)?);
        }
        v
    };
    let ms = t0.elapsed().as_millis();
    print!("{}", of_vectors::table(&reps));
    let (cases, pass, fail, ood, unported) = reps
        .iter()
        .fold((0, 0, 0, 0, 0), |a, r| (a.0 + r.cases, a.1 + r.pass, a.2 + r.fail, a.3 + r.ood, a.4 + r.unported));
    let arch = std::env::consts::ARCH;
    println!(
        "\nTOTAL arch={arch} functions={} cases={cases} pass={pass} fail={fail} ood={ood} unported={unported} ms={ms} dir={}",
        reps.len(),
        dir.display()
    );
    if let Some(p) = json {
        let rows: Vec<serde_json::Value> = reps
            .iter()
            .map(|r| {
                serde_json::json!({
            "suite": r.suite, "function": r.function, "source": r.source, "cases": r.cases, "pass": r.pass,
            "fail": r.fail, "ood": r.ood, "unported": r.unported, "first_fail": r.first_fail })
            })
            .collect();
        std::fs::write(
            &p,
            serde_json::to_string_pretty(&serde_json::json!({
            "arch": arch, "os": std::env::consts::OS, "dir": dir, "ms": ms as u64,
            "totals": {"cases": cases, "pass": pass, "fail": fail, "ood": ood, "unported": unported},
            "functions": rows }))?,
        )?;
    }
    if fail > 0 {
        std::process::exit(1);
    }
    Ok(())
}
