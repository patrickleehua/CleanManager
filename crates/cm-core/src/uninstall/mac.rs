//! macOS decisions of the uninstall flow (UNI-01..11 on macOS; design D7, D16 N3, docs/platforms/macos.md §8).
//!
//! - Routes ([`select`]): a plain app bundle is moved to the user's Trash (`macos.trash-app-bundle`, never deleted:
//!   N3 has no permanent mode); a Homebrew cask runs `brew uninstall --cask <token>` and a formula `brew uninstall
//!   --formula <name>` (fixed argv, the name restricted so it can never read as an option; `--zap` is never used:
//!   Homebrew's zap may remove shared resources, §8). App Store apps and bundles this user cannot change (root-owned,
//!   or in a folder only an administrator may change, which is where installer packages put them) would need
//!   administrator rights; this build has no administrator path on macOS, so they are refused as
//!   `permission_required` with the Finder or Launchpad route, never escalated.
//! - Protected classes ([`protected_refusal`], UNI-10): apps that are part of macOS (`/System/Applications` or the
//!   sealed system volume), security or device-management agents and CleanManager itself have no uninstall entry.
//! - Leftovers ([`leftovers`], UNI-04/05/09): candidates under `~/Library` found by bundle-ID evidence (exact name or
//!   `<bundle id>.` prefix, launchd job label or program inside the removed bundle) or Team-ID evidence (Group
//!   Containers). A match by the app's name alone is `review_only` and never preselected. `~/Library/Containers` is
//!   privacy-protected (App Data) and is only reported (`blocked`, manual); Group Containers are shared by design
//!   (`shared_owner`, kept). Only caches and logs are preselected; personal data and settings never are. Files go to
//!   the Trash (`fs_trash`); there are no registry steps on macOS.
//! - AI command-line tools ([`ai_leftovers`], UNI-07): npm-global packages are removed with `npm uninstall --global
//!   --prefix <their prefix> --ignore-scripts <package>` through a trusted Node.js. Their data folder (`~/.claude`,
//!   `~/.codex`, `~/.gemini`) is offered per entry: sessions, history, settings and credentials are never preselected
//!   (ANA-03: each is reviewed and named on its own), a cache folder that holds session data is treated as session
//!   data, git worktrees are `blocked`, and while another installation of the same tool remains everything is
//!   `shared_owner`.
//! - Orphans ([`orphan_leftovers`], UNI-08): `~/Library` entries named by a bundle ID that no installed app (any
//!   inventory source) and no installer receipt has, idle for [`ORPHAN_IDLE_DAYS`]; Apple's `com.apple.*` never is
//!   one. Always unselected; weaker ties (same developer prefix, same name as an installed app, recent use) keep them
//!   with `owner_unknown` / `shared_owner`.

use super::*;
use crate::apps::HomebrewKind;

/// macOS facts of one installed instance, carried in its [`UninstallRecord`] (and therefore in the plan).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacRecord {
    /// `CFBundleIdentifier` of the bundle (casks and plain bundles; formulae have none).
    pub bundle_id: Option<String>,
    /// Team ID the bundle's signature claims (not validated trust).
    pub team_id: Option<String>,
    /// Homebrew package kind for `homebrew` records.
    pub homebrew_kind: Option<HomebrewKind>,
    /// Location identity of the bundle when it was listed (`ids` spelling): the Trash move acts only on this object.
    pub volume_id: Option<String>,
    pub file_id: Option<String>,
    /// Why this user cannot move the bundle (root-owned, folder not writable); `None` = it can.
    pub admin_reason: Option<String>,
    /// Installer package receipts naming the bundle ID (evidence only, never a file list).
    pub pkg_receipts: Vec<String>,
    /// npm-global AI CLIs (UNI-07): the npm global prefix the package was found under (`<prefix>/lib/node_modules`).
    /// Absent for bundles and Homebrew, so their records serialize, and digest, exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm_prefix: Option<String>,
}

/// Where App Store apps are removed.
pub const APP_STORE_GUIDANCE: &str = "Mac App Store apps belong to the system installer and need administrator rights to remove; this build has no administrator path on macOS. Remove it in Launchpad (hold Option and click the delete button) or with Finder › Move to Trash";
/// Where bundles this user cannot change are removed.
pub const FINDER_GUIDANCE: &str = "this build has no administrator path on macOS and never asks for one; quit the app and use Finder › Move to Trash (Finder asks for an administrator password), or the vendor's own uninstaller";
/// System Settings pane for login and background items (Service Management has no public enumeration API).
pub const LOGIN_ITEMS_URI: &str = "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";

fn refuse_mac(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { code, message: message.into(), settings_uri: None }
}

/// A Homebrew token or formula name that can only be read as a name (never an option, a path or a tap reference).
pub fn safe_brew_name(s: &str) -> bool {
    crate::updates::mac::valid_brew_name(s) && !s.contains('/')
}

/// The Team ID inside a publisher text as `apps` writes it (`Name (TEAMID)` or a bare Team ID).
pub fn team_id_of(publisher: &str) -> Option<String> {
    let t = publisher.trim();
    let candidate = match t.strip_suffix(')').and_then(|r| r.rsplit_once('(')) {
        Some((_, inner)) => inner,
        None => t,
    };
    (candidate.len() == 10 && candidate.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())).then(|| candidate.to_owned())
}

/// UNI-10 on macOS: no uninstall entry at all for CleanManager, parts of macOS and security/management agents.
pub fn protected_refusal(rec: &UninstallRecord) -> Option<Refusal> {
    let lname = rec.name.to_lowercase();
    let bid = rec.mac.as_ref().and_then(|m| m.bundle_id.as_deref()).unwrap_or("").to_lowercase();
    let loc = rec.install_location.as_deref().unwrap_or("").to_lowercase();
    if lname.contains("cleanmanager") || bid.contains("cleanmanager") || loc.trim_end_matches('/').ends_with("/cleanmanager.app") {
        return Some(refuse_mac("policy_denied", "CleanManager never uninstalls itself; quit it and move it to the Trash in Finder"));
    }
    let why = match rec.protected_class? {
        ProtectedReason::OsPackage => "Part of macOS (in /System/Applications or on the sealed system volume): it cannot be removed",
        ProtectedReason::EndpointSecurityOrManagement => {
            "Security or device-management agent: CleanManager offers no uninstall; remove it through your administrator or the vendor's official uninstall process"
        }
        ProtectedReason::SystemComponent => "System component: part of macOS or of another product, not uninstalled separately",
        ProtectedReason::SharedRuntime => "Shared runtime other applications depend on: remove it only with its own uninstaller when nothing needs it",
    };
    Some(refuse_mac("policy_denied", why))
}

