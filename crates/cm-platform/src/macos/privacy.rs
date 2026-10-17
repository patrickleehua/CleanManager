//! Bounded first access to privacy-protected locations (TCC), so a pending macOS privacy prompt never stalls a
//! command silently.
//!
//! Measured on a real Mac (Mac17,14, macOS 27.0.1; docs/platforms/macos-verification.md): when the responsible app
//! has no decision yet for a protected location (Downloads, Desktop, Documents, another app's data), the first
//! `open(2)` of it (what `opendir`/`read_dir` do) blocks in the kernel while tccd shows the "would like to access…"
//! prompt, and returns only once the person answers: there is no timeout and no `EPERM`, also not in a launchd
//! background session. `lstat` of the location itself does not wait. Other threads keep running, and the process can
//! exit while the blocked thread is still waiting. A decision already made returns at once (an allowed one was measured;
//! a denied one returns `EPERM`, which was not measured here).
//!
//! [`bounded`] therefore runs that first access on a worker thread and waits at most a deadline. Past it the caller
//! reports the location as [`PERMISSION_PENDING`] (unknown, never 0 or empty) and moves on; the worker stays parked
//! until the prompt is answered and then ends, its result dropped. While a worker for a path is still parked, a
//! further request for that path is answered pending at once instead of starting another thread (a desktop refresh
//! does not pile them up). Callers that know which prompt a location waits for key the wait on it ([`bounded_as`],
//! [`prompt_key`]): every app's container waits for the same "data from other apps" prompt, so a scan meeting a
//! hundred containers waits once, not a hundred times. Nothing here probes a location that would not be read anyway, retries or changes any
//! privacy setting.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a first access may wait for macOS before the location is reported pending.
pub const PROMPT_WAIT: Duration = Duration::from_secs(5);
/// After this long without an answer, the `--debug` trace says macOS is probably asking for permission.
const TRACE_AFTER: Duration = Duration::from_millis(250);
/// Prefix of every note about a location whose first access did not return in time (docs/platforms/macos.md §6).
pub const PERMISSION_PENDING: &str = "permission_required (permission_cause=privacy_prompt_pending)";

/// Paths whose worker is still parked in the kernel.
static WAITING: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

fn waiting() -> MutexGuard<'static, Option<HashSet<PathBuf>>> {
    WAITING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Marks a path as no longer waiting when dropped, also when the reader panics.
struct Parked(PathBuf);

impl Drop for Parked {
    fn drop(&mut self) {
        if let Some(set) = waiting().as_mut() {
            set.remove(&self.0);
        }
    }
}

/// The note for a location still waiting for macOS, with the next step.
pub fn pending_reason(path: &Path, wait: Duration) -> String {
    format!(
        "{PERMISSION_PENDING}: macOS did not answer within {} s when {} was opened; it is most likely showing a privacy prompt (\"would like to access…\"). Answer it, then measure again. Nothing was read and the size is unknown",
        wait.as_secs_f32(),
        path.display()
    )
}

/// Whether `reason` (a note or an unknown-size reason) reports a pending privacy prompt.
pub fn is_pending(reason: &str) -> bool {
    reason.contains(PERMISSION_PENDING)
}

/// Folders below a home directory whose first open macOS can hold for a prompt (Files and Folders, iCloud Drive).
const HOME_FOLDERS: &[&str] = &["Desktop", "Documents", "Downloads", "Library/Mobile Documents"];
/// Folders below a home directory each child of which is another app's data (App Data protection); listing the folder
/// itself is not guarded.
const HOME_APP_DATA: &[&str] = &["Library/Containers", "Library/Group Containers"];

/// The privacy-protected location `path` is at or below, if any: `<home>/Desktop`, `Documents`, `Downloads` and
/// `Library/Mobile Documents`, one app's `<home>/Library/Containers/<id>` or `Library/Group Containers/<id>`, or a
/// mounted volume `/Volumes/<name>` (Removable / Network Volumes). Homes are `home` (`$HOME`) and every `/Users/<name>`
/// (also spelled through `/System/Volumes/Data`). Decided from the path text alone: nothing is examined.
pub fn protected_root(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    protected(path, home).map(|(root, _)| root)
}

/// The location whose prompt a first access to `path` waits for: its [`protected_root`], or for one app's container the
/// folder of all containers (one App Data prompt covers them).
pub fn prompt_key(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    protected(path, home).map(|(root, app_data)| match root.parent() {
        Some(all) if app_data => all.to_path_buf(),
        _ => root,
    })
}

