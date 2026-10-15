//! PRT-03/04/05: the dual-platform dangerous-path corpus (`tests/corpus/dangerous-paths.json`)
//! against a modelled machine, mutation fuzzing of every corpus entry, and a property test that
//! no path resolving inside a hard-deny root is ever `Allowed`.
//!
//! The fuzzers use a fixed-seed xorshift generator, so every run checks the same cases and a
//! failure names the exact input; no fuzzing dependency is needed.

mod model;

use std::collections::BTreeMap;

use cm_fixtures::dangerous_paths::{self as corpus, DenyBy, Entry};

use self::model::{GUID_C, GUID_D, MAC_TMP, Model, WIN_TMP};
use super::*;

fn corpus_platform(p: Platform) -> corpus::Platform {
    match p {
        Platform::Windows => corpus::Platform::Windows,
        Platform::Macos => corpus::Platform::Macos,
    }
}

fn raw_of_entry(e: &Entry, m: &Model) -> Option<RawPath> {
    match m.platform {
        Platform::Windows => RawPath::windows(e.windows_units(&m.vars)).ok(),
        Platform::Macos => RawPath::unix(e.unix_bytes(&m.vars)).ok(),
    }
}

fn load_into(m: &mut Model) -> Vec<Entry> {
    let c = corpus::load().unwrap_or_else(|e| panic!("{e}"));
    let entries: Vec<Entry> = c.for_platform(corpus_platform(m.platform)).cloned().collect();
    for e in &entries {
        if let Some(l) = &e.link {
            m.add_link(&e.text(&m.vars), e.link_target(&m.vars));
            assert!(l.target.is_some() || e.link_target(&m.vars).is_none());
        }
    }
    entries
}

/// Check every entry; returns counts per category and how many were denied by identity only.
fn run_corpus(mut m: Model) -> (BTreeMap<String, usize>, usize) {
    let entries = load_into(&mut m);
    let mut counts = BTreeMap::new();
    let mut by_identity = 0;
    let mut failures = Vec::new();
    for e in &entries {
        let raw = raw_of_entry(e, &m);
        let text = e.text(&m.vars);
        let textual = raw.as_ref().map_or(Err(TextualDeny::Empty), prefilter);
        let decision = m.decide(raw, &text);
        *counts.entry(e.category.clone()).or_insert(0) += 1;
        if decision.is_allowed() {
            failures.push(format!("ALLOWED {}", e.describe(&m.vars)));
            continue;
        }
        match (e.deny_by, &textual) {
            (DenyBy::Prefilter, Ok(())) => failures.push(format!("expected the textual filter to deny {} ({decision:?})", e.describe(&m.vars))),
            (DenyBy::Identity, Err(t)) => failures.push(format!("expected only identity to deny {}, but the textual filter did: {t}", e.describe(&m.vars))),
            _ => {}
        }
        if textual.is_ok() {
            by_identity += 1;
        }
    }
    assert!(failures.is_empty(), "{} corpus failures:\n{}", failures.len(), failures.join("\n"));
    (counts, by_identity)
}

#[test]
fn protect_corpus_windows_is_denied() {
    let (counts, by_identity) = run_corpus(Model::windows());
    let n: usize = counts.values().sum();
    assert!(n >= 350 && by_identity >= 60, "{n} entries, {by_identity} by identity");
    println!("windows corpus: {n} entries denied ({by_identity} by identity only): {counts:?}");
}

#[test]
fn protect_corpus_macos_is_denied() {
    let (counts, by_identity) = run_corpus(Model::macos());
    let n: usize = counts.values().sum();
    assert!(n >= 200 && by_identity >= 25, "{n} entries, {by_identity} by identity");
    println!("macos corpus: {n} entries denied ({by_identity} by identity only): {counts:?}");
}

