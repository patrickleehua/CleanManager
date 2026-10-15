//! Release layout (MVP 14.1, CLI-01): what a standalone CLI install is made of, where its rule
//! pack comes from, and how the copy was installed.
//!
//! A release is a directory (unpacked from the zip that `tools/package_release.py` builds, or
//! created by `self install`) holding `cleanmanager.exe`, the `rules` pack, notices and
//! [`RELEASE_FILE`]: the list of every file with its SHA-256. The CLI never reads the source tree
//! in a release build; only debug builds fall back to the repository's `rules` directory.

use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Written into the root of every release layout by the packager, `self install` and `self update`.
pub const RELEASE_FILE: &str = "cleanmanager-release.json";
/// Written by `self install`: what that installation changed outside its own directory.
pub const INSTALL_STATE_FILE: &str = "cleanmanager-install.json";
pub const RELEASE_CONTRACT: &str = "cleanmanager-release/1";
pub const INSTALL_CONTRACT: &str = "cleanmanager-install/1";
pub const PRODUCT: &str = "cleanmanager-cli";

pub fn exe_name() -> &'static str {
    if cfg!(windows) { "cleanmanager.exe" } else { "cleanmanager" }
}

/// `windows-x86_64`, `windows-aarch64`, `macos-aarch64`, ...
pub fn target_id() -> String {
    let os = if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "macos" } else { std::env::consts::OS };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// Marker "directory" meaning the rule pack compiled into this program (see `build.rs`).
pub const EMBEDDED_RULES: &str = "<embedded rule pack>";

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_rules.rs"));
}

/// Rule pack location: `CM_RULES_DIR`, then `rules` next to the executable (release layouts, which
/// `self update` can refresh), then the pack embedded at build time. The source tree is never read.
pub fn rules_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CM_RULES_DIR") {
        return PathBuf::from(dir);
    }
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("rules")));
    match beside.filter(|d| d.is_dir()) {
        Some(dir) => dir,
        None => PathBuf::from(EMBEDDED_RULES),
    }
}

/// The `*.json` files directly inside `dir` when `dir` is a subdirectory of the embedded pack
/// (`<embedded rule pack>/registry`, `.../windows`); `None` for real directories.
pub fn embedded_json_files(dir: &Path) -> Option<Vec<(PathBuf, String)>> {
    let rest = dir.to_str()?.strip_prefix(EMBEDDED_RULES)?.trim_start_matches(['/', '\\']);
    let prefix = format!("{rest}/");
    Some(
        embedded::FILES
            .iter()
            .filter(|(path, _)| path.strip_prefix(&prefix).is_some_and(|name| !name.contains('/')))
            .map(|(path, text)| (Path::new(EMBEDDED_RULES).join(path), (*text).to_owned()))
            .collect(),
    )
}

/// One file of a release layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseFile {
    /// Relative, `/`-separated, validated by [`safe_relative`].
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// The parsed [`RELEASE_FILE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseManifest {
    pub version: String,
    pub target: String,
    /// `zip` (packager), `self_install`, or a package manager name when a packager ships the layout.
    pub install_source: String,
    pub files: Vec<ReleaseFile>,
}

impl ReleaseManifest {
    pub fn parse(text: &str) -> Result<ReleaseManifest, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("{RELEASE_FILE} is not JSON: {e}"))?;
        if v["contract"] != RELEASE_CONTRACT || v["product"] != PRODUCT {
            return Err(format!("{RELEASE_FILE} is not a {PRODUCT} release manifest"));
        }
        let s = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(str::to_owned).ok_or_else(|| format!("{RELEASE_FILE}: missing {k}"));
        let mut files = Vec::new();
        for f in v["files"].as_array().ok_or_else(|| format!("{RELEASE_FILE}: missing files"))? {
            let path = f["path"].as_str().ok_or_else(|| format!("{RELEASE_FILE}: file without path"))?;
            safe_relative(path)?;
            let sha256 = f["sha256"].as_str().filter(|h| is_sha256(h)).ok_or_else(|| format!("{RELEASE_FILE}: {path} has no SHA-256"))?;
            let size = f["size"].as_u64().ok_or_else(|| format!("{RELEASE_FILE}: {path} has no size"))?;
            files.push(ReleaseFile { path: path.to_owned(), sha256: sha256.to_owned(), size });
        }
        Ok(ReleaseManifest { version: s("version")?, target: s("target")?, install_source: s("install_source")?, files })
    }

    pub fn to_json(&self) -> String {
        let files: Vec<Value> = self.files.iter().map(|f| json!({"path": f.path, "sha256": f.sha256, "size": f.size})).collect();
        serde_json::to_string_pretty(&json!({
            "contract": RELEASE_CONTRACT, "product": PRODUCT, "version": self.version, "target": self.target,
            "install_source": self.install_source, "files": files,
        }))
        .expect("manifest serializes")
            + "\n"
    }

    pub fn load(dir: &Path) -> Result<Option<ReleaseManifest>, String> {
        match std::fs::read_to_string(dir.join(RELEASE_FILE)) {
            Ok(t) => ReleaseManifest::parse(&t).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("cannot read {RELEASE_FILE}: {e}")),
        }
    }
}

