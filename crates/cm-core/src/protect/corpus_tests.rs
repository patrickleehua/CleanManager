//! PRT-05: dangerous-path corpus and deterministic mutation fuzzing.
//!
//! Each corpus line is checked with [`check_path`] against a small identity model of a
//! machine. The model emulates how the OS resolves aliases (case folding, Win32 separator and
//! trailing-dot normalization, `.`/`..` collapsing, 8.3 short names, volume GUID paths, links),
//! and gives every object an identity derived from its *real* location, so aliases collapse to
//! the same identity exactly as file IDs do on a real volume. Objects that do not exist have no
//! identity (the platform could not open them).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::*;

const GUID_C: &str = "volume{12345678-1234-1234-1234-123456789abc}";

#[derive(Clone)]
enum Raw {
    Win(Vec<u16>),
    Unix(Vec<u8>),
}

impl Raw {
    fn to_raw_path(&self) -> Option<RawPath> {
        match self {
            Raw::Win(u) => RawPath::windows(u.clone()).ok(),
            Raw::Unix(b) => RawPath::unix(b.clone()).ok(),
        }
    }

    fn text(&self) -> String {
        match self {
            Raw::Win(u) => String::from_utf16_lossy(u),
            Raw::Unix(b) => String::from_utf8_lossy(b).into_owned(),
        }
    }
}

struct Line {
    path: Raw,
    /// `Some(None)`: broken link; `Some(Some(t))`: link to `t`.
    link: Option<Option<String>>,
}

fn decode(text: &str, platform: Platform) -> Raw {
    let mut win: Vec<u16> = Vec::new();
    let mut unix: Vec<u8> = Vec::new();
    let mut rest = text;
    if rest == "<EMPTY>" {
        rest = "";
    }
    while !rest.is_empty() {
        if let Some((after, end)) = rest.strip_prefix("<U+").and_then(|a| a.find('>').map(|e| (a, e))) {
            let cp = u32::from_str_radix(&after[..end], 16).expect("hex escape");
            if (0xD800..=0xDFFF).contains(&cp) {
                assert_eq!(platform, Platform::Windows, "lone surrogates only exist on Windows");
                win.push(cp as u16);
            } else {
                let c = char::from_u32(cp).expect("scalar value");
                let mut buf = [0u16; 2];
                win.extend_from_slice(c.encode_utf16(&mut buf));
                let mut b = [0u8; 4];
                unix.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            }
            rest = &after[end + 1..];
            continue;
        }
        if let Some((after, end)) = rest.strip_prefix("<0x").and_then(|a| a.find('>').map(|e| (a, e))) {
            assert_eq!(platform, Platform::Macos, "raw bytes only exist on Unix");
            unix.push(u8::from_str_radix(&after[..end], 16).expect("hex byte"));
            rest = &after[end + 1..];
            continue;
        }
        let c = rest.chars().next().unwrap();
        let mut buf = [0u16; 2];
        win.extend_from_slice(c.encode_utf16(&mut buf));
        let mut b = [0u8; 4];
        unix.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
        rest = &rest[c.len_utf8()..];
    }
    match platform {
        Platform::Windows => Raw::Win(win),
        Platform::Macos => Raw::Unix(unix),
    }
}

fn load_corpus(name: &str, platform: Platform) -> Vec<Line> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fuzz_corpus").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut out = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (p, link) = match line.split_once(" => ") {
            Some((p, "!")) => (p, Some(None)),
            Some((p, t)) => (p, Some(Some(t.to_owned()))),
            None => (line, None),
        };
        out.push(Line { path: decode(p, platform), link });
    }
    out
}

/// A tiny machine: real locations are vectors of folded components, `[root, a, b, ...]`.
struct Model {
    platform: Platform,
    hard: HardDenySet,
    /// Subtrees in which every path exists.
    populated: Vec<Vec<String>>,
    /// Explicit objects; their ancestors exist too.
    existing: HashSet<Vec<String>>,
    links: HashMap<Vec<String>, Option<String>>,
    short_names: HashMap<&'static str, &'static str>,
}

