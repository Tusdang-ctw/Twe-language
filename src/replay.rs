//! Phase 29 session 4: input frame log + replay. web3d-M4: the input
//! command stream.
//!
//! Every simulation tick consumes one [`InputCommand`]: the keys and
//! mouse buttons held and newly pressed, and the mouse position,
//! motion and wheel. The 3D shells build one per tick (see
//! `host3d::InputState`) and pass it through [`step`]; the 2D play loop
//! snapshots its ambients once per frame and calls [`tick`]. Either way
//! the command goes through here, so:
//!
//! - **recording** (`replay.record(path)`) appends it to a log,
//! - **playing** (`replay.play(path)`) swaps in the logged command, so
//!   the script sees exactly the recorded input,
//! - an always-on ring keeps the last 30 s for crash reports.
//!
//! Input is the only thing that enters the simulation from outside, so
//! a log of commands reproduces a run (the net-ready hook: a remote
//! peer's commands would arrive the same way). Logs are text, stored
//! through `save::write_text`, so on the web they live in localStorage.
//!
//! ## Format
//!
//! ```text
//! TWE-REPLAY v2
//! <keys_held>|<keys_pressed>|<mouse_x>|<mouse_y>|<mb_held>|<mb_press>|<mouse_dx>|<mouse_dy>|<wheel>
//! ...
//! ```
//!
//! Each `<keys_*>` and `<mb_*>` field is a comma-separated list of
//! names; blank fields are empty strings between `|`s. v1 logs (the
//! first six fields only, header `TWE-REPLAY v1`) still play.
//!
//! ## What's *not* recorded
//!
//! - Gamepad axes / buttons (a v3 line format slots in when a game
//!   needs them).
//! - Wall-clock time. The determinism contract is "same input, same
//!   output", and time isn't input.
//! - Script-internal RNG state. `random.*` uses a fixed seed by
//!   default; scripts that reseed from a non-deterministic source
//!   break the contract.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use crate::value::{Env, Object, Value};

const HEADER: &str = "TWE-REPLAY v2";
const HEADER_V1: &str = "TWE-REPLAY v1";

/// A recording is written out every this many ticks (and on stop), so
/// a game closed without `replay.stop()` keeps almost all of it.
const FLUSH_EVERY: u32 = 300;

/// State the replay subsystem can be in. At most one recorder and at
/// most one player are active at a time.
enum Mode {
    Idle,
    Recording {
        path: String,
        /// The whole log so far (header included).
        text: String,
        since_flush: u32,
    },
    Playing {
        /// All frames pre-loaded so a step is O(1).
        frames: Vec<InputCommand>,
        /// Index of the next frame to deliver.
        cursor: usize,
    },
}

/// Everything one simulation tick receives from outside.
#[derive(Default, Clone, PartialEq, Debug)]
pub struct InputCommand {
    pub keys_held: Vec<String>,
    pub keys_pressed: Vec<String>,
    pub mouse_x: f64,
    pub mouse_y: f64,
    pub mb_held: Vec<String>,
    pub mb_press: Vec<String>,
    pub mouse_dx: f64,
    pub mouse_dy: f64,
    pub wheel: f64,
}

type Frame = InputCommand;

thread_local! {
    static MODE: RefCell<Mode> = const { RefCell::new(Mode::Idle) };

    // v1.0.1 session 10: always-on input ring. Captures the last
    // ~30s of commands (1800 at 60 Hz). On crash,
    // `dump_ring_to(path)` writes it as a replay log so the bug can
    // be reproduced with `twec replay <script> <log>`. It runs
    // alongside Mode: a crash during a deliberate recording still
    // gets the rolling snapshot.
    static RING: RefCell<Ring> = const { RefCell::new(Ring::new()) };
}

/// Bounded circular buffer of recent input frames. Capacity matches
/// the design target: 30 seconds at 60 Hz.
pub const RING_CAPACITY: usize = 30 * 60;

struct Ring {
    buf: Vec<Frame>,
    head: usize,
    len: usize,
}