/// The filter is not "deny everything": ordinary paths, including aliases of ordinary paths,
/// stay allowed in the same model.
#[test]
fn protect_corpus_model_allows_ordinary_paths() {
    let w = Model::windows();
    for p in [
        format!("{WIN_TMP}\\plain\\x"),
        "C:\\Users\\alice\\AppData\\Local\\Temp\\build-cache".into(),
        "c:/users/alice/appdata/local/npm-cache/_cacache".into(),
        "D:\\cache\\gradle".into(),
        format!("\\\\?\\{WIN_TMP}\\plain\\x"),
        "\\\\?\\Volume{12345678-1234-1234-1234-123456789abc}\\Users\\alice\\AppData\\Local\\Temp\\build-cache".into(),
        "\\\\localhost\\Users\\alice\\Documents\\report.docx".into(),
        "C:\\Users\\alice\\My Documents\\report.docx".into(),
        "C:\\Users\\alice\\AppData\\Local\\Microsoft\\Windows\\INetCookies\\x".into(),
        "C:\\Users\\alice\\AppData\\LocalLow\\x".into(),
    ] {
        let d = w.decide(RawPath::windows(p.encode_utf16().collect()).ok(), &p);
        assert!(d.is_allowed(), "{p}: {d:?}");
    }
    let u = Model::macos();
    for p in [
        format!("{MAC_TMP}/plain/x"),
        "/Users/alice/Library/Caches/com.example.app".into(),
        "/private/var/folders/ab/cd/T/tmp1".into(),
        "/Users/alice/Library/Logs/old.log".into(),
        "/usr/local/Cellar/foo".into(),
        "/opt/homebrew/Cellar".into(),
        "/Users/alice/Documents/report.pages".into(),
    ] {
        let d = u.decide(RawPath::unix(p.as_bytes().to_vec()).ok(), &p);
        assert!(d.is_allowed(), "{p}: {d:?}");
    }
}

// ---- deterministic generator ------------------------------------------------------------------

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        (!items.is_empty()).then(|| &items[self.below(items.len())])
    }
}

// ---- mutation fuzzing of corpus entries ---------------------------------------------------

/// Apply one alias-style mutation to a path, as code units (u16 or u8 widened).
fn mutate_units(units: &mut Vec<u32>, rng: &mut XorShift, windows: bool) {
    let is_sep = |u: u32| u == '\\' as u32 || u == '/' as u32;
    let positions = |units: &[u32], f: &dyn Fn(usize, u32) -> bool| units.iter().enumerate().filter(|(i, u)| f(*i, **u)).map(|(i, _)| i).collect::<Vec<_>>();
    match rng.below(6) {
        0 => {
            let letters = positions(units, &|_, u| char::from_u32(u).is_some_and(|c| c.is_ascii_alphabetic()));
            if let Some(&i) = rng.pick(&letters) {
                units[i] ^= 0x20;
            }
        }
        1 => {
            if let Some(&i) = rng.pick(&positions(units, &|_, u| is_sep(u))) {
                units[i] = if units[i] == '\\' as u32 { '/' as u32 } else { '\\' as u32 };
            }
        }
        2 => units.push(if rng.below(2) == 0 { '.' as u32 } else { ' ' as u32 }),
        3 => {
            if let Some(&i) = rng.pick(&positions(units, &|i, u| i > 0 && is_sep(u))) {
                units.insert(i, if rng.below(2) == 0 { '.' as u32 } else { ' ' as u32 });
            }
        }
        4 => {
            units.splice(0..0, "\\\\?\\".chars().map(|c| c as u32));
        }
        _ => {
            if let Some(&i) = rng.pick(&positions(units, &|_, u| is_sep(u))) {
                let sep = if windows { units[i] } else { '/' as u32 };
                units.insert(i, sep);
            }
        }
    }
}

fn fuzz_corpus(mut m: Model, seed: u64) -> usize {
    let windows = m.platform == Platform::Windows;
    let entries = load_into(&mut m);
    let mut rng = XorShift(seed);
    let mut checked = 0;
    for e in &entries {
        let base: Vec<u32> = if windows { e.windows_units(&m.vars).into_iter().map(u32::from).collect() } else { e.unix_bytes(&m.vars).into_iter().map(u32::from).collect() };
        for _ in 0..48 {
            let mut units = base.clone();
            for _ in 0..=rng.below(3) {
                mutate_units(&mut units, &mut rng, windows);
            }
            let (raw, text) = if windows {
                let u: Vec<u16> = units.iter().map(|&x| x as u16).collect();
                (RawPath::windows(u.clone()).ok(), String::from_utf16_lossy(&u))
            } else {
                let b: Vec<u8> = units.iter().map(|&x| x as u8).collect();
                (RawPath::unix(b.clone()).ok(), String::from_utf8_lossy(&b).into_owned())
            };
            let d = m.decide(raw, &text);
            assert!(d.is_denied(), "mutation {text:?} of {} was allowed", e.describe(&m.vars));
            checked += 1;
        }
    }
    checked
}