/// [`protected_root`], and whether it is one app's data.
fn protected(path: &Path, home: Option<&Path>) -> Option<(PathBuf, bool)> {
    use std::path::Component;
    let comps: Vec<Component<'_>> = path.components().collect();
    let normal = |i: usize| match comps.get(i) {
        Some(Component::Normal(n)) => Some(*n),
        _ => None,
    };
    if comps.first() != Some(&Component::RootDir) {
        return None;
    }
    if normal(1).is_some_and(|c| c == "Volumes")
        && let Some(name) = normal(2)
    {
        return Some((Path::new("/Volumes").join(name), false));
    }
    let mut homes: Vec<PathBuf> = home.filter(|h| h.is_absolute()).map(Path::to_path_buf).into_iter().collect();
    let users_at = if [1, 2, 3].iter().zip(["System", "Volumes", "Data"]).all(|(&i, n)| normal(i).is_some_and(|c| c == n)) { 4 } else { 1 };
    if normal(users_at).is_some_and(|c| c == "Users")
        && let Some(user) = normal(users_at + 1)
    {
        homes.push(comps[..users_at + 1].iter().collect::<PathBuf>().join(user));
    }
    for h in &homes {
        let Ok(rel) = path.strip_prefix(h) else { continue };
        if let Some(f) = HOME_FOLDERS.iter().find(|f| rel.starts_with(f)) {
            return Some((h.join(f), false));
        }
        for d in HOME_APP_DATA {
            if let Ok(inner) = rel.strip_prefix(d)
                && let Some(Component::Normal(app)) = inner.components().next()
            {
                return Some((h.join(d).join(app), true));
            }
        }
    }
    None
}

/// Run `op`, the first access to the protected `path`, waiting at most `wait`. `Err` carries the reason shown to the
/// user: the access is pending ([`pending_reason`]) or no worker thread could be started.
pub(crate) fn bounded<T: Send + 'static>(path: &Path, wait: Duration, op: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    bounded_as(path, path, wait, op)
}

