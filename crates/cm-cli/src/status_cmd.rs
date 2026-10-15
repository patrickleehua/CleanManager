//! `status` (parity STS-01..03/06, tasks 12.1/12.2): one system status snapshot, or with `--watch` / `--interval` /
//! `--count` a JSONL stream of snapshots, one compact record per line. Each record keeps the time it was sampled.

use std::process::ExitCode;

use cm_core::error::ErrorCode;

use crate::args::Parsed;
use crate::fail;

/// (interval seconds, max samples) when streaming; `None` for a single snapshot.
fn stream_params(p: &Parsed) -> Result<Option<(u64, Option<u64>)>, String> {
    let interval = match p.option("interval").map(str::parse::<u64>) {
        None => None,
        Some(Ok(n)) if (1..=3600).contains(&n) => Some(n),
        Some(_) => return Err("--interval must be 1..=3600 seconds".into()),
    };
    let count = match p.option("count").map(str::parse::<u64>) {
        None => None,
        Some(Ok(n)) if (1..=100_000).contains(&n) => Some(n),
        Some(_) => return Err("--count must be 1..=100000".into()),
    };
    Ok((p.flag("watch") || interval.is_some() || count.is_some()).then(|| (interval.unwrap_or(5), count)))
}

pub fn status(req: &str, p: &Parsed) -> ExitCode {
    let stream = match stream_params(p) {
        Ok(s) => s,
        Err(msg) => return fail(req, ErrorCode::InvalidArgument, &msg, p.format),
    };
    run(req, p, stream)
}

#[cfg(windows)]
fn run(req: &str, p: &Parsed, stream: Option<(u64, Option<u64>)>) -> ExitCode {
    use std::time::{Duration, Instant};

    use cm_core::protocol::record;

    use crate::{Format, emit};

    let mut sampler = cm_platform::windows::status::Sampler::new();
    let snapshot = |s: &cm_core::status::StatusSnapshot| record("status_snapshot", req, crate::commands::fields(s.to_wire()));
    let Some((interval, count)) = stream else {
        emit(&snapshot(&sampler.sample()), p.format);
        return ExitCode::SUCCESS;
    };
    let period = Duration::from_secs(interval);
    let mut next = Instant::now();
    let mut n = 0u64;
    loop {
        let s = sampler.sample();
        // JSONL: one compact record per line regardless of the terminal.
        emit(&snapshot(&s), Format::Json);
        n += 1;
        if count.is_some_and(|c| n >= c) {
            return ExitCode::SUCCESS;
        }
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now; // overrun: sample at once, never re-stamp an old sample
        }
    }
}

#[cfg(not(windows))]
fn run(req: &str, p: &Parsed, _stream: Option<(u64, Option<u64>)>) -> ExitCode {
    fail(req, ErrorCode::Unsupported, "status sampling is not implemented on this platform yet", p.format)
}