/// UNI-01 on macOS: the route of one record (after [`protected_refusal`]).
pub fn select(rec: &UninstallRecord, env: &AdapterEnv) -> Result<AdapterChoice, Refusal> {
    let Some(mac) = rec.mac.as_ref() else { return Err(refuse_mac("unsupported", "the macOS bundle facts of this record are missing")) };
    let choice = |kind, program: String, argv: Vec<String>, display: String| AdapterChoice {
        kind, program, argv, raw_args: false, publisher_required: None, microsoft_signed: false, display, keeps_own_ui: false,
    };
    match rec.source {
        RecordSource::AppStore => Err(refuse_mac("permission_required", APP_STORE_GUIDANCE)),
        RecordSource::Homebrew => {
            let kind = mac.homebrew_kind.ok_or_else(|| refuse_mac("unsupported", "the Homebrew package kind is unknown"))?;
            let (flag, adapter, word) = match kind {
                HomebrewKind::Cask => ("--cask", AdapterKind::HomebrewCask, "cask"),
                HomebrewKind::Formula => ("--formula", AdapterKind::HomebrewFormula, "formula"),
            };
            let name = rec.package_name.as_deref().filter(|n| safe_brew_name(n)).ok_or_else(|| refuse_mac("unsupported", "the Homebrew package name is not recognized"))?;
            let brew = env.brew.clone().ok_or_else(|| {
                refuse_mac("unsupported", format!("installed by Homebrew, but a trusted `brew` was not found (/opt/homebrew/bin/brew or /usr/local/bin/brew, owned by you or root); run `brew uninstall {flag} {name}` yourself"))
            })?;
            Ok(choice(adapter, brew, vec!["uninstall".into(), flag.into(), name.to_owned()], format!("brew uninstall {flag} {name} (package manager; the {word}'s zap stanza is never run)")))
        }
        RecordSource::AppBundle => {
            if let Some(why) = &mac.admin_reason {
                return Err(refuse_mac("permission_required", format!("{why}: moving it needs administrator rights; {FINDER_GUIDANCE}")));
            }
            let bundle = rec.install_location.clone().filter(|l| l.starts_with('/') && l.trim_end_matches('/').to_ascii_lowercase().ends_with(".app"));
            let Some(bundle) = bundle else { return Err(refuse_mac("unsupported", "the app bundle location is not recorded")) };
            if mac.volume_id.is_none() || mac.file_id.is_none() {
                return Err(refuse_mac("unsupported", "the bundle's identity could not be read, so the Trash move could not be bound to it; use Finder › Move to Trash"));
            }
            let mut display = format!("move {bundle} to the Trash (recoverable with Finder › Put Back; never deleted)");
            if !mac.pkg_receipts.is_empty() {
                display.push_str("; it came with an installer package, whose other components are not removed");
            }
            Ok(choice(AdapterKind::MacTrashAppBundle, bundle, vec![], display))
        }
        RecordSource::NpmGlobal => {
            let pkg = rec.package_name.as_deref().filter(|p| safe_package_name(p)).ok_or_else(|| refuse_mac("unsupported", "npm package name is not recognized"))?;
            let prefix = mac
                .npm_prefix
                .as_deref()
                .filter(|p| p.starts_with('/') && !p.split('/').any(|c| c == ".." || c == "."))
                .ok_or_else(|| refuse_mac("unsupported", "the npm global prefix of this package is not recorded"))?;
            let (node, npm) = env.node.clone().ok_or_else(|| {
                refuse_mac(
                    "unsupported",
                    format!("a trusted Node.js with npm was not found (Homebrew, /usr/local, nvm or Volta, owned by you or root); run `npm uninstall -g {pkg}` yourself"),
                )
            })?;
            // The prefix is passed explicitly: the runner executes the resolved `node` (Homebrew's lives in its Cellar), from
            // which npm would otherwise derive a different global prefix. Package scripts are never run.
            let argv = vec![npm, "uninstall".into(), "--global".into(), "--prefix".into(), prefix.to_owned(), "--ignore-scripts".into(), pkg.to_owned()];
            Ok(choice(AdapterKind::NpmGlobal, node, argv, format!("npm uninstall --global {pkg} (package manager; global prefix {prefix})")))
        }
        _ => Err(refuse_mac("unsupported", "not a macOS source")),
    }
}

// ---------------------------------------------------------------------------------------------
// UNI-02: processes never ended on macOS

/// Process names that are never ended, whatever a plan says.
pub const CRITICAL_PROCESSES: &[&str] = &["launchd", "kernel_task", "windowserver", "loginwindow", "finder", "dock", "systemuiserver", "controlcenter"];

/// UNI-02 on macOS: why a process may not be ended (`None` = may be ended after a person confirmed it).
pub fn end_refusal(p: &RunningProcess, own_pid: u32) -> Option<&'static str> {
    let name = p.name.to_lowercase();
    if p.pid == own_pid || name.starts_with("cleanmanager") || name.starts_with("cm-desktop") {
        return Some("CleanManager never ends itself");
    }
    if p.pid <= 1 || CRITICAL_PROCESSES.contains(&name.as_str()) {
        return Some("system-critical process");
    }
    match &p.path {
        None => Some("process image unknown"),
        Some(path) if ["/System/", "/usr/", "/bin/", "/sbin/", "/Library/Apple/"].iter().any(|r| path.starts_with(r)) => Some("part of macOS"),
        Some(_) => None,
    }
}

// ---------------------------------------------------------------------------------------------
// UNI-04/05/09: leftovers under ~/Library

/// The `~/Library` folders leftovers are looked for in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Area {
    ApplicationSupport,
    Caches,
    Logs,
    Preferences,
    ByHost,
    SavedState,
    HttpStorages,
    WebKit,
    LaunchAgents,
    Containers,
    GroupContainers,
}

impl Area {
    pub const ALL: [Area; 11] = [
        Area::ApplicationSupport, Area::Caches, Area::Logs, Area::Preferences, Area::ByHost, Area::SavedState, Area::HttpStorages, Area::WebKit, Area::LaunchAgents,
        Area::Containers, Area::GroupContainers,
    ];

    /// Folder below `~/Library`.
    pub fn rel(self) -> &'static str {
        match self {
            Area::ApplicationSupport => "Application Support",
            Area::Caches => "Caches",
            Area::Logs => "Logs",
            Area::Preferences => "Preferences",
            Area::ByHost => "Preferences/ByHost",
            Area::SavedState => "Saved Application State",
            Area::HttpStorages => "HTTPStorages",
            Area::WebKit => "WebKit",
            Area::LaunchAgents => "LaunchAgents",
            Area::Containers => "Containers",
            Area::GroupContainers => "Group Containers",
        }
    }

    /// Privacy-protected (App Data) and shared: only names are read there, never contents or sizes.
    pub fn privacy_protected(self) -> bool {
        matches!(self, Area::Containers | Area::GroupContainers)
    }

    fn group(self) -> Group {
        match self {
            Area::Caches | Area::Logs => Group::CachesLogs,
            Area::Preferences | Area::ByHost | Area::SavedState => Group::Settings,
            Area::LaunchAgents => Group::Startup,
            Area::ApplicationSupport | Area::HttpStorages | Area::WebKit | Area::Containers | Area::GroupContainers => Group::PersonalData,
        }
    }
}

/// One entry of a `~/Library` area as the platform listed it (names only; launchd plists also parsed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    pub area: Area,
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// LaunchAgents: the job's `Label` and its program (`Program`, else `ProgramArguments[0]`).
    pub launch_label: Option<String>,
    pub launch_program: Option<String>,
}

/// Another installed macOS instance (for `shared_owner` / `app_reappeared`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacInstalled {
    pub instance_id: String,
    pub name: String,
    pub bundle_id: Option<String>,
    pub location: Option<String>,
}

