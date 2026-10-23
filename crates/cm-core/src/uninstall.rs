//! Application uninstall (parity UNI-01..10, tasks 9.1–9.8, design D7): two separately reviewed `uninstall` plans.
//!
//! - Stage 1 runs the uninstall itself: `app_quit` (UNI-02: a close request, never an automatic forced end) and one
//!   `tool_adapter` chosen by the instance's verifiable source (UNI-01): the registered command (only the record's own
//!   `UninstallString`, after path, hash and Authenticode publisher checks: UNI-03), `msiexec /x` for MSI products, the
//!   package manager for MSIX, winget, Scoop or npm-global tools. Unknown or unverifiable sources get a capability
//!   explanation and the Settings entry; deleting the install folder is never offered as an uninstall.
//! - After it ran, the result is not inferred from the exit code: the install folder and registration are scanned
//!   again and anything left makes the outcome `partial` (UNI-03). Leftovers are a second plan, built from that
//!   operation and reviewed on its own (D7): nothing found before the uninstaller ran is authorized by stage 1.
//! - Leftovers need identity evidence (UNI-04): exact name variants of the product (generic words never match), the
//!   publisher\product convention, the recorded install location and registration, package family names. Objects that
//!   another installed instance (a beta of the same app) may own are `shared_owner`, weak matches are `owner_unknown`,
//!   and if the app is installed again everything is `app_reappeared`; all three are kept, never planned.
//! - The preview is grouped (UNI-05); only caches and logs are preselected, sensitive names carry warnings and are never
//!   preselected, and files elsewhere that mention the app are read-only hints. Registry leftovers are backed up before
//!   their delete (UNI-06) and restored only where the key was not recreated.
//! - AI command-line tools (UNI-07) uninstall through npm; their sessions and settings are separate, unselected
//!   objects and git worktrees inside their data are blocked. Orphans (UNI-08) are `review_only`. Protected classes
//!   (UNI-10) have no uninstall entry at all.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::apps::{AppSource, ProtectedReason};
use crate::plan::v2::{ActionMeta, Hive, PlanAction, PlanKind};
use crate::startup::{RegValue, RegView};

/// The Settings page every unsupported or refused uninstall points to.
pub const SETTINGS_URI: &str = "ms-settings:appsfeatures";
pub const SETTINGS_GUIDANCE: &str = "Use Settings > Apps > Installed apps (ms-settings:appsfeatures) or the vendor's own uninstaller";
/// Days a leftover must have been idle before it is offered as an orphan (UNI-08, design D7).
pub const ORPHAN_IDLE_DAYS: u64 = 30;
/// Default bounded wait after the close request (UNI-02).
pub const QUIT_WAIT_SECONDS: u64 = 15;
/// Upper bound for a vendor uninstaller run (it keeps its own UI, so a person may take a while).
pub const UNINSTALLER_TIMEOUT_SECONDS: u32 = 3600;

/// Where a registry key lives. `root` is the physical redirection it was read through (empty = the real hives; tests
/// use a throwaway key under `HKCU\Software\CleanManagerTest\`), so a plan made against one root never acts on another.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegLocation {
    pub root: String,
    pub hive: Hive,
    pub view: RegView,
    pub key_path: String,
}

impl RegLocation {
    pub fn display(&self) -> String {
        let view = match self.view {
            RegView::Wow64_32 => " (32-bit view)",
            _ => "",
        };
        if self.root.is_empty() { format!(r"{}\{}{view}", self.hive.as_str(), self.key_path) } else { format!(r"HKCU\{}\{}\{}{view}", self.root, self.hive.as_str(), self.key_path) }
    }
}

/// How the instance was installed, as far as a record proves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordSource {
    RegistryUninstall,
    Msi,
    Msix,
    Scoop,
    NpmGlobal,
    /// A macOS application bundle (no package manager record).
    AppBundle,
    /// A Homebrew cask or formula.
    Homebrew,
    /// A Mac App Store app.
    AppStore,
    Unknown,
}

/// winget's own evidence on a registration it wrote (`WinGetPackageIdentifier`, `WinGetSourceIdentifier`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WingetEvidence {
    pub package_id: String,
    pub source: Option<String>,
}

/// Everything the uninstall decision reads about one installed instance. Registry text is data, never a shell command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UninstallRecord {
    pub instance_id: String,
    pub name: String,
    pub version: Option<String>,
    pub publisher: Option<String>,
    pub source: RecordSource,
    /// The uninstall registration (registry sources).
    pub key: Option<RegLocation>,
    pub key_name: Option<String>,
    pub product_code: Option<String>,
    pub package_full_name: Option<String>,
    pub package_family: Option<String>,
    pub uninstall_string: Option<String>,
    pub quiet_uninstall_string: Option<String>,
    /// Absolute install location as recorded (environment expanded).
    pub install_location: Option<String>,
    pub system_component: bool,
    pub winget: Option<WingetEvidence>,
    /// Scoop app name or npm package name.
    pub package_name: Option<String>,
    /// Id of a known AI command-line tool ([`AI_CLIS`]).
    pub ai_tool: Option<String>,
    pub protected_class: Option<ProtectedReason>,
    /// macOS facts (bundle ID, Team ID, Homebrew kind, bundle identity, writability). Absent on Windows, so Windows
    /// records serialize, and digest, exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<mac::MacRecord>,
}

impl UninstallRecord {
    /// Protocol `app_instance.source` value.
    pub fn wire_source(&self) -> &'static str {
        match self.source {
            RecordSource::RegistryUninstall => "registry_uninstall",
            RecordSource::Msi => "msi",
            RecordSource::Msix => "msix_store",
            RecordSource::Scoop => "scoop",
            RecordSource::NpmGlobal => "npm_global",
            RecordSource::AppBundle => "app_bundle",
            RecordSource::Homebrew => "homebrew",
            RecordSource::AppStore => "app_store",
            RecordSource::Unknown => "unknown",
        }
    }

    /// Digest of the record fields an uninstall decision depends on; recomputed live before the uninstaller runs.
    pub fn digest(&self) -> String {
        crate::c14n::digest(&serde_json::to_value(self).expect("record serializes")).expect("plain JSON")
    }
}

/// Instance id for sources the registry inventory does not list (Scoop apps, npm-global tools): same shape as
/// [`crate::apps::instance_id_of`], different material, so it never collides with a registry instance.
pub fn tool_instance_id(source: RecordSource, name: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"cm-app-instance-v1\0");
    h.update(format!("{source:?}\0{}", name.to_lowercase()).as_bytes());
    let hex: String = h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("app_{hex}")
}

/// The record source of a registry-inventory [`AppSource`].
pub fn record_source(s: &AppSource) -> RecordSource {
    match s {
        AppSource::RegistryUninstall { .. } => RecordSource::RegistryUninstall,
        AppSource::Msi { .. } => RecordSource::Msi,
        AppSource::Msix { .. } => RecordSource::Msix,
        AppSource::AppBundle { .. } => RecordSource::AppBundle,
        AppSource::AppStore { .. } => RecordSource::AppStore,
        AppSource::Homebrew { .. } => RecordSource::Homebrew,
        AppSource::Unknown => RecordSource::Unknown,
    }
}