impl Ring {
    const fn new() -> Self {
        Self {
            buf: Vec::new(),
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, f: Frame) {
        if self.buf.len() < RING_CAPACITY {
            self.buf.push(f);
            self.len = self.buf.len();
            self.head = self.len % RING_CAPACITY;
            return;
        }
        // At capacity — overwrite the oldest entry.
        self.buf[self.head] = f;
        self.head = (self.head + 1) % RING_CAPACITY;
        self.len = RING_CAPACITY;
    }

    /// Snapshot the ring in chronological order (oldest first). Used
    /// by the crash-dump path; the live ring is never drained.
    fn snapshot(&self) -> Vec<Frame> {
        if self.len < RING_CAPACITY {
            self.buf.clone()
        } else {
            let mut out = Vec::with_capacity(self.len);
            for i in 0..self.len {
                let idx = (self.head + i) % RING_CAPACITY;
                out.push(self.buf[idx].clone());
            }
            out
        }
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.buf.clear();
        self.head = 0;
        self.len = 0;
    }
}

/// Begin recording commands to `path` (a file natively, a
/// localStorage key on the web). Replaces anything stored there. A
/// recording or replay already in flight is stopped first.
pub fn start_recording(path: &str) -> Result<(), String> {
    stop();
    let text = format!("{HEADER}\n");
    crate::save::write_text(Path::new(path), &text).map_err(|e| format!("replay.record: {e}"))?;
    MODE.with(|m| {
        *m.borrow_mut() = Mode::Recording {
            path: path.to_string(),
            text,
            since_flush: 0,
        }
    });
    Ok(())
}

/// Begin playing back commands from `path`. Reads the whole log up
/// front so each step is allocation-free.
pub fn start_playing(path: &str) -> Result<(), String> {
    stop();
    let src = crate::save::read_text(Path::new(path)).map_err(|e| format!("replay.play: {e}"))?;
    let frames = parse_log(&src)?;
    MODE.with(|m| {
        *m.borrow_mut() = Mode::Playing { frames, cursor: 0 };
    });
    Ok(())
}

/// End any active recording (writing it out) or replay.
pub fn stop() {
    let finished = MODE.with(|m| std::mem::replace(&mut *m.borrow_mut(), Mode::Idle));
    if let Mode::Recording { path, text, .. } = finished {
        flush(&path, &text);
    }
}

fn flush(path: &str, text: &str) {
    // A debug recording that can't be written shouldn't stop the game.
    if let Err(e) = crate::save::write_text(Path::new(path), text) {
        eprintln!("[twec] replay record write failed: {e}");
    }
}

/// True when the replay subsystem is feeding recorded input.
pub fn is_playing() -> bool {
    MODE.with(|m| matches!(*m.borrow(), Mode::Playing { .. }))
}

/// True when the replay subsystem is capturing.
pub fn is_recording() -> bool {
    MODE.with(|m| matches!(*m.borrow(), Mode::Recording { .. }))
}

/// One simulation tick's input. Returns the command the tick should
/// see: `cmd` itself, or the logged one while playing. When the log
/// runs out, playback stops and live input takes over.
pub fn step(cmd: InputCommand) -> InputCommand {
    RING.with(|r| r.borrow_mut().push(cmd.clone()));
    let mut out = None;
    let mut ended = false;
    let mut to_flush = None;
    MODE.with(|m| match &mut *m.borrow_mut() {
        Mode::Idle => {}
        Mode::Recording {
            path,
            text,
            since_flush,
        } => {
            push_line(text, &cmd);
            *since_flush += 1;
            if *since_flush >= FLUSH_EVERY {
                *since_flush = 0;
                to_flush = Some((path.clone(), text.clone()));
            }
        }
        Mode::Playing { frames, cursor } => match frames.get(*cursor) {
            Some(f) => {
                out = Some(f.clone());
                *cursor += 1;
            }
            None => ended = true,
        },
    });
    if let Some((path, text)) = to_flush {
        flush(&path, &text);
    }
    if ended {
        stop();
    }
    out.unwrap_or(cmd)
}

/// The 2D play loop's per-frame hook, called after it has refreshed the
/// input ambients: records them, or overwrites them while playing.
pub fn tick(env: &mut Env) {
    let live = snapshot_inputs(env);
    let seen = step(live.clone());
    if seen != live {
        apply_frame(env, &seen);
    }
}

fn snapshot_inputs(env: &Env) -> Frame {
    Frame {
        keys_held: collect_true_field_names(env, "key"),
        keys_pressed: collect_true_field_names(env, "key_press"),
        mouse_x: read_mouse_axis(env, "x"),
        mouse_y: read_mouse_axis(env, "y"),
        mb_held: collect_true_field_names(env, "mouse_held"),
        mb_press: collect_true_field_names(env, "mouse_press"),
        mouse_dx: read_mouse_axis(env, "dx"),
        mouse_dy: read_mouse_axis(env, "dy"),
        wheel: read_mouse_axis(env, "wheel"),
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

fn read_mouse_axis(env: &Env, key: &str) -> f64 {
    let opt = env.get("mouse");
    let Some(v) = opt.as_ref() else {
        return 0.0;
    };
    if !v.is_object() {
        return 0.0;
    }
    let rc = v.as_object();
    let o = rc.borrow();
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
    write_mouse(env, f);
}

fn set_bool_ambient(env: &mut Env, name: &str, true_keys: &[String]) {
    let opt = env.get(name);
    if let Some(v) = opt.as_ref() {
        if v.is_object() {
            let rc = v.as_object();
            let mut o = rc.borrow_mut();
            // Reset every existing key to false, then set the
            // recorded ones true, so a key held last frame but not
            // this one goes back to false.
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

fn write_mouse(env: &mut Env, f: &Frame) {
    let fields = [
        ("x", f.mouse_x),
        ("y", f.mouse_y),
        ("dx", f.mouse_dx),
        ("dy", f.mouse_dy),
        ("wheel", f.wheel),
    ];
    let opt = env.get("mouse");
    if let Some(v) = opt.as_ref() {
        if v.is_object() {
            let rc = v.as_object();
            let mut o = rc.borrow_mut();
            for (k, x) in fields {
                o.insert_field(k, Value::from_float(x));
            }
            o.insert_field(
                "pos",
                Value::from_tuple(vec![
                    Value::from_float(f.mouse_x),
                    Value::from_float(f.mouse_y),
                ]),
            );
            return;
        }
    }
    let map: HashMap<String, Value> = fields
        .iter()
        .map(|(k, x)| (k.to_string(), Value::from_float(*x)))
        .collect();
    env.set(
        "mouse".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: map,
            kind: "input",
        }))),
    );
}

// ---------- I/O ----------

/// v1.0.1 session 10: write the in-memory ring to a replay log at
/// `path`. Called by the crash-reporter hook in
/// `cli::install_crash_reporter`. Returns the number of frames
/// written. An empty ring still writes a header so the file is
/// recognisable as a Twe replay log.
pub fn dump_ring_to(path: &Path) -> std::io::Result<usize> {
    let frames = RING.with(|r| r.borrow().snapshot());
    let mut text = format!("{HEADER}\n");
    for fr in &frames {
        push_line(&mut text, fr);
    }
    std::fs::write(path, text)?;
    Ok(frames.len())
}

/// Drop the ring contents — used by tests that need a clean baseline.
#[cfg(test)]
pub fn clear_ring_for_test() {
    RING.with(|r| r.borrow_mut().clear());
}

#[cfg(test)]
pub fn ring_len_for_test() -> usize {
    RING.with(|r| r.borrow().len)
}

fn push_line(out: &mut String, f: &Frame) {
    use std::fmt::Write;
    let _ = writeln!(
        out,
        "{}|{}|{}|{}|{}|{}|{}|{}|{}",
        f.keys_held.join(","),
        f.keys_pressed.join(","),
        f.mouse_x,
        f.mouse_y,
        f.mb_held.join(","),
        f.mb_press.join(","),
        f.mouse_dx,
        f.mouse_dy,
        f.wheel,
    );
}

fn parse_log(src: &str) -> Result<Vec<Frame>, String> {
    let mut lines = src.lines();
    let header = lines.next().ok_or("replay.play: empty file")?.trim();
    let fields = match header {
        HEADER => 9,
        HEADER_V1 => 6,
        other => {
            return Err(format!(
                "replay.play: bad header (expected `{HEADER}`, got `{other}`)"
            ))
        }
    };
    let num = |s: &str, what: &str, line: usize| -> Result<f64, String> {
        s.parse()
            .map_err(|e| format!("replay.play: line {line}: bad {what} ({e})"))
    };
    let mut frames = Vec::new();
    for (i, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let n = i + 2;
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() != fields {
            return Err(format!(
                "replay.play: line {n} has {} fields, expected {fields}",
                parts.len()
            ));
        }
        let mut f = Frame {
            keys_held: split_csv(parts[0]),
            keys_pressed: split_csv(parts[1]),
            mouse_x: num(parts[2], "mouse_x", n)?,
            mouse_y: num(parts[3], "mouse_y", n)?,
            mb_held: split_csv(parts[4]),
            mb_press: split_csv(parts[5]),
            ..Frame::default()
        };
        if fields == 9 {
            f.mouse_dx = num(parts[6], "mouse_dx", n)?;
            f.mouse_dy = num(parts[7], "mouse_dy", n)?;
            f.wheel = num(parts[8], "wheel", n)?;
        }
        frames.push(f);
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
        let body = format!("{HEADER_V1}\nleft,space|space|123.5|45.0|left|left\n||320|240||\n");
        std::fs::write(path, body).unwrap();
        let frames = parse_log(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[0].keys_held,
            vec!["left".to_string(), "space".to_string()]
        );
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

    // --- v1.0.1 session 10: input ring + crash dump ---

    #[test]
    fn ring_dump_writes_last_n_frames_after_overflow() {
        // The ring is shared thread-local state across the test
        // suite; serialise via a dedicated thread + clear before.
        clear_ring_for_test();
        // Push more than RING_CAPACITY frames and confirm the ring
        // only retains the last RING_CAPACITY (oldest entries
        // evicted in arrival order).
        let n = RING_CAPACITY + 50;
        for i in 0..n {
            let f = Frame {
                keys_held: vec![format!("k{i}")],
                ..Default::default()
            };
            RING.with(|r| r.borrow_mut().push(f));
        }
        let path =
            std::env::temp_dir().join(format!("twec-ring-dump-{}.replay", std::process::id()));
        let written = dump_ring_to(&path).expect("dump");
        assert_eq!(written, RING_CAPACITY);
        // First line of body should be the OLDEST surviving frame,
        // which is the (n - RING_CAPACITY)-th frame pushed.
        let body = std::fs::read_to_string(&path).unwrap();
        let first_data_line = body.lines().nth(1).expect("body");
        let expected_first_key = format!("k{}", n - RING_CAPACITY);
        assert!(
            first_data_line.starts_with(&expected_first_key),
            "wrong first frame: got `{first_data_line}`, want `{expected_first_key}`"
        );
        let _ = std::fs::remove_file(&path);
        clear_ring_for_test();
    }

    #[test]
    fn ring_dump_round_trips_through_parse_log() {
        clear_ring_for_test();
        for i in 0..5u32 {
            let f = Frame {
                keys_held: vec![if i % 2 == 0 { "up" } else { "down" }.into()],
                mouse_x: i as f64 * 10.0,
                mouse_y: i as f64 * 20.0,
                ..Default::default()
            };
            RING.with(|r| r.borrow_mut().push(f));
        }
        let path =
            std::env::temp_dir().join(format!("twec-ring-roundtrip-{}.replay", std::process::id()));
        dump_ring_to(&path).expect("dump");
        let body = std::fs::read_to_string(&path).unwrap();
        let frames = parse_log(&body).expect("parse");
        assert_eq!(frames.len(), 5);
        assert_eq!(frames[0].keys_held, vec!["up".to_string()]);
        assert_eq!(frames[1].mouse_x, 10.0);
        assert_eq!(frames[2].keys_held, vec!["up".to_string()]);
        let _ = std::fs::remove_file(&path);
        clear_ring_for_test();
    }
}
