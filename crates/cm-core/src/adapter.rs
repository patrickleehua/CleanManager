//! Owner-tool delegation adapters (CAT-05, design D1 `tool_adapter`).
//!
//! An adapter is a compiled table entry: the tool, where the platform may look for its executable
//! (trusted install locations only, never `PATH` or the working directory), a fixed argv for the
//! read-only preview and one for the cleanup, a timeout and an output bound. Rules only name an
//! adapter by id; no rule, user or model text ever becomes an argument, and nothing runs through a
//! shell. A missing tool, a timeout or a failure is `unavailable` — never a reason to delete the
//! tool's directory directly. Cleanup argv is declared for review but not executed in this build.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Where the platform may find the tool. Every variant resolves to an absolute path under a
/// location written by an installer, never a search path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Locator {
    /// Windows: `node.exe` in the Node.js install directory (HKLM `SOFTWARE\Node.js\InstallPath`,
    /// else `%ProgramFiles%\nodejs`), running `script` relative to that directory.
    NodeJsScript { script: &'static [&'static str] },
    /// Windows: `%ProgramFiles%\<path>`.
    ProgramFiles { path: &'static [&'static str] },
}

/// How a successful preview output is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKind {
    /// First non-empty stdout line is the tool's cache directory.
    CachePath,
    /// Output is shown as a bounded summary only.
    Listing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdapterSpec {
    pub adapter_id: &'static str,
    pub version: &'static str,
    pub tool_id: &'static str,
    pub tool_name: &'static str,
    pub locator: Locator,
    pub preview_argv: &'static [&'static str],
    pub preview_kind: PreviewKind,
    pub cleanup_argv: &'static [&'static str],
    /// What the cleanup command does, for review.
    pub cleanup_effect: &'static str,
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
}

pub const ADAPTERS: &[AdapterSpec] = &[
    AdapterSpec {
        adapter_id: "npm.cache-verify",
        version: "1.0.0",
        tool_id: "npm",
        tool_name: "npm",
        locator: Locator::NodeJsScript { script: &["node_modules", "npm", "bin", "npm-cli.js"] },
        preview_argv: &["config", "get", "cache"],
        preview_kind: PreviewKind::CachePath,
        cleanup_argv: &["cache", "verify"],
        cleanup_effect: "npm verifies its content-addressed cache and garbage-collects entries no longer referenced by its index.",
        timeout_ms: 20_000,
        max_output_bytes: 16 * 1024,
    },
    AdapterSpec {
        adapter_id: "go.build-cache-clean",
        version: "1.0.0",
        tool_id: "go",
        tool_name: "Go",
        locator: Locator::ProgramFiles { path: &["Go", "bin", "go.exe"] },
        preview_argv: &["env", "GOCACHE"],
        preview_kind: PreviewKind::CachePath,
        cleanup_argv: &["clean", "-cache"],
        cleanup_effect: "go removes its whole build cache; the next build recompiles every package.",
        timeout_ms: 20_000,
        max_output_bytes: 16 * 1024,
    },
    AdapterSpec {
        adapter_id: "dotnet.nuget-http-cache",
        version: "1.0.0",
        tool_id: "dotnet",
        tool_name: ".NET SDK (NuGet)",
        locator: Locator::ProgramFiles { path: &["dotnet", "dotnet.exe"] },
        preview_argv: &["nuget", "locals", "http-cache", "--list"],
        preview_kind: PreviewKind::Listing,
        cleanup_argv: &["nuget", "locals", "http-cache", "--clear"],
        cleanup_effect: "NuGet clears its HTTP response cache; package metadata is downloaded again on the next restore.",
        timeout_ms: 30_000,
        max_output_bytes: 16 * 1024,
    },
];

pub fn adapter(id: &str) -> Option<&'static AdapterSpec> {
    ADAPTERS.iter().find(|a| a.adapter_id == id)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    #[error("adapter {0}: argument {1:?} is not a plain token")]
    Argument(String, String),
    #[error("adapter {0}: {1}")]
    Spec(String, String),
}

/// A fixed argument: letters, digits and `-._` only, at most 64 bytes, not starting with `@`
/// (response files) or `/` (Windows switches that a tool might read as a path). This excludes
/// every shell metacharacter, quote, space, `%` expansion and path separator.
pub fn plain_token(arg: &str) -> bool {
    !arg.is_empty() && arg.len() <= 64 && !arg.starts_with(['@', '/']) && arg.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
}

impl AdapterSpec {
    /// Static checks applied before any process is started.
    pub fn validate(&self) -> Result<(), AdapterError> {
        let err = |m: &str| AdapterError::Spec(self.adapter_id.into(), m.into());
        for a in self.preview_argv.iter().chain(self.cleanup_argv) {
            if !plain_token(a) {
                return Err(AdapterError::Argument(self.adapter_id.into(), (*a).into()));
            }
        }
        let parts: &[&str] = match self.locator {
            Locator::NodeJsScript { script } => script,
            Locator::ProgramFiles { path } => path,
        };
        if parts.is_empty() || parts.iter().any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains(['/', '\\', ':', '\0'])) {
            return Err(err("locator components must be plain names"));
        }
        if self.timeout_ms == 0 || self.timeout_ms > 120_000 || self.max_output_bytes == 0 || self.max_output_bytes > 1 << 20 {
            return Err(err("timeout and output bound must be finite and small"));
        }
        if self.preview_argv.is_empty() || self.cleanup_argv.is_empty() {
            return Err(err("preview and cleanup argv are required"));
        }
        Ok(())
    }
}

