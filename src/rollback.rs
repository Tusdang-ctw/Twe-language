//! Phase 37 sessions 2–6: rollback netcode state.
//!
//! Per `docs/changes/2026-05-11-rollback-rfc.md` rollback is a second
//! netcode mode alongside lockstep. The mode is per-session, opt-in
//! via `net.set_mode("rollback")`. Within rollback mode:
//!
//! - The runner snapshots rollback-tagged state at every tick.
//! - When a peer's real input arrives N frames late, the runner
//!   rewinds to tick (current - N), restores the snapshot, replaces
//!   the predicted input with the real input, and re-simulates
//!   forward to the current tick.
//! - The local player sees a smoothed-position render (lerped across
//!   the rewind) so corrections look like minor drift rather than
//!   teleport snap-backs.
//!
//! This module owns: the snapshot ring buffer, the mode flag, the
//! input-prediction policy, the smoothing flag, and the runtime
//! `is_replaying` flag (true during a rewind so scripts can suppress
//! side effects like particle spawns).
//!
//! What this module does NOT own:
//!
//! - The `entity Fighter: rollback = true` parser/AST (Phase 37
//!   session 5). The runtime marker `entities.is_rollback(e)` reads
//!   the AST field set there.
//! - The lockstep runner (`src/net.rs`). Mode switching is observed
//!   from `net.rs` via `rollback::mode()`; the runner branches
//!   internally.
//! - Visual smoothing math beyond the per-entity `_smoothed_x` /
//!   `_smoothed_y` cache (Phase 37 session 6 wires the render-time
//!   interpolation).
//!
//! Wire format is unchanged from lockstep — rollback peers exchange
//! the same `MSG_INPUT` payload at the same per-tick cadence. The
//! mode-mismatch check lives in `net.rs`'s `MSG_HELLO` handshake.

#![cfg(not(target_arch = "wasm32"))]

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;

use crate::json;
use crate::value::Value;

/// Active netcode mode for the current session. Per the RFC, the
/// mode is per-session, not per-build — both runners ship; the
/// script picks at `net.create_lobby` / `net.host` time. Default is
/// `Lockstep` (preserves Phase 31 behaviour for every existing
/// example).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Lockstep,
    Rollback,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Lockstep => "lockstep",
            Mode::Rollback => "rollback",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "lockstep" => Some(Mode::Lockstep),
            "rollback" => Some(Mode::Rollback),
            _ => None,
        }
    }
}

/// Predicted-input policy used when a remote peer's input frame for
/// a given tick is missing (the rollback runner needs *something*
/// to feed the simulation forward; the choice is which heuristic).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputPrediction {
    /// Use the peer's most recent received frame, repeated. Cheap,
    /// trivially deterministic across peers, works well for
    /// held-key fighters (kick still-being-held → kick again next
    /// frame). Default.
    LastInputRepeat,
    /// Use the peer's last-known position + velocity to extrapolate
    /// where they "would be" this frame. More expensive; better fit
    /// for FPS where movement velocity is a stronger signal than
    /// the held-key bitmap. Determinism requires the velocity field
    /// to live on each rollback entity and be snapshotted alongside.
    VelocityExtrapolate,
}

impl InputPrediction {
    pub fn as_str(self) -> &'static str {
        match self {
            InputPrediction::LastInputRepeat => "last-input-repeat",
            InputPrediction::VelocityExtrapolate => "velocity-extrapolate",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "last-input-repeat" => Some(InputPrediction::LastInputRepeat),
            "velocity-extrapolate" => Some(InputPrediction::VelocityExtrapolate),
            _ => None,
        }
    }
}

/// Soft cap on how far the rollback runner will rewind. If a peer's
/// real input is more than `max_rewind_frames` ticks late, the
/// runner gives up + treats the predicted simulation as authoritative
/// (a desync may follow; the per-60-tick state-hash check will catch
/// it). Default 8 — sized for the typical 0–120ms peer latency window
/// at 60Hz.
pub const DEFAULT_MAX_REWIND_FRAMES: u32 = 8;

/// One snapshot ring entry — the full set of `(name → JSON-encoded
/// value)` saved at one tick. The lockstep runner doesn't populate
/// this; only the rollback runner + scripts calling
/// `rollback.snapshot` do.
#[derive(Debug, Default)]
struct RingEntry {
    tick: u32,
    snapshots: HashMap<String, json::Value>,
}

