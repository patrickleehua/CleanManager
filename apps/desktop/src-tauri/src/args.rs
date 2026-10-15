//! IPC argument validation. Every value arriving from the WebView is untrusted: identifiers,
//! sort/filter words, page sizes and cursors are parsed here into typed values before any
//! store call, and nothing is coerced silently (an out-of-range page size is an error, not a
//! clamp — IDX-02).

use cm_store::index::{DEFAULT_PAGE, KindFilter, MAX_PAGE, Sort};
use serde::Serialize;

/// Upper bound of treemap tiles requested per layer (one directory level, D11).
pub const MAX_TREEMAP_LIMIT: u32 = 200;
pub const DEFAULT_TREEMAP_LIMIT: u32 = 60;
/// Selection summaries are bounded like pages.
pub const MAX_SUMMARY_IDS: usize = 500;
const MAX_NAME_FILTER_CHARS: usize = 256;
const MAX_CURSOR_LEN: usize = 4096;
const MAX_ID_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArgError(pub String);

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

type R<T> = Result<T, ArgError>;

fn err<T>(msg: impl Into<String>) -> R<T> {
    Err(ArgError(msg.into()))
}

pub fn parse_sort(s: &str) -> R<Sort> {
    match s {
        "logical_desc" => Ok(Sort::LogicalDesc),
        "allocated_desc" => Ok(Sort::AllocatedDesc),
        "name_asc" => Ok(Sort::NameAsc),
        other => err(format!("unknown sort {other:?}; expected logical_desc, allocated_desc or name_asc")),
    }
}

pub fn parse_kind(s: &str) -> R<KindFilter> {
    match s {
        "any" => Ok(KindFilter::Any),
        "file" => Ok(KindFilter::File),
        "directory" => Ok(KindFilter::Directory),
        other => err(format!("unknown kind filter {other:?}; expected any, file or directory")),
    }
}

/// `None` means the protocol default (50). Zero and anything above 500 are rejected.
pub fn page_size(n: Option<u32>) -> R<u32> {
    match n {
        None => Ok(DEFAULT_PAGE),
        Some(v) if (1..=MAX_PAGE).contains(&v) => Ok(v),
        Some(v) => err(format!("page_size {v} is outside 1..={MAX_PAGE}")),
    }
}

pub fn treemap_limit(n: Option<u32>) -> R<u32> {
    match n {
        None => Ok(DEFAULT_TREEMAP_LIMIT),
        Some(v) if (1..=MAX_TREEMAP_LIMIT).contains(&v) => Ok(v),
        Some(v) => err(format!("limit {v} is outside 1..={MAX_TREEMAP_LIMIT}")),
    }
}

/// Entry IDs travel as decimal strings (they are u64 in the index). No sign, no leading zero,
/// no whitespace, never zero.
pub fn entry_id(s: &str) -> R<u64> {
    if s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0')) {
        return err(format!("entry id must be a positive decimal string, got {s:?}"));
    }
    match s.parse::<u64>() {
        Ok(0) | Err(_) => err(format!("entry id out of range: {s:?}")),
        Ok(v) => Ok(v),
    }
}

/// Scan IDs come from the store (CLI `s_<request>` or desktop `s_desk_...`).
pub fn scan_id(s: &str) -> R<String> {
    if s.is_empty() || s.len() > MAX_ID_LEN || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.') {
        return err("scan id must be 1-128 characters of [A-Za-z0-9_.-]");
    }
    Ok(s.to_owned())
}

/// Opaque root tokens handed out by `list_scan_roots` / `choose_folder`.
pub fn root_token(s: &str) -> R<String> {
    if s.is_empty() || s.len() > 32 || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return err("root token is malformed");
    }
    Ok(s.to_owned())
}

/// Name filter: trimmed; empty means no filter; control characters are rejected.
pub fn name_filter(s: Option<&str>) -> R<Option<String>> {
    let Some(raw) = s else { return Ok(None) };
    let t = raw.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.chars().count() > MAX_NAME_FILTER_CHARS {
        return err(format!("name filter longer than {MAX_NAME_FILTER_CHARS} characters"));
    }
    if t.chars().any(char::is_control) {
        return err("name filter contains control characters");
    }
    Ok(Some(t.to_owned()))
}

