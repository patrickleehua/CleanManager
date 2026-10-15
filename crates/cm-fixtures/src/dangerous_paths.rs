//! Loader for the dual-platform dangerous-path corpus (`tests/corpus/dangerous-paths.json`,
//! parity PRT-03/04/05). Every entry is a path that the protection layer MUST deny; the
//! consumers are `cm_core::protect` (against a modelled machine) and the Windows/macOS
//! hard-deny identity tests in `cm-platform` (against this machine, with real link fixtures).
//!
//! Format (schema `cleanmanager.dangerous-paths/1`):
//! - `path` is a JSON string, or an array of parts where a string part is text and an integer
//!   array part is raw code units (UTF-16 units on Windows, bytes on macOS) for inputs JSON text
//!   cannot carry (lone surrogates, invalid UTF-8). Control characters use JSON `\u` escapes.
//! - Placeholders in text: `{home}` (a user profile), `{user}` (its name), `{tmp}` (an ordinary
//!   scratch directory inside the profile where link fixtures are created) and, on Windows,
//!   `{sysvol}` (`\\?\Volume{GUID}\` of the system volume).
//! - `link` makes the entry a link fixture: `{"kind": "junction"|"symlink"|"file_symlink"|"mount",
//!   "target": "<path>" | null}`; a `null` target is a link whose target is gone.
//! - `deny_by` is `prefilter` (the textual filter alone must deny), `identity` (the textual filter
//!   passes and the identity check must deny) or `any` (default).
//! - `model_only` entries depend on the modelled machine (8.3 names of fixture files, a second
//!   data disk, loopback shares...) and are skipped on a real machine.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

pub const SCHEMA: &str = "cleanmanager.dangerous-paths/1";