// ---------------------------------------------------------------------------------------------
// UNI-01 / UNI-10: adapter selection

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    RegisteredCommand,
    Msi,
    Msix,
    Winget,
    Scoop,
    NpmGlobal,
    /// macOS: move the app bundle to the user's Trash (`macos.trash-app-bundle`).
    MacTrashAppBundle,
    /// `brew uninstall --cask <token>`.
    HomebrewCask,
    /// `brew uninstall --formula <token>`.
    HomebrewFormula,
}

impl AdapterKind {
    pub fn adapter_id(self) -> &'static str {
        match self {
            AdapterKind::RegisteredCommand => "windows.registered-uninstaller",
            AdapterKind::Msi => "windows.msiexec-uninstall",
            AdapterKind::Msix => "windows.msix-remove-package",
            AdapterKind::Winget => "winget.uninstall",
            AdapterKind::Scoop => "scoop.uninstall",
            AdapterKind::NpmGlobal => "npm.uninstall-global",
            AdapterKind::MacTrashAppBundle => "macos.trash-app-bundle",
            AdapterKind::HomebrewCask => "homebrew.uninstall-cask",
            AdapterKind::HomebrewFormula => "homebrew.uninstall-formula",
        }
    }

    pub fn from_adapter_id(id: &str) -> Option<AdapterKind> {
        [
            AdapterKind::RegisteredCommand,
            AdapterKind::Msi,
            AdapterKind::Msix,
            AdapterKind::Winget,
            AdapterKind::Scoop,
            AdapterKind::NpmGlobal,
            AdapterKind::MacTrashAppBundle,
            AdapterKind::HomebrewCask,
            AdapterKind::HomebrewFormula,
        ]
        .into_iter()
            .find(|k| k.adapter_id() == id)
    }

    /// A package manager (not the vendor) removes the files (UNI-01 "由包管理器卸载").
    pub fn package_manager(self) -> bool {
        matches!(self, AdapterKind::Msix | AdapterKind::Winget | AdapterKind::Scoop | AdapterKind::NpmGlobal | AdapterKind::HomebrewCask | AdapterKind::HomebrewFormula)
    }
}

pub const ADAPTER_VERSION: &str = "1.0.0";

/// Trusted locations the platform resolved for this machine (never `PATH` or the working directory).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdapterEnv {
    /// `%SystemRoot%\System32`.
    pub system_dir: String,
    /// Scoop root (`%SCOOP%` or `%USERPROFILE%\scoop`) when its own `scoop.ps1` exists.
    pub scoop_root: Option<String>,
    /// winget.exe inside the App Installer package folder.
    pub winget_exe: Option<String>,
    /// (`node.exe`, `npm-cli.js`) from the Node.js install location.
    pub node: Option<(String, String)>,
    /// `AppxManifest.xml` of the instance's package.
    pub msix_manifest: Option<String>,
    /// macOS: Homebrew's `brew`, resolved and accepted by the user-owned trust policy (never `PATH`).
    pub brew: Option<String>,
}

/// The fixed invocation a stage-1 plan binds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterChoice {
    pub kind: AdapterKind,
    /// Program run (for MSIX: the package manifest the removal is bound to; no process is started).
    pub program: String,
    pub argv: Vec<String>,
    /// `argv[0]` is the registered argument tail, passed verbatim (no re-quoting).
    pub raw_args: bool,
    /// The program's Authenticode signer must match the record's publisher.
    pub publisher_required: Option<String>,
    /// The program must be signed by Microsoft (msiexec, Windows PowerShell).
    pub microsoft_signed: bool,
    /// Shown in review.
    pub display: String,
    /// Silent switches are never added: the uninstaller keeps its own UI (UNI-03).
    pub keeps_own_ui: bool,
}

/// Why an instance cannot be uninstalled by this build; `settings_uri` is the alternative offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
    pub settings_uri: Option<&'static str>,
}

fn refuse(code: &'static str, message: impl Into<String>, settings: bool) -> Refusal {
    Refusal { code, message: message.into(), settings_uri: settings.then_some(SETTINGS_URI) }
}

/// Why the program a route runs was refused (UNI-03 signer checks, at plan time and again before it runs): the protocol
/// code, the specific reason word interfaces explain in their own words (`None` = the code says it) and the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramRefusal {
    pub code: &'static str,
    pub reason: Option<&'static str>,
    pub message: String,
}

impl ProgramRefusal {
    pub fn new(code: &'static str, message: impl Into<String>) -> ProgramRefusal {
        ProgramRefusal { code, reason: None, message: message.into() }
    }

    pub fn because(code: &'static str, reason: &'static str, message: impl Into<String>) -> ProgramRefusal {
        ProgramRefusal { code, reason: Some(reason), message: message.into() }
    }
}

/// Reason words of signer refusals (`policy_denied`; WFA-76): the uninstaller has no valid signature, is signed by
/// someone other than the recorded publisher, or is not signed by Microsoft where it must be (msiexec, PowerShell).
pub const UNINSTALLER_UNSIGNED: &str = "uninstaller_unsigned";
pub const UNINSTALLER_SIGNER_MISMATCH: &str = "uninstaller_signer_mismatch";
pub const UNINSTALLER_NOT_MICROSOFT_SIGNED: &str = "uninstaller_not_microsoft_signed";

/// UNI-10: classes that never get an uninstall entry, with the official route.
pub fn protected_refusal(rec: &UninstallRecord) -> Option<Refusal> {
    if rec.mac.is_some() {
        return mac::protected_refusal(rec);
    }
    let lname = rec.name.to_lowercase();
    if lname.contains("cleanmanager") || rec.install_location.as_deref().is_some_and(|l| l.to_lowercase().ends_with("\\cleanmanager")) {
        return Some(refuse("policy_denied", "CleanManager is removed by its own installer, never by itself", true));
    }
    let why = match rec.protected_class? {
        ProtectedReason::EndpointSecurityOrManagement => {
            "Security or device-management agent: CleanManager offers no uninstall; remove it through your administrator or the vendor's official uninstall process"
        }
        ProtectedReason::SystemComponent => "System component: part of Windows or of another product, not uninstalled separately",
        ProtectedReason::OsPackage => "Windows package: part of the operating system",
        ProtectedReason::SharedRuntime => "Shared runtime other applications depend on: remove it only through Settings when nothing needs it",
    };
    Some(refuse("policy_denied", why, rec.protected_class != Some(ProtectedReason::EndpointSecurityOrManagement)))
}

fn is_product_code(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 38
        && b[0] == b'{'
        && b[37] == b'}'
        && b[1..37].iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// A ProductCode inside `msiexec /X{…}` or `/I{…}` (the modify form is never used: it is mapped to `/x`).
fn msi_code_in(tail: &str) -> Option<String> {
    let start = tail.find('{')?;
    let code = tail.get(start..start + 38)?;
    is_product_code(code).then(|| code.to_ascii_uppercase())
}

fn file_name_lower(p: &str) -> String {
    p.rsplit(['\\', '/']).next().unwrap_or(p).to_ascii_lowercase()
}

/// Package names passed to a package manager: letters, digits and `@/._-` only (never an option or a path).
pub fn safe_package_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 214 && !s.starts_with('-') && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || "@/._-".contains(c))
}

