//! cm-fixtures: prepare, verify and destroy reproducible fixture trees.
//!
//! Usage:
//!   cm-fixtures plan --profile P [--seed N]
//!   cm-fixtures sentinel --path DIR
//!   cm-fixtures create --profile P [--seed N] --root DIR --sentinel DIR
//!   cm-fixtures verify --root DIR [--sentinel DIR]
//!   cm-fixtures destroy --root DIR [--sentinel DIR]
//! Profiles: tiny, wide, deep, p0-10k, p0-100k, p0-1m. Output is one JSON document on stdout.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use cm_fixtures::{Profile, create, destroy, plan, prepare_sentinel, verify, verify_sentinel};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("cm-fixtures: {msg}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<String, String> {
    let (cmd, rest) = args.split_first().ok_or("missing command")?;
    let mut opts = HashMap::new();
    let mut it = rest.iter();
    while let Some(k) = it.next() {
        let key = k.strip_prefix("--").ok_or_else(|| format!("unexpected argument {k}"))?;
        let v = it.next().ok_or_else(|| format!("--{key} needs a value"))?;
        if opts.insert(key.to_owned(), v.clone()).is_some() {
            return Err(format!("--{key} given twice"));
        }
    }
    let path = |k: &str| opts.get(k).map(PathBuf::from).ok_or_else(|| format!("--{k} is required"));
    let profile = || {
        let p = opts.get("profile").ok_or("--profile is required")?;
        Profile::parse(p).ok_or_else(|| format!("unknown profile {p}"))
    };
    let seed = || opts.get("seed").map_or(Ok(0), |s| s.parse::<u64>().map_err(|e| format!("bad seed: {e}")));
    let json = |v: serde_json::Value| serde_json::to_string_pretty(&v).expect("serializable");
    let to_err = |e: cm_fixtures::FixtureError| e.0;
    match cmd.as_str() {
        "plan" => Ok(serde_json::to_string_pretty(&plan(profile()?, seed()?)).expect("serializable")),
        "sentinel" => Ok(serde_json::to_string_pretty(&prepare_sentinel(&path("path")?).map_err(to_err)?).expect("serializable")),
        "create" => {
            let m = create(profile()?, seed()?, &path("root")?, &path("sentinel")?).map_err(to_err)?;
            Ok(serde_json::to_string_pretty(&m).expect("serializable"))
        }
        "verify" => {
            let m = verify(&path("root")?).map_err(to_err)?;
            if let Ok(s) = path("sentinel") {
                verify_sentinel(&s).map_err(to_err)?;
            }
            Ok(json(serde_json::json!({"verified": true, "entries_sha256": m.plan.entries_sha256})))
        }
        "destroy" => {
            destroy(&path("root")?).map_err(to_err)?;
            let sentinel_ok = match path("sentinel") {
                Ok(s) => Some(verify_sentinel(&s).map_err(to_err).is_ok()),
                Err(_) => None,
            };
            Ok(json(serde_json::json!({"destroyed": true, "sentinel_intact": sentinel_ok})))
        }
        other => Err(format!("unknown command {other}")),
    }
}