/// Outcome of running an adapter's preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterStatus {
    Available,
    /// Tool missing, outside a trusted location, or rejected before it ran.
    Unavailable,
    TimedOut,
    /// Ran and exited non-zero (or could not be waited for).
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterPreview {
    pub adapter_id: String,
    pub version: String,
    pub tool_id: String,
    pub status: AdapterStatus,
    pub reason: Option<String>,
    /// Absolute executable that ran (display form) and its SHA-256 at launch time.
    pub executable: Option<String>,
    pub executable_sha256: Option<String>,
    /// Script the interpreter ran, when the locator is a script (display form and SHA-256).
    pub script: Option<String>,
    pub script_sha256: Option<String>,
    pub argv: Vec<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub output_summary: String,
    /// `CachePath` previews: the directory the tool reports.
    pub reported_path: Option<String>,
    pub cleanup_argv: Vec<String>,
    pub cleanup_effect: String,
}

impl AdapterPreview {
    pub fn unavailable(spec: &AdapterSpec, reason: impl Into<String>) -> Self {
        AdapterPreview {
            adapter_id: spec.adapter_id.into(),
            version: spec.version.into(),
            tool_id: spec.tool_id.into(),
            status: AdapterStatus::Unavailable,
            reason: Some(reason.into()),
            executable: None,
            executable_sha256: None,
            script: None,
            script_sha256: None,
            argv: spec.preview_argv.iter().map(|s| (*s).to_owned()).collect(),
            exit_code: None,
            duration_ms: 0,
            output_summary: String::new(),
            reported_path: None,
            cleanup_argv: spec.cleanup_argv.iter().map(|s| (*s).to_owned()).collect(),
            cleanup_effect: spec.cleanup_effect.into(),
        }
    }
}

/// Bounded, printable summary of tool output: lossy UTF-8, control characters (other than
/// newline and tab) escaped, at most `limit` bytes of text with a truncation marker.
pub fn summarize_output(bytes: &[u8], total_len: usize, limit: usize) -> String {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(limit)]);
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '\n' | '\t' => out.push(ch),
            '\r' => {}
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    let out = out.trim_end().to_owned();
    if total_len > limit { format!("{out}\n[truncated: {} more bytes]", total_len - limit) } else { out }
}

/// First non-empty line of a `CachePath` preview, if it looks like an absolute path.
pub fn reported_path(summary: &str) -> Option<String> {
    let line = summary.lines().map(str::trim).find(|l| !l.is_empty())?;
    let b = line.as_bytes();
    let windows_abs = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    (windows_abs || line.starts_with('/')).then(|| line.to_owned())
}

/// Lower-case hex SHA-256 of a byte stream.
pub fn sha256_hex(mut reader: impl std::io::Read) -> std::io::Result<String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_adapter_is_fixed_and_plain() {
        for a in ADAPTERS {
            a.validate().unwrap_or_else(|e| panic!("{e}"));
        }
        let mut ids: Vec<_> = ADAPTERS.iter().map(|a| a.adapter_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ADAPTERS.len());
    }

    #[test]
    fn argument_injection_is_rejected() {
        for bad in [
            "cache; rm -rf /", "&& del /s /q C:\\", "a|b", "$(whoami)", "`id`", "%COMSPEC%", "\"quoted\"", "'q'", "a b", "--prefix=C:\\x",
            "..\\..\\evil", "/c", "@args.txt", "x\ny", "x\0y", "^&", "<in", ">out", "",
        ] {
            assert!(!plain_token(bad), "{bad:?} must not be accepted as an argument");
            let spec = AdapterSpec { preview_argv: Box::leak(vec![Box::leak(bad.to_owned().into_boxed_str()) as &str].into_boxed_slice()), ..ADAPTERS[0].clone() };
            assert!(matches!(spec.validate(), Err(AdapterError::Argument(..))), "{bad:?}");
        }
        for ok in ["config", "get", "cache", "-cache", "--list", "http-cache", "GOCACHE"] {
            assert!(plain_token(ok), "{ok}");
        }
    }

    #[test]
    fn locators_cannot_escape_their_install_root() {
        for bad in [&["..", "cmd.exe"][..], &["C:", "x.exe"], &["a\\b.exe"], &[]] {
            let spec = AdapterSpec { locator: Locator::ProgramFiles { path: Box::leak(bad.to_vec().into_boxed_slice()) }, ..ADAPTERS[1].clone() };
            assert!(spec.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn rules_can_only_name_adapters_by_id() {
        assert!(adapter("npm.cache-verify").is_some());
        assert!(adapter("npm.cache-verify --force").is_none());
        assert!(adapter("cmd.exe /c del").is_none());
    }

    #[test]
    fn output_is_bounded_and_printable() {
        let s = summarize_output(b"C:\\cache\r\n\x1b[31mred\x07\n", 20, 64);
        assert_eq!(s, "C:\\cache\n\\u{1b}[31mred\\u{7}");
        let long = [b'a'; 100];
        let t = summarize_output(&long[..10], 100, 10);
        assert!(t.starts_with("aaaaaaaaaa\n[truncated: 90 more bytes]"), "{t}");
        assert_eq!(reported_path("\n  C:\\Users\\x\\npm-cache  \n"), Some("C:\\Users\\x\\npm-cache".into()));
        assert_eq!(reported_path("npm warn something"), None);
    }

    #[test]
    fn unavailable_preview_never_claims_a_result() {
        let p = AdapterPreview::unavailable(&ADAPTERS[0], "tool not installed");
        assert_eq!(p.status, AdapterStatus::Unavailable);
        assert!(p.exit_code.is_none() && p.reported_path.is_none() && p.executable.is_none());
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(sha256_hex(&b"abc"[..]).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