/// [`bounded`] with the wait keyed on `prompt` (see [`prompt_key`]) rather than on `path`: while an access keyed on
/// `prompt` is parked, this one is answered pending at once.
pub(crate) fn bounded_as<T: Send + 'static>(prompt: &Path, path: &Path, wait: Duration, op: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    let key = prompt.to_path_buf();
    if !waiting().get_or_insert_with(HashSet::new).insert(key.clone()) {
        cm_core::debug_trace!("privacy", "{} is still waiting for an earlier macOS privacy prompt ({}); reported pending", path.display(), prompt.display());
        return Err(pending_reason(path, wait));
    }
    let (tx, rx) = mpsc::sync_channel(1);
    let worker_key = key.clone();
    let started = Instant::now();
    let spawned = std::thread::Builder::new().name("cm-privacy-gate".into()).spawn(move || {
        let parked = Parked(worker_key.clone());
        let out = op();
        drop(parked); // before the result is sent: a caller that has it may open the path again at once
        if tx.send(out).is_err() {
            cm_core::debug_trace!("privacy", "{} answered after {} ms, after it was reported pending", worker_key.display(), started.elapsed().as_millis());
        }
    });
    if let Err(e) = spawned {
        drop(Parked(key));
        return Err(format!("{} was not read: no reader thread could be started ({e})", path.display()));
    }
    let first = wait.min(TRACE_AFTER);
    match rx.recv_timeout(first) {
        Ok(v) => return Ok(v),
        Err(RecvTimeoutError::Disconnected) => return Err(format!("{} was not read: the reader thread ended without a result", path.display())),
        Err(RecvTimeoutError::Timeout) => {}
    }
    cm_core::debug_trace!(
        "privacy",
        "opening {} has waited {} ms; macOS may be showing a privacy prompt. Waiting up to {} s",
        path.display(),
        started.elapsed().as_millis(),
        wait.as_secs_f32()
    );
    match rx.recv_timeout(wait.saturating_sub(first)) {
        Ok(v) => {
            cm_core::debug_trace!("privacy", "{} answered after {} ms", path.display(), started.elapsed().as_millis());
            Ok(v)
        }
        Err(RecvTimeoutError::Timeout) => {
            cm_core::debug_trace!("privacy", "no answer for {} after {} s; reported {PERMISSION_PENDING}", path.display(), wait.as_secs_f32());
            Err(pending_reason(path, wait))
        }
        Err(RecvTimeoutError::Disconnected) => Err(format!("{} was not read: the reader thread ended without a result", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    /// A reader that blocks until the returned sender is used or dropped (an unanswered prompt).
    fn blocking_reader() -> (mpsc::Sender<()>, impl FnOnce() -> u32 + Send + 'static) {
        let (release, gate): (mpsc::Sender<()>, Receiver<()>) = mpsc::channel();
        (release, move || {
            let _ = gate.recv();
            7
        })
    }

    #[test]
    fn a_prompt_that_is_not_answered_is_reported_pending_after_the_deadline() {
        let path = Path::new("/nonexistent/cm-privacy-test/pending");
        let (release, reader) = blocking_reader();
        let t = Instant::now();
        let got = bounded(path, Duration::from_millis(150), reader);
        let took = t.elapsed();
        let why = got.expect_err("an unanswered first access is not waited for");
        assert!(is_pending(&why), "{why}");
        assert!(why.contains("/nonexistent/cm-privacy-test/pending") && why.contains("size is unknown"), "{why}");
        assert!(took >= Duration::from_millis(150) && took < Duration::from_secs(3), "waited {took:?}");

        // Still parked: a second request is answered at once, without a second thread.
        let t = Instant::now();
        let again = bounded(path, Duration::from_secs(30), || 1u32);
        assert!(again.is_err_and(|w| is_pending(&w)));
        assert!(t.elapsed() < Duration::from_secs(1), "a parked path is not waited for again");

        // Answered later: the worker ends, and the path is read normally from then on.
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match bounded(path, Duration::from_secs(1), || 2u32) {
                Ok(v) => break assert_eq!(v, 2),
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                Err(w) => panic!("the path stayed pending after the answer: {w}"),
            }
        }
    }

    #[test]
    fn an_answer_within_the_deadline_returns_the_result() {
        let path = Path::new("/nonexistent/cm-privacy-test/answered");
        let got = bounded(path, Duration::from_secs(5), || {
            std::thread::sleep(Duration::from_millis(300)); // past the trace point, well within the deadline
            Err::<(), _>(std::io::Error::from_raw_os_error(libc::EPERM))
        });
        let inner = got.expect("answered in time");
        assert_eq!(inner.unwrap_err().raw_os_error(), Some(libc::EPERM), "a denial is passed through as the read's own error");
        assert_eq!(bounded(path, Duration::from_secs(5), || 3u32), Ok(3), "nothing stays parked");
    }

    #[test]
    fn protected_roots_are_named_from_the_path_alone() {
        let home = Some(Path::new("/opt/home/kim"));
        let root = |p: &str| protected_root(Path::new(p), home).map(|r| r.display().to_string());
        assert_eq!(root("/Users/ann/Downloads").as_deref(), Some("/Users/ann/Downloads"));
        assert_eq!(root("/Users/ann/Documents/a/b").as_deref(), Some("/Users/ann/Documents"));
        assert_eq!(root("/Users/ann/Library/Mobile Documents/com~apple~CloudDocs").as_deref(), Some("/Users/ann/Library/Mobile Documents"));
        assert_eq!(root("/Users/ann/Library/Containers/com.docker.docker/Data/log").as_deref(), Some("/Users/ann/Library/Containers/com.docker.docker"));
        assert_eq!(root("/Users/ann/Library/Group Containers/group.x").as_deref(), Some("/Users/ann/Library/Group Containers/group.x"));
        assert_eq!(root("/System/Volumes/Data/Users/ann/Desktop/x").as_deref(), Some("/System/Volumes/Data/Users/ann/Desktop"));
        assert_eq!(root("/opt/home/kim/Desktop").as_deref(), Some("/opt/home/kim/Desktop"));
        assert_eq!(root("/Volumes/Stick/.Trashes/501").as_deref(), Some("/Volumes/Stick"));
        let key = |p: &str| prompt_key(Path::new(p), home).map(|r| r.display().to_string());
        assert_eq!(key("/Users/ann/Library/Containers/com.a/Data").as_deref(), Some("/Users/ann/Library/Containers"), "one prompt for all containers");
        assert_eq!(key("/Users/ann/Library/Group Containers/group.x").as_deref(), Some("/Users/ann/Library/Group Containers"));
        assert_eq!(key("/Users/ann/Downloads/x").as_deref(), Some("/Users/ann/Downloads"));
        assert_eq!(key("/Users/ann/Projects"), None);
        for open in [
            "/Users/ann",
            "/Users/ann/Library/Containers",
            "/Users/ann/Library/Caches/x",
            "/Users/ann/DownloadsOld",
            "/Users/ann/Projects/Downloads",
            "/Volumes",
            "/private/tmp/Downloads",
            "relative/Downloads",
        ] {
            assert_eq!(root(open), None, "{open}");
        }
    }

    #[test]
    fn a_reader_that_panics_is_not_reported_as_a_result() {
        let path = Path::new("/nonexistent/cm-privacy-test/panics");
        let got: Result<u32, String> = bounded(path, Duration::from_secs(5), || panic!("reader failed"));
        let why = got.unwrap_err();
        assert!(!is_pending(&why) && why.contains("without a result"), "{why}");
    }
}