/// Workspace location of the corpus.
pub fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/dangerous-paths.json")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    Macos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenyBy {
    Prefilter,
    Identity,
    #[default]
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// Directory junction (Windows) / directory symlink (macOS).
    Junction,
    /// Directory symbolic link.
    Symlink,
    /// File symbolic link.
    FileSymlink,
    /// Junction whose target is a volume root (`\\?\Volume{GUID}\`): a mount point.
    Mount,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub kind: LinkKind,
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Part {
    Text(String),
    Units(Vec<u32>),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum PathSpec {
    Text(String),
    Parts(Vec<Part>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    platform: Platform,
    category: String,
    path: PathSpec,
    #[serde(default)]
    deny_by: DenyBy,
    #[serde(default)]
    link: Option<Link>,
    #[serde(default)]
    model_only: bool,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCorpus {
    schema: String,
    #[allow(dead_code)]
    about: Vec<String>,
    categories: BTreeMap<String, String>,
    entries: Vec<RawEntry>,
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// Position in the file (for messages).
    pub index: usize,
    pub platform: Platform,
    pub category: String,
    pub parts: Vec<Part>,
    pub deny_by: DenyBy,
    pub link: Option<Link>,
    pub model_only: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Corpus {
    /// Category id → description.
    pub categories: BTreeMap<String, String>,
    pub entries: Vec<Entry>,
}

const PLACEHOLDERS: &[&str] = &["{home}", "{user}", "{tmp}", "{sysvol}"];

/// Placeholder values for one context (a model or this machine).
#[derive(Debug, Clone, Default)]
pub struct Vars(pub Vec<(&'static str, String)>);

impl Vars {
    pub fn new(pairs: &[(&'static str, &str)]) -> Self {
        Vars(pairs.iter().map(|(k, v)| (*k, (*v).to_owned())).collect())
    }

    pub fn expand(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (k, v) in &self.0 {
            out = out.replace(k, v);
        }
        out
    }
}

/// Every `{word}` (lower-case letters only) must be a known placeholder; other braces, such as
/// volume GUIDs, are literal.
fn check_placeholders(text: &str, at: usize) -> Result<(), String> {
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        let tail = &rest[i + 1..];
        if let Some(end) = tail.find('}') {
            let word = &tail[..end];
            if !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()) && !PLACEHOLDERS.contains(&format!("{{{word}}}").as_str()) {
                return Err(format!("entry {at}: unknown placeholder {{{word}}} in {text:?}"));
            }
        }
        rest = tail;
    }
    Ok(())
}

impl Entry {
    /// Lossy text with placeholders expanded (raw unit parts become U+FFFD).
    pub fn text(&self, vars: &Vars) -> String {
        self.parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => vars.expand(t),
                Part::Units(u) => "\u{FFFD}".repeat(u.len()),
            })
            .collect()
    }

    /// Exact UTF-16 units (Windows entries).
    pub fn windows_units(&self, vars: &Vars) -> Vec<u16> {
        let mut out = Vec::new();
        for p in &self.parts {
            match p {
                Part::Text(t) => out.extend(vars.expand(t).encode_utf16()),
                Part::Units(u) => out.extend(u.iter().map(|x| *x as u16)),
            }
        }
        out
    }

    /// Exact bytes (macOS entries).
    pub fn unix_bytes(&self, vars: &Vars) -> Vec<u8> {
        let mut out = Vec::new();
        for p in &self.parts {
            match p {
                Part::Text(t) => out.extend(vars.expand(t).into_bytes()),
                Part::Units(u) => out.extend(u.iter().map(|x| *x as u8)),
            }
        }
        out
    }

    /// Link target with placeholders expanded; `None` for a link whose target is gone.
    pub fn link_target(&self, vars: &Vars) -> Option<String> {
        self.link.as_ref().and_then(|l| l.target.as_deref()).map(|t| vars.expand(t))
    }

    /// Short description for assertion messages.
    pub fn describe(&self, vars: &Vars) -> String {
        format!("#{} [{}] {:?}", self.index, self.category, self.text(vars))
    }
}

/// Load and validate the corpus.
pub fn load() -> Result<Corpus, String> {
    let path = corpus_path();
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text)
}

pub fn parse(text: &str) -> Result<Corpus, String> {
    let raw: RawCorpus = serde_json::from_str(text).map_err(|e| format!("dangerous-path corpus: {e}"))?;
    if raw.schema != SCHEMA {
        return Err(format!("dangerous-path corpus: schema {:?}, expected {SCHEMA:?}", raw.schema));
    }
    let mut entries = Vec::with_capacity(raw.entries.len());
    for (index, e) in raw.entries.into_iter().enumerate() {
        if !raw.categories.contains_key(&e.category) {
            return Err(format!("entry {index}: undeclared category {:?}", e.category));
        }
        let parts = match e.path {
            PathSpec::Text(t) => vec![Part::Text(t)],
            PathSpec::Parts(p) => p,
        };
        for p in &parts {
            match p {
                Part::Text(t) => check_placeholders(t, index)?,
                Part::Units(u) => {
                    let max = if e.platform == Platform::Windows { 0xFFFF } else { 0xFF };
                    if u.is_empty() || u.iter().any(|x| *x > max) {
                        return Err(format!("entry {index}: raw units out of range for {:?}", e.platform));
                    }
                }
            }
        }
        if let Some(t) = e.link.as_ref().and_then(|l| l.target.as_deref()) {
            check_placeholders(t, index)?;
        }
        entries.push(Entry { index, platform: e.platform, category: e.category, parts, deny_by: e.deny_by, link: e.link, model_only: e.model_only, note: e.note });
    }
    Ok(Corpus { categories: raw.categories, entries })
}

impl Corpus {
    pub fn for_platform(&self, platform: Platform) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |e| e.platform == platform)
    }

    /// Entry count per category for one platform.
    pub fn counts(&self, platform: Platform) -> BTreeMap<&str, usize> {
        let mut out = BTreeMap::new();
        for e in self.for_platform(platform) {
            *out.entry(e.category.as_str()).or_insert(0) += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_loads_and_every_category_is_used_on_some_platform() {
        let c = load().unwrap();
        assert!(c.entries.len() >= 300, "{} entries", c.entries.len());
        for cat in c.categories.keys() {
            assert!(c.entries.iter().any(|e| &e.category == cat), "category {cat} has no entries");
        }
        for p in [Platform::Windows, Platform::Macos] {
            assert!(c.for_platform(p).count() >= 120, "{p:?}");
        }
    }

    #[test]
    fn raw_units_and_placeholders_decode() {
        let c = parse(
            r#"{"schema":"cleanmanager.dangerous-paths/1","about":[],"categories":{"x":"x"},"entries":[
            {"platform":"windows","category":"x","path":["{home}\\a",[55296]]},
            {"platform":"macos","category":"x","path":["/a/",[255],"b"],"deny_by":"prefilter"}]}"#,
        )
        .unwrap();
        let v = Vars::new(&[("{home}", "C:\\Users\\alice")]);
        assert_eq!(c.entries[0].windows_units(&v).last(), Some(&0xD800));
        assert_eq!(c.entries[1].unix_bytes(&v), b"/a/\xFFb");
        assert!(parse(r#"{"schema":"cleanmanager.dangerous-paths/1","about":[],"categories":{"x":"x"},"entries":[{"platform":"windows","category":"x","path":"{nope}"}]}"#).is_err());
    }
}