struct RollbackState {
    mode: Mode,
    smoothing: bool,
    input_prediction: InputPrediction,
    max_rewind_frames: u32,
    /// Most recently advanced tick. `snapshot(name, value)` writes
    /// into the entry for this tick.
    current_tick: u32,
    /// Whether the runner is currently in a rewind-and-replay
    /// loop. Scripts read this via `rollback.is_replaying()` to
    /// guard side effects (particle spawns, audio cues, etc.).
    is_replaying: bool,
    /// Snapshot ring — newest entry at the back, oldest at the
    /// front. Capped at `max_rewind_frames + 1` entries (the +1
    /// covers the current tick).
    ring: VecDeque<RingEntry>,
    /// Stats counters surfaced by `rollback.stats()`.
    predicted_total: u64,
    corrected_total: u64,
    last_correction_frames: u32,
}

impl Default for RollbackState {
    fn default() -> Self {
        Self {
            mode: Mode::Lockstep,
            smoothing: true,
            input_prediction: InputPrediction::LastInputRepeat,
            max_rewind_frames: DEFAULT_MAX_REWIND_FRAMES,
            current_tick: 0,
            is_replaying: false,
            ring: VecDeque::new(),
            predicted_total: 0,
            corrected_total: 0,
            last_correction_frames: 0,
        }
    }
}

thread_local! {
    static STATE: RefCell<RollbackState> = RefCell::new(RollbackState::default());
}

// ---- mode + knob setters/getters ----

pub fn set_mode(mode: Mode) {
    STATE.with(|s| s.borrow_mut().mode = mode);
}

pub fn mode() -> Mode {
    STATE.with(|s| s.borrow().mode)
}

pub fn set_smoothing(on: bool) {
    STATE.with(|s| s.borrow_mut().smoothing = on);
}

pub fn smoothing() -> bool {
    STATE.with(|s| s.borrow().smoothing)
}

pub fn set_input_prediction(p: InputPrediction) {
    STATE.with(|s| s.borrow_mut().input_prediction = p);
}

pub fn input_prediction() -> InputPrediction {
    STATE.with(|s| s.borrow().input_prediction)
}

pub fn set_max_rewind_frames(n: u32) {
    STATE.with(|s| {
        s.borrow_mut().max_rewind_frames = n.clamp(1, 60);
    });
}

pub fn max_rewind_frames() -> u32 {
    STATE.with(|s| s.borrow().max_rewind_frames)
}

pub fn is_replaying() -> bool {
    STATE.with(|s| s.borrow().is_replaying)
}

/// Set the replaying flag. Called by the rewind-and-replay loop in
/// session 3.
pub fn set_replaying(on: bool) {
    STATE.with(|s| s.borrow_mut().is_replaying = on);
}

// ---- ring buffer ----

/// Bump current_tick + ensure a ring entry exists for it. Drops the
/// oldest entry if the ring is over capacity.
pub fn advance_tick(tick: u32) {
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.current_tick = tick;
        // Drop entries older than the rewind window.
        let cap = st.max_rewind_frames as usize + 1;
        while st.ring.len() >= cap {
            st.ring.pop_front();
        }
        // Add a fresh entry for this tick if not already present.
        if st.ring.back().map(|e| e.tick) != Some(tick) {
            st.ring.push_back(RingEntry {
                tick,
                snapshots: HashMap::new(),
            });
        }
    });
}

pub fn current_tick() -> u32 {
    STATE.with(|s| s.borrow().current_tick)
}

/// Save `value` under `name` at the current tick. The script calls
/// this once per rollback-tracked variable per tick; the rewind
/// engine looks them up at rewind time.
///
/// Determinism: the value is JSON-encoded via
/// `crate::save::encode`, which is the canonical-JSON path Phase 36
/// already certifies as deterministic. The same value snapshotted
/// on two different peers produces byte-identical JSON.
pub fn snapshot(name: &str, value: &Value) -> Result<(), String> {
    let json = crate::save::encode(value)?;
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        // Find the entry for current_tick (advance_tick guarantees
        // it exists if called by the runner — but if a script calls
        // snapshot before any tick has advanced, push one now).
        let current = st.current_tick;
        if st.ring.back().map(|e| e.tick) != Some(current) {
            st.ring.push_back(RingEntry {
                tick: current,
                snapshots: HashMap::new(),
            });
        }
        if let Some(entry) = st.ring.back_mut() {
            entry.snapshots.insert(name.to_string(), json);
        }
    });
    Ok(())
}

/// Look up a snapshot by name. Returns the most recent value
/// snapshotted under that name (across the whole ring — newer wins).
/// Used by the rewind engine + by scripts that want to read a past
/// state.
pub fn restore(name: &str) -> Option<Value> {
    STATE.with(|s| {
        let st = s.borrow();
        for entry in st.ring.iter().rev() {
            if let Some(j) = entry.snapshots.get(name) {
                return Some(crate::save::decode(j));
            }
        }
        None
    })
}

