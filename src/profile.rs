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

// ─────────────────────────────────────────────────────────────────────
// web3d-M10 session 2: per-frame sample ring for the Studio profiler
// panel. Separate from the Chrome-trace events above (which are
// function-level): this is one compact sample per frame — frame time,
// its sim/render split, GC cost, live bytes, draw calls, entity count —
// kept in a small ring the panel polls. The push is a single struct
// into a bounded deque, so the play loop populates it every frame.
// ─────────────────────────────────────────────────────────────────────

/// One frame's performance sample.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameSample {
    pub frame_ms: f32,
    pub sim_ms: f32,
    pub render_ms: f32,
    pub gc_ms: f32,
    pub bytes_alive: u64,
    pub draw_calls: u32,
    pub entity_count: u32,
    pub sim_time: f64,
}

impl FrameSample {
    /// Assemble a sample from the live `Env` + the caller's timings,
    /// pulling the GC cost, live bytes, entity count and sim time from
    /// the engine so the play loop only supplies frame / sim / render
    /// milliseconds and the draw-call count.
    pub fn from_env(
        env: &crate::value::Env,
        frame_ms: f32,
        sim_ms: f32,
        render_ms: f32,
        draw_calls: u32,
    ) -> Self {
        FrameSample {
            frame_ms,
            sim_ms,
            render_ms,
            gc_ms: crate::heap::gc_last_collect_ns() as f32 / 1.0e6,
            bytes_alive: crate::heap::gc_bytes_alive() as u64,
            draw_calls,
            entity_count: env.debug_entity_count() as u32,
            sim_time: env.sim_time,
        }
    }
}

/// How many frames of history the ring keeps (~4 s at 60 fps).
pub const SAMPLE_RING_CAP: usize = 240;

thread_local! {
    static SAMPLES: RefCell<std::collections::VecDeque<FrameSample>> =
        const { RefCell::new(std::collections::VecDeque::new()) };
}

/// Record one frame's sample (called once per frame by the play loop).
pub fn push_sample(s: FrameSample) {
    SAMPLES.with(|c| {
        let mut ring = c.borrow_mut();
        if ring.len() >= SAMPLE_RING_CAP {
            ring.pop_front();
        }
        ring.push_back(s);
    });
}

/// The most recent `n` frame samples (oldest first), for the panel.
pub fn recent_samples(n: usize) -> Vec<FrameSample> {
    SAMPLES.with(|c| {
        let ring = c.borrow();
        let start = ring.len().saturating_sub(n);
        ring.iter().skip(start).copied().collect()
    })
}

/// Clear the sample ring (e.g. on hot reload).
pub fn clear_samples() {
    SAMPLES.with(|c| c.borrow_mut().clear());
}

/// Serialise the most recent `n` samples as a JSON array for Studio.
pub fn samples_json(n: usize) -> String {
    use crate::json::{self, Value};
    let arr: Vec<Value> = recent_samples(n)
        .iter()
        .map(|s| {
            json::obj([
                ("frame_ms", Value::Float(s.frame_ms as f64)),
                ("sim_ms", Value::Float(s.sim_ms as f64)),
                ("render_ms", Value::Float(s.render_ms as f64)),
                ("gc_ms", Value::Float(s.gc_ms as f64)),
                ("bytes_alive", Value::Int(s.bytes_alive as i64)),
                ("draw_calls", Value::Int(s.draw_calls as i64)),
                ("entity_count", Value::Int(s.entity_count as i64)),
                ("sim_time", Value::Float(s.sim_time)),
            ])
        })
        .collect();
    json::to_string(&Value::Array(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The enable flag is process-global: tests that flip it hold this
    /// lock, or one test's `disable()` lands mid-way through another's
    /// trace (seen as an intermittent failure in the full suite).
    static PROFILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        PROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn disabled_scope_emits_no_events() {
        let _guard = lock();
        disable();
        EVENTS.with(|c| c.borrow_mut().clear());
        {
            let _s = scope("noop");
        }
        EVENTS.with(|c| assert!(c.borrow().is_empty()));
    }

    #[test]
    fn enabled_scope_records_event() {
        let _guard = lock();
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
        let _guard = lock();
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
    fn frame_samples_ring_and_json() {
        clear_samples();
        for i in 0..(SAMPLE_RING_CAP + 10) {
            push_sample(FrameSample {
                frame_ms: i as f32 * 0.1,
                entity_count: i as u32,
                bytes_alive: 1000 + i as u64,
                ..Default::default()
            });
        }
        // The ring is bounded and keeps the newest samples.
        let all = recent_samples(usize::MAX);
        assert_eq!(all.len(), SAMPLE_RING_CAP);
        assert_eq!(
            all.last().unwrap().entity_count,
            (SAMPLE_RING_CAP + 9) as u32
        );
        // JSON of the last few is a parseable array of objects.
        let text = samples_json(3);
        let v = crate::json::parse(&text).expect("samples JSON parses");
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), 3);
        assert!(arr[0].get("frame_ms").is_some());
        assert!(arr[0].get("entity_count").is_some());
        clear_samples();
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
