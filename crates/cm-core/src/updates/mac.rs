//! macOS update channels, the platform-independent part (parity 10.1, UPD-01/04; design D8).
//!
//! - Homebrew: [`parse_brew_outdated`] reads `brew outdated --json=v2` (run with auto-update off, so the answer is
//!   Homebrew's local copy of its package index and the result says so). A cask or formula Homebrew knows and does
//!   not list is `up_to_date`; one it lists is `update_available` with the fixed-argv upgrade route. Casks marked
//!   `auto_updates` (or `version :latest`) are not tracked by `brew outdated`, so Homebrew's silence is not taken as
//!   an answer for them.
//! - Sparkle: the app's `SUFeedURL` appcast, fetched only by a user-triggered check over https, parsed on macOS into
//!   [`AppcastItem`]s. The newest item for this macOS (default channel, `sparkle:os` macOS or absent, within its
//!   minimum/maximum system version) is compared with the app's `CFBundleVersion` by Sparkle's own ordering
//!   ([`compare_versions`]); `sparkle:shortVersionString` is what is shown. Status only: the app installs its own
//!   updates (verified against its `SUPublicEDKey`), CleanManager never downloads or installs them.
//! - Mac App Store: the store checks its own apps; `cannot_check` routed to its Updates page.
//! - Electron (`Electron Framework.framework` with `app-update.yml` or Squirrel.Mac): detection only, `cannot_check`
//!   naming the app's own updater.
//! - Everything else is `cannot_check` with [`MAC_NO_CHANNEL`], never `up_to_date`.

use std::cmp::Ordering;

use crate::apps::{AppInstance, AppSource, HomebrewKind, ProtectedReason};

use super::{Channel, Route, UpdateCheck, UpdateState};

/// The Mac App Store's Updates page.
pub const MAC_APP_STORE_UPDATES_URI: &str = "macappstore://showUpdatesPage";
/// Software Update settings (apps that are part of macOS).
pub const SOFTWARE_UPDATE_URI: &str = "x-apple.systempreferences:com.apple.preferences.softwareupdate";
pub const MAC_NO_CHANNEL: &str = "No update channel: no Sparkle feed in Info.plist, not from the Mac App Store or Homebrew, and no recognizable update framework";
pub const MAS_SELF: &str = "The Mac App Store checks its own apps; open its Updates page";
pub const OS_COMPONENT: &str = "Part of macOS; it is updated with the system through Software Update";
/// Sparkle's XML namespace (`xmlns:sparkle`).
pub const SPARKLE_NS: &str = "http://www.andymatuschak.org/xml-namespaces/sparkle";
/// Largest appcast read (bytes).
pub const APPCAST_MAX: u64 = 4 << 20;

/// One entry of `brew outdated --json=v2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewOutdated {
    /// Formula full name (`wget`, `user/tap/tool`) or cask token.
    pub name: String,
    pub kind: HomebrewKind,
    pub installed_versions: Vec<String>,
    pub current_version: String,
    pub pinned: bool,
}

/// A Homebrew formula name or cask token that may be passed to `brew` as one argument: letters, digits and
/// `@ . _ + - /`, not starting with `-` or `/`, no `..`, at most 200 bytes.
pub fn valid_brew_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with(['-', '/', '.'])
        && !s.ends_with('/')
        && !s.contains("..")
        && !s.contains("//")
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "@._+-/".contains(c))
}