fn join(platform: Platform, comps: &[String]) -> String {
    match platform {
        Platform::Windows if comps.len() == 1 => format!("{}\\", comps[0]),
        Platform::Windows => comps.join("\\"),
        Platform::Macos => format!("/{}", comps[1..].join("/")),
    }
}

impl Model {
    fn windows() -> Self {
        let mut m = Model::new(Platform::Windows);
        m.short_names = HashMap::from([
            ("progra~1", "program files"),
            ("progra~2", "program files (x86)"),
            ("progra~3", "programdata"),
            ("docume~1", "documents and settings"),
            ("system~1", "system volume information"),
            ("$recyc~1", "$recycle.bin"),
            ("config~1.msi", "config.msi"),
        ]);
        use HardDenyClass::*;
        let roots: &[(&str, HardDenyClass)] = &[
            ("C:\\", VolumeRoot),
            ("D:\\", VolumeRoot),
            ("C:\\Windows", SystemRoot),
            ("C:\\Windows\\System32", SystemRoot),
            ("C:\\Windows\\WinSxS", SystemRoot),
            ("C:\\Program Files", ProgramFiles),
            ("C:\\Program Files (x86)", ProgramFiles),
            ("C:\\ProgramData", ProgramDataRoot),
            ("C:\\ProgramData\\Microsoft\\Windows Defender", SecuritySoftware),
            ("C:\\ProgramData\\Microsoft\\Windows Defender Advanced Threat Protection", SecuritySoftware),
            ("C:\\Users", UsersRoot),
            ("C:\\Users\\Default", DefaultProfile),
            ("C:\\Users\\Public", PublicProfile),
            ("C:\\Users\\alice", UserProfileRoot),
            ("C:\\Users\\alice\\AppData\\Roaming\\Microsoft\\Protect", Credentials),
            ("C:\\Users\\alice\\AppData\\Roaming\\Microsoft\\Credentials", Credentials),
            ("C:\\Users\\alice\\AppData\\Local\\Microsoft\\Vault", Credentials),
            ("C:\\Users\\alice\\AppData\\Local\\CleanManager\\store", ToolStore),
            ("C:\\System Volume Information", VolumeMetadata),
            ("D:\\System Volume Information", VolumeMetadata),
            ("C:\\$Recycle.Bin", RecycleBinRoot),
            ("D:\\$Recycle.Bin", RecycleBinRoot),
            ("C:\\Recovery", Recovery),
            ("C:\\Config.Msi", InstallerState),
        ];
        let existing = [
            "C:\\Users\\alice\\AppData\\Local\\Temp\\build-cache",
            "C:\\Users\\alice\\AppData\\Local\\npm-cache\\_cacache",
            "C:\\Users\\alice\\AppData\\Local\\Microsoft\\Windows\\UsrClass.dat",
            "C:\\Users\\alice\\Documents\\report.docx",
            "D:\\cache\\gradle",
            "D:\\links",
            "C:\\Boot\\BCD",
            "C:\\EFI",
            "C:\\pagefile.sys",
        ];
        m.install(roots, &existing, &[("C:\\Documents and Settings", "C:\\Users"), ("C:\\Users\\All Users", "C:\\ProgramData"), ("C:\\Users\\Default User", "C:\\Users\\Default")]);
        m
    }