#[test]
fn protect_fuzz_windows_corpus_mutations_stay_denied() {
    let n = fuzz_corpus(Model::windows(), 0x9E37_79B9_7F4A_7C15);
    println!("windows corpus mutations: {n} denied");
}

#[test]
fn protect_fuzz_macos_corpus_mutations_stay_denied() {
    let n = fuzz_corpus(Model::macos(), 0xD1B5_4A32_D192_ED03);
    println!("macos corpus mutations: {n} denied");
}

// ---- property: nothing that resolves inside a hard-deny root is allowed ---------------------

/// Byte offset of every component start (after a separator) and of the path end.
fn split_points(path: &str, seps: &[char]) -> Vec<usize> {
    path.char_indices().filter(|(_, c)| seps.contains(c)).map(|(i, _)| i).chain([path.len()]).collect()
}

/// Rewrite `path` into an alias of (usually) the same object, or a near-miss, as the OS or a
/// caller could spell it. Windows-only and macOS-only spellings are picked by the model.
fn alias(path: &str, m: &mut Model, rng: &mut XorShift) -> String {
    let windows = m.platform == Platform::Windows;
    let sep = if windows { '\\' } else { '/' };
    let seps: &[char] = if windows { &['\\', '/'] } else { &['/'] };
    let mut s = path.to_owned();
    let replace_prefix = |s: &str, from: &str, to: &str| -> Option<String> {
        (s.len() >= from.len() && s.is_char_boundary(from.len()) && s[..from.len()].eq_ignore_ascii_case(from)).then(|| format!("{to}{}", &s[from.len()..]))
    };
    match rng.below(if windows { 16 } else { 12 }) {
        // ASCII case flip
        0 => {
            let letters: Vec<usize> = s.char_indices().filter(|(_, c)| c.is_ascii_alphabetic()).map(|(i, _)| i).collect();
            if let Some(&i) = rng.pick(&letters) {
                let c = s.as_bytes()[i] ^ 0x20;
                s.replace_range(i..=i, &(c as char).to_string());
            }
        }
        // doubled separator
        1 => {
            let pts: Vec<usize> = split_points(&s, seps).into_iter().filter(|&i| i < s.len() && i > 2).collect();
            if let Some(&i) = rng.pick(&pts) {
                s.insert(i, sep);
            }
        }
        // `.` component
        2 => {
            let pts: Vec<usize> = split_points(&s, seps).into_iter().filter(|&i| i < s.len() && i > 2).collect();
            if let Some(&i) = rng.pick(&pts) {
                s.insert_str(i, &format!("{sep}."));
            }
        }
        // `zz/..` detour
        3 => {
            let pts: Vec<usize> = split_points(&s, seps).into_iter().filter(|&i| i > 2).collect();
            if let Some(&i) = rng.pick(&pts) {
                s.insert_str(i, &format!("{sep}zz{sep}.."));
            }
        }
        // a descendant
        4 => {
            s.push(sep);
            s.push_str(["cache", "x.tmp", "Sub Dir", "a.b.c"][rng.below(4)]);
        }
        // link substitution: a fresh link to some prefix, then the rest below it
        5 | 6 => {
            let pts: Vec<usize> = split_points(&s, seps).into_iter().filter(|&i| i > 3).collect();
            if let Some(&i) = rng.pick(&pts) {
                let prefix = s[..i].to_owned();
                let rest = s[i..].to_owned();
                let mut link = m.new_link(&prefix);
                if rng.below(3) == 0 {
                    link = m.new_link(&link);
                }
                s = format!("{link}{rest}");
            }
        }
        // trailing slash (macOS) / trailing dot or space on a component (Windows)
        7 => {
            if windows {
                let pts: Vec<usize> = split_points(&s, seps).into_iter().filter(|&i| i > 3).collect();
                if let Some(&i) = rng.pick(&pts) {
                    s.insert(i, if rng.below(2) == 0 { '.' } else { ' ' });
                }
            } else {
                s.push('/');
            }
        }
        // Unicode case variants: long s / Kelvin sign (fold on APFS, distinct on NTFS), Turkish i
        8 => {
            let cands: Vec<(usize, char)> = s.char_indices().filter(|(_, c)| matches!(c, 's' | 'S' | 'k' | 'K' | 'i' | 'I')).collect();
            if let Some(&(i, c)) = rng.pick(&cands) {
                let r = match c {
                    's' | 'S' => '\u{17F}',
                    'k' | 'K' => '\u{212A}',
                    'i' => '\u{131}',
                    _ => '\u{130}',
                };
                s.replace_range(i..i + 1, &r.to_string());
            }
        }
        // macOS firmlink / private aliases
        9 if !windows => {
            for (from, to) in [("/private/var", "/var"), ("/private/etc", "/etc"), ("/private/tmp", "/tmp"), ("/var", "/private/var"), ("/Users", "/System/Volumes/Data/Users"), ("/Library", "/System/Volumes/Data/Library")] {
                if let Some(n) = replace_prefix(&s, from, to).filter(|n| n.len() == to.len() || n[to.len()..].starts_with('/')) {
                    s = n;
                    break;
                }
            }
        }
        // NFD of a precomposed letter
        10 if !windows => s = s.replace('\u{e9}', "e\u{301}"),
        11 if !windows => s = s.replace("e\u{301}", "\u{e9}"),
        // verbatim prefix
        9 => {
            if s.as_bytes().get(1) == Some(&b':') {
                s = format!("\\\\?\\{s}");
            }
        }
        // volume GUID instead of the drive letter
        10 => {
            if let Some(n) = replace_prefix(&s, "C:", &format!("\\\\?\\{}", GUID_C)).or_else(|| replace_prefix(&s, "D:", &format!("\\\\?\\{}", GUID_D))) {
                s = n;
            }
        }
        // loopback share of a local folder
        11 => {
            let host = ["localhost", "127.0.0.1", "LOCALHOST"][rng.below(3)];
            for (from, share) in [("C:\\Windows", "winshare"), ("C:\\Users", "Users"), ("C:\\", "cshare"), ("D:\\", "d$")] {
                if let Some(n) = replace_prefix(&s, from, &format!("\\\\{host}\\{share}")) {
                    s = n;
                    break;
                }
            }
        }
        // 8.3 short names
        12 => {
            for (long, short) in [("Program Files (x86)", "PROGRA~2"), ("Program Files", "PROGRA~1"), ("ProgramData", "PROGRA~3"), ("Microsoft", "MICROS~1"), ("System Volume Information", "SYSTEM~1"), ("CleanManager", "CLEANM~1"), ("Windows Defender", "WINDOW~1")] {
                if let Some(i) = s.find(long) {
                    s.replace_range(i..i + long.len(), short);
                    break;
                }
            }
        }
        // compatibility junctions
        13 => {
            for (from, to) in [
                ("C:\\Users\\alice\\AppData\\Roaming", "C:\\Users\\alice\\Application Data"),
                ("C:\\Users\\alice\\AppData\\Local", "C:\\Users\\alice\\Local Settings"),
                ("C:\\ProgramData", "C:\\Users\\All Users"),
                ("C:\\Users", "C:\\Documents and Settings"),
            ] {
                if let Some(n) = replace_prefix(&s, from, to) {
                    s = n;
                    break;
                }
            }
        }
        // mount point of the data volume
        14 => {
            if let Some(n) = replace_prefix(&s, "D:\\", "C:\\mnt\\data\\") {
                s = n;
            }
        }
        // forward slashes
        _ => s = s.replace('\\', "/"),
    }
    s
}

