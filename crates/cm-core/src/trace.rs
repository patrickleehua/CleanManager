//! Opt-in diagnostic trace (CLI-14). Off unless a host calls [`enable`] (the CLI's global
//! `--debug`); when off, every call is one relaxed atomic load and nothing is formatted.
//!
//! Lines go to standard error only, one per event: `[debug +12ms] phase: message`. The elapsed
//! time counts from [`enable`]. Standard output and every machine record stay byte-identical to a
//! run without the trace, and the trace never feeds back into selection, authorization or
//! execution. Callers pass only names, counts, codes, backends and durations: never secrets,
//! provider keys or authorization receipt contents. Every line passes through the host's
//! redactor (the CLI installs the Doctor report's home/user/host rules) before it is written.

use std::fmt;
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

type Redact = Box<dyn Fn(&str) -> String + Send + Sync>;

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATE: OnceLock<(Instant, Redact)> = OnceLock::new();

/// Turn the trace on for the rest of the process. `redact` rewrites every line before it is
/// written (path display rules). Only the first call takes effect.
pub fn enable(redact: Redact) {
    if STATE.set((Instant::now(), redact)).is_ok() {
        ENABLED.store(true, Ordering::Relaxed);
    }
}

/// Whether the trace is on. Callers may skip building expensive details when it is off.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Write one trace line (no-op when off). Prefer the [`debug_trace!`](crate::debug_trace) macro.
pub fn line(phase: &str, message: fmt::Arguments<'_>) {
    let Some((start, redact)) = STATE.get().filter(|_| enabled()) else { return };
    let text = format!("{phase}: {message}");
    // One event per line, whatever the message carries.
    let text = redact(&text).replace('\r', "\\r").replace('\n', "\\n");
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "[debug +{}ms] {text}", start.elapsed().as_millis());
    let _ = err.flush();
}

/// `debug_trace!("scan", "backend={} entries={}", name, n)`: one stderr line when the trace is on;
/// the arguments are not even formatted when it is off.
#[macro_export]
macro_rules! debug_trace {
    ($phase:expr, $($arg:tt)+) => {
        if $crate::trace::enabled() {
            $crate::trace::line($phase, format_args!($($arg)+));
        }
    };
}

/// A timed phase: `start` writes `phase: start`, drop writes `phase: done in Nms`.
pub struct Span {
    phase: &'static str,
    started: Option<Instant>,
}

impl Span {
    pub fn elapsed(&self) -> Duration {
        self.started.map(|s| s.elapsed()).unwrap_or_default()
    }
}

/// Start a timed phase (inert when the trace is off).
pub fn span(phase: &'static str) -> Span {
    if !enabled() {
        return Span { phase, started: None };
    }
    line(phase, format_args!("start"));
    Span { phase, started: Some(Instant::now()) }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(s) = self.started {
            line(self.phase, format_args!("done in {}ms", s.elapsed().as_millis()));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn off_by_default_and_inert() {
        // This test binary never enables the trace: spans and lines are no-ops.
        assert!(!super::enabled());
        let s = super::span("phase");
        assert_eq!(s.elapsed(), std::time::Duration::ZERO);
        crate::debug_trace!("phase", "{}", "nothing is written");
    }
}
