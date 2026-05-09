//! Phase 29 session 4: input frame log + replay.
//!
//! The play loop calls [`tick`] once per simulation step (after
//! `update_key_state` has filled the input ambients, before
//! `tick_frame` runs). When recording, [`tick`] reads the ambients
//! and appends one line per frame to the active recording file.
//! When playing, [`tick`] reads the next line and overwrites the
//! ambients so the script sees synthetic input identical to the
//! captured run.
//!
//! The file format is a small line-based text format chosen to be:
//! - easily diff-friendly (humans can eyeball a regression),
//! - cheap to parse (one `split('|')` per frame),
//! - forwards-compatible (a `v1` header lets later versions stay
//!   readable from a single parser),
//! - free of external dependencies (no serde, no JSON crate; we
//!   already have `src/json.rs` for places that need JSON, but
//!   replay logs benefit from being grep-greppable).
//!
//! ## Format (v1)
//!
//! ```text
//! TWE-REPLAY v1
//! <keys_held>|<keys_pressed>|<mouse_x>|<mouse_y>|<mb_held>|<mb_press>
//! <keys_held>|<keys_pressed>|<mouse_x>|<mouse_y>|<mb_held>|<mb_press>
//! ...
//! ```
//!
//! Each `<keys_*>` and `<mb_*>` field is a comma-separated list of
//! the names whose ambient flag was true that frame. `<mouse_x>` /
//! `<mouse_y>` are decimal floats. Blank fields (no keys held) are
//! the empty string between `|` separators — `||0|0||`.
//!
//! ## What's *not* recorded
//!
//! - Gamepad axes / buttons. v0.1 of replay only covers keyboard +
//!   mouse — the pressure-test target (rhythm, fighting) doesn't use
//!   gamepad in the canonical examples. A v2 line format slots in
//!   when a contributor needs gamepad replay.
//! - System time. Scripts that read wall-clock time (`os.now()` if
//!   it ever ships) will diverge between record + replay; the
//!   determinism contract is "same input → same output", and time
//!   isn't input.
//! - Script-internal RNG state. `random.*` uses a fixed seed by
//!   default; that's enough for replay determinism. Scripts that
//!   reseed from a non-deterministic source break this contract.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::rc::Rc;

use crate::value::{Env, Object, Value};

const HEADER: &str = "TWE-REPLAY v1";

/// State the replay subsystem can be in. Three discrete modes —
/// at most one recorder and at most one player are active at a time.
enum Mode {
    Idle,
    Recording {
        /// Open file handle. Buffered writes; flushed on stop()
        /// (or on Drop via implicit close).
        file: std::io::BufWriter<fs::File>,
    },
    Playing {
        /// All frames pre-loaded so `tick` is O(1) — record format
        /// is small (~60 bytes per frame at typical input density).
        frames: Vec<Frame>,
        /// Index of the next frame to deliver.
        cursor: usize,
    },
}

#[derive(Default, Clone, PartialEq, Debug)]
struct Frame {
    keys_held: Vec<String>,
    keys_pressed: Vec<String>,
    mouse_x: f64,
    mouse_y: f64,
    mb_held: Vec<String>,
    mb_press: Vec<String>,
}

thread_local! {
    static MODE: RefCell<Mode> = const { RefCell::new(Mode::Idle) };
}

/// Begin recording inputs to `path`. Truncates any pre-existing file.
/// If a recording or replay was already in flight, it's stopped
/// first so only one stream is active at a time.
pub fn start_recording(path: &str) -> Result<(), String> {
    stop();
    let f = fs::File::create(path).map_err(|e| format!("replay.record: {e}"))?;
    let mut w = std::io::BufWriter::new(f);
    writeln!(w, "{HEADER}").map_err(|e| format!("replay.record: {e}"))?;
    MODE.with(|m| *m.borrow_mut() = Mode::Recording { file: w });
    Ok(())
}

/// Begin playing back inputs from `path`. Reads the entire file into
/// memory eagerly so per-frame `tick` is allocation-free.
pub fn start_playing(path: &str) -> Result<(), String> {
    stop();
    let src = fs::read_to_string(path).map_err(|e| format!("replay.play: {e}"))?;
    let frames = parse_log(&src)?;
    MODE.with(|m| {
        *m.borrow_mut() = Mode::Playing { frames, cursor: 0 };
    });
    Ok(())
}

