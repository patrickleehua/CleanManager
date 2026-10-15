//! A small model of a Windows and a macOS machine for the dangerous-path corpus.
//!
//! The model reports what the platform backends would report for a path (an [`IdentityChain`]):
//! it emulates how the OS resolves aliases (case folding as measured on NTFS / Unicode folding
//! and NFD as on APFS, Win32 separator and trailing-dot/space normalization, `.`/`..`
//! collapsing, 8.3 short names, volume GUID paths, loopback SMB shares, firmlinks, junctions,
//! symlinks and mount points) and gives every object an identity derived from its *real*
//! location, so aliases collapse to one identity exactly as file IDs do. Objects that do not
//! exist have no identity. Hard-deny roots mirror the platform candidate lists
//! (`cm-platform` `windows/hard_deny.rs`, `macos/hard_deny.rs`).

use std::collections::{HashMap, HashSet};

use cm_fixtures::dangerous_paths::Vars;

use crate::capability::Platform;
use crate::path::RawPath;
use crate::protect::{HardDenyClass, HardDenyRoot, HardDenySet, IdentityChain, LinkResolution, ObjectIdentity, ProtectionDecision, ResolvedTarget, check_path};

pub const GUID_C: &str = "volume{12345678-1234-1234-1234-123456789abc}";
pub const GUID_D: &str = "volume{87654321-4321-4321-4321-cba987654321}";
pub const WIN_TMP: &str = "C:\\Users\\alice\\AppData\\Local\\Temp\\cm-corpus";
pub const MAC_TMP: &str = "/Users/alice/Library/Caches/cm-corpus";

/// NTFS name comparison as measured on Windows 11: ASCII letters fold among themselves, and no
/// non-ASCII unit folds onto ASCII (dotless i, long s and the Kelvin sign stay distinct).
pub fn fold_ntfs(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii() {
                return c.to_ascii_lowercase();
            }
            let mut up = c.to_uppercase();
            match (up.next(), up.next()) {
                (Some(u), None) if !u.is_ascii() => u,
                _ => c,
            }
        })
        .collect()
}

fn decompose(c: char) -> Option<(char, char)> {
    const GRAVE: char = '\u{300}';
    const ACUTE: char = '\u{301}';
    const CIRC: char = '\u{302}';
    const TILDE: char = '\u{303}';
    const DIAER: char = '\u{308}';
    const RING: char = '\u{30A}';
    const CEDILLA: char = '\u{327}';
    Some(match c {
        'à' => ('a', GRAVE),
        'á' => ('a', ACUTE),
        'â' => ('a', CIRC),
        'ã' => ('a', TILDE),
        'ä' => ('a', DIAER),
        'å' => ('a', RING),
        'ç' => ('c', CEDILLA),
        'è' => ('e', GRAVE),
        'é' => ('e', ACUTE),
        'ê' => ('e', CIRC),
        'ë' => ('e', DIAER),
        'ì' => ('i', GRAVE),
        'í' => ('i', ACUTE),
        'î' => ('i', CIRC),
        'ï' => ('i', DIAER),
        'ñ' => ('n', TILDE),
        'ò' => ('o', GRAVE),
        'ó' => ('o', ACUTE),
        'ô' => ('o', CIRC),
        'õ' => ('o', TILDE),
        'ö' => ('o', DIAER),
        'ù' => ('u', GRAVE),
        'ú' => ('u', ACUTE),
        'û' => ('u', CIRC),
        'ü' => ('u', DIAER),
        _ => return None,
    })
}

/// APFS case-insensitive comparison, modelled generously (more aliases = stricter test):
/// Unicode full case folding (long s → s, ß → ss, Kelvin → k, İ → i̇) and NFD for the
/// precomposed Latin letters the corpus uses.
pub fn fold_apfs(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let lower: String = match c {
            'ſ' => "s".into(),
            'ß' | 'ẞ' => "ss".into(),
            'ﬁ' => "fi".into(),
            'ﬂ' => "fl".into(),
            _ => c.to_lowercase().collect(),
        };
        for l in lower.chars() {
            match decompose(l) {
                Some((base, mark)) => {
                    out.push(base);
                    out.push(mark);
                }
                None => out.push(l),
            }
        }
    }
    out
}