/// What ties an entry to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tie {
    /// Bundle-ID (or Team-ID) evidence.
    Identity(String),
    /// Only the app's name matched.
    NameOnly(String),
}

fn id_tie(name: &str, bid: &str) -> Option<String> {
    let (n, b) = (name.to_lowercase(), bid.to_lowercase());
    if n == b {
        Some(format!("bundle ID {bid} (exact name)"))
    } else if n.starts_with(&format!("{b}.")) {
        Some(format!("bundle ID prefix {bid}. ({name})"))
    } else {
        None
    }
}

fn tie(e: &LibraryEntry, bid: Option<&str>, team: Option<&str>, bundle: Option<&str>, names: &[String]) -> Option<Tie> {
    let lname = e.name.to_lowercase();
    let by_id = |n: &str| bid.and_then(|b| id_tie(n, b));
    let found = match e.area {
        Area::ApplicationSupport | Area::Caches | Area::Logs | Area::HttpStorages | Area::WebKit | Area::Containers => by_id(&e.name),
        Area::Preferences | Area::ByHost => {
            let stem = e.name.strip_suffix(".plist").filter(|_| !e.is_dir);
            stem.and_then(by_id)
        }
        Area::SavedState => e.name.strip_suffix(".savedState").and_then(|s| bid.filter(|b| s.eq_ignore_ascii_case(b)).map(|b| format!("bundle ID {b} (saved window state)"))),
        Area::LaunchAgents => {
            let stem = e.name.strip_suffix(".plist").filter(|_| !e.is_dir);
            let program_inside = |p: &str| bundle.is_some_and(|b| p.starts_with(&format!("{}/", b.trim_end_matches('/'))));
            stem.and_then(|s| by_id(s).map(|t| format!("launchd job file: {t}")))
                .or_else(|| e.launch_label.as_deref().and_then(|l| by_id(l).map(|t| format!("launchd job label {l}: {t}"))))
                .or_else(|| e.launch_program.as_deref().filter(|p| program_inside(p)).map(|p| format!("launchd job runs {p}, inside the removed bundle")))
        }
        Area::GroupContainers => team
            .filter(|t| lname.starts_with(&format!("{}.", t.to_lowercase())))
            .map(|t| format!("Team ID {t} (app group container of this developer)"))
            .or_else(|| bid.and_then(|b| e.name.strip_prefix("group.").and_then(|rest| id_tie(rest, b)))),
    };
    if let Some(why) = found {
        return Some(Tie::Identity(why));
    }
    // Name-only evidence: exact name variants (generic words never match), in the folders apps name after themselves.
    if matches!(e.area, Area::ApplicationSupport | Area::Caches | Area::Logs) && names.contains(&lname) {
        return Some(Tie::NameOnly(lname));
    }
    None
}

/// Name variants of the app (display name and bundle file name) that may identify a folder; generic words never do.
fn mac_name_variants(identity: &AppIdentity) -> Vec<String> {
    let mut out: Vec<String> = name_variants(&identity.name);
    if let Some(stem) = identity.install_location.as_deref().and_then(|l| l.trim_end_matches('/').rsplit('/').next()).and_then(|f| f.strip_suffix(".app")) {
        out.extend(name_variants(stem));
    }
    out.sort();
    out.dedup();
    out
}