/// Path under `root` (case-insensitive, separator-aware).
pub fn path_within(path: &str, root: &str) -> bool {
    let p = path.trim_end_matches(['\\', '/']).to_lowercase().replace('/', "\\");
    let r = root.trim_end_matches(['\\', '/']).to_lowercase().replace('/', "\\");
    !r.is_empty() && (p == r || p.starts_with(&format!("{r}\\")))
}

/// UNI-01: choose how this instance is uninstalled. `exists` answers path existence (for unquoted commands).
pub fn select_adapter(rec: &UninstallRecord, env: &AdapterEnv, exists: &dyn Fn(&str) -> bool) -> Result<AdapterChoice, Refusal> {
    if let Some(r) = protected_refusal(rec) {
        return Err(r);
    }
    let choice = |kind, program: String, argv: Vec<String>, display: String| AdapterChoice {
        kind, program, argv, raw_args: false, publisher_required: None, microsoft_signed: false, display, keeps_own_ui: true,
    };
    match rec.source {
        // macOS npm-global records carry the prefix they were found in; their route names it explicitly.
        RecordSource::NpmGlobal if rec.mac.is_some() => return mac::select(rec, env),
        RecordSource::NpmGlobal => {
            let pkg = rec.package_name.as_deref().filter(|p| safe_package_name(p)).ok_or_else(|| refuse("unsupported", "npm package name is not recognized", false))?;
            let (node, npm) = env.node.clone().ok_or_else(|| refuse("unsupported", "Node.js/npm was not found in its install location; uninstall the tool with the package manager that installed it", false))?;
            return Ok(choice(AdapterKind::NpmGlobal, node, vec![npm, "uninstall".into(), "--global".into(), pkg.to_owned()], format!("npm uninstall --global {pkg} (package manager)")));
        }
        RecordSource::Scoop => {
            let app = rec.package_name.as_deref().filter(|p| safe_package_name(p) && !p.contains('/') && !p.contains('@'));
            let (Some(app), Some(root)) = (app, env.scoop_root.as_deref()) else {
                return Err(refuse("unsupported", "Scoop was not found or the app name is not recognized; run `scoop uninstall` yourself", false));
            };
            let ps = format!(r"{}\WindowsPowerShell\v1.0\powershell.exe", env.system_dir);
            let script = format!(r"{root}\apps\scoop\current\bin\scoop.ps1");
            let mut c = choice(AdapterKind::Scoop, ps, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", &script, "uninstall", app].map(str::to_owned).to_vec(),
                format!("scoop uninstall {app} (package manager)"));
            c.microsoft_signed = true;
            return Ok(c);
        }
        RecordSource::Msix => {
            let full = rec.package_full_name.as_deref().ok_or_else(|| refuse("unsupported", "package full name unknown", true))?;
            let manifest = env.msix_manifest.clone().ok_or_else(|| refuse("unsupported", "the package manifest could not be found; remove it in Settings", true))?;
            return Ok(choice(AdapterKind::Msix, manifest, vec!["remove".into(), full.to_owned()], format!("remove package {full} (Windows package manager, current user)")));
        }
        // macOS records have their own routes (bundle to the Trash, Homebrew), never the Windows registry adapters below.
        RecordSource::AppBundle | RecordSource::Homebrew | RecordSource::AppStore => return mac::select(rec, env),
        _ => {}
    }
    if let Some(w) = &rec.winget {
        if !safe_package_name(&w.package_id) {
            return Err(refuse("unsupported", "winget package identifier is not recognized", true));
        }
        let exe = env.winget_exe.clone().ok_or_else(|| refuse("unsupported", "installed by winget, but winget was not found in its package location; run `winget uninstall` yourself", true))?;
        let mut argv = vec!["uninstall".to_owned(), "--id".into(), w.package_id.clone(), "--exact".into()];
        if let Some(s) = w.source.as_deref().filter(|s| safe_package_name(s)) {
            argv.extend(["--source".to_owned(), s.to_owned()]);
        }
        let display = format!("winget uninstall --id {} --exact (package manager)", w.package_id);
        return Ok(choice(AdapterKind::Winget, exe, argv, display));
    }
    let msiexec = format!(r"{}\msiexec.exe", env.system_dir);
    let msi = |code: &str| {
        let mut c = choice(AdapterKind::Msi, msiexec.clone(), vec!["/x".into(), code.to_owned()], format!("msiexec /x {code} (Windows Installer)"));
        c.microsoft_signed = true;
        c
    };
    if rec.source == RecordSource::Msi {
        let code = rec.product_code.as_deref().filter(|c| is_product_code(c)).ok_or_else(|| refuse("unsupported", "MSI product code is not valid", true))?;
        return Ok(msi(&code.to_ascii_uppercase()));
    }
    let Some(command) = rec.uninstall_string.as_deref().filter(|s| !s.trim().is_empty()) else {
        return Err(refuse("unsupported", "no uninstall command is registered for this application; CleanManager never deletes the install folder as an uninstall", true));
    };
    let target = crate::startup::split_command(command, exists).ok_or_else(|| refuse("unsupported", "the registered uninstall command could not be read", true))?;
    let name = file_name_lower(&target.program);
    if name == "msiexec.exe" || name == "msiexec" {
        let code = msi_code_in(command).ok_or_else(|| refuse("unsupported", "msiexec command without a product code", true))?;
        return Ok(msi(&code));
    }
    if target.bare {
        return Err(refuse("unsupported", "the registered command names a program without its full path", true));
    }
    if crate::startup::is_script_host(&target.program) || !name.ends_with(".exe") {
        return Err(refuse("unsupported", "the registered command is a script or host program, not a verifiable uninstaller", true));
    }
    let tail = command_tail(command, &target.program);
    if tail.len() > 512 {
        return Err(refuse("unsupported", "the registered uninstall command is unusually long and is not run", true));
    }
    let publisher = rec.publisher.clone().ok_or_else(|| refuse("policy_denied", "no publisher is recorded, so the uninstaller's signature cannot be matched", true))?;
    let display = if tail.is_empty() { format!("\"{}\" (vendor uninstaller, its own UI)", target.program) } else { format!("\"{}\" {tail} (vendor uninstaller, its own UI)", target.program) };
    Ok(AdapterChoice {
        kind: AdapterKind::RegisteredCommand, program: target.program, argv: if tail.is_empty() { vec![] } else { vec![tail] }, raw_args: true,
        publisher_required: Some(publisher), microsoft_signed: false, display, keeps_own_ui: true,
    })
}

/// What follows the program in a registered command (quotes of the program removed, the rest verbatim).
fn command_tail(command: &str, program: &str) -> String {
    let c = command.trim();
    let rest = if let Some(r) = c.strip_prefix('"') { r.find('"').map(|e| &r[e + 1..]).unwrap_or("") } else { c.get(program.len()..).unwrap_or("") };
    rest.trim().to_owned()
}