/// End any active recording or replay. Drops the file handle
/// (flushes buffered writes) and returns to Idle.
pub fn stop() {
    MODE.with(|m| {
        let mut mode = m.borrow_mut();
        if let Mode::Recording { file } = &mut *mode {
            // Best-effort flush — if the disk is full or the file
            // was unlinked, dropping the BufWriter still drains it
            // but errors are swallowed.
            let _ = file.flush();
        }
        *mode = Mode::Idle;
    });
}

/// True when the replay subsystem is feeding synthetic input — used
/// by the play loop to decide whether to skip the real
/// `update_key_state` call.
pub fn is_playing() -> bool {
    MODE.with(|m| matches!(*m.borrow(), Mode::Playing { .. }))
}

/// True when the replay subsystem is capturing — used by tests +
/// `replay.is_recording()` if it ever ships.
#[allow(dead_code)]
pub fn is_recording() -> bool {
    MODE.with(|m| matches!(*m.borrow(), Mode::Recording { .. }))
}

/// One simulation tick. Called by the play loop AFTER input
/// ambients have been refreshed (or, in playback mode, BEFORE —
/// see `is_playing`). When recording, snapshots the ambients to
/// the log. When playing, overwrites them with the next frame
/// from the log. When the log runs out of frames during playback,
/// switches automatically back to Idle so the player can take over
/// (the script keeps running with whatever real input arrives).
pub fn tick(env: &mut Env) {
    let snap = snapshot_inputs(env);
    let mut should_stop_after = false;
    MODE.with(|m| {
        let mut mode = m.borrow_mut();
        match &mut *mode {
            Mode::Idle => {}
            Mode::Recording { file } => {
                // Errors writing to the log are surfaced once and
                // then ignored — the game shouldn't crash because
                // a debug recording stream broke.
                if let Err(e) = write_frame(file, &snap) {
                    eprintln!("[twec] replay record write failed: {e}");
                }
            }
            Mode::Playing { frames, cursor } => {
                if let Some(f) = frames.get(*cursor).cloned() {
                    apply_frame(env, &f);
                    *cursor += 1;
                } else {
                    // End-of-log: stop replaying and let the next
                    // frame's real input flow through normally.
                    should_stop_after = true;
                }
            }
        }
    });
    if should_stop_after {
        stop();
    }
}

fn snapshot_inputs(env: &Env) -> Frame {
    Frame {
        keys_held: collect_true_field_names(env, "key"),
        keys_pressed: collect_true_field_names(env, "key_press"),
        mouse_x: read_mouse_axis(env, 0),
        mouse_y: read_mouse_axis(env, 1),
        mb_held: collect_true_field_names(env, "mouse_held"),
        mb_press: collect_true_field_names(env, "mouse_press"),
    }
}

fn collect_true_field_names(env: &Env, ambient: &str) -> Vec<String> {
    let opt = env.get(ambient);
    let Some(v) = opt.as_ref() else {
        return Vec::new();
    };
    if !v.is_object() {
        return Vec::new();
    }
    let rc = v.as_object();
    let o = rc.borrow();
    let mut names: Vec<String> = o
        .fields
        .iter()
        .filter_map(|(k, v)| {
            if v.is_bool() && v.as_bool() {
                Some(k.clone())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

fn read_mouse_axis(env: &Env, axis: usize) -> f64 {
    let opt = env.get("mouse");
    let Some(v) = opt.as_ref() else {
        return 0.0;
    };
    if !v.is_object() {
        return 0.0;
    }
    let rc = v.as_object();
    let o = rc.borrow();
    let key = if axis == 0 { "x" } else { "y" };
    if let Some(f) = o.fields.get(key) {
        if f.is_float() {
            return f.as_float();
        }
        if f.is_int_or_boxed_int() {
            return f.as_int() as f64;
        }
    }
    0.0
}

fn apply_frame(env: &mut Env, f: &Frame) {
    set_bool_ambient(env, "key", &f.keys_held);
    set_bool_ambient(env, "key_press", &f.keys_pressed);
    set_bool_ambient(env, "mouse_held", &f.mb_held);
    set_bool_ambient(env, "mouse_press", &f.mb_press);
    write_mouse_pos(env, f.mouse_x, f.mouse_y);
}

fn set_bool_ambient(env: &mut Env, name: &str, true_keys: &[String]) {
    let opt = env.get(name);
    if let Some(v) = opt.as_ref() {
        if v.is_object() {
            let rc = v.as_object();
            let mut o = rc.borrow_mut();
            // Reset every existing key to false, then set the
            // recorded ones true. Ensures keys held in the previous
            // frame but absent from the current frame go back to
            // false (otherwise a held key would stay sticky).
            for (_, slot) in o.fields.iter_mut() {
                if slot.is_bool() {
                    *slot = Value::from_bool(false);
                }
            }
            for k in true_keys {
                o.insert_field(k, Value::from_bool(true));
            }
            return;
        }
    }
    // Lazy install if the ambient doesn't exist yet.
    let mut fields: HashMap<String, Value> = HashMap::new();
    for k in true_keys {
        fields.insert(k.clone(), Value::from_bool(true));
    }
    env.set(
        name.to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields,
            kind: "input",
        }))),
    );
}

