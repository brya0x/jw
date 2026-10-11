//! `JW_TRACE=<file>`: one `µs,name,value` line per mark, for measuring how long
//! a workspace switch takes (addendum 14). `µs` counts from the first call;
//! a span logs when it ends, with its duration as the value. Without the
//! variable every call is one load and a branch.

use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Sink {
    file: Mutex<File>,
    t0: Instant,
}

static SINK: OnceLock<Option<Sink>> = OnceLock::new();

fn sink() -> Option<&'static Sink> {
    SINK.get_or_init(|| {
        let path = std::env::var_os("JW_TRACE")?;
        let file = File::create(path).ok()?;
        Some(Sink {
            file: Mutex::new(file),
            t0: Instant::now(),
        })
    })
    .as_ref()
}

pub fn on() -> bool {
    sink().is_some()
}

/// A point in time, with a number (bytes, a count) or 0.
pub fn mark(name: &str, value: u64) {
    if let Some(s) = sink() {
        write(s, name, value);
    }
}

/// Runs `f` and logs how many µs it took.
pub fn span<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let Some(s) = sink() else {
        return f();
    };
    let start = Instant::now();
    let out = f();
    write(s, name, start.elapsed().as_micros() as u64);
    out
}

fn write(s: &Sink, name: &str, value: u64) {
    let us = s.t0.elapsed().as_micros();
    if let Ok(mut f) = s.file.lock() {
        let _ = writeln!(f, "{us},{name},{value}");
    }
}