/// `brew outdated --json=v2` output. Any entry without a valid name or current version makes the whole answer
/// unusable (`Err`): a partial reading could show an outdated package as up to date.
pub fn parse_brew_outdated(json: &str) -> Result<Vec<BrewOutdated>, String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).map_err(|e| format!("brew outdated output is not JSON: {e}"))?;
    let obj = v.as_object().ok_or("brew outdated --json=v2 output is not a JSON object")?;
    if !obj.contains_key("formulae") && !obj.contains_key("casks") {
        return Err("brew outdated --json=v2 output has neither `formulae` nor `casks`".into());
    }
    let mut out = Vec::new();
    for (key, kind) in [("formulae", HomebrewKind::Formula), ("casks", HomebrewKind::Cask)] {
        let list = match obj.get(key) {
            None | Some(serde_json::Value::Null) => continue,
            Some(serde_json::Value::Array(a)) => a,
            Some(_) => return Err(format!("`{key}` in brew outdated output is not an array")),
        };
        for e in list {
            let name = e.get("name").and_then(serde_json::Value::as_str).filter(|n| valid_brew_name(n)).ok_or_else(|| format!("a `{key}` entry has no valid name"))?;
            let current = e
                .get("current_version")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("`{name}` has no current_version"))?;
            let installed = match e.get("installed_versions") {
                Some(serde_json::Value::Array(a)) => a.iter().filter_map(serde_json::Value::as_str).map(str::to_owned).collect(),
                Some(serde_json::Value::String(s)) => vec![s.clone()],
                _ => Vec::new(),
            };
            let pinned = e.get("pinned").and_then(serde_json::Value::as_bool).unwrap_or(false);
            out.push(BrewOutdated { name: name.to_owned(), kind, installed_versions: installed, current_version: current.to_owned(), pinned });
        }
    }
    Ok(out)
}

/// `channel_id` / `package_id` of a Homebrew package: `cask:<token>` or `formula:<name>`.
pub fn homebrew_package_id(kind: HomebrewKind, name: &str) -> String {
    format!("{}:{name}", kind.as_str())
}

/// Inverse of [`homebrew_package_id`]; `None` for any other shape or an invalid name.
pub fn parse_homebrew_package_id(id: &str) -> Option<(HomebrewKind, &str)> {
    let (kind, name) = id.split_once(':')?;
    let kind = match kind {
        "cask" => HomebrewKind::Cask,
        "formula" => HomebrewKind::Formula,
        _ => return None,
    };
    valid_brew_name(name).then_some((kind, name))
}

/// One `<item>` of a Sparkle appcast (element or legacy `<enclosure>` attribute values).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppcastItem {
    pub title: Option<String>,
    /// `sparkle:version` (compared with `CFBundleVersion`).
    pub version: Option<String>,
    /// `sparkle:shortVersionString` (shown).
    pub short_version: Option<String>,
    pub minimum_system_version: Option<String>,
    pub maximum_system_version: Option<String>,
    /// `sparkle:channel`; items on a named channel are not offered to the default channel.
    pub channel: Option<String>,
    /// `sparkle:os` of the enclosure (`macos`, `windows`, ...).
    pub os: Option<String>,
    /// The enclosure carries `sparkle:edSignature`.
    pub ed_signature: bool,
}

/// What an app's `Info.plist` and bundle say about Sparkle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SparkleFeed {
    /// `SUFeedURL`; `None` when the bundle embeds Sparkle but declares no feed (set at run time).
    pub feed_url: Option<String>,
    /// `SUPublicEDKey` is present.
    pub public_ed_key: bool,
    /// `CFBundleVersion`.
    pub bundle_version: Option<String>,
    /// `CFBundleShortVersionString`.
    pub short_version: Option<String>,
}

/// File-level facts the platform probe found for one app (no network).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MacProbe {
    pub sparkle: Option<SparkleFeed>,
    /// The Electron app's own updater (`electron-updater (app-update.yml, provider github)`, `Squirrel.Mac`).
    pub electron: Option<String>,
    /// The cask is marked `auto_updates` or has `version :latest`: `brew outdated` does not track it.
    pub cask_self_updating: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Number,
    Text,
    Separator,
}

fn part_of(c: char) -> Part {
    if c.is_ascii_digit() {
        Part::Number
    } else if c.is_whitespace() || c.is_ascii_punctuation() {
        Part::Separator
    } else {
        Part::Text
    }
}

fn parts(v: &str) -> Vec<(Part, String)> {
    let mut out: Vec<(Part, String)> = Vec::new();
    for c in v.chars() {
        let p = part_of(c);
        match out.last_mut() {
            Some((last, s)) if *last == p && p != Part::Separator => s.push(c),
            _ => out.push((p, c.to_string())),
        }
    }
    out
}