    fn unix() -> Self {
        let mut m = Model::new(Platform::Macos);
        use HardDenyClass::*;
        let roots: &[(&str, HardDenyClass)] = &[
            ("/", VolumeRoot),
            ("/System", SystemRoot),
            ("/bin", SystemRoot),
            ("/sbin", SystemRoot),
            ("/usr", SystemRoot),
            ("/dev", SystemRoot),
            ("/private/etc", SystemRoot),
            ("/private/var/db", SystemRoot),
            ("/private/var/root", SystemRoot),
            ("/private/var/vm", SystemRoot),
            ("/Library/Keychains", Credentials),
            ("/Library/Application Support/com.apple.TCC", Credentials),
            ("/Users", UsersRoot),
            ("/Users/alice", UserProfileRoot),
            ("/Users/Shared", PublicProfile),
            ("/Users/alice/Library/Keychains", Credentials),
            ("/Users/alice/Library/Application Support/CleanManager/store", ToolStore),
            ("/Users/alice/.Trash", RecycleBinRoot),
            ("/Volumes/Data", VolumeRoot),
            ("/Volumes/Data/.Spotlight-V100", VolumeMetadata),
            ("/Volumes/Data/.fseventsd", VolumeMetadata),
            ("/Volumes/Data/.Trashes", RecycleBinRoot),
            ("/.Spotlight-V100", VolumeMetadata),
            ("/.fseventsd", VolumeMetadata),
        ];
        let existing = [
            "/Users/alice/Library/Caches/com.example.app",
            "/Users/alice/Library/Logs/old.log",
            "/private/var/folders/ab/cd/T/tmp1",
            "/usr/local/Cellar/foo",
            "/opt/homebrew/Cellar",
            "/Applications/Foo.app",
            "/home/bob",
            "/Network/Servers",
            "/cores",
            "/Library/Apple/System",
        ];
        m.install(roots, &existing, &[("/var", "/private/var"), ("/etc", "/private/etc"), ("/tmp", "/private/tmp")]);
        m
    }

    fn new(platform: Platform) -> Self {
        Model { platform, hard: HardDenySet::default(), populated: vec![], existing: HashSet::new(), links: HashMap::new(), short_names: HashMap::new() }
    }

    fn install(&mut self, roots: &[(&str, HardDenyClass)], existing: &[&str], links: &[(&str, &str)]) {
        for (text, class) in roots {
            let comps = self.lexical(text).expect("fixture root");
            let id = self.ident(&comps);
            if class.covers_descendants() {
                self.populated.push(comps.clone());
            }
            self.existing.insert(comps);
            self.hard.insert(HardDenyRoot { class: *class, volume_id: id.volume_id, file_id: id.file_id, display: (*text).to_owned() });
        }
        for text in existing {
            self.existing.insert(self.lexical(text).expect("fixture path"));
        }
        for (l, t) in links {
            self.add_link(l, Some(t.to_string()));
        }
    }

    fn add_link(&mut self, link: &str, target: Option<String>) {
        let comps = self.lexical(link).expect("link path");
        self.links.insert(comps, target);
    }