// ---------------------------------------------------------------------------------------------
// UNI-03: publisher check and outcome

const COMPANY_SUFFIXES: &[&str] = &[
    "inc", "incorporated", "ltd", "limited", "llc", "corp", "corporation", "co", "company", "gmbh", "ag", "sa", "sas", "bv", "plc", "pty", "kk", "oy", "ab", "srl",
    "spa", "the", "lp", "llp", "se", "nv", "as",
];

fn company_tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && !COMPANY_SUFFIXES.contains(t))
        .map(str::to_owned)
        .collect()
}

/// The signer of the uninstaller matches the record's publisher: equal company names once legal suffixes and
/// punctuation are ignored, or one is a leading-word prefix of the other ("Mozilla" / "Mozilla Corporation").
pub fn publisher_matches(signer: &str, publisher: &str) -> bool {
    let (a, b) = (company_tokens(signer), company_tokens(publisher));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let (short, long) = if a.len() <= b.len() { (&a, &b) } else { (&b, &a) };
    long.starts_with(short)
}

/// Signers that count as Microsoft for system programs (msiexec, Windows PowerShell).
pub fn is_microsoft_signer(signer: &str) -> bool {
    matches!(signer, "Microsoft Windows" | "Microsoft Windows Publisher" | "Microsoft Corporation")
}

/// How an uninstaller's exit code reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitClass {
    Success,
    RestartRequired,
    CancelledByUser,
    /// The product was not installed (MSI 1605): nothing ran.
    NotInstalled,
    Failed,
}

pub fn classify_exit(kind: AdapterKind, code: i32) -> ExitClass {
    match (kind, code) {
        (_, 0) => ExitClass::Success,
        (AdapterKind::Msi, 3010 | 1641) => ExitClass::RestartRequired,
        (AdapterKind::Msi, 1602) => ExitClass::CancelledByUser,
        (AdapterKind::Msi, 1605) => ExitClass::NotInstalled,
        _ => ExitClass::Failed,
    }
}

/// What was found again after the uninstaller (UNI-03: never inferred from the exit code). `None` = could not check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Rescan {
    pub install_dir_present: Option<bool>,
    pub registration_present: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Succeeded,
    Skipped,
    Failed,
    Unknown,
}

/// Result codes that make an otherwise complete uninstall operation `partial`.
pub const PARTIAL_CODES: &[&str] = &["leftovers_remain", "restart_required"];

/// One uninstaller run as recorded: exit code and duration, then the rescan.
pub fn uninstaller_outcome(kind: AdapterKind, exit: Option<i32>, duration_ms: u64, rescan: Rescan) -> (StepStatus, &'static str, String) {
    let ran = match (kind, exit) {
        (AdapterKind::MacTrashAppBundle, Some(0)) => format!("moved to the Trash in {duration_ms} ms (Finder › Put Back restores it)"),
        (AdapterKind::HomebrewCask | AdapterKind::HomebrewFormula, Some(c)) => format!("brew exited with code {c} after {duration_ms} ms"),
        (_, Some(c)) => format!("uninstaller exited with code {c} after {duration_ms} ms"),
        (_, None) => format!("uninstaller did not report an exit code after {duration_ms} ms"),
    };
    let (folder, registration, gone) = match kind {
        // A bundle has no registration of its own: the macOS backend reports it as gone (`Some(false)`).
        AdapterKind::MacTrashAppBundle => ("the app bundle", "a registration", "the app bundle is gone"),
        AdapterKind::HomebrewCask => ("the app bundle", "the Homebrew Caskroom record", "the app bundle and the Homebrew Caskroom record are gone"),
        AdapterKind::HomebrewFormula => ("the Homebrew keg", "the Homebrew Cellar record", "the Homebrew keg and its Cellar record are gone"),
        _ => ("the install folder", "the uninstall registration", "install folder and registration are gone"),
    };
    let remains: Vec<&str> = [(rescan.install_dir_present, folder), (rescan.registration_present, registration)]
        .iter()
        .filter(|(p, _)| *p != Some(false))
        .map(|(p, what)| if p.is_some() { *what } else { "an unverifiable location" })
        .collect();
    let rest = if remains.is_empty() { format!("rescan: {gone}") } else { format!("rescan: {} still present", remains.join(" and ")) };
    let next = "review leftovers with `cleanmanager apps uninstall leftovers --operation <this operation>`";
    match exit.map(|c| classify_exit(kind, c)) {
        Some(ExitClass::CancelledByUser) => (StepStatus::Skipped, "cancelled_by_user", format!("{ran}: cancelled in the uninstaller; {rest}")),
        Some(ExitClass::Failed) => (StepStatus::Failed, "uninstaller_failed", format!("{ran}; {rest}")),
        None => (StepStatus::Failed, "uninstaller_failed", format!("{ran}; {rest}")),
        Some(ExitClass::RestartRequired) => (StepStatus::Succeeded, "restart_required", format!("{ran} (restart required to finish); {rest}; {next}")),
        Some(ExitClass::Success | ExitClass::NotInstalled) if !remains.is_empty() => (StepStatus::Succeeded, "leftovers_remain", format!("{ran}; {rest}: partial; {next}")),
        Some(_) => (StepStatus::Succeeded, "done", format!("{ran}; {rest}")),
    }
}

/// Operation status of an uninstall plan: complete only when nothing was left behind (UNI-03, UNI-09).
pub fn overall_status(base: &'static str, codes: &[&str]) -> &'static str {
    if base == "completed" && codes.iter().any(|c| PARTIAL_CODES.contains(c)) { "partial" } else { base }
}

// ---------------------------------------------------------------------------------------------
// UNI-02: running processes

/// A running process of the app (image under its install location).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningProcess {
    pub pid: u32,
    pub name: String,
    pub path: Option<String>,
}

impl RunningProcess {
    pub fn describe(&self) -> String {
        format!("{} (pid {})", self.name, self.pid)
    }
}

/// Whether the app runs: known list, or unknown (the uninstall is then skipped, UNI-02).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum RunningState {
    Known { processes: Vec<RunningProcess> },
    Unknown { reason: String },
}

/// Processes never ended, whatever a plan says: Windows' own session and shell processes, security services and
/// CleanManager itself.
pub const CRITICAL_PROCESSES: &[&str] = &[
    "system", "registry", "memory compression", "secure system", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "lsaiso.exe",
    "svchost.exe", "explorer.exe", "dwm.exe", "fontdrvhost.exe", "sihost.exe", "taskhostw.exe", "ctfmon.exe", "runtimebroker.exe", "searchhost.exe",
    "startmenuexperiencehost.exe", "shellexperiencehost.exe", "audiodg.exe", "conhost.exe", "spoolsv.exe", "msmpeng.exe", "nissrv.exe", "securityhealthservice.exe",
    "msiexec.exe", "trustedinstaller.exe", "tiworker.exe", "cleanmanager.exe", "cleanmanager-desktop.exe",
];

