//! Phase 11 session 5: lightweight tracing profiler.
//!
//! Records function-entry / function-exit events into a thread-local
//! buffer when profiling is enabled, and dumps them as a Chrome
//! Tracing JSON file (`{"traceEvents":[...]}`) that loads in
//! `chrome://tracing` or Perfetto. The fast path (profiling off) is a
//! single `AtomicBool::load` per call site so the unprofiled play
//! loop pays close to nothing.
//!
//! Events recorded:
//!   - `call_function` / `call_method` on the tree-walker.
//!     (Bytecode VM instrumentation defers — the dispatch loop is
//!     hot enough that adding a per-instruction probe would skew the
//!     numbers we're trying to measure.)
//!   - Frame-level `tick` / `render` events via `enter_frame_phase`.
//!
//! Format: durations are emitted as `ph: "X"` (complete) events with
//! microsecond `ts` + `dur`, which is the simplest format chrome-trace
//! accepts and produces a flame-graph view in Perfetto without any
//! extra metadata.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static EVENTS: RefCell<Vec<TraceEvent>> = const { RefCell::new(Vec::new()) };
    static T0: RefCell<Option<Instant>> = const { RefCell::new(None) };
}

pub struct TraceEvent {
    pub name: String,
    /// Microseconds since profile start.
    pub ts_us: u64,
    /// Duration in microseconds.
    pub dur_us: u64,
}

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
    T0.with(|c| *c.borrow_mut() = Some(Instant::now()));
    EVENTS.with(|c| c.borrow_mut().clear());
}

pub fn disable() {
    ENABLED.store(false, Ordering::Relaxed);
}

#[inline]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Scope guard — records an event covering its lifetime.
pub struct Scope {
    name: String,
    start: Instant,
}

impl Scope {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            start: Instant::now(),
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if !is_enabled() {
            return;
        }
        let dur_us = self.start.elapsed().as_micros() as u64;
        let ts_us = T0.with(|c| {
            c.borrow()
                .as_ref()
                .map(|t0| self.start.duration_since(*t0).as_micros() as u64)
                .unwrap_or(0)
        });
        let name = std::mem::take(&mut self.name);
        EVENTS.with(|c| {
            c.borrow_mut().push(TraceEvent {
                name,
                ts_us,
                dur_us,
            })
        });
    }
}

/// Open a scope only when profiling is enabled. Returns `None` on
/// the fast path so callers can hold an `Option<Scope>` for free.
#[inline]
pub fn scope(name: &str) -> Option<Scope> {
    if is_enabled() {
        Some(Scope::new(name))
    } else {
        None
    }
}

/// Serialize the buffered events as a Chrome Tracing JSON document
/// and write it to `path`. Drains the event buffer.
pub fn dump_to_path(path: &std::path::Path) -> Result<(), String> {
    let body = render_chrome_trace();
    std::fs::write(path, body).map_err(|e| format!("cannot write trace: {e}"))
}

fn render_chrome_trace() -> String {
    let mut out = String::from("{\"traceEvents\":[");
    let mut first = true;
    EVENTS.with(|c| {
        let mut events = c.borrow_mut();
        for ev in events.drain(..) {
            if !first {
                out.push(',');
            }
            first = false;
            // Escape quotes + backslashes in `name`. Twe identifiers
            // can't contain either today, but stdlib builtin names
            // include dots, which are JSON-safe.
            let safe = ev.name.replace('\\', "\\\\").replace('"', "\\\"");
            out.push_str(&format!(
                r#"{{"name":"{name}","cat":"twe","ph":"X","pid":1,"tid":1,"ts":{ts},"dur":{dur}}}"#,
                name = safe,
                ts = ev.ts_us,
                dur = ev.dur_us,
            ));
        }
    });
    out.push_str("]}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_scope_emits_no_events() {
        disable();
        EVENTS.with(|c| c.borrow_mut().clear());
        {
            let _s = scope("noop");
        }
        EVENTS.with(|c| assert!(c.borrow().is_empty()));
    }

    #[test]
    fn enabled_scope_records_event() {
        enable();
        {
            let _s = scope("hot");
        }
        let n = EVENTS.with(|c| c.borrow().len());
        assert_eq!(n, 1);
        disable();
    }

    #[test]
    fn dump_renders_chrome_trace_json_envelope() {
        enable();
        {
            let _s = scope("alpha");
        }
        let body = render_chrome_trace();
        disable();
        assert!(body.starts_with("{\"traceEvents\":["));
        assert!(body.ends_with("]}"));
        assert!(body.contains("\"name\":\"alpha\""));
        assert!(body.contains("\"ph\":\"X\""));
    }

    #[test]
    fn name_quotes_get_escaped() {
        enable();
        EVENTS.with(|c| c.borrow_mut().clear());
        {
            let _s = scope("a\"b");
        }
        let body = render_chrome_trace();
        disable();
        assert!(body.contains(r#""name":"a\"b""#), "got {body}");
    }
}