/// UNI-04/05/09 on macOS: the grouped leftover preview of one removed app. `entries` are the `~/Library` entries the
/// platform listed; `installed` the instances still installed (the removed one included if it came back);
/// `bundle_present` the bundle path when it still exists after the removal.
pub fn leftovers(identity: &AppIdentity, mac: Option<&MacRecord>, entries: &[LibraryEntry], installed: &[MacInstalled], bundle_present: Option<&str>) -> Vec<Leftover> {
    let bid = mac.and_then(|m| m.bundle_id.as_deref()).filter(|b| b.len() >= 3 && b.contains('.'));
    let team = mac.and_then(|m| m.team_id.as_deref());
    let bundle = identity.install_location.as_deref();
    let names = mac_name_variants(identity);
    let lower = |s: &str| s.trim_end_matches('/').to_lowercase();
    let reappeared = installed.iter().find(|o| o.instance_id == identity.instance_id || (bundle.is_some() && o.location.as_deref().map(lower) == bundle.map(lower)));
    let same_id: Vec<&MacInstalled> = installed
        .iter()
        .filter(|o| o.instance_id != identity.instance_id && bid.is_some() && o.bundle_id.as_deref().map(str::to_lowercase) == bid.map(str::to_lowercase))
        .collect();
    let mut out = Vec::new();
    for e in entries {
        let Some(t) = tie(e, bid, team, bundle, &names) else { continue };
        let kind = if e.is_dir { LeftoverKind::Directory } else { LeftoverKind::File };
        let mut evidence = Vec::new();
        let (mut marker, mut note): (Option<(Marker, String)>, Option<String>) = (None, None);
        let name_only = matches!(t, Tie::NameOnly(_));
        match &t {
            Tie::Identity(why) => evidence.push(why.clone()),
            Tie::NameOnly(v) => {
                evidence.push(format!("name only: \"{v}\" is the app's name; no bundle-ID evidence"));
                if let Some(o) = installed.iter().find(|o| o.instance_id != identity.instance_id && mac_name_variants(&AppIdentity { name: o.name.clone(), install_location: o.location.clone(), ..identity.clone() }).contains(v)) {
                    marker = Some((Marker::SharedOwner, format!("{} (installed) has the same name and may use it", o.name)));
                }
            }
        }
        if !name_only && let Some(o) = same_id.first() {
            marker = Some((Marker::SharedOwner, format!("{} (installed at {}) has the same bundle ID and uses it too", o.name, o.location.as_deref().unwrap_or("an unknown location"))));
        }
        if e.area == Area::GroupContainers {
            marker = Some((Marker::SharedOwner, "app group containers are shared by this developer's apps and extensions; kept".into()));
        }
        if let Some(o) = reappeared {
            marker = Some((Marker::AppReappeared, format!("{} is installed again", o.name)));
        }
        let binding = if e.area.privacy_protected() {
            Binding::Manual {
                guidance: format!(
                    "{} is protected by macOS privacy controls (App Data) and is not read or removed by CleanManager; if you are sure it belongs to the removed app, remove it in Finder",
                    e.path
                ),
            }
        } else {
            Binding::Path { path: e.path.clone() }
        };
        if e.area == Area::Containers && marker.is_none() {
            note = Some("permission_required: ~/Library/Containers is privacy-protected (App Data); reported only".into());
        }
        if e.area == Area::LaunchAgents {
            note = Some("launchd job definition: moving it to the Trash stops it from loading at the next login; a job loaded now keeps running until then (launchctl is not run)".into());
        }
        if let Some((_, why)) = &marker {
            evidence.push(why.clone());
        }
        let tier = if e.area == Area::Containers && marker.is_none() { "blocked" } else { "review_only" };
        let mut l = finish_leftover(Leftover {
            object_id: object_id(kind, &e.path), kind, group: e.area.group(), display: e.path.clone(), evidence, marker: marker.map(|(m, _)| m),
            tier: tier.into(), selected: false, warnings: vec![], logical_bytes: None, note, requires_admin: false, binding,
        });
        if name_only && l.marker.is_none() {
            l.selected = false;
            l.tier = "review_only".into();
            l.note = Some(format!("matched by name only; select it explicitly if it belongs to {}", identity.name));
        }
        out.push(l);
    }
    if let Some(path) = bundle_present {
        out.push(finish_leftover(Leftover {
            object_id: object_id(LeftoverKind::Directory, path), kind: LeftoverKind::Directory, group: Group::Bundle, display: path.to_owned(),
            evidence: vec!["the app bundle is still at its location after the removal".into()], marker: reappeared.map(|_| Marker::AppReappeared),
            tier: "review_only".into(), selected: false, warnings: vec![], logical_bytes: None, note: None, requires_admin: false,
            binding: Binding::Manual { guidance: format!("move {path} to the Trash in Finder (whole folders are not moved by this build)") },
        }));
    }
    if identity.source != RecordSource::Homebrew || bid.is_some() {
        // UNI-09: login and background items have no public enumeration API; reported with the place to review them.
        let display = format!("login and background items of {}", identity.name);
        out.push(Leftover {
            object_id: object_id(LeftoverKind::StartupEntry, &display), kind: LeftoverKind::StartupEntry, group: Group::Startup, display, evidence: vec![],
            marker: None, tier: "review_only".into(), selected: false, warnings: vec![], logical_bytes: None,
            note: Some("macOS keeps no public list of them; review them in System Settings".into()), requires_admin: false,
            binding: Binding::Manual { guidance: format!("System Settings › General › Login Items & Extensions ({LOGIN_ITEMS_URI})") },
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// UNI-07: AI command-line tools

/// One entry of an AI tool's data folder as the platform listed it (links are never followed or listed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiDataEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub logical_bytes: Option<u64>,
    /// A git worktree (a `.git` entry) lies inside, within a few levels.
    pub worktree: bool,
    /// Session-like names lie inside ([`ai_session_name`]), within a few levels.
    pub sessions_inside: bool,
}

/// Names that mark conversation data in an AI tool's folder, wherever they appear.
const AI_SESSION_NAMES: &[&str] = &[
    "session", "sessions", "chat", "chats", "checkpoint", "checkpoints", "history", "conversation", "conversations", "projects", "transcripts", "todos",
    "logs.json", "shell_history",
];

/// A session-like name (`sessions`, `chats`, `checkpoint-x.json`, `history.jsonl`, any `.jsonl` transcript…).
pub fn ai_session_name(name: &str) -> bool {
    let l = name.to_lowercase();
    l.ends_with(".jsonl") || AI_SESSION_NAMES.iter().any(|n| l == *n || [".", "-", "_"].iter().any(|sep| l.starts_with(&format!("{n}{sep}"))))
}

/// Note on a worktree inside an AI tool's data (no git is run, so uncommitted work cannot be ruled out).
pub const WORKTREE_NOTE: &str = "contains a git worktree: uncommitted work cannot be ruled out; remove it with `git worktree remove` yourself";

/// UNI-07 on macOS: the data-folder objects of a removed AI CLI. `data_dir` is shown as evidence; `home_files` are the
/// tool's files directly in the home folder (path, bytes); `still_installed` another installation of the same tool
/// that remains (its data is then shared and kept).
pub fn ai_leftovers(tool: &AiCli, data_dir: &str, entries: &[AiDataEntry], home_files: &[(String, u64)], still_installed: Option<&MacInstalled>) -> Vec<Leftover> {
    let mut out = Vec::new();
    let shared = |l: &mut Leftover| {
        if let Some(o) = still_installed {
            l.marker = Some(Marker::SharedOwner);
            l.evidence.push(format!("{} is still installed at {} and uses this data", o.name, o.location.as_deref().unwrap_or("another location")));
        }
    };
    for e in entries {
        let kind = if e.is_dir { LeftoverKind::Directory } else { LeftoverKind::File };
        let mut group = ai_child_group(tool, &e.name, e.is_dir);
        let mut note = None;
        // ANA-03: a cache folder that holds conversations is conversation data, never preselected.
        if group == Group::CachesLogs && (ai_session_name(&e.name) || e.sessions_inside) {
            group = Group::PersonalData;
            note = Some("holds session data (chats, checkpoints, history or transcripts): review it and select it explicitly".to_owned());
        }
        let mut l = Leftover {
            object_id: object_id(kind, &e.path), kind, group, display: e.path.clone(), evidence: vec![format!("{} data folder ({data_dir})", tool.name)], marker: None,
            tier: String::new(), selected: false, warnings: vec![], logical_bytes: e.logical_bytes, note, requires_admin: false, binding: Binding::Path { path: e.path.clone() },
        };
        if e.worktree {
            l.tier = "blocked".into();
            l.note = Some(WORKTREE_NOTE.into());
        }
        shared(&mut l);
        let mut l = finish_leftover(l);
        if matches!(l.group, Group::PersonalData | Group::Settings) && l.note.is_none() && l.tier == "review_only" {
            l.note = Some(format!("{} sessions, settings or credentials: never preselected; select it explicitly after reviewing it", tool.name));
        }
        out.push(l);
    }
    for (path, bytes) in home_files {
        let mut l = Leftover {
            object_id: object_id(LeftoverKind::File, path), kind: LeftoverKind::File, group: Group::Settings, display: path.clone(), evidence: vec![format!("{} settings file", tool.name)],
            marker: None, tier: "review_only".into(), selected: false, warnings: vec![], logical_bytes: Some(*bytes), note: None, requires_admin: false,
            // A file directly in the home folder is not planned (binding it would mean scanning the whole home folder).
            binding: Binding::Manual { guidance: "settings file in your home folder: delete it yourself after review if you no longer need it".into() },
        };
        shared(&mut l);
        out.push(finish_leftover(l));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// UNI-08: orphans under ~/Library

/// `~/Library` areas looked at for orphans (Containers only reported; Group Containers are shared by design).
pub const ORPHAN_AREAS: [Area; 7] = [Area::ApplicationSupport, Area::Caches, Area::Logs, Area::Preferences, Area::SavedState, Area::Containers, Area::LaunchAgents];

/// A reverse-DNS bundle identifier: at least three labels of letters, digits, `-` or `_`, the first a short word.
pub fn bundle_id_shaped(s: &str) -> bool {
    let labels: Vec<&str> = s.split('.').collect();
    labels.len() >= 3
        && s.len() <= 255
        && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        && (2..=12).contains(&labels[0].len())
        && labels[0].chars().all(|c| c.is_ascii_alphabetic())
}

/// Programs of macOS itself (a launchd job running one is never an app's orphan).
fn system_program(p: &str) -> bool {
    ["/System/", "/usr/", "/bin/", "/sbin/", "/Library/Apple/"].iter().any(|r| p.starts_with(r))
}

/// The identifier an entry's name implies, if it is bundle-ID shaped.
fn orphan_id(e: &LibraryEntry) -> Option<String> {
    let raw = match e.area {
        Area::ApplicationSupport | Area::Caches | Area::Logs | Area::Containers => Some(e.name.as_str()),
        Area::Preferences => e.name.strip_suffix(".plist").filter(|_| !e.is_dir),
        Area::SavedState => e.name.strip_suffix(".savedState"),
        Area::LaunchAgents => e.launch_label.as_deref().or_else(|| e.name.strip_suffix(".plist").filter(|_| !e.is_dir)),
        _ => None,
    }?;
    bundle_id_shaped(raw).then(|| raw.to_owned())
}

/// `a` and `b` name the same app or one names a component of the other (`com.foo.app` / `com.foo.app.helper`).
fn related_ids(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    a == b || a.starts_with(&format!("{b}.")) || b.starts_with(&format!("{a}."))
}

/// The developer part of an identifier (`com.vendor`).
fn vendor_of(id: &str) -> String {
    id.to_lowercase().split('.').take(2).collect::<Vec<_>>().join(".")
}

/// UNI-08 on macOS: orphaned `~/Library` entries of apps removed earlier. `installed` is every installed instance (all
/// inventory sources), `receipts` the installer package identifiers on record; `idle_of` gives the seconds since the
/// newest change of an entry (never asked for Containers) and `exists` whether a launchd job's program exists.
///
/// An entry is a candidate only by identity: its bundle-ID-shaped name (or launchd label) matches no installed app and
/// no receipt; a name alone never makes one. Apple's `com.apple.*` and CleanManager's own entries never are. Candidates
/// are never preselected; a developer prefix shared with something installed, a name shared with an installed app, or
/// use within [`ORPHAN_IDLE_DAYS`] keeps them (`owner_unknown` / `shared_owner`). Containers are reported only.
pub fn orphan_leftovers(
    entries: &[LibraryEntry], installed: &[MacInstalled], receipts: &[String], idle_of: &dyn Fn(&LibraryEntry) -> Option<u64>, exists: &dyn Fn(&str) -> bool,
) -> Vec<Leftover> {
    let owned: Vec<String> = installed.iter().filter_map(|i| i.bundle_id.as_deref()).chain(receipts.iter().map(String::as_str)).map(str::to_lowercase).collect();
    let vendors: BTreeSet<String> = owned.iter().filter(|b| bundle_id_shaped(b)).map(|b| vendor_of(b)).collect();
    let squash = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
    let names: Vec<(String, &MacInstalled)> = installed.iter().flat_map(|i| name_variants(&i.name).into_iter().map(move |v| (squash(&v), i))).filter(|(v, _)| v.len() >= 4).collect();
    let mut out = Vec::new();
    for e in entries.iter().filter(|e| ORPHAN_AREAS.contains(&e.area)) {
        let lname = e.name.to_lowercase();
        if lname.contains("cleanmanager") {
            continue;
        }
        let mut evidence = Vec::new();
        let id = orphan_id(e);
        if e.area == Area::LaunchAgents {
            // A job is an orphan only when the program it starts is gone and something identifies the app it belonged to.
            let Some(program) = e.launch_program.as_deref().filter(|p| p.starts_with('/') && !system_program(p)) else { continue };
            if exists(program) {
                continue;
            }
            let bundle = program.find(".app/").map(|i| &program[..i + 4]);
            if id.is_none() && bundle.is_none() {
                continue;
            }
            evidence.push(match bundle {
                Some(b) => format!("launchd job runs {program}; {b} no longer exists"),
                None => format!("launchd job runs {program}, which no longer exists"),
            });
        }
        if let Some(i) = &id {
            let li = i.to_lowercase();
            if li == "com.apple" || li.starts_with("com.apple.") || owned.iter().any(|o| related_ids(&li, o)) {
                continue;
            }
            evidence.insert(0, format!("bundle ID {i}: no installed app (/Applications, ~/Applications, /System/Applications, Homebrew, App Store) and no installer receipt has it"));
        } else if e.area != Area::LaunchAgents {
            continue;
        }
        let mut marker: Option<(Marker, String)> = None;
        if let Some(i) = &id {
            let v = vendor_of(i);
            let last = squash(i.rsplit('.').next().unwrap_or(""));
            if let Some((_, o)) = names.iter().find(|(n, _)| *n == last) {
                marker = Some((Marker::SharedOwner, format!("{} (installed) has the same name and may use it", o.name)));
            } else if vendors.contains(&v) {
                marker = Some((Marker::OwnerUnknown, format!("another installed app or package of the same developer ({v}) may use it")));
            }
        }
        let protected = e.area.privacy_protected();
        if marker.is_none() && !protected {
            match idle_of(e) {
                Some(s) if s >= ORPHAN_IDLE_DAYS * 86_400 => evidence.push(format!("not changed for {} days", s / 86_400)),
                Some(_) => marker = Some((Marker::OwnerUnknown, format!("changed within the last {ORPHAN_IDLE_DAYS} days: something may still use it"))),
                None => marker = Some((Marker::OwnerUnknown, "when it was last used is unknown".into())),
            }
        }
        if let Some((_, why)) = &marker {
            evidence.push(why.clone());
        }
        let kind = if e.is_dir { LeftoverKind::Directory } else { LeftoverKind::File };
        let (binding, tier, note) = if protected {
            (
                Binding::Manual { guidance: format!("{} is protected by macOS privacy controls (App Data) and is not read or removed by CleanManager; remove it in Finder if you are sure its app is gone", e.path) },
                "blocked",
                "permission_required: ~/Library/Containers is privacy-protected (App Data); reported only".to_owned(),
            )
        } else {
            let what = id.as_deref().unwrap_or("its program");
            (Binding::Path { path: e.path.clone() }, "review_only", format!("orphan of a removed app ({what}): never preselected; select it explicitly after review"))
        };
        let mut l = finish_leftover(Leftover {
            object_id: object_id(kind, &e.path), kind, group: e.area.group(), display: e.path.clone(), evidence, marker: marker.map(|(m, _)| m), tier: tier.into(),
            selected: false, warnings: vec![], logical_bytes: None, note: Some(note), requires_admin: false, binding,
        });
        // UNI-08: orphans are review_only, whatever their group (caches included).
        l.selected = false;
        if l.tier == "default_selected" {
            l.tier = "review_only".into();
        }
        out.push(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac(bid: &str) -> MacRecord {
        MacRecord {
            bundle_id: Some(bid.into()), team_id: Some("ABCDE12345".into()), homebrew_kind: None, volume_id: Some("v".into()), file_id: Some("f".into()),
            admin_reason: None, pkg_receipts: vec![], npm_prefix: None,
        }
    }

    fn rec(source: RecordSource, m: MacRecord) -> UninstallRecord {
        UninstallRecord {
            instance_id: "app_m".into(), name: "Foo Editor".into(), version: Some("1.0".into()), publisher: Some("Foo Inc. (ABCDE12345)".into()), source, key: None,
            key_name: None, product_code: None, package_full_name: None, package_family: None, uninstall_string: None, quiet_uninstall_string: None,
            install_location: Some("/Applications/Foo Editor.app".into()), system_component: false, winget: None, package_name: None, ai_tool: None,
            protected_class: None, mac: Some(m),
        }
    }

    #[test]
    fn routes_per_macos_source() {
        let env = AdapterEnv { brew: Some("/opt/homebrew/bin/brew".into()), ..Default::default() };
        let c = select_adapter(&rec(RecordSource::AppBundle, mac("com.foo.editor")), &env, &|_| false).unwrap();
        assert_eq!((c.kind, c.program.as_str(), c.argv.len()), (AdapterKind::MacTrashAppBundle, "/Applications/Foo Editor.app", 0));
        assert!(c.display.contains("Trash") && !c.kind.package_manager());

        let mut cask = rec(RecordSource::Homebrew, MacRecord { homebrew_kind: Some(HomebrewKind::Cask), ..mac("com.foo.editor") });
        cask.package_name = Some("foo-editor".into());
        let c = select_adapter(&cask, &env, &|_| false).unwrap();
        assert_eq!((c.kind, c.program.as_str()), (AdapterKind::HomebrewCask, "/opt/homebrew/bin/brew"));
        assert_eq!(c.argv, ["uninstall", "--cask", "foo-editor"]);
        assert!(!c.argv.iter().any(|a| a.contains("zap")));
        let mut formula = rec(RecordSource::Homebrew, MacRecord { homebrew_kind: Some(HomebrewKind::Formula), bundle_id: None, ..mac("x.y") });
        formula.package_name = Some("python@3.12".into());
        assert_eq!(select_adapter(&formula, &env, &|_| false).unwrap().argv, ["uninstall", "--formula", "python@3.12"]);
        // Names that could read as an option, a path or a tap reference are refused.
        for bad in ["--force", "-f", "../x", "user/tap/foo", "a b", ""] {
            formula.package_name = Some(bad.into());
            assert_eq!(select_adapter(&formula, &env, &|_| false).unwrap_err().code, "unsupported", "{bad}");
        }
        // No trusted brew: unsupported with the manual command.
        cask.package_name = Some("foo-editor".into());
        let err = select_adapter(&cask, &AdapterEnv::default(), &|_| false).unwrap_err();
        assert!(err.code == "unsupported" && err.message.contains("brew uninstall --cask foo-editor"), "{err:?}");

        // App Store and bundles this user cannot change: permission_required with guidance, never escalated.
        let err = select_adapter(&rec(RecordSource::AppStore, mac("com.foo.editor")), &env, &|_| false).unwrap_err();
        assert!(err.code == "permission_required" && err.message.contains("Launchpad"), "{err:?}");
        let root = rec(RecordSource::AppBundle, MacRecord { admin_reason: Some("owned by root".into()), ..mac("com.foo.editor") });
        let err = select_adapter(&root, &env, &|_| false).unwrap_err();
        assert!(err.code == "permission_required" && err.message.contains("Finder › Move to Trash") && err.settings_uri.is_none(), "{err:?}");
        // No identity: never moved blindly.
        let blind = rec(RecordSource::AppBundle, MacRecord { file_id: None, ..mac("com.foo.editor") });
        assert_eq!(select_adapter(&blind, &env, &|_| false).unwrap_err().code, "unsupported");
    }

    #[test]
    fn protected_classes_have_no_entry() {
        let env = AdapterEnv::default();
        let mut r = rec(RecordSource::AppBundle, mac("com.apple.Safari"));
        r.protected_class = Some(ProtectedReason::OsPackage);
        let err = select_adapter(&r, &env, &|_| false).unwrap_err();
        assert!(err.code == "policy_denied" && err.message.contains("Part of macOS"), "{err:?}");
        let mut me = rec(RecordSource::AppBundle, mac("com.cleanmanager.desktop"));
        me.name = "Something".into();
        assert_eq!(select_adapter(&me, &env, &|_| false).unwrap_err().code, "policy_denied");
        let mut av = rec(RecordSource::AppBundle, mac("com.vendor.agent"));
        av.protected_class = Some(ProtectedReason::EndpointSecurityOrManagement);
        assert!(select_adapter(&av, &env, &|_| false).unwrap_err().message.contains("vendor's official uninstall"));
    }

    #[test]
    fn team_ids_are_read_from_publisher_text() {
        assert_eq!(team_id_of("Foo Inc. (ABCDE12345)").as_deref(), Some("ABCDE12345"));
        assert_eq!(team_id_of("ABCDE12345").as_deref(), Some("ABCDE12345"));
        assert_eq!(team_id_of("Apple"), None);
        assert_eq!(team_id_of("Foo (abc)"), None);
    }

    #[test]
    fn processes_never_ended_on_macos() {
        let p = |pid, name: &str, path: Option<&str>| RunningProcess { pid, name: name.into(), path: path.map(str::to_owned) };
        assert!(end_refusal(&p(7, "Foo", Some("/Applications/Foo.app/Contents/MacOS/Foo")), 99).is_none());
        assert!(end_refusal(&p(99, "Foo", Some("/x")), 99).is_some());
        assert!(end_refusal(&p(8, "Finder", Some("/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder")), 99).is_some());
        assert!(end_refusal(&p(9, "helper", Some("/usr/libexec/helper")), 99).is_some());
        assert!(end_refusal(&p(10, "x", None), 99).is_some());
    }

    fn entry(area: Area, name: &str, is_dir: bool) -> LibraryEntry {
        LibraryEntry { area, name: name.into(), path: format!("/Users/u/Library/{}/{name}", area.rel()), is_dir, launch_label: None, launch_program: None }
    }

    fn by<'a>(items: &'a [Leftover], suffix: &str) -> &'a Leftover {
        items.iter().find(|l| l.display.ends_with(suffix)).unwrap_or_else(|| panic!("{suffix}: {items:#?}"))
    }

    #[test]
    fn leftovers_need_bundle_or_team_evidence_and_name_only_matches_stay_review_only() {
        let r = rec(RecordSource::AppBundle, mac("com.foo.editor"));
        let id = AppIdentity::of(&r);
        let mut agent = entry(Area::LaunchAgents, "com.other.updater.plist", false);
        agent.launch_program = Some("/Applications/Foo Editor.app/Contents/Library/LoginItems/upd".into());
        let entries = vec![
            entry(Area::Caches, "com.foo.editor", true),
            entry(Area::Caches, "com.foo.editor.ShipIt", true),
            entry(Area::Caches, "Foo Editor", true),
            entry(Area::Caches, "com.foo.editorial", true),
            entry(Area::Logs, "com.foo.editor", true),
            entry(Area::ApplicationSupport, "com.foo.editor", true),
            entry(Area::ApplicationSupport, "Foo Editor", true),
            entry(Area::ApplicationSupport, "Caches", true),
            entry(Area::Preferences, "com.foo.editor.plist", false),
            entry(Area::ByHost, "com.foo.editor.0123-4567.plist", false),
            entry(Area::SavedState, "com.foo.editor.savedState", true),
            entry(Area::HttpStorages, "com.foo.editor.binarycookies", false),
            entry(Area::WebKit, "com.foo.editor", true),
            agent,
            entry(Area::LaunchAgents, "com.foo.editor.helper.plist", false),
            entry(Area::Containers, "com.foo.editor", true),
            entry(Area::GroupContainers, "ABCDE12345.com.foo.shared", true),
            entry(Area::GroupContainers, "ZZZZZ99999.com.other", true),
        ];
        let items = leftovers(&id, r.mac.as_ref(), &entries, &[], None);
        let cache = by(&items, "Caches/com.foo.editor");
        assert_eq!((cache.group, cache.selected, cache.tier.as_str()), (Group::CachesLogs, true, "default_selected"));
        assert!(by(&items, "Caches/com.foo.editor.ShipIt").selected, "bundle-ID prefix is identity evidence");
        let decoy = by(&items, "Caches/Foo Editor");
        assert_eq!((decoy.selected, decoy.tier.as_str(), decoy.marker), (false, "review_only", None), "name only: never preselected");
        assert!(decoy.evidence[0].starts_with("name only"));
        assert!(!items.iter().any(|l| l.display.ends_with("com.foo.editorial") || l.display.ends_with("Support/Caches") || l.display.contains("ZZZZZ")));
        assert!(by(&items, "Logs/com.foo.editor").selected);
        let support = by(&items, "Application Support/com.foo.editor");
        assert_eq!((support.group, support.selected), (Group::PersonalData, false));
        assert_eq!((by(&items, "com.foo.editor.plist").group, by(&items, "com.foo.editor.plist").selected), (Group::Settings, false));
        assert_eq!(by(&items, "ByHost/com.foo.editor.0123-4567.plist").group, Group::Settings);
        assert_eq!(by(&items, "com.foo.editor.savedState").group, Group::Settings);
        let cookies = by(&items, "com.foo.editor.binarycookies");
        assert!(!cookies.selected && !cookies.warnings.is_empty(), "{cookies:?}");
        let job = by(&items, "com.other.updater.plist");
        assert_eq!((job.group, job.selected, matches!(job.binding, Binding::Path { .. })), (Group::Startup, false, true));
        assert!(job.evidence[0].contains("inside the removed bundle"));
        assert!(by(&items, "com.foo.editor.helper.plist").evidence[0].contains("launchd job file"));
        let container = by(&items, "Containers/com.foo.editor");
        assert_eq!((container.tier.as_str(), container.selected, container.logical_bytes), ("blocked", false, None));
        assert!(matches!(container.binding, Binding::Manual { .. }) && container.note.as_deref().unwrap().starts_with("permission_required"));
        let group = by(&items, "ABCDE12345.com.foo.shared");
        assert_eq!((group.marker, group.tier.as_str(), group.selected), (Some(Marker::SharedOwner), "kept", false));
        let login = items.iter().find(|l| l.kind == LeftoverKind::StartupEntry).unwrap();
        assert!(matches!(&login.binding, Binding::Manual { guidance } if guidance.contains("Login Items")));
        // UNI-05: nothing but caches and logs is ever preselected.
        assert!(items.iter().all(|l| !l.selected || (l.group == Group::CachesLogs && l.tier == "default_selected")));
    }

    #[test]
    fn another_copy_with_the_same_bundle_id_or_a_reinstall_keeps_everything() {
        let r = rec(RecordSource::AppBundle, mac("com.foo.editor"));
        let id = AppIdentity::of(&r);
        let entries = vec![entry(Area::Caches, "com.foo.editor", true), entry(Area::Caches, "Foo Editor", true)];
        let copy = MacInstalled { instance_id: "app_other".into(), name: "Foo Editor".into(), bundle_id: Some("com.foo.editor".into()), location: Some("/Users/u/Applications/Foo Editor.app".into()) };
        let items = leftovers(&id, r.mac.as_ref(), &entries, std::slice::from_ref(&copy), None);
        assert!(items.iter().filter(|l| l.kind == LeftoverKind::Directory).all(|l| l.marker == Some(Marker::SharedOwner) && l.tier == "kept" && !l.selected), "{items:#?}");
        let again = MacInstalled { instance_id: "app_new".into(), location: Some("/Applications/Foo Editor.app".into()), ..copy };
        let items = leftovers(&id, r.mac.as_ref(), &entries, &[again], Some("/Applications/Foo Editor.app"));
        assert!(items.iter().filter(|l| l.kind == LeftoverKind::Directory).all(|l| l.marker == Some(Marker::AppReappeared)), "{items:#?}");
    }

    fn npm_rec(pkg: &str, prefix: Option<&str>) -> UninstallRecord {
        let mut r = rec(RecordSource::NpmGlobal, MacRecord { bundle_id: None, team_id: None, volume_id: None, file_id: None, npm_prefix: prefix.map(str::to_owned), ..mac("x.y.z") });
        r.name = "Claude Code".into();
        r.publisher = None;
        r.package_name = Some(pkg.into());
        r.ai_tool = Some("claude-code".into());
        r.install_location = prefix.map(|p| format!("{p}/lib/node_modules/{pkg}"));
        r
    }

    #[test]
    fn npm_tools_are_removed_with_a_fixed_argv_naming_their_prefix() {
        let env = AdapterEnv { node: Some(("/opt/homebrew/bin/node".into(), "/opt/homebrew/lib/node_modules/npm/bin/npm-cli.js".into())), ..Default::default() };
        let c = select_adapter(&npm_rec("@anthropic-ai/claude-code", Some("/opt/homebrew")), &env, &|_| false).unwrap();
        assert_eq!((c.kind, c.program.as_str()), (AdapterKind::NpmGlobal, "/opt/homebrew/bin/node"));
        assert_eq!(
            c.argv,
            ["/opt/homebrew/lib/node_modules/npm/bin/npm-cli.js", "uninstall", "--global", "--prefix", "/opt/homebrew", "--ignore-scripts", "@anthropic-ai/claude-code"]
        );
        assert!(c.kind.package_manager() && c.display.contains("npm uninstall --global @anthropic-ai/claude-code"));
        for bad in ["--force", "-g", "../x", "a b", "x;rm", ""] {
            assert_eq!(select_adapter(&npm_rec(bad, Some("/opt/homebrew")), &env, &|_| false).unwrap_err().code, "unsupported", "{bad}");
        }
        for prefix in [None, Some("relative/prefix"), Some("/opt/../etc")] {
            assert_eq!(select_adapter(&npm_rec("@openai/codex", prefix), &env, &|_| false).unwrap_err().code, "unsupported", "{prefix:?}");
        }
        let err = select_adapter(&npm_rec("@openai/codex", Some("/opt/homebrew")), &AdapterEnv::default(), &|_| false).unwrap_err();
        assert!(err.code == "unsupported" && err.message.contains("npm uninstall -g @openai/codex"), "{err:?}");
        // Windows npm records (no macOS facts) keep their own route.
        let mut win = npm_rec("@openai/codex", None);
        win.mac = None;
        let env = AdapterEnv { node: Some((r"C:\node\node.exe".into(), r"C:\node\npm-cli.js".into())), ..Default::default() };
        assert_eq!(select_adapter(&win, &env, &|_| false).unwrap().argv, [r"C:\node\npm-cli.js", "uninstall", "--global", "@openai/codex"]);
    }

    fn ai(name: &str, is_dir: bool) -> AiDataEntry {
        AiDataEntry { name: name.into(), path: format!("/Users/u/.claude/{name}"), is_dir, logical_bytes: Some(10), worktree: false, sessions_inside: false }
    }

    #[test]
    fn ai_sessions_settings_and_credentials_are_never_preselected() {
        let tool = ai_cli("claude-code").unwrap();
        let mut wt = ai("worktrees", true);
        wt.worktree = true;
        let mut chats_in_cache = ai("statsig", true);
        chats_in_cache.sessions_inside = true;
        let entries = vec![
            ai("shell-snapshots", true), ai("projects", true), ai("todos", true), ai("settings.json", false), ai(".credentials.json", false), ai("history.jsonl", false),
            wt, chats_in_cache, ai("ide", true),
        ];
        let items = ai_leftovers(tool, "~/.claude", &entries, &[("/Users/u/.claude.json".into(), 5)], None);
        let by = |n: &str| items.iter().find(|l| l.display.ends_with(n)).unwrap_or_else(|| panic!("{n}: {items:#?}"));
        assert_eq!((by("shell-snapshots").group, by("shell-snapshots").selected), (Group::CachesLogs, true));
        assert!(by("ide").selected);
        for n in ["projects", "todos", "settings.json", ".credentials.json", "history.jsonl"] {
            assert!(!by(n).selected && by(n).tier == "review_only", "{n}: {:?}", by(n));
        }
        assert!(!by(".credentials.json").warnings.is_empty());
        let wt = by("worktrees");
        assert_eq!((wt.tier.as_str(), wt.selected, wt.note.as_deref()), ("blocked", false, Some(WORKTREE_NOTE)));
        let s = by("statsig");
        assert_eq!((s.group, s.selected), (Group::PersonalData, false), "a cache holding sessions is session data");
        let home = by(".claude.json");
        assert!(matches!(home.binding, Binding::Manual { .. }) && !home.selected);
        assert!(items.iter().all(|l| !l.selected || l.group == Group::CachesLogs));

        // Another installation of the same tool keeps all of it.
        let other = MacInstalled { instance_id: "app_b".into(), name: "Claude Code".into(), bundle_id: None, location: Some("/Users/u/.nvm/x".into()) };
        let kept = ai_leftovers(tool, "~/.claude", &entries, &[], Some(&other));
        assert!(kept.iter().all(|l| !l.selected && (l.tier == "kept" || l.tier == "blocked")), "{kept:#?}");
        assert!(ai_session_name("checkpoint-abc.json") && ai_session_name("chats") && ai_session_name("rollout.jsonl") && !ai_session_name("cache"));
    }

    fn lib(area: Area, name: &str, is_dir: bool) -> LibraryEntry {
        entry(area, name, is_dir)
    }

    #[test]
    fn orphans_need_a_bundle_id_nobody_installed_has_and_are_never_preselected() {
        let installed = vec![
            MacInstalled { instance_id: "a".into(), name: "Quillpad".into(), bundle_id: Some("com.example.quill".into()), location: Some("/Applications/Quillpad.app".into()) },
            MacInstalled { instance_id: "b".into(), name: "Notes".into(), bundle_id: Some("com.apple.Notes".into()), location: None },
        ];
        let receipts = vec!["com.pkgvendor.tool.pkg".to_owned()];
        let mut agent = lib(Area::LaunchAgents, "com.gone.updater.plist", false);
        agent.launch_label = Some("com.gone.updater".into());
        agent.launch_program = Some("/Applications/Gone.app/Contents/MacOS/upd".into());
        let mut live_agent = lib(Area::LaunchAgents, "com.live.agent.plist", false);
        live_agent.launch_program = Some("/Applications/Live.app/Contents/MacOS/x".into());
        let mut sys_agent = lib(Area::LaunchAgents, "com.sys.agent.plist", false);
        sys_agent.launch_program = Some("/usr/libexec/thing".into());
        let entries = vec![
            lib(Area::ApplicationSupport, "com.gone.app", true),
            lib(Area::Caches, "com.gone.app", true),
            lib(Area::Preferences, "com.gone.app.plist", false),
            lib(Area::SavedState, "com.gone.app.savedState", true),
            lib(Area::Containers, "com.gone.app", true),
            lib(Area::Caches, "com.example.quill", true),
            lib(Area::Caches, "com.example.quill.ShipIt", true),
            lib(Area::Caches, "com.apple.Safari", true),
            lib(Area::Caches, "Decoy Tool", true),
            lib(Area::Caches, "com.otherdev.quillpad", true),
            lib(Area::Caches, "com.example.sibling", true),
            lib(Area::Caches, "com.pkgvendor.tool", true),
            lib(Area::Caches, "com.recent.app", true),
            lib(Area::Caches, "com.cleanmanager.desktop", true),
            lib(Area::GroupContainers, "ABCDE12345.com.gone", true),
            agent, live_agent, sys_agent,
        ];
        let idle = |e: &LibraryEntry| Some(if e.name.contains("recent") { 3_600 } else { 90 * 86_400 });
        let exists = |p: &str| p.starts_with("/Applications/Live.app");
        let items = orphan_leftovers(&entries, &installed, &receipts, &idle, &exists);
        let find = |suffix: &str| items.iter().find(|l| l.display.ends_with(suffix));
        let listed = |suffix: &str| find(suffix).unwrap_or_else(|| panic!("{suffix}: {items:#?}"));
        for s in ["Application Support/com.gone.app", "Caches/com.gone.app", "com.gone.app.plist", "com.gone.app.savedState"] {
            let l = listed(s);
            assert_eq!((l.tier.as_str(), l.selected, l.marker), ("review_only", false, None), "{l:?}");
            assert!(l.evidence[0].contains("bundle ID com.gone.app") && matches!(l.binding, Binding::Path { .. }), "{l:?}");
        }
        let c = listed("Containers/com.gone.app");
        assert_eq!((c.tier.as_str(), c.selected, matches!(c.binding, Binding::Manual { .. })), ("blocked", false, true));
        let job = listed("com.gone.updater.plist");
        assert_eq!((job.group, job.tier.as_str()), (Group::Startup, "review_only"));
        assert!(job.evidence.iter().any(|e| e.contains("/Applications/Gone.app no longer exists")), "{job:?}");
        // Installed (any component of it), Apple's, receipts, name-only folders, Group Containers, CleanManager and live or
        // system launchd jobs are never orphans.
        for s in ["com.example.quill", "com.example.quill.ShipIt", "com.apple.Safari", "Decoy Tool", "com.pkgvendor.tool", "com.cleanmanager.desktop", "ABCDE12345.com.gone", "com.live.agent.plist", "com.sys.agent.plist"] {
            assert!(find(s).is_none(), "{s}: {items:#?}");
        }
        // Weak ties keep the entry: the same name as an installed app, the developer of an installed app, recent use.
        assert_eq!((listed("com.otherdev.quillpad").marker, listed("com.otherdev.quillpad").tier.as_str()), (Some(Marker::SharedOwner), "kept"));
        assert_eq!(listed("com.example.sibling").marker, Some(Marker::OwnerUnknown));
        assert_eq!(listed("com.recent.app").marker, Some(Marker::OwnerUnknown));
        assert!(items.iter().all(|l| !l.selected));
        assert!(bundle_id_shaped("com.foo.bar") && !bundle_id_shaped("com.foo") && !bundle_id_shaped("Foo Bar.baz.x") && !bundle_id_shaped("1com.foo.bar"));
    }
}