/// UNI-02: why a process may not be ended (`None` = may be ended after a person confirmed it).
pub fn end_refusal(p: &RunningProcess, own_pid: u32, system_root: &str) -> Option<&'static str> {
    let name = p.name.to_lowercase();
    if p.pid == own_pid || name.starts_with("cleanmanager") {
        return Some("CleanManager never ends itself");
    }
    if p.pid <= 4 || CRITICAL_PROCESSES.contains(&name.as_str()) {
        return Some("system-critical process");
    }
    match &p.path {
        None => Some("process image unknown"),
        Some(path) if path_within(path, system_root) => Some("part of Windows"),
        Some(_) => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Execution data bound into plans

/// Identity of the uninstalled app carried from stage 1 into the leftovers plan (UNI-04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppIdentity {
    pub instance_id: String,
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub source: RecordSource,
    pub install_location: Option<String>,
    pub key: Option<RegLocation>,
    pub key_name: Option<String>,
    pub product_code: Option<String>,
    pub package_family: Option<String>,
    pub package_name: Option<String>,
    pub ai_tool: Option<String>,
}

impl AppIdentity {
    pub fn of(rec: &UninstallRecord) -> AppIdentity {
        AppIdentity {
            instance_id: rec.instance_id.clone(), name: rec.name.clone(), publisher: rec.publisher.clone(), version: rec.version.clone(), source: rec.source,
            install_location: rec.install_location.clone(), key: rec.key.clone(), key_name: rec.key_name.clone(), product_code: rec.product_code.clone(),
            package_family: rec.package_family.clone(), package_name: rec.package_name.clone(), ai_tool: rec.ai_tool.clone(),
        }
    }
}

/// Per-action execution data of `uninstall` (and its `recovery`) plans; checked against the digest-covered body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
// Stored execution data, built once per action: the size difference does not matter here.
#[allow(clippy::large_enum_variant)]
pub enum UninstallMeta {
    /// `app_quit` and `tool_adapter` of stage 1: the record as planned (its digest is the action's `preview_digest`).
    App { identity: AppIdentity, record: Box<UninstallRecord>, adapter: AdapterChoice, signer: Option<String> },
    /// A registry key a `registry_backup_delete` or `registry_restore` acts on.
    RegistryKey { location: RegLocation },
}

/// `preview_digest` of a stage-1 tool_adapter action: the record and the exact invocation.
pub fn adapter_digest(rec: &UninstallRecord, choice: &AdapterChoice) -> String {
    crate::c14n::digest(&serde_json::json!({"record": rec.digest(), "adapter": choice})).expect("plain JSON")
}

/// A registry key as read: values and subkeys (recursively), for backups and change detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KeyTree {
    pub values: Vec<(String, RegValue)>,
    pub subkeys: Vec<(String, KeyTree)>,
}

impl KeyTree {
    pub fn count(&self) -> (usize, usize) {
        self.subkeys.iter().fold((self.values.len(), self.subkeys.len()), |(v, k), (_, t)| {
            let (a, b) = t.count();
            (v + a, k + b)
        })
    }
}

/// Digest binding a key location to its observed content; recomputed live before the delete.
pub fn key_digest(loc: &RegLocation, tree: &KeyTree) -> String {
    crate::c14n::digest(&serde_json::json!({"location": loc, "tree": tree})).expect("plain JSON")
}

/// Backup of a deleted key, stored with the operation before the delete (UNI-06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyBackup {
    pub backup_id: String,
    pub location: RegLocation,
    pub tree: KeyTree,
    pub owner_evidence: String,
    pub created_at: String,
}

impl KeyBackup {
    pub fn sha256(&self) -> String {
        crate::c14n::digest(&serde_json::to_value(self).expect("backup serializes")).expect("plain JSON")
    }
}

/// Backup ids of uninstall key backups (startup backups use another prefix).
pub const KEY_BACKUP_PREFIX: &str = "ub_";

/// Actions whose execution data is an [`UninstallMeta`]: stage-1 quit/uninstaller, leftover registry deletes and their restores.
pub fn binds_meta(kind: PlanKind, a: &PlanAction) -> bool {
    match (kind, a) {
        (PlanKind::Uninstall, PlanAction::AppQuit(_) | PlanAction::ToolAdapter(_) | PlanAction::RegistryBackupDelete(_)) => true,
        (PlanKind::Recovery, PlanAction::RegistryRestore(r)) => r.backup_id.starts_with(KEY_BACKUP_PREFIX),
        _ => false,
    }
}

/// The execution data must be what the digest-covered action describes.
pub fn check_action_meta(a: &PlanAction, m: &ActionMeta) -> Result<(), String> {
    let meta = m.uninstall.as_ref().ok_or("missing uninstall execution data")?;
    match (a, meta) {
        (PlanAction::AppQuit(q), UninstallMeta::App { identity, .. }) if q.app_instance_id == identity.instance_id => Ok(()),
        (PlanAction::ToolAdapter(t), UninstallMeta::App { record, adapter, .. })
            if t.preview_digest == adapter_digest(record, adapter) && t.adapter_id == adapter.kind.adapter_id() && t.argv == adapter.argv => Ok(()),
        (PlanAction::RegistryBackupDelete(r), UninstallMeta::RegistryKey { location })
            if r.hive == location.hive && r.key_path == location.key_path && r.value_name.is_none() && r.observed_sha256.is_some() => Ok(()),
        (PlanAction::RegistryRestore(r), UninstallMeta::RegistryKey { location }) if r.hive == location.hive && r.key_path == location.key_path => Ok(()),
        _ => Err("execution data does not match the action".into()),
    }
}

/// Platform side of uninstall plans. Every change re-reads live state first and changes nothing unless it still matches.
pub trait UninstallActions {
    fn is_admin(&self) -> bool;
    /// Processes whose image lies under the app's install location.
    fn running(&self, identity: &AppIdentity) -> RunningState;
    /// Ask each process to close (WM_CLOSE to its top-level windows) and wait up to the bounded time; returns what still runs.
    fn request_quit(&self, processes: &[RunningProcess]) -> Vec<RunningProcess>;
    /// A person confirms ending these processes now (non-interactive callers: always false).
    fn confirm_end(&self, processes: &[RunningProcess]) -> bool;
    fn end_process(&self, process: &RunningProcess) -> Result<(), String>;
    /// Re-read the record and verify program path, hash and signer; `Err((code, message))` stops before anything runs.
    fn verify(&self, rec: &UninstallRecord, choice: &AdapterChoice, sha256: &str, signer: Option<&str>) -> Result<(), (String, String)>;
    /// Run the bound invocation; (exit code, duration in ms) or an error before it started.
    fn run(&self, choice: &AdapterChoice, timeout_seconds: u32) -> Result<(Option<i32>, u64), String>;
    /// [`run`](Self::run) with the step's own result when it did not complete normally: `Err((status, code, message))`.
    /// The default reports any error as `start_failed` (nothing ran); a macOS Trash move reports the Trash's own codes
    /// (`access_denied`, `trash_unavailable`, `identity_changed`, an unconfirmed move as `unknown`...).
    fn run_step(&self, choice: &AdapterChoice, timeout_seconds: u32) -> Result<(Option<i32>, u64), (StepStatus, String, String)> {
        self.run(choice, timeout_seconds).map_err(|e| (StepStatus::Failed, "start_failed".to_owned(), format!("{e}; nothing was run")))
    }
    fn rescan(&self, identity: &AppIdentity) -> Rescan;
    fn read_key(&self, loc: &RegLocation) -> Result<Option<KeyTree>, String>;
    fn delete_key(&self, loc: &RegLocation) -> Result<(), (String, String)>;
    /// Recreate a key from its backup; refuses (`destination_exists`) when the key exists again.
    fn restore_key(&self, backup: &KeyBackup) -> Result<(), (String, String)>;
}

