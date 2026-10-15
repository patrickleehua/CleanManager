//! CLI-01 / CLI-03 smoke: the binary runs without any GUI and writes exactly one clean JSON record.

use std::process::Command;

fn run(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cleanmanager")).args(args).output().expect("binary runs");
    let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    (out.status.code().unwrap_or(-1), stdout)
}

fn single_record(stdout: &str) -> serde_json::Value {
    assert!(!stdout.starts_with('\u{feff}'), "no BOM");
    assert!(!stdout.contains('\u{1b}'), "no ANSI");
    assert_eq!(stdout.lines().count(), 1, "exactly one record on stdout");
    serde_json::from_str(stdout.trim_end()).expect("valid JSON")
}

#[test]
fn capabilities_without_gui() {
    let (code, stdout) = run(&["capabilities", "--json"]);
    assert_eq!(code, 0);
    let v = single_record(&stdout);
    assert_eq!(v["kind"], "capabilities");
    assert_eq!(v["schema_version"], "1.0");
    assert!(v["capabilities"].as_array().unwrap().iter().all(|c| c["state"] != "available"));
}

#[test]
fn unknown_command_is_typed_invalid_argument() {
    let (code, stdout) = run(&["no-such-command", "--json"]);
    assert_eq!(code, 2);
    assert_eq!(single_record(&stdout)["code"], "invalid_argument");
}

#[test]
fn conflicting_formats_are_rejected() {
    let (code, _) = run(&["capabilities", "--json", "--format", "text"]);
    assert_eq!(code, 2);
}