/// A lexically normalized path: folded components `[root, a, b, ...]`, the last component as
/// given (macOS real paths keep it), and the UNC form when reached through a loopback share.
#[derive(Debug, Clone)]
pub struct Lexed {
    pub comps: Vec<String>,
    pub last_name: String,
    /// `\\?\UNC\host\share` and how many local components the share stands for.
    pub unc: Option<(String, usize)>,
}

pub struct Model {
    pub platform: Platform,
    pub hard: HardDenySet,
    pub vars: Vars,
    roots: Vec<(Vec<String>, HardDenyClass)>,
    /// Subtrees in which every path exists.
    populated: Vec<Vec<String>>,
    /// Explicit objects; their ancestors exist too.
    existing: HashSet<Vec<String>>,
    links: HashMap<Vec<String>, Option<String>>,
    short_names: HashMap<&'static str, &'static str>,
    /// Loopback shares `(host, share)` → local folder they publish.
    shares: HashMap<(&'static str, &'static str), &'static str>,
    next_link: u32,
}

pub fn join(platform: Platform, comps: &[String]) -> String {
    match platform {
        Platform::Windows if comps.len() == 1 => format!("{}\\", comps[0]),
        Platform::Windows => comps.join("\\"),
        Platform::Macos => format!("/{}", comps[1..].join("/")),
    }
}

impl Model {
    pub fn windows() -> Self {
        let mut m = Model::new(Platform::Windows, Vars::new(&[("{home}", "C:\\Users\\alice"), ("{user}", "alice"), ("{tmp}", WIN_TMP), ("{sysvol}", "\\\\?\\Volume{12345678-1234-1234-1234-123456789abc}\\")]));
        m.short_names = HashMap::from([
            ("progra~1", "program files"),
            ("progra~2", "program files (x86)"),
            ("progra~3", "programdata"),
            ("docume~1", "documents and settings"),
            ("system~1", "system volume information"),
            ("$recyc~1", "$recycle.bin"),
            ("config~1.msi", "config.msi"),
            ("micros~1", "microsoft"),
            ("window~1", "windows defender"),
            ("cleanm~1", "cleanmanager"),
            ("ntuser~1.log", "ntuser.dat.log1"),
            ("usrcla~1.log", "usrclass.dat.log1"),
            ("$windo~1.bt", "$windows.~bt"),
        ]);
        m.shares = HashMap::new();
        for host in ["localhost", "127.0.0.1"] {
            for (share, local) in [("c$", "C:\\"), ("d$", "D:\\"), ("admin$", "C:\\Windows"), ("users", "C:\\Users"), ("cshare", "C:\\"), ("winshare", "C:\\Windows")] {
                m.shares.insert((host, share), local);
            }
        }
        use HardDenyClass::*;
        let mut roots: Vec<(String, HardDenyClass)> = Vec::new();
        let mut add = |p: &str, c| roots.push((p.to_owned(), c));
        for p in ["C:\\Windows", "C:\\Windows\\System32", "C:\\Windows\\SysWOW64", "C:\\Windows\\WinSxS"] {
            add(p, SystemRoot);
        }
        add("C:\\Program Files", ProgramFiles);
        add("C:\\Program Files (x86)", ProgramFiles);
        add("C:\\ProgramData", ProgramDataRoot);
        for p in ["Windows Defender", "Windows Defender Advanced Threat Protection", "Windows Security Health"] {
            add(&format!("C:\\ProgramData\\Microsoft\\{p}"), SecuritySoftware);
        }
        add("C:\\ProgramData\\Microsoft\\Crypto", Credentials);
        add("C:\\Users", UsersRoot);
        add("C:\\Users\\Default", DefaultProfile);
        add("C:\\Users\\Public", PublicProfile);
        for profile in ["C:\\Users\\alice", "C:\\Users\\jos\u{e9}"] {
            for p in ["Protect", "Credentials", "SystemCertificates", "Crypto", "Vault"] {
                add(&format!("{profile}\\AppData\\Roaming\\Microsoft\\{p}"), Credentials);
            }
            add(&format!("{profile}\\AppData\\Local\\Microsoft\\Credentials"), Credentials);
            add(&format!("{profile}\\AppData\\Local\\Microsoft\\Vault"), Credentials);
            add(profile, UserProfileRoot);
        }
        for v in ["C:", "D:"] {
            add(&format!("{v}\\"), VolumeRoot);
            add(&format!("{v}\\System Volume Information"), VolumeMetadata);
            add(&format!("{v}\\Recovery"), Recovery);
            add(&format!("{v}\\Config.Msi"), InstallerState);
            add(&format!("{v}\\.cleanmanager-quarantine"), ToolStore);
            add(&format!("{v}\\$Recycle.Bin"), RecycleBinRoot);
        }
        add("C:\\$Recycle.Bin\\S-1-5-21-1-2-3-1001", RecycleBinRoot);
        add("C:\\Users\\alice\\AppData\\Local\\CleanManager\\store", ToolStore);
        let existing = [
            "C:\\Users\\alice\\AppData\\Local\\Temp\\cm-corpus\\plain\\x",
            "C:\\Users\\alice\\AppData\\Local\\Temp\\build-cache",
            "C:\\Users\\alice\\AppData\\Local\\npm-cache\\_cacache",
            "C:\\Users\\alice\\AppData\\Local\\Microsoft\\Windows\\UsrClass.dat",
            "C:\\Users\\alice\\AppData\\Local\\Microsoft\\Windows\\UsrClass.dat.LOG1",
            "C:\\Users\\alice\\AppData\\Local\\Microsoft\\Windows\\INetCookies\\x",
            "C:\\Users\\alice\\AppData\\LocalLow\\x",
            "C:\\Users\\alice\\NTUSER.DAT",
            "C:\\Users\\alice\\ntuser.dat.LOG1",
            "C:\\Users\\alice\\Documents\\report.docx",
            "C:\\Users\\jos\u{e9}\\AppData\\Local\\Temp\\x",
            "D:\\cache\\gradle",
            "D:\\links",
            "C:\\mnt",
            "C:\\Boot\\BCD",
            "C:\\EFI\\Microsoft\\Boot",
            "C:\\pagefile.sys",
            "C:\\hiberfil.sys",
        ];
        let links = [
            ("C:\\Documents and Settings", "C:\\Users"),
            ("C:\\Users\\All Users", "C:\\ProgramData"),
            ("C:\\Users\\Default User", "C:\\Users\\Default"),
            ("C:\\Users\\alice\\Application Data", "C:\\Users\\alice\\AppData\\Roaming"),
            ("C:\\Users\\alice\\Local Settings", "C:\\Users\\alice\\AppData\\Local"),
            ("C:\\Users\\alice\\AppData\\Local\\Application Data", "C:\\Users\\alice\\AppData\\Local"),
            ("C:\\Users\\alice\\My Documents", "C:\\Users\\alice\\Documents"),
            ("C:\\Users\\alice\\Cookies", "C:\\Users\\alice\\AppData\\Local\\Microsoft\\Windows\\INetCookies"),
            ("C:\\ProgramData\\Application Data", "C:\\ProgramData"),
            ("C:\\ProgramData\\Desktop", "C:\\Users\\Public\\Desktop"),
        ];
        m.install(&roots, &existing, &links);
        m
    }