/// Cursors are opaque base64url tokens from a previous page; the store validates the binding.
pub fn cursor(s: Option<&str>) -> R<Option<String>> {
    match s {
        None => Ok(None),
        Some("") => err("cursor must be omitted, not empty"),
        Some(c) if c.len() > MAX_CURSOR_LEN => err("cursor is too long"),
        Some(c) if !c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') => err("cursor is malformed"),
        Some(c) => Ok(Some(c.to_owned())),
    }
}

pub fn summary_ids(ids: &[String]) -> R<Vec<u64>> {
    if ids.is_empty() {
        return err("select at least one entry");
    }
    if ids.len() > MAX_SUMMARY_IDS {
        return err(format!("at most {MAX_SUMMARY_IDS} entries per summary request"));
    }
    ids.iter().map(|s| entry_id(s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_size_defaults_and_bounds() {
        assert_eq!(page_size(None), Ok(50));
        assert_eq!(page_size(Some(1)), Ok(1));
        assert_eq!(page_size(Some(500)), Ok(500));
        assert!(page_size(Some(0)).is_err());
        assert!(page_size(Some(501)).is_err(), "501 is a parameter error, never clamped");
        assert!(page_size(Some(u32::MAX)).is_err());
    }

    #[test]
    fn treemap_limit_bounds() {
        assert_eq!(treemap_limit(None), Ok(DEFAULT_TREEMAP_LIMIT));
        assert_eq!(treemap_limit(Some(200)), Ok(200));
        assert!(treemap_limit(Some(0)).is_err());
        assert!(treemap_limit(Some(201)).is_err());
    }

    #[test]
    fn sort_words_parse_exactly() {
        assert_eq!(parse_sort("logical_desc"), Ok(Sort::LogicalDesc));
        assert_eq!(parse_sort("allocated_desc"), Ok(Sort::AllocatedDesc));
        assert_eq!(parse_sort("name_asc"), Ok(Sort::NameAsc));
        for bad in ["", "LOGICAL_DESC", "size", "name_desc", " logical_desc"] {
            assert!(parse_sort(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn kind_words_parse_exactly() {
        assert_eq!(parse_kind("any"), Ok(KindFilter::Any));
        assert_eq!(parse_kind("file"), Ok(KindFilter::File));
        assert_eq!(parse_kind("directory"), Ok(KindFilter::Directory));
        for bad in ["", "dir", "File", "symlink"] {
            assert!(parse_kind(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn entry_ids_are_strict_decimal_u64() {
        assert_eq!(entry_id("1"), Ok(1));
        assert_eq!(entry_id("18446744073709551615"), Ok(u64::MAX));
        for bad in ["", "0", "01", "-1", "+1", "1.0", " 1", "18446744073709551616", "1e3", "abc"] {
            assert!(entry_id(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn scan_ids_and_tokens_reject_path_like_input() {
        assert!(scan_id("s_desk_1760000000_42").is_ok());
        assert!(scan_id("s_01J9ZX").is_ok());
        for bad in ["", "../x", "s 1", "s\\1", "s/1", &"x".repeat(129)] {
            assert!(scan_id(bad).is_err(), "{bad:?}");
        }
        assert!(root_token("r3").is_ok());
        assert!(root_token("C:\\Users").is_err());
    }

    #[test]
    fn name_filter_trims_and_rejects_control_chars() {
        assert_eq!(name_filter(None), Ok(None));
        assert_eq!(name_filter(Some("   ")), Ok(None));
        assert_eq!(name_filter(Some(" cache ")), Ok(Some("cache".into())));
        assert!(name_filter(Some("a\u{0}b")).is_err());
        assert!(name_filter(Some("a\nb")).is_err());
        assert!(name_filter(Some(&"x".repeat(257))).is_err());
        assert!(name_filter(Some(&"字".repeat(256))).is_ok(), "limit counts characters, not bytes");
    }

    #[test]
    fn cursor_must_be_base64url() {
        assert_eq!(cursor(None), Ok(None));
        assert!(cursor(Some("")).is_err());
        assert!(cursor(Some("eyJ2IjoxfQ")).is_ok());
        assert!(cursor(Some("a+b/c=")).is_err());
        assert!(cursor(Some(&"a".repeat(4097))).is_err());
    }

    #[test]
    fn summary_ids_are_bounded() {
        assert!(summary_ids(&[]).is_err());
        assert_eq!(summary_ids(&["1".into(), "7".into()]), Ok(vec![1, 7]));
        assert!(summary_ids(&vec!["1".to_string(); 501]).is_err());
        assert!(summary_ids(&["1".into(), "x".into()]).is_err());
    }
}