fn property(mut m: Model, seed: u64, rounds: usize) -> (usize, usize) {
    if m.platform == Platform::Windows {
        m.add_link("C:\\mnt\\data", Some(format!("\\\\?\\{GUID_D}\\")));
    }
    let mut seeds: Vec<String> = Vec::new();
    for (text, class) in m.root_texts() {
        let sep = if m.platform == Platform::Windows { "\\" } else { "/" };
        seeds.push(text.clone());
        if class.covers_descendants() {
            seeds.push(format!("{}{sep}child{sep}leaf.dat", text.trim_end_matches(sep)));
        }
    }
    seeds.sort();
    let mut rng = XorShift(seed);
    let (mut checked, mut inside) = (0, 0);
    for s in &seeds {
        assert_eq!(m.resolves_into_hard_deny(s), Some(true), "seed {s} must resolve into a hard-deny root");
        for _ in 0..rounds {
            let mut p = s.clone();
            for _ in 0..=rng.below(4) {
                p = alias(&p, &mut m, &mut rng);
            }
            let raw = match m.platform {
                Platform::Windows => RawPath::windows(p.encode_utf16().collect()).ok(),
                Platform::Macos => RawPath::unix(p.as_bytes().to_vec()).ok(),
            };
            checked += 1;
            if m.resolves_into_hard_deny(&p) == Some(true) {
                inside += 1;
                let d = m.decide(raw, &p);
                assert!(d.is_denied(), "{p:?} (from {s:?}) resolves inside a hard-deny root but was allowed");
            }
        }
    }
    (checked, inside)
}