    pub fn macos() -> Self {
        let mut m = Model::new(Platform::Macos, Vars::new(&[("{home}", "/Users/alice"), ("{user}", "alice"), ("{tmp}", MAC_TMP)]));
        use HardDenyClass::*;
        let mut roots: Vec<(String, HardDenyClass)> = Vec::new();
        let mut add = |p: &str, c| roots.push((p.to_owned(), c));
        for p in ["/System", "/bin", "/sbin", "/dev", "/private/etc", "/private/var/db", "/private/var/root", "/private/var/vm", "/Library/Apple"] {
            add(p, SystemRoot);
        }
        for p in ["/usr", "/usr/local", "/private", "/private/var", "/private/var/folders", "/private/tmp", "/Library", "/opt/homebrew"] {
            add(p, SystemContainer);
        }
        for p in ["bin", "lib", "libexec", "sbin", "share", "standalone"] {
            add(&format!("/usr/{p}"), SystemRoot);
        }
        add("/Library/Keychains", Credentials);
        add("/Library/Application Support/com.apple.TCC", Credentials);
        add("/Library/SystemExtensions", SecuritySoftware);
        add("/Library/Updates", InstallerState);
        add("/Users", UsersRoot);
        add("/Users/Shared", PublicProfile);
        add("/", VolumeRoot);
        add("/System/Volumes/Data", VolumeRoot);
        for p in ["/.Spotlight-V100", "/.fseventsd", "/.DocumentRevisions-V100"] {
            add(p, VolumeMetadata);
        }
        add("/Volumes/Data", VolumeRoot);
        for p in [".Spotlight-V100", ".fseventsd", ".DocumentRevisions-V100"] {
            add(&format!("/Volumes/Data/{p}"), VolumeMetadata);
        }
        add("/Volumes/Data/.Trashes", RecycleBinRoot);
        for profile in ["/Users/alice", "/Users/jos\u{e9}"] {
            add(&format!("{profile}/Library/Keychains"), Credentials);
            add(&format!("{profile}/Library/Application Support/com.apple.TCC"), Credentials);
            add(&format!("{profile}/Library"), SystemContainer);
            add(&format!("{profile}/.Trash"), RecycleBinRoot);
            add(profile, UserProfileRoot);
        }
        add("/Users/alice/Library/Application Support/CleanManager/store", ToolStore);
        let existing = [
            "/Users/alice/Library/Caches/cm-corpus/plain/x",
            "/Users/alice/Library/Caches/com.example.app",
            "/Users/alice/Library/Logs/old.log",
            "/Users/alice/Documents/report.pages",
            "/private/var/folders/ab/cd/T/tmp1",
            "/private/etc/passwd",
            "/usr/local/Cellar/foo",
            "/opt/homebrew/Cellar",
            "/Applications/Foo.app",
            "/home/bob",
            "/Network/Servers",
            "/cores",
        ];
        m.install(&roots, &existing, &[("/var", "/private/var"), ("/etc", "/private/etc"), ("/tmp", "/private/tmp")]);
        m
    }

