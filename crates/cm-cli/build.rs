//! Embed the platform rule pack and the tested registry (`rules/<platform>/*.json`,
//! `rules/registry/*.json`) so a standalone `cleanmanager` never needs the source tree (MVP 14.1,
//! CLI-01). A `rules` directory next to the program or `CM_RULES_DIR` still takes precedence.
//! Only file contents are embedded; no build-machine path reaches the binary.

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let rules = Path::new(&manifest).join("..").join("..").join("rules");
    let platform = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") { "windows" } else { "macos" };
    let mut out = String::from("/// (path relative to the rule pack, contents), sorted.\npub static FILES: &[(&str, &str)] = &[\n");
    for sub in ["registry", platform] {
        let dir = rules.join(sub);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("json"))
            .collect();
        files.sort();
        for f in files {
            let name = f.file_name().and_then(|n| n.to_str()).expect("rule file names are UTF-8");
            let abs = std::fs::canonicalize(&f).expect("rule file is readable");
            writeln!(out, "    ({:?}, include_str!({:?})),", format!("{sub}/{name}"), abs.display().to_string()).unwrap();
        }
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("embedded_rules.rs");
    std::fs::write(dest, out).expect("OUT_DIR is writable");
}