#[test]
fn protect_property_windows_nothing_inside_hard_deny_is_allowed() {
    let (checked, inside) = property(Model::windows(), 0x2545_F491_4F6C_DD1D, 160);
    assert!(inside * 3 >= checked, "the generator must mostly stay inside hard-deny roots: {inside}/{checked}");
    println!("windows property: {checked} generated, {inside} resolved inside a hard-deny root, all denied");
}

#[test]
fn protect_property_macos_nothing_inside_hard_deny_is_allowed() {
    let (checked, inside) = property(Model::macos(), 0x9FB2_1C65_1E98_DF25, 160);
    assert!(inside * 3 >= checked, "the generator must mostly stay inside hard-deny roots: {inside}/{checked}");
    println!("macos property: {checked} generated, {inside} resolved inside a hard-deny root, all denied");
}

#[test]
fn protect_fuzz_ordinary_paths_survive_case_and_separator_changes() {
    // Benign aliases of ordinary paths stay allowed (the filter is not "deny everything").
    let w = Model::windows();
    let mut rng = XorShift(7);
    for p in ["C:\\Users\\alice\\AppData\\Local\\Temp\\build-cache", "D:\\cache\\gradle", "\\\\localhost\\Users\\alice\\Documents\\report.docx"] {
        for _ in 0..64 {
            let mut s = p.to_owned();
            for _ in 0..3 {
                let letters: Vec<usize> = s.char_indices().filter(|(_, c)| c.is_ascii_alphabetic()).map(|(i, _)| i).collect();
                let i = letters[rng.below(letters.len())];
                let c = s.as_bytes()[i] ^ 0x20;
                s.replace_range(i..=i, &(c as char).to_string());
                if rng.below(2) == 0 && s.starts_with("C:") {
                    s = s.replacen('\\', "/", 1 + rng.below(3));
                }
            }
            let d = w.decide(RawPath::windows(s.encode_utf16().collect()).ok(), &s);
            assert!(d.is_allowed(), "{s:?}: {d:?}");
        }
    }
    let u = Model::macos();
    for p in ["/Users/alice/Library/Caches/com.example.app", "/private/var/folders/ab/cd/T/tmp1"] {
        for _ in 0..64 {
            let mut s = p.to_owned();
            let letters: Vec<usize> = s.char_indices().filter(|(_, c)| c.is_ascii_alphabetic()).map(|(i, _)| i).collect();
            let i = letters[rng.below(letters.len())];
            let c = s.as_bytes()[i] ^ 0x20;
            s.replace_range(i..=i, &(c as char).to_string());
            let d = u.decide(RawPath::unix(s.as_bytes().to_vec()).ok(), &s);
            assert!(d.is_allowed(), "{s:?}: {d:?}");
        }
    }
}