    /// Lexical OS normalization, without following links.
    fn lexical(&self, text: &str) -> Option<Vec<String>> {
        match self.platform {
            Platform::Windows => {
                let (verbatim, body) = match text.strip_prefix("\\\\?\\") {
                    Some(r) => (true, r.to_owned()),
                    None => (false, text.replace('/', "\\")),
                };
                let lower = body.to_lowercase();
                let (root, rest) = if let Some(r) = lower.strip_prefix("unc\\").filter(|_| verbatim).or_else(|| lower.strip_prefix("\\\\").filter(|_| !verbatim)) {
                    let mut it = r.splitn(3, '\\');
                    let (host, share) = (it.next()?, it.next()?);
                    (format!("\\\\{host}\\{share}"), it.next().unwrap_or("").to_owned())
                } else if let Some(r) = lower.strip_prefix(GUID_C).filter(|_| verbatim) {
                    ("c:".to_owned(), r.to_owned())
                } else {
                    let b = lower.as_bytes();
                    if b.len() < 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' || (b.len() > 2 && b[2] != b'\\') {
                        return None;
                    }
                    (lower[..2].to_owned(), lower[2..].to_owned())
                };
                let mut comps = vec![root];
                for c in rest.split('\\').filter(|c| !c.is_empty()) {
                    let c = if verbatim { c } else { c.trim_end_matches(['.', ' ']) };
                    match c {
                        "" | "." if !verbatim => {}
                        ".." if !verbatim => {
                            if comps.len() > 1 {
                                comps.pop();
                            }
                        }
                        _ => comps.push(self.short_names.get(c).map_or_else(|| c.to_owned(), |l| (*l).to_owned())),
                    }
                }
                Some(comps)
            }
            Platform::Macos => {
                let rest = text.strip_prefix('/')?;
                let mut comps = vec!["/".to_owned()];
                for c in rest.split('/').filter(|c| !c.is_empty() && *c != ".") {
                    if c == ".." {
                        if comps.len() > 1 {
                            comps.pop();
                        }
                    } else {
                        comps.push(c.to_lowercase());
                    }
                }
                Some(comps)
            }
        }
    }

    fn ident(&self, comps: &[String]) -> ObjectIdentity {
        ObjectIdentity::new(format!("vol:{}", comps[0]), join(self.platform, comps))
    }

    fn link_ident(&self, comps: &[String]) -> ObjectIdentity {
        ObjectIdentity::new(format!("vol:{}", comps[0]), format!("link:{}", join(self.platform, comps)))
    }

    fn exists(&self, comps: &[String]) -> bool {
        comps.len() == 1
            || self.links.contains_key(comps)
            || self.populated.iter().any(|p| comps.starts_with(p))
            || self.existing.iter().any(|e| e.starts_with(comps))
    }

    fn raw(&self, comps: &[String]) -> RawPath {
        let text = join(self.platform, comps);
        match self.platform {
            Platform::Windows => RawPath::windows(text.encode_utf16().collect()).unwrap(),
            Platform::Macos => RawPath::unix(text.into_bytes()).unwrap(),
        }
    }

    /// Fully resolved real location of `comps`.
    fn real(&self, comps: &[String], depth: u32) -> Result<Vec<String>, String> {
        if depth > 8 {
            return Err("too many levels of links".into());
        }
        let mut cur = vec![comps[0].clone()];
        for c in &comps[1..] {
            cur.push(c.clone());
            if let Some(t) = self.links.get(&cur) {
                let t = t.as_ref().ok_or("dangling link")?;
                let lex = self.lexical(t).ok_or("bad link target")?;
                cur = self.real(&lex, depth + 1)?;
            }
        }
        Ok(cur)
    }

    fn resolve_target(&self, target: &Option<String>) -> Result<ResolvedTarget, String> {
        let lex = self.lexical(target.as_deref().ok_or("dangling link")?).ok_or("bad link target")?;
        let real = self.real(&lex, 1)?;
        if !self.exists(&real) {
            return Err("link target does not exist".into());
        }
        Ok(ResolvedTarget { path: self.raw(&real), object: self.ident(&real), ancestors: (1..real.len()).map(|i| self.ident(&real[..i])).collect() })
    }

    /// What the platform would report for `text`.
    fn chain(&self, text: &str) -> IdentityChain {
        let mut chain = IdentityChain::default();
        let Some(comps) = self.lexical(text) else { return chain };
        let mut cur = vec![comps[0].clone()];
        for (i, c) in comps.iter().enumerate().skip(1) {
            chain.ancestors.push(Some(self.ident(&cur)));
            cur.push(c.clone());
            let Some(target) = self.links.get(&cur) else { continue };
            let resolved = self.resolve_target(target);
            let on_object = i == comps.len() - 1;
            chain.links.push(LinkResolution { link: self.raw(&cur), on_object, target: resolved.clone() });
            if on_object {
                chain.object = Some(self.link_ident(&cur));
                chain.real_path = Some(self.raw(&cur));
                return chain;
            }
            chain.ancestors.push(Some(self.link_ident(&cur)));
            match resolved {
                Ok(t) => cur = self.lexical(&t.path.lossy()).expect("real path"),
                Err(_) => return chain,
            }
        }
        if self.exists(&cur) {
            chain.real_path = Some(self.raw(&cur));
            chain.object = Some(if self.links.contains_key(&cur) { self.link_ident(&cur) } else { self.ident(&cur) });
        }
        chain
    }

    fn decide(&self, raw: &Raw) -> ProtectionDecision {
        match raw.to_raw_path() {
            None => ProtectionDecision::Denied { layer: DenyLayer::HardDeny, reason: "not representable as a path".into() },
            Some(p) => check_path(&p, &self.chain(&raw.text()), &self.hard),
        }
    }
}