    fn new(platform: Platform, vars: Vars) -> Self {
        Model {
            platform,
            hard: HardDenySet::default(),
            vars,
            roots: vec![],
            populated: vec![],
            existing: HashSet::new(),
            links: HashMap::new(),
            short_names: HashMap::new(),
            shares: HashMap::new(),
            next_link: 0,
        }
    }

    fn install(&mut self, roots: &[(String, HardDenyClass)], existing: &[&str], links: &[(&str, &str)]) {
        for (text, class) in roots {
            let comps = self.lexical(text).expect("fixture root").comps;
            let id = self.ident(&comps);
            if class.covers_descendants() {
                self.populated.push(comps.clone());
            }
            self.existing.insert(comps.clone());
            self.roots.push((comps, *class));
            self.hard.insert(HardDenyRoot { class: *class, volume_id: id.volume_id, file_id: id.file_id, display: text.clone() });
        }
        for text in existing {
            let comps = self.lexical(text).expect("fixture path").comps;
            self.existing.insert(comps);
        }
        for (l, t) in links {
            self.add_link(l, Some((*t).to_owned()));
        }
    }

    /// Register a link (`None`: its target is gone).
    pub fn add_link(&mut self, link: &str, target: Option<String>) {
        let comps = self.lexical(link).expect("link path").comps;
        self.links.insert(comps, target);
    }

    /// A fresh link inside the scratch directory pointing at `target`; returns its path text.
    pub fn new_link(&mut self, target: &str) -> String {
        self.next_link += 1;
        let sep = if self.platform == Platform::Windows { "\\" } else { "/" };
        let tmp = if self.platform == Platform::Windows { WIN_TMP } else { MAC_TMP };
        let link = format!("{tmp}{sep}fz{}", self.next_link);
        self.add_link(&link, Some(target.to_owned()));
        link
    }

    /// Lexical OS normalization of an absolute path, without following links.
    pub fn lexical(&self, text: &str) -> Option<Lexed> {
        self.lexical_from(text, None)
    }

    /// As [`Model::lexical`]; a relative `text` (a link target) is taken relative to `base`.
    fn lexical_from(&self, text: &str, base: Option<&[String]>) -> Option<Lexed> {
        match self.platform {
            Platform::Windows => self.lex_windows(text, base),
            Platform::Macos => self.lex_macos(text, base),
        }
    }