/// What `self install` recorded about changes outside the install directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallState {
    pub installed_at: String,
    /// The exact entry appended to the user's PATH, if any.
    pub path_entry_added: Option<String>,
}

impl InstallState {
    pub fn load(dir: &Path) -> Option<InstallState> {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(INSTALL_STATE_FILE)).ok()?).ok()?;
        (v["contract"] == INSTALL_CONTRACT).then(|| InstallState {
            installed_at: v["installed_at"].as_str().unwrap_or_default().to_owned(),
            path_entry_added: v["path_entry_added"].as_str().map(str::to_owned),
        })
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&json!({
            "contract": INSTALL_CONTRACT, "installed_at": self.installed_at, "path_entry_added": self.path_entry_added,
        }))
        .expect("state serializes")
            + "\n"
    }
}

pub fn is_sha256(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A relative path made only of normal components with conservative characters: no `..`, no
/// root, drive, alternate stream, device name or trailing dot/space.
pub fn safe_relative(path: &str) -> Result<PathBuf, String> {
    let bad = || format!("unsafe path in manifest: {path:?}");
    if path.is_empty() || path.len() > 240 || path.starts_with('/') || path.contains('\\') {
        return Err(bad());
    }
    let mut out = PathBuf::new();
    for part in path.split('/') {
        let upper = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&upper.as_str())
            || ((upper.starts_with("COM") || upper.starts_with("LPT")) && upper.len() == 4 && upper.as_bytes()[3].is_ascii_digit());
        let ok_chars = part.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.@+".contains(&b));
        if part.is_empty() || part == "." || part == ".." || reserved || !ok_chars || part.ends_with('.') || part.ends_with(' ') {
            return Err(bad());
        }
        out.push(part);
    }
    if out.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(bad());
    }
    Ok(out)
}

pub fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    loop {
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        n += k as u64;
        h.update(&buf[..k]);
    }
    Ok((hex(&h.finalize()), n))
}

pub fn sha256_bytes(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Display text for a path with the home directory shown as `~` (records never need the user name).
pub fn display(p: &Path) -> String {
    crate::doctor_cmd::Redactor::from_env().text(&p.display().to_string())
}

/// Lexically normalized absolute path (no symlink resolution), for comparisons.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    let (a, b) = (normalize(a), normalize(b));
    if cfg!(windows) { a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() } else { a == b }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_paths_are_confined() {
        for ok in ["cleanmanager.exe", "rules/windows/a.rule.json", "rules/registry/x@1.0.0.json"] {
            assert!(safe_relative(ok).is_ok(), "{ok}");
        }
        for bad in ["", "../x", "a/../../x", "/abs", "C:/x", "a\\b", "rules/con", "a/b.", "x:stream", "a//b", "a/./b", "nul.txt", "COM1"] {
            assert!(safe_relative(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn manifest_round_trips_and_rejects_foreign_files() {
        let m = ReleaseManifest {
            version: "1.2.3".into(), target: "windows-x86_64".into(), install_source: "zip".into(),
            files: vec![ReleaseFile { path: "cleanmanager.exe".into(), sha256: "a".repeat(64), size: 3 }],
        };
        assert_eq!(ReleaseManifest::parse(&m.to_json()).unwrap(), m);
        assert!(ReleaseManifest::parse("{\"contract\":\"other\"}").is_err());
        let evil = m.to_json().replace("cleanmanager.exe", "../evil.exe");
        assert!(ReleaseManifest::parse(&evil).is_err());
    }

    #[test]
    fn the_embedded_pack_is_the_repository_pack_and_loads() {
        // In tests the executable has no `rules` beside it, so the embedded pack is used.
        if std::env::var_os("CM_RULES_DIR").is_none() {
            assert_eq!(rules_dir(), PathBuf::from(EMBEDDED_RULES));
        }
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules");
        let platform = if cfg!(windows) { "windows" } else { "macos" };
        for sub in ["registry", platform] {
            let embedded = embedded_json_files(&Path::new(EMBEDDED_RULES).join(sub)).unwrap();
            let mut on_disk: Vec<String> = std::fs::read_dir(repo.join(sub)).unwrap().filter_map(Result::ok)
                .map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).map(|p| std::fs::read_to_string(p).unwrap()).collect();
            let mut texts: Vec<String> = embedded.into_iter().map(|(_, t)| t).collect();
            on_disk.sort();
            texts.sort();
            assert_eq!(texts, on_disk, "{sub}");
        }
        let embedded = crate::analysis_cmd::load_rules(Path::new(EMBEDDED_RULES)).unwrap();
        let disk = crate::analysis_cmd::load_rules(&repo).unwrap();
        assert_eq!(embedded.digest, disk.digest, "same rule set, same digest");
        assert!(embedded_json_files(Path::new("C:/real/rules/registry")).is_none());
    }
}