/// Load a corpus, register its links in the model, and return the checked paths.
fn prepare(name: &str, model: &mut Model) -> Vec<Raw> {
    let lines = load_corpus(name, model.platform);
    assert!(lines.len() >= 60, "{name} has only {} entries", lines.len());
    for l in &lines {
        if let Some(t) = &l.link {
            model.add_link(&l.path.text(), t.clone());
        }
    }
    lines.into_iter().map(|l| l.path).collect()
}

fn run_corpus(name: &str, mut model: Model) -> (usize, usize) {
    let paths = prepare(name, &mut model);
    let mut by_identity = 0;
    for p in &paths {
        let d = model.decide(p);
        assert!(d.is_denied(), "{name}: {:?} was allowed", p.text());
        if p.to_raw_path().is_some_and(|r| prefilter(&r).is_ok()) {
            by_identity += 1;
        }
    }
    (paths.len(), by_identity)
}

#[test]
fn protect_corpus_windows_is_denied() {
    let (n, by_identity) = run_corpus("dangerous_paths_windows.txt", Model::windows());
    assert!(by_identity >= 15, "identity-only denials exercise the model ({by_identity})");
    println!("windows corpus: {n} entries denied, {by_identity} by identity only");
}

#[test]
fn protect_corpus_unix_is_denied() {
    let (n, by_identity) = run_corpus("dangerous_paths_unix.txt", Model::unix());
    assert!(by_identity >= 10, "identity-only denials exercise the model ({by_identity})");
    println!("unix corpus: {n} entries denied, {by_identity} by identity only");
}

#[test]
fn protect_corpus_model_allows_ordinary_paths() {
    let w = Model::windows();
    for p in ["C:\\Users\\alice\\AppData\\Local\\Temp\\build-cache", "c:/users/alice/appdata/local/npm-cache/_cacache", "D:\\cache\\gradle", "\\\\?\\C:\\Users\\alice\\AppData\\Local\\Temp\\build-cache"] {
        assert!(w.decide(&decode(p, Platform::Windows)).is_allowed(), "{p}");
    }
    let u = Model::unix();
    for p in ["/Users/alice/Library/Caches/com.example.app", "/private/var/folders/ab/cd/T/tmp1", "/Users/alice/Library/Logs/old.log", "/var/folders/ab/cd/T/tmp1"] {
        assert!(u.decide(&decode(p, Platform::Macos)).is_allowed(), "{p}");
    }
}

// ---- deterministic mutation fuzzing -------------------------------------------------------

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
}

/// Apply one random alias-style mutation to a path, as code units (u16 or u8 widened).
fn mutate(units: &mut Vec<u32>, rng: &mut XorShift, windows: bool) {
    let is_sep = |u: u32| u == '\\' as u32 || u == '/' as u32;
    match rng.below(6) {
        0 => {
            let letters: Vec<usize> = units.iter().enumerate().filter(|(_, u)| char::from_u32(**u).is_some_and(|c| c.is_ascii_alphabetic())).map(|(i, _)| i).collect();
            if !letters.is_empty() {
                units[letters[rng.below(letters.len())]] ^= 0x20;
            }
        }
        1 => {
            let seps: Vec<usize> = units.iter().enumerate().filter(|(_, u)| is_sep(**u)).map(|(i, _)| i).collect();
            if !seps.is_empty() {
                let i = seps[rng.below(seps.len())];
                units[i] = if units[i] == '\\' as u32 { '/' as u32 } else { '\\' as u32 };
            }
        }
        2 => units.push(if rng.below(2) == 0 { '.' as u32 } else { ' ' as u32 }),
        3 => {
            let seps: Vec<usize> = units.iter().enumerate().filter(|(i, u)| *i > 0 && is_sep(**u)).map(|(i, _)| i).collect();
            if !seps.is_empty() {
                let i = seps[rng.below(seps.len())];
                units.insert(i, if rng.below(2) == 0 { '.' as u32 } else { ' ' as u32 });
            }
        }
        4 => {
            let prefix: Vec<u32> = "\\\\?\\".chars().map(|c| c as u32).collect();
            units.splice(0..0, prefix);
        }
        _ => {
            let seps: Vec<usize> = units.iter().enumerate().filter(|(_, u)| is_sep(**u)).map(|(i, _)| i).collect();
            if !seps.is_empty() {
                let i = seps[rng.below(seps.len())];
                let sep = if windows { units[i] } else { '/' as u32 };
                units.insert(i, sep);
            }
        }
    }
}