fn write_mouse_pos(env: &mut Env, x: f64, y: f64) {
    let opt = env.get("mouse");
    if let Some(v) = opt.as_ref() {
        if v.is_object() {
            let rc = v.as_object();
            let mut o = rc.borrow_mut();
            o.insert_field("x", Value::from_float(x));
            o.insert_field("y", Value::from_float(y));
            return;
        }
    }
    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("x".to_string(), Value::from_float(x));
    fields.insert("y".to_string(), Value::from_float(y));
    env.set(
        "mouse".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields,
            kind: "input",
        }))),
    );
}

// ---------- I/O ----------

fn write_frame(w: &mut std::io::BufWriter<fs::File>, f: &Frame) -> std::io::Result<()> {
    writeln!(
        w,
        "{}|{}|{}|{}|{}|{}",
        f.keys_held.join(","),
        f.keys_pressed.join(","),
        f.mouse_x,
        f.mouse_y,
        f.mb_held.join(","),
        f.mb_press.join(","),
    )
}

fn parse_log(src: &str) -> Result<Vec<Frame>, String> {
    let mut lines = src.lines();
    let header = lines.next().ok_or("replay.play: empty file")?;
    if header.trim() != HEADER {
        return Err(format!(
            "replay.play: bad header (expected `{HEADER}`, got `{}`)",
            header.trim()
        ));
    }
    let mut frames = Vec::new();
    for (i, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() != 6 {
            return Err(format!(
                "replay.play: line {} has {} fields, expected 6",
                i + 2,
                parts.len()
            ));
        }
        let mouse_x: f64 = parts[2]
            .parse()
            .map_err(|e| format!("replay.play: line {}: bad mouse_x ({e})", i + 2))?;
        let mouse_y: f64 = parts[3]
            .parse()
            .map_err(|e| format!("replay.play: line {}: bad mouse_y ({e})", i + 2))?;
        frames.push(Frame {
            keys_held: split_csv(parts[0]),
            keys_pressed: split_csv(parts[1]),
            mouse_x,
            mouse_y,
            mb_held: split_csv(parts[4]),
            mb_press: split_csv(parts[5]),
        });
    }
    Ok(frames)
}

fn split_csv(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(',').map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_two_frame_log() {
        let path = std::env::temp_dir().join("twe-replay-rt-test.log");
        let path = path.to_str().unwrap();
        // Manually build a log file matching the format.
        let body = format!(
            "{HEADER}\nleft,space|space|123.5|45.0|left|left\n||320|240||\n"
        );
        std::fs::write(path, body).unwrap();
        let frames = parse_log(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].keys_held, vec!["left".to_string(), "space".to_string()]);
        assert_eq!(frames[0].keys_pressed, vec!["space".to_string()]);
        assert_eq!(frames[0].mouse_x, 123.5);
        assert_eq!(frames[1].keys_held.len(), 0);
        assert_eq!(frames[1].mouse_x, 320.0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_bad_header() {
        let err = parse_log("WRONG-HEADER\n").err().unwrap();
        assert!(err.contains("bad header"));
    }
}