/// Look up a snapshot by name *at a specific tick*. Returns None if
/// the tick is no longer in the ring or the name wasn't snapshotted
/// at that tick. Used by the rewind engine in session 3.
pub fn restore_at_tick(name: &str, tick: u32) -> Option<Value> {
    STATE.with(|s| {
        let st = s.borrow();
        for entry in &st.ring {
            if entry.tick == tick {
                if let Some(j) = entry.snapshots.get(name) {
                    return Some(crate::save::decode(j));
                }
            }
        }
        None
    })
}

/// Remove entries with tick > `tick` from the ring. Called by the
/// rewind engine before re-simulating; we don't want stale "future"
/// snapshots from before the rewind to confuse `restore_at_tick`.
pub fn discard_after(tick: u32) {
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.ring.retain(|e| e.tick <= tick);
    });
}

// ---- stats ----

pub fn record_prediction() {
    STATE.with(|s| s.borrow_mut().predicted_total += 1);
}

pub fn record_correction(frames_rewound: u32) {
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.corrected_total += 1;
        st.last_correction_frames = frames_rewound;
    });
}

#[derive(Debug, Clone)]
pub struct Stats {
    pub predicted: u64,
    pub corrected: u64,
    pub last_correction_frames: u32,
    pub ring_len: usize,
}

pub fn stats() -> Stats {
    STATE.with(|s| {
        let st = s.borrow();
        Stats {
            predicted: st.predicted_total,
            corrected: st.corrected_total,
            last_correction_frames: st.last_correction_frames,
            ring_len: st.ring.len(),
        }
    })
}

/// Reset the rollback state. Called from `net::close` so the next
/// session starts from a clean slate.
pub fn reset() {
    STATE.with(|s| *s.borrow_mut() = RollbackState::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() {
        reset();
    }

    #[test]
    fn mode_round_trips() {
        fresh();
        assert_eq!(mode(), Mode::Lockstep);
        set_mode(Mode::Rollback);
        assert_eq!(mode(), Mode::Rollback);
        assert_eq!(Mode::parse("rollback"), Some(Mode::Rollback));
        assert_eq!(Mode::parse("garbage"), None);
        assert_eq!(Mode::Rollback.as_str(), "rollback");
    }

    #[test]
    fn input_prediction_round_trips() {
        fresh();
        assert_eq!(input_prediction(), InputPrediction::LastInputRepeat);
        set_input_prediction(InputPrediction::VelocityExtrapolate);
        assert_eq!(input_prediction(), InputPrediction::VelocityExtrapolate);
        assert_eq!(
            InputPrediction::parse("velocity-extrapolate"),
            Some(InputPrediction::VelocityExtrapolate)
        );
    }

    #[test]
    fn snapshot_and_restore_round_trip() {
        fresh();
        advance_tick(1);
        snapshot("hp", &Value::from_int(100)).unwrap();
        snapshot("x", &Value::from_float(123.5)).unwrap();
        let hp = restore("hp").expect("hp present");
        let x = restore("x").expect("x present");
        assert_eq!(hp.as_int(), 100);
        assert_eq!(x.as_float(), 123.5);
    }

    #[test]
    fn ring_caps_at_max_rewind_plus_one() {
        fresh();
        set_max_rewind_frames(3);
        // Advance through more ticks than the cap. The oldest should
        // be evicted.
        for t in 1..=10 {
            advance_tick(t);
            snapshot("x", &Value::from_int(t as i64)).unwrap();
        }
        // We should still be able to restore the most recent value.
        assert_eq!(restore("x").map(|v| v.as_int()), Some(10));
        // Cap = max_rewind_frames + 1 = 4. So tick 7..=10 are in the
        // ring; ticks 1..=6 are gone.
        assert_eq!(restore_at_tick("x", 10).map(|v| v.as_int()), Some(10));
        assert_eq!(restore_at_tick("x", 7).map(|v| v.as_int()), Some(7));
        assert!(restore_at_tick("x", 6).is_none());
    }

    #[test]
    fn discard_after_drops_future_ticks() {
        fresh();
        for t in 1..=5 {
            advance_tick(t);
            snapshot("x", &Value::from_int(t as i64)).unwrap();
        }
        discard_after(3);
        assert!(restore_at_tick("x", 5).is_none());
        assert!(restore_at_tick("x", 4).is_none());
        assert_eq!(restore_at_tick("x", 3).map(|v| v.as_int()), Some(3));
    }

    #[test]
    fn stats_count_predictions_and_corrections() {
        fresh();
        for _ in 0..5 {
            record_prediction();
        }
        record_correction(4);
        let st = stats();
        assert_eq!(st.predicted, 5);
        assert_eq!(st.corrected, 1);
        assert_eq!(st.last_correction_frames, 4);
    }

    #[test]
    fn max_rewind_frames_clamps_to_60() {
        fresh();
        set_max_rewind_frames(0);
        assert_eq!(max_rewind_frames(), 1);
        set_max_rewind_frames(120);
        assert_eq!(max_rewind_frames(), 60);
    }
}