// ---------------------------------------------------------------------------------------------
// UNI-04: identity-evidence leftover discovery

/// Words that never identify an app on their own.
pub const GENERIC_WORDS: &[&str] = &[
    "app", "apps", "application", "applications", "data", "cache", "caches", "common", "common files", "shared", "update", "updater", "updates", "installer", "install",
    "setup", "uninstall", "program", "programs", "program files", "software", "tool", "tools", "utilities", "utility", "temp", "tmp", "log", "logs", "config", "settings",
    "user", "users", "local", "roaming", "locallow", "packages", "microsoft", "windows", "google", "apple", "intel", "nvidia", "amd", "system", "service", "services",
    "client", "driver", "drivers", "runtime", "sdk", "plugin", "plugins", "helper", "launcher", "bin", "default", "documents", "desktop", "downloads", "crashpad",
    "crash reports", "backup", "backups", "cloud", "sync", "web", "browser", "the", "studio", "editor", "player", "viewer", "manager", "assistant", "agent", "desktop app",
    "x64", "x86", "64-bit", "32-bit", "free", "pro", "home", "beta", "preview", "insiders",
];

/// Release channel words: an app "Foo Beta" shares identity with "Foo".
const CHANNEL_WORDS: &[&str] = &["beta", "preview", "insiders", "insider", "canary", "dev", "nightly", "alpha", "rc", "edge"];

fn is_generic(s: &str) -> bool {
    let squash = |t: &str| t.chars().filter(|c| !matches!(c, ' ' | '-' | '_' | '.')).collect::<String>();
    let l = squash(&s.trim().to_lowercase());
    l.chars().count() < 3 || GENERIC_WORDS.iter().any(|g| squash(g) == l) || l.chars().all(|c| c.is_ascii_digit())
}

/// Display name without version, architecture and bracketed suffixes.
fn base_name(name: &str) -> String {
    let mut s = name.to_owned();
    if let Some(i) = s.find(['(', '[']) {
        s.truncate(i);
    }
    let words: Vec<&str> = s
        .split_whitespace()
        .filter(|w| {
            let l = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
            let version = l.trim_start_matches('v').chars().all(|c| c.is_ascii_digit() || c == '.') && l.chars().any(|c| c.is_ascii_digit());
            !(version || matches!(l.as_str(), "x64" | "x86" | "64-bit" | "32-bit" | "amd64" | "arm64" | "-" | ""))
        })
        .collect();
    words.join(" ").trim_matches(|c: char| c == '-' || c == '_' || c.is_whitespace()).to_owned()
}

fn without_channel(name: &str) -> Option<String> {
    let words: Vec<&str> = name.split_whitespace().collect();
    let kept: Vec<&str> = words.iter().copied().filter(|w| !CHANNEL_WORDS.contains(&w.to_lowercase().as_str())).collect();
    (kept.len() < words.len() && !kept.is_empty()).then(|| kept.join(" "))
}

/// Exact folder/key names that identify a product (UNI-04): the cleaned display name, without spaces, hyphenated.
/// Generic words and names shorter than three characters never match.
pub fn name_variants(name: &str) -> Vec<String> {
    let base = base_name(name);
    let mut out = BTreeSet::new();
    for v in [base.clone(), base.replace(' ', ""), base.replace(' ', "-"), base.replace(' ', "_")] {
        if !is_generic(&v) {
            out.insert(v.to_lowercase());
        }
    }
    out.into_iter().collect()
}

/// Variants of the channel-less name ("Foo" for "Foo Beta"): evidence only for `shared_owner`/`owner_unknown`.
pub fn family_variants(name: &str) -> Vec<String> {
    without_channel(&base_name(name)).map(|b| name_variants(&b)).unwrap_or_default()
}

/// Publisher folder names ("Contoso" for "Contoso Ltd."); never a target by themselves.
pub fn publisher_variants(publisher: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    let trimmed: Vec<String> = publisher
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_owned())
        .filter(|w| !w.is_empty() && !COMPANY_SUFFIXES.contains(&w.to_lowercase().as_str()))
        .collect();
    let joined = trimmed.join(" ");
    if is_generic(&joined) {
        return Vec::new();
    }
    for v in [publisher.trim().trim_end_matches('.').to_owned(), joined.clone(), joined.replace(' ', "")] {
        if !is_generic(&v) {
            out.insert(v.to_lowercase());
        }
    }
    out.into_iter().collect()
}

/// Groups of the uninstall review (UNI-05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Bundle,
    CachesLogs,
    Settings,
    PersonalData,
    Startup,
    System,
}