    fn lex_windows(&self, text: &str, base: Option<&[String]>) -> Option<Lexed> {
        let (verbatim, body) = match text.strip_prefix("\\\\?\\") {
            Some(r) => (true, r.to_owned()),
            None => (false, text.replace('/', "\\")),
        };
        let lower = body.to_ascii_lowercase();
        let mut unc = None;
        let (mut comps, rest): (Vec<String>, String) = if let Some(r) = lower.strip_prefix("unc\\").filter(|_| verbatim).or_else(|| lower.strip_prefix("\\\\").filter(|_| !verbatim)) {
            let start = body.len() - r.len();
            let mut it = body[start..].splitn(3, '\\');
            let (host, share) = (fold_ntfs(it.next()?), fold_ntfs(it.next()?));
            if host.is_empty() || share.is_empty() || host == "." || host == "?" {
                return None;
            }
            let rest = it.next().unwrap_or("").to_owned();
            match self.shares.iter().find(|((h, s), _)| *h == host && *s == share) {
                Some((_, local)) => {
                    let base = self.lexical(local)?.comps;
                    unc = Some((format!("\\\\?\\UNC\\{host}\\{share}"), base.len()));
                    (base, rest)
                }
                None => (vec![format!("\\\\{host}\\{share}")], rest),
            }
        } else if verbatim && (lower.starts_with(GUID_C) || lower.starts_with(GUID_D)) {
            let tail = &body[GUID_C.len()..];
            if !(tail.is_empty() || tail.starts_with('\\')) {
                return None;
            }
            (vec![if lower.starts_with(GUID_C) { "c:".to_owned() } else { "d:".to_owned() }], tail.to_owned())
        } else {
            let b = lower.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                if b.len() > 2 && b[2] != b'\\' || (b.len() == 2 && !verbatim) {
                    return None;
                }
                (vec![lower[..2].to_owned()], body[2..].to_owned())
            } else {
                let base = base.filter(|_| !verbatim && !body.starts_with('\\'))?;
                (base.to_vec(), body.clone())
            }
        };
        let mut names = comps.clone();
        for c in rest.split('\\').filter(|c| !c.is_empty()) {
            let c = if verbatim { c } else { c.trim_end_matches(['.', ' ']) };
            match c {
                "" | "." if !verbatim => {}
                ".." if !verbatim => {
                    if comps.len() > 1 {
                        comps.pop();
                        names.pop();
                    }
                    if let Some((_, n)) = &mut unc {
                        *n = (*n).min(comps.len());
                    }
                }
                _ => {
                    let f = fold_ntfs(c);
                    comps.push(self.short_names.get(f.as_str()).map_or(f, |l| (*l).to_owned()));
                    names.push(c.to_owned());
                }
            }
        }
        Some(Lexed { last_name: names.pop().unwrap_or_default(), comps, unc })
    }

    fn lex_macos(&self, text: &str, base: Option<&[String]>) -> Option<Lexed> {
        let (mut comps, rest) = match text.strip_prefix('/') {
            Some(r) => (vec!["/".to_owned()], r),
            None => (base?.to_vec(), text),
        };
        let mut names = comps.clone();
        for c in rest.split('/').filter(|c| !c.is_empty() && *c != ".") {
            if c == ".." {
                if comps.len() > 1 {
                    comps.pop();
                    names.pop();
                }
            } else {
                comps.push(fold_apfs(c));
                names.push(c.to_owned());
            }
        }
        // Firmlinks: the data volume's user-visible folders are the same objects as `/<name>`.
        if comps.len() > 4 && comps[1] == "system" && comps[2] == "volumes" && comps[3] == "data" {
            comps.drain(1..4);
        }
        Some(Lexed { last_name: names.pop().unwrap_or_default(), comps, unc: None })
    }

    pub fn ident(&self, comps: &[String]) -> ObjectIdentity {
        ObjectIdentity::new(format!("vol:{}", comps[0]), join(self.platform, comps))
    }

    fn link_ident(&self, comps: &[String]) -> ObjectIdentity {
        ObjectIdentity::new(format!("vol:{}", comps[0]), format!("link:{}", join(self.platform, comps)))
    }

    fn exists(&self, comps: &[String]) -> bool {
        let local_root = !comps[0].starts_with("\\\\");
        (comps.len() == 1 && local_root) || self.links.contains_key(comps) || self.populated.iter().any(|p| comps.starts_with(p)) || self.existing.iter().any(|e| e.starts_with(comps))
    }

    fn raw(&self, text: &str) -> RawPath {
        match self.platform {
            Platform::Windows => RawPath::windows(text.encode_utf16().collect()).unwrap(),
            Platform::Macos => RawPath::unix(text.as_bytes().to_vec()).unwrap(),
        }
    }

    fn raw_comps(&self, comps: &[String]) -> RawPath {
        self.raw(&join(self.platform, comps))
    }

    /// Fully resolved real location of `comps` (every link followed, including the last).
    pub fn real(&self, comps: &[String], depth: u32) -> Result<Vec<String>, String> {
        if depth > 8 {
            return Err("too many levels of links".into());
        }
        let mut cur = vec![comps[0].clone()];
        for c in &comps[1..] {
            cur.push(c.clone());
            if let Some(t) = self.links.get(&cur) {
                let t = t.as_ref().ok_or("dangling link")?;
                let lex = self.lexical_from(t, Some(&cur[..cur.len() - 1])).ok_or("bad link target")?;
                cur = self.real(&lex.comps, depth + 1)?;
            }
        }
        Ok(cur)
    }

    fn resolve_target(&self, link: &[String], target: &Option<String>) -> Result<ResolvedTarget, String> {
        let lex = self.lexical_from(target.as_deref().ok_or("dangling link")?, Some(&link[..link.len() - 1])).ok_or("bad link target")?;
        let real = self.real(&lex.comps, 1)?;
        if !self.exists(&real) {
            return Err("link target does not exist".into());
        }
        Ok(ResolvedTarget { path: self.raw_comps(&real), object: self.ident(&real), ancestors: (1..real.len()).map(|i| self.ident(&real[..i])).collect() })
    }

    /// What the platform would report for `text`.
    pub fn chain(&self, text: &str) -> IdentityChain {
        let mut chain = IdentityChain::default();
        let Some(lex) = self.lexical(text) else { return chain };
        let comps = &lex.comps;
        let mut cur = vec![comps[0].clone()];
        let mut crossed_link = false;
        for (i, c) in comps.iter().enumerate().skip(1) {
            chain.ancestors.push(Some(self.ident(&cur)));
            cur.push(c.clone());
            let Some(target) = self.links.get(&cur) else { continue };
            crossed_link = true;
            let resolved = self.resolve_target(&cur, target);
            let on_object = i == comps.len() - 1;
            chain.links.push(LinkResolution { link: self.raw_comps(&cur), on_object, target: resolved.clone() });
            if on_object {
                chain.object = Some(self.link_ident(&cur));
                chain.real_path = Some(self.raw_comps(&cur));
                return chain;
            }
            chain.ancestors.push(Some(self.link_ident(&cur)));
            match resolved {
                Ok(t) => cur = self.lexical(&t.path.lossy()).expect("real path").comps,
                Err(_) => return chain,
            }
        }
        if !self.exists(&cur) {
            return chain;
        }
        chain.object = Some(if self.links.contains_key(&cur) { self.link_ident(&cur) } else { self.ident(&cur) });
        chain.real_path = Some(match (self.platform, &lex.unc) {
            // Over SMB the final path keeps the share form.
            (Platform::Windows, Some((prefix, n))) if !crossed_link => {
                let tail = cur[*n..].join("\\");
                self.raw(&if tail.is_empty() { format!("{prefix}\\") } else { format!("{prefix}\\{tail}") })
            }
            // realpath(parent) + the last name as given.
            (Platform::Macos, _) if cur.len() > 1 && !crossed_link => {
                let parent = join(self.platform, &cur[..cur.len() - 1]);
                self.raw(&format!("{}/{}", parent.trim_end_matches('/'), lex.last_name))
            }
            _ => self.raw_comps(&cur),
        });
        chain
    }

    /// Hard-deny layer for one raw path.
    pub fn decide(&self, raw: Option<RawPath>, text: &str) -> ProtectionDecision {
        match raw {
            None => ProtectionDecision::Denied { layer: crate::protect::DenyLayer::HardDeny, reason: "not representable as a path".into() },
            Some(p) => check_path(&p, &self.chain(text), &self.hard),
        }
    }

    /// Oracle for the property test: the object's fully resolved real location is a hard-deny
    /// root or lies inside one that covers its descendants. `None` when the path names nothing
    /// the model can resolve.
    pub fn resolves_into_hard_deny(&self, text: &str) -> Option<bool> {
        let lex = self.lexical(text)?;
        let real = self.real(&lex.comps, 0).ok()?;
        if !self.exists(&real) {
            return None;
        }
        Some(self.roots.iter().any(|(r, class)| real == *r || (class.covers_descendants() && real.starts_with(r))))
    }

    /// Text of every hard-deny root (for fuzz seeds).
    pub fn root_texts(&self) -> Vec<(String, HardDenyClass)> {
        self.hard.roots().map(|r| (r.display.clone(), r.class)).collect()
    }
}