fn cmp_numbers(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// Sparkle's `SUStandardVersionComparator` ordering: versions split into runs of digits, runs of other characters
/// and single separators; numbers compare numerically, a number beats text (`1.0` > `1.0b1`), and when one version
/// runs out, the longer one is newer unless its next part is text (`1.5.1` > `1.5`, `1.5` > `1.5b3`).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (pa, pb) = (parts(a.trim()), parts(b.trim()));
    for ((ta, sa), (tb, sb)) in pa.iter().zip(&pb) {
        let o = if ta == tb {
            if *ta == Part::Number { cmp_numbers(sa, sb) } else { sa.cmp(sb) }
        } else if *ta != Part::Text && *tb == Part::Text {
            Ordering::Greater
        } else if *ta == Part::Text && *tb != Part::Text {
            Ordering::Less
        } else if *ta == Part::Number {
            Ordering::Greater
        } else {
            Ordering::Less
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    let n = pa.len().min(pb.len());
    match pa.len().cmp(&pb.len()) {
        Ordering::Equal => Ordering::Equal,
        Ordering::Greater => {
            if pa[n].0 == Part::Text { Ordering::Less } else { Ordering::Greater }
        }
        Ordering::Less => {
            if pb[n].0 == Part::Text { Ordering::Greater } else { Ordering::Less }
        }
    }
}

/// The item's macOS requirement is not met by `os` (`None`: the item has no requirement or the OS is unknown).
fn os_excluded(item: &AppcastItem, os: Option<&str>) -> Option<String> {
    let os = os?;
    if let Some(min) = &item.minimum_system_version
        && compare_versions(os, min) == Ordering::Less
    {
        return Some(format!("needs macOS {min} or later"));
    }
    if let Some(max) = &item.maximum_system_version
        && compare_versions(os, max) == Ordering::Greater
    {
        return Some(format!("supports macOS up to {max}"));
    }
    None
}

fn offered_here(item: &AppcastItem) -> bool {
    item.version.is_some() && item.channel.is_none() && item.os.as_deref().is_none_or(|o| o.eq_ignore_ascii_case("macos"))
}

fn shown(item: &AppcastItem) -> String {
    match (&item.short_version, &item.version) {
        (Some(s), Some(v)) if s != v => format!("{s} ({v})"),
        (Some(s), _) => s.clone(),
        (None, Some(v)) => v.clone(),
        (None, None) => "?".into(),
    }
}

fn sparkle(app: &AppInstance, feed_id: Option<String>, available: Option<String>, state: UpdateState, reason: String, route: Route, checked_at: &str) -> UpdateCheck {
    UpdateCheck::new(app, Some(Channel::Sparkle), feed_id, available, state, Some(reason), Some(checked_at.into()), route)
}

/// UPD-01 for a Sparkle app. `fetched` is the parsed appcast (only requested for an https `SUFeedURL`).
pub fn sparkle_check(app: &AppInstance, feed: &SparkleFeed, fetched: Option<Result<Vec<AppcastItem>, String>>, os: Option<&str>, checked_at: &str) -> UpdateCheck {
    let own = "the app installs its own updates (open it and choose Check for Updates); CleanManager only reports and never downloads or installs them";
    let Some(url) = feed.feed_url.clone() else {
        return sparkle(
            app,
            None,
            None,
            UpdateState::CannotCheck,
            format!("Uses Sparkle but its Info.plist declares no SUFeedURL (the feed is chosen at run time), so it cannot be checked here; {own}"),
            Route::OpenApp,
            checked_at,
        );
    };
    if !url.starts_with("https://") {
        return sparkle(app, Some(url), None, UpdateState::CannotCheck, format!("The Sparkle feed is not an https URL, so it was not fetched; {own}"), Route::OpenApp, checked_at);
    }
    let items = match fetched {
        None => return sparkle(app, Some(url), None, UpdateState::CannotCheck, format!("The Sparkle feed was not fetched; {own}"), Route::OpenApp, checked_at),
        Some(Err(e)) => return sparkle(app, Some(url), None, UpdateState::CannotCheck, format!("The Sparkle feed could not be read: {e}; {own}"), Route::OpenApp, checked_at),
        Some(Ok(items)) => items,
    };
    let Some(installed) = feed.bundle_version.as_deref().or(feed.short_version.as_deref()) else {
        return sparkle(app, Some(url), None, UpdateState::CannotCheck, "The app's Info.plist has no CFBundleVersion to compare with the feed".into(), Route::OpenApp, checked_at);
    };
    let offered: Vec<&AppcastItem> = items.iter().filter(|i| offered_here(i)).collect();
    if offered.is_empty() {
        return sparkle(app, Some(url), None, UpdateState::CannotCheck, "The Sparkle feed lists no macOS release with a sparkle:version".into(), Route::OpenApp, checked_at);
    }
    let newest = |list: &[&AppcastItem]| -> Option<AppcastItem> {
        list.iter().copied().max_by(|a, b| compare_versions(a.version.as_deref().unwrap_or(""), b.version.as_deref().unwrap_or(""))).cloned()
    };
    let eligible: Vec<&AppcastItem> = offered.iter().copied().filter(|i| os_excluded(i, os).is_none()).collect();
    let newer = |i: &AppcastItem| compare_versions(i.version.as_deref().unwrap_or(""), installed) == Ordering::Greater;
    let key = if feed.public_ed_key { "updates are verified by the app against the EdDSA key (SUPublicEDKey) in its Info.plist" } else { "its Info.plist has no SUPublicEDKey, so the app cannot verify updates by EdDSA signature" };
    match newest(&eligible) {
        Some(best) if newer(&best) => {
            let unsigned = if best.ed_signature { "" } else { "; this release carries no sparkle:edSignature" };
            sparkle(
                app,
                Some(url),
                Some(best.short_version.clone().or(best.version.clone()).unwrap_or_default()),
                UpdateState::UpdateAvailable,
                format!("Sparkle feed offers {}; {own}; {key}{unsigned}", shown(&best)),
                Route::OpenApp,
                checked_at,
            )
        }
        _ => {
            let blocked = newest(&offered).filter(|i| newer(i)).and_then(|i| os_excluded(&i, os).map(|why| format!("; the newer {} {why}", shown(&i))));
            sparkle(app, Some(url), None, UpdateState::UpToDate, format!("Compared with the Sparkle feed (installed {installed}){}", blocked.unwrap_or_default()), Route::None, checked_at)
        }
    }
}

/// UPD-01 for a Homebrew cask or formula from one `brew outdated` answer.
pub fn homebrew_check(app: &AppInstance, kind: HomebrewKind, token: &str, answer: Option<&Result<Vec<BrewOutdated>, String>>, checked_at: &str) -> UpdateCheck {
    let id = Some(homebrew_package_id(kind, token));
    let at = Some(checked_at.to_owned());
    let list = match answer {
        None => return UpdateCheck::new(app, Some(Channel::Homebrew), id, None, UpdateState::CannotCheck, Some("Homebrew was not queried".into()), at, Route::None),
        Some(Err(e)) => return UpdateCheck::new(app, Some(Channel::Homebrew), id, None, UpdateState::CannotCheck, Some(format!("Homebrew could not be queried: {e}")), at, Route::None),
        Some(Ok(list)) => list,
    };
    let entry = list.iter().find(|o| o.kind == kind && (o.name == token || o.name.rsplit('/').next() == Some(token)));
    let index = "compared with Homebrew's local copy of its package index (auto-update off; `brew update` refreshes it)";
    match entry {
        Some(o) => {
            let mut a = app.clone();
            if let Some(v) = o.installed_versions.last() {
                a.version = Some(v.clone());
            }
            let (route, reason) = if o.pinned {
                (Route::None, format!("Pinned in Homebrew (`brew unpin {token}` to allow the upgrade); {index}"))
            } else {
                (Route::PackageManager, format!("Homebrew upgrade available; {index}; Homebrew verifies the download's checksum before installing"))
            };
            UpdateCheck::new(&a, Some(Channel::Homebrew), id, Some(o.current_version.clone()), UpdateState::UpdateAvailable, Some(reason), at, route)
        }
        None => UpdateCheck::new(app, Some(Channel::Homebrew), id, None, UpdateState::UpToDate, Some(format!("Not outdated in Homebrew; {index}")), at, Route::None),
    }
}

/// Inputs of one macOS detection pass. Only `fetch_appcast` reaches the network (and only for https feeds);
/// `homebrew` is the already-run `brew outdated` answer (`None`: not queried).
pub struct MacDetectInput<'a> {
    pub apps: &'a [AppInstance],
    pub homebrew: Option<Result<Vec<BrewOutdated>, String>>,
    pub probe: &'a dyn Fn(&AppInstance) -> MacProbe,
    pub fetch_appcast: &'a dyn Fn(&str) -> Result<Vec<AppcastItem>, String>,
    /// This Mac's product version (`15.6.1`); `None` = unknown, system-version limits are then not applied.
    pub os_version: Option<&'a str>,
    pub checked_at: &'a str,
}

/// UPD-01 on macOS: one result per app. Order: part of macOS, Mac App Store, Homebrew (unless the cask updates
/// itself), Sparkle, Electron, Homebrew self-updating cask, no channel.
pub fn mac_detect(input: &MacDetectInput<'_>) -> Vec<UpdateCheck> {
    let at = || Some(input.checked_at.to_owned());
    input
        .apps
        .iter()
        .map(|app| {
            if app.protected_class == Some(ProtectedReason::OsPackage) {
                let mut c = UpdateCheck::new(app, None, None, None, UpdateState::CannotCheck, Some(OS_COMPONENT.into()), at(), Route::OpenOfficialPage);
                c.official_url = Some(SOFTWARE_UPDATE_URI.into());
                return c;
            }
            if let AppSource::AppStore { bundle_id, .. } = &app.source {
                let mut c = UpdateCheck::new(app, Some(Channel::MacAppStore), Some(bundle_id.clone()), None, UpdateState::CannotCheck, Some(MAS_SELF.into()), at(), Route::OpenStore);
                c.official_url = Some(MAC_APP_STORE_UPDATES_URI.into());
                return c;
            }
            let probe = (input.probe)(app);
            let homebrew = match &app.source {
                AppSource::Homebrew { token, kind } => Some((*kind, token.as_str())),
                _ => None,
            };
            if let Some((kind, token)) = homebrew
                && !(kind == HomebrewKind::Cask && probe.cask_self_updating)
            {
                return homebrew_check(app, kind, token, input.homebrew.as_ref(), input.checked_at);
            }
            if let Some(feed) = &probe.sparkle {
                let fetched = feed.feed_url.as_deref().filter(|u| u.starts_with("https://")).map(|u| (input.fetch_appcast)(u));
                return sparkle_check(app, feed, fetched, input.os_version, input.checked_at);
            }
            if let Some(updater) = &probe.electron {
                let reason = format!("Updates itself through {updater}; CleanManager does not read or contact that feed, so open the app and use its own update command");
                return UpdateCheck::new(app, Some(Channel::Electron), None, None, UpdateState::CannotCheck, Some(reason), at(), Route::OpenApp);
            }
            if let Some((kind, token)) = homebrew {
                let reason = "The cask is marked auto_updates (or version :latest), so Homebrew does not track its updates; the app updates itself";
                return UpdateCheck::new(app, Some(Channel::Homebrew), Some(homebrew_package_id(kind, token)), None, UpdateState::CannotCheck, Some(reason.into()), at(), Route::OpenApp);
            }
            UpdateCheck::new(app, None, None, None, UpdateState::CannotCheck, Some(MAC_NO_CHANNEL.into()), at(), Route::None)
        })
        .collect()
}

#[cfg(test)]
mod tests;