impl Group {
    /// The protocol word (`caches_logs`, `personal_data`, ...), used in plan notes instead of debug text.
    pub fn as_str(self) -> &'static str {
        match self {
            Group::Bundle => "bundle",
            Group::CachesLogs => "caches_logs",
            Group::Settings => "settings",
            Group::PersonalData => "personal_data",
            Group::Startup => "startup",
            Group::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Marker {
    SharedOwner,
    OwnerUnknown,
    AppReappeared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeftoverKind {
    Directory,
    File,
    RegistryKey,
    StartupEntry,
    Shortcut,
    Service,
}

/// How a leftover would be acted on; `Manual` items are reported with guidance only (no action type for them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Binding {
    Path { path: String },
    Registry { location: RegLocation },
    Startup { startup_item_id: String },
    Manual { guidance: String },
}

/// One leftover object in the grouped preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leftover {
    pub object_id: String,
    pub kind: LeftoverKind,
    pub group: Group,
    pub display: String,
    pub evidence: Vec<String>,
    /// Kept objects (never planned): shared, unproven or reinstalled ownership.
    pub marker: Option<Marker>,
    /// `default_selected`, `review_only` (select explicitly), `blocked` or `kept` (a marker applies).
    pub tier: String,
    pub selected: bool,
    pub warnings: Vec<String>,
    pub logical_bytes: Option<u64>,
    pub note: Option<String>,
    pub requires_admin: bool,
    pub binding: Binding,
}

pub fn object_id(kind: LeftoverKind, display: &str) -> String {
    let mut h = Sha256::new();
    h.update(format!("cm-leftover-v1\0{kind:?}\0{}", display.to_lowercase()).as_bytes());
    let hex: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("lo_{hex}")
}

const CACHE_NAMES: &[&str] = &[
    "cache", "caches", "code cache", "gpucache", "dawncache", "dawnwebgpucache", "dawngraphitecache", "grshadercache", "shadercache", "graphitedawncache", "logs", "log",
    "crashpad", "crash reports", "crashreports", "crashdumps", "dumps", "temp", "tmp", "cacheddata", "cachedextensions", "cachedprofilesdata", "webcache",
    "component_crx_cache", "statsig", "shell-snapshots",
];
const PERSONAL_NAMES: &[&str] = &[
    "models", "model", "checkpoints", "weights", "documents", "projects", "sessions", "session", "history", "conversations", "chats", "saves", "saved", "database",
    "databases", "db", "todos", "worktrees", "workspaces", "workspace", "notes", "library", "media", "recordings", "downloads", "user data",
];
const SETTINGS_NAMES: &[&str] = &["settings", "preferences", "prefs", "config", "configuration", "profiles", "profile", "local storage", "keybindings", "themes"];
const MODEL_NAMES: &[&str] = &["models", "model", "checkpoints", "weights"];
const SENSITIVE: &[&str] = &[
    "credential", "password", "passwd", "secret", "token", "cookies", "login data", "wallet", "keychain", ".ssh", "ssh", ".aws", ".azure", ".kube", ".gnupg", "gnupg",
    "id_rsa", "id_ed25519", "auth.json", "oauth", "accounts.json", ".pem", ".key", ".pfx", ".p12", "documents", "vault",
];

/// UNI-05: group of an object inside a matched data folder, by its own name.
pub fn classify_child(name: &str, is_dir: bool) -> Group {
    let l = name.to_lowercase();
    let ext = l.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    if CACHE_NAMES.contains(&l.as_str()) || (!is_dir && matches!(ext, "log" | "dmp" | "tmp" | "etl")) {
        return Group::CachesLogs;
    }
    if PERSONAL_NAMES.contains(&l.as_str()) || (!is_dir && matches!(ext, "gguf" | "safetensors" | "ckpt" | "onnx" | "pt" | "sqlite" | "db" | "jsonl")) {
        return Group::PersonalData;
    }
    if SETTINGS_NAMES.contains(&l.as_str()) || (!is_dir && matches!(ext, "json" | "ini" | "toml" | "yaml" | "yml" | "xml" | "config" | "cfg" | "md")) {
        return Group::Settings;
    }
    // Unrecognized contents are treated as personal data: never preselected.
    Group::PersonalData
}

/// Sensitive-pattern warnings (credentials, SSH/cloud configuration, documents).
pub fn sensitive_warnings(path_or_name: &str) -> Vec<String> {
    let l = path_or_name.to_lowercase();
    let last = l.rsplit(['\\', '/']).next().unwrap_or(&l).to_owned();
    SENSITIVE.iter().filter(|p| last.contains(*p)).map(|p| format!("sensitive: name matches \"{p}\" (credentials, keys or documents); review before selecting")).collect()
}

/// The protocol's leftover tiers (`uninstall_leftover.tier`); [`finish_leftover`] always yields one of them.
pub const LEFTOVER_TIERS: &[&str] = &["default_selected", "review_only", "blocked", "kept"];

/// UNI-05: tier and preselection of a leftover. Only caches and logs are preselected; sensitive objects never are.
pub fn finish_leftover(mut l: Leftover) -> Leftover {
    if l.warnings.is_empty() {
        l.warnings = sensitive_warnings(&l.display);
    }
    if l.marker.is_some() {
        l.tier = "kept".into();
        l.selected = false;
        return l;
    }
    if l.tier == "blocked" || matches!(l.binding, Binding::Manual { .. }) {
        l.selected = false;
        // Guidance-only (manual) items are for a person to review: whatever tier the platform left unset (a startup
        // entry whose program still exists, a desktop shortcut) reads `review_only`, never an empty or unknown tier
        // and never preselected (WFA-113).
        if !matches!(l.tier.as_str(), "blocked" | "kept" | "review_only") {
            l.tier = "review_only".into();
        }
        return l;
    }
    l.selected = l.group == Group::CachesLogs && l.warnings.is_empty() && !l.requires_admin;
    l.tier = if l.selected { "default_selected".into() } else { "review_only".into() };
    if l.group == Group::PersonalData && l.note.is_none() {
        let lower = l.display.to_lowercase();
        let name = lower.rsplit(['\\', '/']).next().unwrap_or("");
        if MODEL_NAMES.contains(&name) {
            l.note = Some("models: large, usually re-downloadable by the app, but only if you still have access to them".into());
        }
    }
    l
}

/// A directory below a data root as the platform listed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntryInfo {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub logical_bytes: Option<u64>,
    /// Seconds since the last modification (newest of the entry itself), if known.
    pub idle_seconds: Option<u64>,
}

/// Another installed instance (for `shared_owner`) as the inventory lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    pub instance_id: String,
    pub name: String,
    pub publisher: Option<String>,
    pub install_location: Option<String>,
    pub key_name: Option<String>,
}

/// Where a folder name matched and how strongly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameMatch {
    pub variant: String,
    /// Under `<Publisher>\` (strong), or a top-level name.
    pub under_publisher: bool,
    /// Only the channel-less family name matched (weak: owner unknown).
    pub family_only: bool,
}

/// Does `dir_name` identify `identity` (exact variant, case-insensitive)?
pub fn match_name(identity: &AppIdentity, dir_name: &str, under_publisher: bool) -> Option<NameMatch> {
    let d = dir_name.to_lowercase();
    if is_generic(&d) {
        return None;
    }
    let mut names = name_variants(&identity.name);
    if let Some(k) = identity.key_name.as_deref().filter(|k| !is_product_code(k) && !k.contains('_')) {
        names.extend(name_variants(k));
    }
    if let Some(p) = identity.package_name.as_deref() {
        names.extend(name_variants(p.rsplit('/').next().unwrap_or(p)));
    }
    if names.contains(&d) {
        return Some(NameMatch { variant: d, under_publisher, family_only: false });
    }
    family_variants(&identity.name).contains(&d).then_some(NameMatch { variant: d, under_publisher, family_only: true })
}

/// Marker of a matched object: reinstalled, possibly shared with another installed instance, or unproven (UNI-04).
pub fn ownership_marker(identity: &AppIdentity, path: &str, m: &NameMatch, installed: &[InstalledApp]) -> Option<(Marker, String)> {
    for other in installed {
        let same = other.instance_id == identity.instance_id
            || (identity.key_name.is_some() && other.key_name == identity.key_name)
            || (identity.install_location.is_some() && other.install_location.as_deref().map(str::to_lowercase) == identity.install_location.as_deref().map(str::to_lowercase));
        if same {
            return Some((Marker::AppReappeared, format!("{} is installed again", other.name)));
        }
    }
    for other in installed {
        let loc_overlap = other.install_location.as_deref().is_some_and(|l| path_within(l, path) || path_within(path, l));
        let other_names: Vec<String> = name_variants(&other.name).into_iter().chain(family_variants(&other.name)).collect();
        if loc_overlap || other_names.contains(&m.variant) {
            return Some((Marker::SharedOwner, format!("{} (installed) may use it too", other.name)));
        }
    }
    if m.family_only {
        return Some((Marker::OwnerUnknown, format!("only the family name \"{}\" matches; another edition may own it", m.variant)));
    }
    if !m.under_publisher && m.variant.chars().count() < 4 {
        return Some((Marker::OwnerUnknown, format!("the short name \"{}\" alone does not prove ownership", m.variant)));
    }
    None
}