fn widen(raw: &Raw) -> Vec<u32> {
    match raw {
        Raw::Win(u) => u.iter().map(|&x| u32::from(x)).collect(),
        Raw::Unix(b) => b.iter().map(|&x| u32::from(x)).collect(),
    }
}

fn narrow(units: &[u32], windows: bool) -> Raw {
    if windows { Raw::Win(units.iter().map(|&x| x as u16).collect()) } else { Raw::Unix(units.iter().map(|&x| x as u8).collect()) }
}

fn fuzz(name: &str, mut model: Model, seed: u64) -> usize {
    let windows = model.platform == Platform::Windows;
    let paths = prepare(name, &mut model);
    let mut rng = XorShift(seed);
    let mut checked = 0;
    for p in &paths {
        for _ in 0..64 {
            let mut units = widen(p);
            for _ in 0..=rng.below(3) {
                mutate(&mut units, &mut rng, windows);
            }
            let m = narrow(&units, windows);
            let d = model.decide(&m);
            assert!(d.is_denied(), "{name}: mutation {:?} of {:?} was allowed", m.text(), p.text());
            checked += 1;
        }
    }
    checked
}

#[test]
fn protect_fuzz_windows_mutations_stay_denied() {
    let n = fuzz("dangerous_paths_windows.txt", Model::windows(), 0x9E37_79B9_7F4A_7C15);
    println!("windows fuzz: {n} mutations denied");
}

#[test]
fn protect_fuzz_unix_mutations_stay_denied() {
    let n = fuzz("dangerous_paths_unix.txt", Model::unix(), 0xD1B5_4A32_D192_ED03);
    println!("unix fuzz: {n} mutations denied");
}

#[test]
fn protect_fuzz_ordinary_paths_survive_case_and_separator_changes() {
    // The filter is not "deny everything": benign aliases of ordinary paths stay allowed.
    let w = Model::windows();
    let mut rng = XorShift(7);
    for p in ["C:\\Users\\alice\\AppData\\Local\\Temp\\build-cache", "D:\\cache\\gradle"] {
        for _ in 0..64 {
            let mut units = widen(&decode(p, Platform::Windows));
            for _ in 0..3 {
                let mut r = XorShift(rng.next() | 1);
                // case flips (op 0) and separator swaps (op 1) only
                let before = units.clone();
                mutate(&mut units, &mut r, true);
                if units.len() != before.len() {
                    units = before;
                }
            }
            let m = narrow(&units, true);
            assert!(w.decide(&m).is_allowed(), "{:?}", m.text());
        }
    }
    let u = Model::unix();
    for p in ["/Users/alice/Library/Caches/com.example.app", "/private/var/folders/ab/cd/T/tmp1"] {
        for _ in 0..64 {
            let mut units = widen(&decode(p, Platform::Macos));
            let letters: Vec<usize> = units.iter().enumerate().filter(|(_, u)| (**u as u8).is_ascii_alphabetic()).map(|(i, _)| i).collect();
            units[letters[rng.below(letters.len())]] ^= 0x20;
            let m = narrow(&units, false);
            assert!(u.decide(&m).is_allowed(), "{:?}", m.text());
        }
    }
}