/// Registry keys that are never touched, whatever matched. Allowed shapes only: `Software\<Product>`,
/// `Software\<Publisher>\<Product>` (both also under `WOW6432Node`), and one app's own `...\CurrentVersion\Uninstall\<key>`
/// or `...\CurrentVersion\App Paths\<exe>` registration (UNI-09 registration cleanup).
pub fn registry_denied(key_path: &str) -> bool {
    let l = key_path.to_lowercase().replace('/', "\\");
    let parts: Vec<&str> = l.split('\\').collect();
    if let ["software", "microsoft", "windows", "currentversion", "uninstall" | "app paths", key]
    | ["software", "wow6432node", "microsoft", "windows", "currentversion", "uninstall" | "app paths", key] = parts.as_slice()
    {
        return key.is_empty();
    }
    let after_software = match parts.as_slice() {
        ["software", "wow6432node", rest @ ..] | ["software", rest @ ..] => rest,
        _ => return true,
    };
    match after_software.first() {
        None => true,
        Some(&first) => matches!(first, "microsoft" | "classes" | "policies" | "wow6432node" | "windows" | "clients" | "registeredapplications") || after_software.len() > 2,
    }
}

// ---------------------------------------------------------------------------------------------
// UNI-07: AI command-line tools

/// One known AI CLI: its npm package and the layout of its data folder in the user's home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AiCli {
    pub id: &'static str,
    pub name: &'static str,
    pub npm_package: &'static str,
    pub home_dir: &'static str,
    /// Files in the home folder itself that belong to the tool (settings).
    pub home_files: &'static [&'static str],
    /// (name inside the data folder, group); anything else is personal data.
    pub layout: &'static [(&'static str, Group)],
}

pub const AI_CLIS: &[AiCli] = &[
    AiCli {
        id: "claude-code", name: "Claude Code", npm_package: "@anthropic-ai/claude-code", home_dir: ".claude", home_files: &[".claude.json"],
        layout: &[
            ("projects", Group::PersonalData), ("todos", Group::PersonalData), ("shell-snapshots", Group::CachesLogs), ("statsig", Group::CachesLogs),
            ("ide", Group::CachesLogs), ("settings.json", Group::Settings), ("settings.local.json", Group::Settings), ("CLAUDE.md", Group::Settings),
            ("commands", Group::Settings), ("agents", Group::Settings), ("plugins", Group::Settings), (".credentials.json", Group::Settings),
        ],
    },
    AiCli {
        id: "codex", name: "Codex CLI", npm_package: "@openai/codex", home_dir: ".codex", home_files: &[],
        layout: &[
            ("sessions", Group::PersonalData), ("history.jsonl", Group::PersonalData), ("log", Group::CachesLogs), ("config.toml", Group::Settings),
            ("auth.json", Group::Settings), ("AGENTS.md", Group::Settings), ("version.json", Group::CachesLogs),
        ],
    },
    AiCli {
        id: "gemini-cli", name: "Gemini CLI", npm_package: "@google/gemini-cli", home_dir: ".gemini", home_files: &[],
        layout: &[
            ("tmp", Group::CachesLogs), ("settings.json", Group::Settings), ("oauth_creds.json", Group::Settings), ("google_accounts.json", Group::Settings),
            ("GEMINI.md", Group::Settings), ("history", Group::PersonalData),
        ],
    },
];

pub fn ai_cli(id: &str) -> Option<&'static AiCli> {
    AI_CLIS.iter().find(|t| t.id == id)
}

/// Group of an entry of an AI tool's data folder (sessions and settings are never preselected; UNI-07).
pub fn ai_child_group(tool: &AiCli, name: &str, is_dir: bool) -> Group {
    tool.layout.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, g)| *g).unwrap_or_else(|| match classify_child(name, is_dir) {
        Group::CachesLogs => Group::CachesLogs,
        _ => Group::PersonalData,
    })
}

// ---------------------------------------------------------------------------------------------
// UNI-08: orphans

/// A startup entry whose program is gone, with what identifies the app it belonged to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokenReference {
    pub startup_item_id: String,
    pub name: String,
    pub missing_target: String,
}

/// The app folder name a missing target implies (`...\Programs\Foo\foo.exe` → `Foo`), if not generic.
pub fn app_folder_of(target: &str) -> Option<String> {
    let parts: Vec<&str> = target.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    let n = parts.len();
    (n >= 3).then(|| parts[n - 2].to_owned()).filter(|d| !is_generic(d))
}

/// UNI-08: orphan candidates are always review_only, unselected, and explain the missing target. Data folders count only
/// when no installed instance claims the name, the name is not generic and the folder was idle for [`ORPHAN_IDLE_DAYS`].
pub fn orphan_leftovers(refs: &[BrokenReference], data_dirs: &[DirEntryInfo], installed: &[InstalledApp]) -> Vec<Leftover> {
    let mut out = Vec::new();
    let claimed = |name: &str| installed.iter().any(|a| name_variants(&a.name).contains(&name.to_lowercase()) || family_variants(&a.name).contains(&name.to_lowercase()));
    for r in refs {
        let mut l = Leftover {
            object_id: object_id(LeftoverKind::StartupEntry, &r.startup_item_id), kind: LeftoverKind::StartupEntry, group: Group::Startup,
            display: format!("startup entry {} → {}", r.name, r.missing_target), evidence: vec![format!("program missing: {}", r.missing_target)],
            marker: None, tier: "review_only".into(), selected: false, warnings: vec![], logical_bytes: None,
            note: Some("orphan: the program it starts no longer exists".into()), requires_admin: false,
            binding: Binding::Startup { startup_item_id: r.startup_item_id.clone() },
        };
        l.warnings = sensitive_warnings(&l.display);
        out.push(l);
        let Some(folder) = app_folder_of(&r.missing_target) else { continue };
        if claimed(&folder) {
            continue;
        }
        for d in data_dirs.iter().filter(|d| d.is_dir && d.name.eq_ignore_ascii_case(&folder)) {
            let idle = d.idle_seconds.is_some_and(|s| s >= ORPHAN_IDLE_DAYS * 86_400);
            out.push(Leftover {
                object_id: object_id(LeftoverKind::Directory, &d.path), kind: LeftoverKind::Directory, group: Group::PersonalData, display: d.path.clone(),
                evidence: vec![format!("same folder name as the missing program's folder ({folder})"), format!("referenced by startup entry {}", r.name)],
                marker: (!idle).then_some(Marker::OwnerUnknown), tier: if idle { "review_only".into() } else { "kept".into() }, selected: false,
                warnings: sensitive_warnings(&d.path), logical_bytes: d.logical_bytes,
                note: Some(if idle { format!("orphan: idle for at least {ORPHAN_IDLE_DAYS} days") } else { format!("used within the last {ORPHAN_IDLE_DAYS} days; kept") }),
                requires_admin: false, binding: Binding::Path { path: d.path.clone() },
            });
        }
    }
    out
}

pub mod mac;

#[cfg(test)]
mod tests;
