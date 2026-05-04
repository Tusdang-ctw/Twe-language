//! `twec play` — interactive macroquad backend.
//!
//! Owns the real game loop: reads keyboard input each frame, ticks the
//! interpreter's active scene, runs `on render():` handlers inside a
//! macroquad draw frame. The headless `twec run` path remains unchanged
//! and is still the test entry point.
//!
//! Hot reload: the file's mtime is polled at the top of every frame.
//! If it changed, the script is re-lexed, re-parsed, and a fresh `Env`
//! initialised from scratch. Phase-2 simplification: scene fields are
//! NOT preserved across reloads. Iteration is fast; tweaking values
//! and saving brings the change in immediately, but in-flight game
//! state is lost.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::SystemTime;

use macroquad::prelude::*;

use crate::value::{Env, Object, Value};

const KEYS: &[(&str, KeyCode)] = &[
    ("right", KeyCode::Right),
    ("left", KeyCode::Left),
    ("up", KeyCode::Up),
    ("down", KeyCode::Down),
    ("space", KeyCode::Space),
    ("escape", KeyCode::Escape),
    ("enter", KeyCode::Enter),
    ("r", KeyCode::R),
    ("w", KeyCode::W),
    ("a", KeyCode::A),
    ("s", KeyCode::S),
    ("d", KeyCode::D),
];

const MOUSE_BUTTONS: &[(&str, MouseButton)] = &[
    ("left", MouseButton::Left),
    ("middle", MouseButton::Middle),
    ("right", MouseButton::Right),
];

/// Phase 9 session 5: Twe field name → gilrs::Button mapping. Order
/// must mirror the `GAMEPAD_BUTTON_NAMES` const in stdlib (since
/// scripts read fields by name, the Rust list is the source of truth
/// for which buttons we poll).
const GAMEPAD_BUTTONS: &[(&str, gilrs::Button)] = &[
    ("a", gilrs::Button::South),
    ("b", gilrs::Button::East),
    ("x", gilrs::Button::West),
    ("y", gilrs::Button::North),
    ("lb", gilrs::Button::LeftTrigger),
    ("rb", gilrs::Button::RightTrigger),
    ("lt", gilrs::Button::LeftTrigger2),
    ("rt", gilrs::Button::RightTrigger2),
    ("start", gilrs::Button::Start),
    ("select", gilrs::Button::Select),
    ("dup", gilrs::Button::DPadUp),
    ("ddown", gilrs::Button::DPadDown),
    ("dleft", gilrs::Button::DPadLeft),
    ("dright", gilrs::Button::DPadRight),
];

/// Twe axis name → gilrs::Axis mapping. Triggers `lt` / `rt` use
/// `LeftZ` / `RightZ` (gilrs's analog trigger axes); the `lt` / `rt`
/// fields on `gamepad` and `gamepad_press` are the thresholded
/// boolean variants.
const GAMEPAD_AXES: &[(&str, gilrs::Axis)] = &[
    ("lx", gilrs::Axis::LeftStickX),
    ("ly", gilrs::Axis::LeftStickY),
    ("rx", gilrs::Axis::RightStickX),
    ("ry", gilrs::Axis::RightStickY),
    ("lt", gilrs::Axis::LeftZ),
    ("rt", gilrs::Axis::RightZ),
];

thread_local! {
    /// Lazily-initialised gilrs context. None means "tried and failed
    /// to initialise" — we log the error once and continue without
    /// gamepad input. Re-initialisation is not supported (matches
    /// gilrs's lifecycle expectations).
    static GILRS: RefCell<GilrsState> = const { RefCell::new(GilrsState::Uninit) };
    /// Previous-frame button states for edge-triggered detection.
    /// Cleared on hot reload by `clear_gamepad_state`.
    static PREV_GAMEPAD: RefCell<[bool; 14]> = const { RefCell::new([false; 14]) };
}

enum GilrsState {
    Uninit,
    // Boxed: gilrs::Gilrs is ~230 bytes (XInput buffers + connection
    // table); without the Box, clippy's large_enum_variant fires
    // because Uninit / Failed carry no data.
    Active(Box<gilrs::Gilrs>),
    Failed,
}

/// Reset the previous-frame gamepad state so a hot reload doesn't
/// produce phantom press events.
pub fn clear_gamepad_state() {
    PREV_GAMEPAD.with(|p| *p.borrow_mut() = [false; 14]);
}

/// Phase 11 session 4: hot-reload reliability gate.
///
/// Editor save sequences are typically `truncate → write → close` on
/// POSIX or `write-temp → rename` on Windows. The Phase 1 mtime-poll
/// raced both: a poll between truncate and the final write read a
/// half-empty file and produced a parse error. The gate fixes that
/// by debouncing — when mtime first changes, start a countdown; the
/// reload only fires after the mtime has been stable for
/// `DEBOUNCE_FRAMES` consecutive polls. A mid-debounce mtime change
/// resets the countdown.
///
/// `reloaded_mtime` is the mtime we've already loaded against. The
/// gate ignores polls that don't differ from it.
pub struct ReloadGate {
    /// Last mtime that was successfully loaded (or initial mtime).
    reloaded_mtime: Option<SystemTime>,
    /// Pending mtime + frames remaining before we attempt the reload.
    pending: Option<(SystemTime, u32)>,
}

const RELOAD_DEBOUNCE_FRAMES: u32 = 6;

impl ReloadGate {
    pub fn new(initial_mtime: Option<SystemTime>) -> Self {
        Self {
            reloaded_mtime: initial_mtime,
            pending: None,
        }
    }

    /// Drive the gate one frame forward. Returns `true` exactly once
    /// per stable mtime change — when the debounce countdown reaches
    /// zero. `cur` is the file's current mtime (None if the path is
    /// unreadable; we treat that as "no change" rather than panic).
    pub fn should_reload(&mut self, cur: Option<SystemTime>) -> bool {
        let Some(cur) = cur else {
            return false;
        };
        if Some(cur) == self.reloaded_mtime {
            // Stable at the loaded version. Drop any pending state —
            // the file came back to the same mtime (e.g. a touch then
            // an undo).
            self.pending = None;
            return false;
        }
        match self.pending {
            None => {
                self.pending = Some((cur, RELOAD_DEBOUNCE_FRAMES));
                false
            }
            Some((p, _)) if p != cur => {
                // Mid-debounce mtime change — restart the countdown
                // with the newer mtime.
                self.pending = Some((cur, RELOAD_DEBOUNCE_FRAMES));
                false
            }
            Some((p, frames)) => {
                if frames > 1 {
                    self.pending = Some((p, frames - 1));
                    false
                } else {
                    self.pending = None;
                    self.reloaded_mtime = Some(p);
                    true
                }
            }
        }
    }
}

pub fn launch(path: String) -> i32 {
    let conf = window_conf();
    macroquad::Window::from_config(conf, run_loop(path));
    0
}

/// `twec play --vm bytecode <file>` entry. Mirrors `launch` but
/// drives the bytecode VM (`vm.tick(dt)` + `vm.render()`) instead
/// of `eval::tick_frame` + `eval::render_frame`. Hot reload still
/// rebuilds from scratch on file change. The macroquad input /
/// screen-size pumps go through the same Rc-shared Objects the
/// stdlib installs, so they reach both interpreters identically.
pub fn launch_bytecode(path: String) -> i32 {
    let conf = window_conf();
    macroquad::Window::from_config(conf, run_loop_bytecode(path));
    0
}

/// Phase 12 session 4: launch the runtime against an in-memory
/// `main.twe` source, with no hot-reload (the source lives in the
/// `.exe`; nothing to watch). Used by self-extracting binaries —
/// `cli::run` detects an embedded bundle, extracts main.twe, sets
/// the bundle as the active asset source, then calls this.
pub fn launch_embedded(source: String) -> i32 {
    let conf = window_conf();
    macroquad::Window::from_config(conf, run_loop_embedded(source));
    0
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Twec play".to_string(),
        window_width: 640,
        window_height: 480,
        high_dpi: true,
        fullscreen: false,
        sample_count: 1,
        window_resizable: true,
        platform: Default::default(),
        icon: None,
    }
}

async fn run_loop(path: String) {
    let path_ref = path.clone();
    let mut env = match initialize(&path_ref) {
        Ok(e) => e,
        Err(()) => return,
    };
    let mut gate = ReloadGate::new(current_mtime(&path_ref));
    let mut idle = IdleAutoPause::new();
    let mut blur = BlurAutoPause::new();
    flush_output(&mut env);

    loop {
        if is_key_pressed(KeyCode::Escape) {
            break;
        }

        // Hot reload: debounced mtime poll. The gate buffers
        // mid-write reads (truncate → write → close on POSIX)
        // and only fires the reload after the file's mtime has
        // been stable for `RELOAD_DEBOUNCE_FRAMES` frames. If the
        // reload itself fails (transient parse error), the gate
        // doesn't retry until the next mtime change.
        if gate.should_reload(current_mtime(&path_ref)) {
            match initialize(&path_ref) {
                Ok(new_env) => {
                    eprintln!("[twec] hot reload: {path_ref}");
                    crate::stdlib::clear_asset_caches();
                    clear_gamepad_state();
                    env = new_env;
                    flush_output(&mut env);
                }
                Err(()) => {
                    eprintln!("[twec] hot reload failed; keeping previous script live");
                }
            }
        }

        update_key_state(&mut env);
        let dt = get_frame_time() as f64;
        hud_record(dt);
        idle.tick(dt);
        idle.apply();
        blur.tick(crate::window_focus::is_focused());
        // Phase 10 session 8: when paused, skip `tick_frame` so no
        // fibers advance and no every-clocks fire, but keep the
        // render path live so a "PAUSED" overlay or settings menu
        // can draw. Render-side `button` / `slider` etc. read mouse
        // state directly so the pause menu still interacts.
        if !crate::stdlib::is_paused() {
            if let Err(e) = crate::eval::tick_frame(&mut env, dt) {
                eprintln!("{path_ref}: runtime error: {e}");
                break;
            }
        }
        flush_output(&mut env);

        clear_background(BLACK);
        env.in_render = true;
        // Phase 9 session 2: drive a macroquad Camera2D from the Twe
        // `camera` ambient (pos / zoom) plus the runtime shake offset.
        // Backward-compat carve-out: when pos == (0, 0), zoom == 1.0,
        // and shake is silent we leave macroquad's default camera in
        // place — every existing example that draws at pixel coords
        // (origin top-left, +y down) keeps working unchanged.
        // Convention when opted in: camera.pos is the world-space
        // coordinate that ends up at the screen center.
        crate::stdlib::camera_tick(dt);
        let ((px, py), zoom) = crate::stdlib::camera_view(&env);
        let (sx, sy) = crate::stdlib::camera_shake_offset(&mut env);
        let cam_active = px != 0.0 || py != 0.0 || zoom != 1.0 || sx != 0.0 || sy != 0.0;
        if cam_active {
            let cam = build_camera2d(px + sx, py + sy, zoom);
            set_camera(&cam);
        }
        let render_result = crate::eval::render_frame(&mut env);
        if cam_active {
            set_default_camera();
        }
        if let Err(e) = render_result {
            eprintln!("{path_ref}: runtime error: {e}");
            env.in_render = false;
            break;
        }
        env.in_render = false;
        flush_output(&mut env);

        hud_draw();
        write_pending_screenshot();

        next_frame().await;
    }
}

/// Phase 11 session 1: F12 / `screenshot(path)` writer. Honor the
/// `screenshot` builtin's queued path first; otherwise check whether
/// F12 was pressed this frame and write a timestamped default.
fn write_pending_screenshot() {
    let path = if let Some(p) = crate::stdlib::take_pending_screenshot() {
        Some(p)
    } else if is_key_pressed(KeyCode::F12) {
        Some(default_screenshot_path())
    } else {
        None
    };
    let Some(path) = path else { return };
    // `export_png` panics on web and unwraps on save failure; catch it
    // so a bad path (read-only dir, etc.) doesn't kill the game.
    let img = get_screen_data();
    let p = path.clone();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || img.export_png(&p))) {
        Ok(()) => eprintln!("[twec] screenshot saved: {path}"),
        Err(_) => eprintln!("[twec] screenshot failed: {path}"),
    }
}

fn default_screenshot_path() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("screenshot-{secs}.png")
}

// Phase 11 session 11: idle-pause tracking. The "real" auto-pause-
// on-window-blur defers to a winit-integration session — macroquad
// 0.4 still doesn't expose desktop focus events. Until then we
// approximate the player-walked-away case with an idle timer: when
// no keyboard or mouse input has arrived for `auto_pause_when_idle`
// seconds, the play loop sets `pause(true)` and notes that *it*
// did the pausing (so input resuming auto-resumes). The whole
// machinery is opt-in; with the threshold at 0 the timer + the
// auto-pause path are bypassed.
struct IdleAutoPause {
    /// Seconds since last input event. Resets to 0 on any key /
    /// mouse / wheel motion this frame.
    idle_secs: f64,
    /// Mouse position last frame; used to detect motion.
    last_mouse: (f32, f32),
    /// Wheel-y last frame.
    last_wheel: f32,
    /// Key-down state we last saw — non-empty value means at least
    /// one of the polled keys was held this frame.
    last_any_key: bool,
    /// True when the auto-pause path drove `pause(true)` — input
    /// resuming will then drive `pause(false)`.
    paused_by_us: bool,
}

impl IdleAutoPause {
    fn new() -> Self {
        Self {
            idle_secs: 0.0,
            last_mouse: (0.0, 0.0),
            last_wheel: 0.0,
            last_any_key: false,
            paused_by_us: false,
        }
    }

    /// Drive the idle timer one frame forward. Returns the new
    /// idle-secs value; the caller compares it to the threshold.
    fn tick(&mut self, dt: f64) {
        let mp = mouse_position();
        let (_wx, wy) = mouse_wheel();
        let any_key = KEYS.iter().any(|(_, k)| is_key_down(*k));
        let mouse_moved =
            (mp.0 - self.last_mouse.0).abs() > 0.5 || (mp.1 - self.last_mouse.1).abs() > 0.5;
        let wheel_moved = wy.abs() > 0.001 || self.last_wheel.abs() > 0.001;
        let any_button = is_mouse_button_down(MouseButton::Left)
            || is_mouse_button_down(MouseButton::Right)
            || is_mouse_button_down(MouseButton::Middle);
        let active = any_key || mouse_moved || wheel_moved || any_button;
        self.last_mouse = mp;
        self.last_wheel = wy;
        self.last_any_key = any_key;
        if active {
            self.idle_secs = 0.0;
        } else {
            self.idle_secs += dt;
        }
    }

    /// Apply the auto-pause decision against the user-set threshold.
    fn apply(&mut self) {
        let threshold = crate::stdlib::auto_pause_idle_threshold();
        if threshold <= 0.0 {
            // Disabled — also clear our flag so we don't auto-resume
            // a pause the user set after disabling auto-pause.
            self.paused_by_us = false;
            return;
        }
        if self.idle_secs >= threshold {
            if !crate::stdlib::is_paused() {
                crate::stdlib::set_paused(true);
                self.paused_by_us = true;
            }
        } else if self.paused_by_us && self.idle_secs == 0.0 {
            // Input came back; the *paused-by-us* flag means we drove
            // the pause, so dropping it is also our call.
            crate::stdlib::set_paused(false);
            self.paused_by_us = false;
        }
    }
}

// Phase 11 follow-on (deeper): the real auto-pause-on-window-blur
// machinery the Phase-11 closeout punted on. macroquad 0.4 still has no
// public focus-event API; this layer polls
// `window_focus::is_focused()` (Win32 `GetForegroundWindow` on
// Windows, `true` stub on other platforms) once per frame and drives
// the pause flag on transitions. State-machine summary:
//
// * Off (auto_pause_on_blur(false)): paused_by_us cleared every frame
//   so a manual pause never gets auto-resumed.
// * Focused → Unfocused: if not already paused, set paused + remember
//   we did it.
// * Unfocused → Focused: if we drove the pause, clear it; otherwise
//   the pause was set manually, leave it alone.
//
// Symmetry with `IdleAutoPause` is intentional — the two state
// machines are independent and either can drive the pause flag, but
// only the one that *did* drive it auto-resumes.
struct BlurAutoPause {
    /// Was the window focused last frame? Initial state is `true` so
    /// startup-while-unfocused doesn't fire a spurious pause.
    last_focused: bool,
    /// True when we drove `pause(true)` — focus return will then drive
    /// `pause(false)`. Manually set pause stays paused.
    paused_by_us: bool,
}

impl BlurAutoPause {
    fn new() -> Self {
        Self {
            last_focused: true,
            paused_by_us: false,
        }
    }

    fn tick(&mut self, focused: bool) {
        if !crate::stdlib::auto_pause_on_blur_enabled() {
            // Disabled — clear our flag so a previously-driven pause
            // doesn't auto-resume after the script flips the toggle.
            self.paused_by_us = false;
            self.last_focused = focused;
            return;
        }
        if self.last_focused && !focused {
            // Focused → Unfocused.
            if !crate::stdlib::is_paused() {
                crate::stdlib::set_paused(true);
                self.paused_by_us = true;
            }
        } else if !self.last_focused && focused && self.paused_by_us {
            // Unfocused → Focused, and we drove the pause.
            crate::stdlib::set_paused(false);
            self.paused_by_us = false;
        }
        self.last_focused = focused;
    }
}

// Phase 11 session 2: frame-time HUD overlay (F3 to toggle).
// Ring-buffered recent frame deltas; the HUD shows current ms +
// rolling 60-frame average + fps. Drawn after the Twe render so it
// overlays game content. Off by default; toggling persists for the
// life of the process (hot reload is irrelevant — the HUD is dev-loop
// only and isn't game state). 120-frame history gives 2 seconds of
// data at 60fps and lets us catch the occasional hitch.
const HUD_HISTORY: usize = 120;

thread_local! {
    static HUD_VISIBLE: RefCell<bool> = const { RefCell::new(false) };
    static HUD_FRAMES: RefCell<FrameRing> = const {
        RefCell::new(FrameRing { samples: [0.0; HUD_HISTORY], idx: 0, len: 0 })
    };
}

struct FrameRing {
    samples: [f64; HUD_HISTORY],
    idx: usize,
    len: usize,
}

impl FrameRing {
    fn push(&mut self, dt: f64) {
        self.samples[self.idx] = dt;
        self.idx = (self.idx + 1) % HUD_HISTORY;
        if self.len < HUD_HISTORY {
            self.len += 1;
        }
    }
    fn avg_ms(&self) -> f64 {
        if self.len == 0 {
            return 0.0;
        }
        let sum: f64 = self.samples.iter().take(self.len).sum();
        (sum / self.len as f64) * 1000.0
    }
    fn max_ms(&self) -> f64 {
        let mut m = 0.0_f64;
        for s in self.samples.iter().take(self.len) {
            if *s > m {
                m = *s;
            }
        }
        m * 1000.0
    }
}

#[cfg(test)]
mod reload_gate_tests {
    use super::*;
    use std::time::Duration;

    fn t0() -> SystemTime {
        std::time::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    #[test]
    fn no_change_never_fires() {
        let mut g = ReloadGate::new(Some(t0()));
        for _ in 0..100 {
            assert!(!g.should_reload(Some(t0())));
        }
    }

    #[test]
    fn stable_change_fires_after_debounce() {
        let mut g = ReloadGate::new(Some(t0()));
        let new = t0() + Duration::from_secs(1);
        // First change: pending starts.
        assert!(!g.should_reload(Some(new)));
        // Stable polls draining the countdown.
        for _ in 0..(RELOAD_DEBOUNCE_FRAMES - 1) {
            assert!(!g.should_reload(Some(new)));
        }
        // The next stable poll fires the reload.
        assert!(g.should_reload(Some(new)));
        // After firing, the gate is at the new mtime; no further fires.
        for _ in 0..10 {
            assert!(!g.should_reload(Some(new)));
        }
    }

    #[test]
    fn churning_mtime_resets_countdown() {
        let mut g = ReloadGate::new(Some(t0()));
        let mid = t0() + Duration::from_secs(1);
        let later = t0() + Duration::from_secs(2);
        // Editor's truncate seen as `mid`.
        assert!(!g.should_reload(Some(mid)));
        // Editor's final write seen as `later` partway through debounce.
        assert!(!g.should_reload(Some(later)));
        // Now we need a full debounce against `later` before firing.
        for _ in 0..(RELOAD_DEBOUNCE_FRAMES - 1) {
            assert!(!g.should_reload(Some(later)));
        }
        assert!(g.should_reload(Some(later)));
    }

    #[test]
    fn unreadable_file_does_not_fire() {
        let mut g = ReloadGate::new(Some(t0()));
        for _ in 0..10 {
            assert!(!g.should_reload(None));
        }
    }

    #[test]
    fn revert_to_loaded_mtime_clears_pending() {
        let mut g = ReloadGate::new(Some(t0()));
        let new = t0() + Duration::from_secs(1);
        // Mid-debounce.
        assert!(!g.should_reload(Some(new)));
        // User undoes the save; mtime returns to the loaded version.
        assert!(!g.should_reload(Some(t0())));
        // No fire even after many stable polls.
        for _ in 0..100 {
            assert!(!g.should_reload(Some(t0())));
        }
    }
}

#[cfg(test)]
mod hud_tests {
    use super::FrameRing;
    use super::HUD_HISTORY;

    fn empty_ring() -> FrameRing {
        FrameRing {
            samples: [0.0; HUD_HISTORY],
            idx: 0,
            len: 0,
        }
    }

    #[test]
    fn avg_and_max_are_zero_when_empty() {
        let r = empty_ring();
        assert_eq!(r.avg_ms(), 0.0);
        assert_eq!(r.max_ms(), 0.0);
    }

    #[test]
    fn avg_is_mean_in_milliseconds() {
        let mut r = empty_ring();
        r.push(0.016);
        r.push(0.020);
        r.push(0.012);
        let avg = r.avg_ms();
        assert!((avg - 16.0).abs() < 1e-9, "got {avg}");
    }

    #[test]
    fn max_is_largest_sample_in_milliseconds() {
        let mut r = empty_ring();
        r.push(0.016);
        r.push(0.033);
        r.push(0.012);
        assert!((r.max_ms() - 33.0).abs() < 1e-9);
    }

    #[test]
    fn ring_evicts_oldest_after_capacity() {
        let mut r = empty_ring();
        for _ in 0..HUD_HISTORY {
            r.push(0.010);
        }
        // Capacity reached; pushing a heavy sample displaces a 10ms one.
        r.push(0.100);
        assert_eq!(r.len, HUD_HISTORY);
        assert!((r.max_ms() - 100.0).abs() < 1e-9);
    }
}

fn hud_record(dt: f64) {
    if is_key_pressed(KeyCode::F3) {
        HUD_VISIBLE.with(|v| {
            let mut b = v.borrow_mut();
            *b = !*b;
        });
    }
    HUD_FRAMES.with(|c| c.borrow_mut().push(dt));
}

fn hud_draw() {
    let visible = HUD_VISIBLE.with(|v| *v.borrow());
    if !visible {
        return;
    }
    let (cur_ms, avg_ms, max_ms) = HUD_FRAMES.with(|c| {
        let r = c.borrow();
        let cur = if r.len == 0 {
            0.0
        } else {
            // last pushed sample is at (idx + HUD_HISTORY - 1) % HUD_HISTORY
            let last = (r.idx + HUD_HISTORY - 1) % HUD_HISTORY;
            r.samples[last] * 1000.0
        };
        (cur, r.avg_ms(), r.max_ms())
    });
    let fps = if avg_ms > 0.0 { 1000.0 / avg_ms } else { 0.0 };
    let text = format!(
        "frame {cur_ms:>5.1}ms  avg {avg_ms:>5.1}ms  max {max_ms:>5.1}ms  {fps:>5.1} fps"
    );
    // Top-right anchored. Measure first to know how wide the panel is.
    let font_size: u16 = 14;
    let dim = measure_text(&text, None, font_size, 1.0);
    let pad = 6.0;
    let x = screen_width() - dim.width - pad * 2.0;
    let y = pad;
    draw_rectangle(
        x,
        y,
        dim.width + pad * 2.0,
        dim.height + pad * 2.0,
        Color::new(0.0, 0.0, 0.0, 0.7),
    );
    draw_text(
        &text,
        x + pad,
        y + pad + dim.height,
        f32::from(font_size),
        WHITE,
    );
}

/// Build a macroquad `Camera2D` that puts world-coord `(cx, cy)` at
/// the screen center, with `zoom > 1.0` zooming in and `< 1.0` zooming
/// out. Y axis stays inverted (+y down) so call-site coordinates keep
/// matching the screen's pixel orientation.
fn build_camera2d(cx: f64, cy: f64, zoom: f64) -> Camera2D {
    let w = screen_width();
    let h = screen_height();
    let z = zoom as f32;
    Camera2D {
        target: vec2(cx as f32, cy as f32),
        // zoom.x positive: world +x → screen +x.
        // zoom.y negative: world +y → screen +y (orthographic flip
        // would otherwise put +y up, which contradicts the rest of
        // the runtime's pixel-coord convention).
        zoom: vec2(2.0 / w * z, -2.0 / h * z),
        offset: vec2(0.0, 0.0),
        rotation: 0.0,
        render_target: None,
        viewport: None,
    }
}

async fn run_loop_bytecode(path: String) {
    let path_ref = path.clone();
    let mut vm = match initialize_bytecode(&path_ref) {
        Ok(v) => v,
        Err(()) => return,
    };
    let mut gate = ReloadGate::new(current_mtime(&path_ref));
    let mut idle = IdleAutoPause::new();
    let mut blur = BlurAutoPause::new();
    flush_vm_output(&mut vm);

    loop {
        if is_key_pressed(KeyCode::Escape) {
            break;
        }

        if gate.should_reload(current_mtime(&path_ref)) {
            match initialize_bytecode(&path_ref) {
                Ok(new_vm) => {
                    eprintln!("[twec] hot reload: {path_ref}");
                    crate::stdlib::clear_asset_caches();
                    clear_gamepad_state();
                    vm = new_vm;
                    flush_vm_output(&mut vm);
                }
                Err(()) => {
                    eprintln!("[twec] hot reload failed; keeping previous script live");
                }
            }
        }

        update_vm_input(&vm);
        let dt = get_frame_time() as f64;
        hud_record(dt);
        idle.tick(dt);
        idle.apply();
        blur.tick(crate::window_focus::is_focused());
        if let Err(e) = vm.tick(dt) {
            eprintln!("{path_ref}: runtime error: {e}");
            break;
        }
        flush_vm_output(&mut vm);

        clear_background(BLACK);
        if let Err(e) = vm.render() {
            eprintln!("{path_ref}: runtime error: {e}");
            break;
        }
        flush_vm_output(&mut vm);

        hud_draw();
        write_pending_screenshot();

        next_frame().await;
    }
}

// Phase 12 session 4: embedded-bundle play loop. Mirrors `run_loop`
// but with no hot-reload (the source lives in the running .exe;
// nothing to watch). Uses the tree-walker — production default per
// CLAUDE.md — since shipped games shouldn't gate on the in-flight
// bytecode-VM perf gap from Phase 8.5. A future session can add a
// `--vm` toggle in the embedded boot path if a bundled game wants
// the bytecode backend explicitly.
async fn run_loop_embedded(source: String) {
    const LABEL: &str = "<embedded>main.twe";
    let mut env = match initialize_from_source(&source, LABEL) {
        Ok(e) => e,
        Err(()) => return,
    };
    let mut idle = IdleAutoPause::new();
    let mut blur = BlurAutoPause::new();
    flush_output(&mut env);

    loop {
        if is_key_pressed(KeyCode::Escape) {
            break;
        }

        update_key_state(&mut env);
        let dt = get_frame_time() as f64;
        hud_record(dt);
        idle.tick(dt);
        idle.apply();
        blur.tick(crate::window_focus::is_focused());
        if !crate::stdlib::is_paused() {
            if let Err(e) = crate::eval::tick_frame(&mut env, dt) {
                eprintln!("{LABEL}: runtime error: {e}");
                break;
            }
        }
        flush_output(&mut env);

        clear_background(BLACK);
        env.in_render = true;
        crate::stdlib::camera_tick(dt);
        let ((px, py), zoom) = crate::stdlib::camera_view(&env);
        let (sx, sy) = crate::stdlib::camera_shake_offset(&mut env);
        let cam_active = px != 0.0 || py != 0.0 || zoom != 1.0 || sx != 0.0 || sy != 0.0;
        if cam_active {
            let cam = build_camera2d(px + sx, py + sy, zoom);
            set_camera(&cam);
        }
        let render_result = crate::eval::render_frame(&mut env);
        if cam_active {
            set_default_camera();
        }
        if let Err(e) = render_result {
            eprintln!("{LABEL}: runtime error: {e}");
            env.in_render = false;
            break;
        }
        env.in_render = false;
        flush_output(&mut env);

        hud_draw();
        write_pending_screenshot();

        next_frame().await;
    }
}

fn initialize_bytecode(path: &str) -> Result<crate::vm::VM, ()> {
    let src = match std::fs::read_to_string(Path::new(path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return Err(());
        }
    };
    initialize_bytecode_from_source(&src, path)
}

/// Phase 12 session 4: shared bytecode init for both file-backed
/// and embedded-bundle launches.
fn initialize_bytecode_from_source(src: &str, label: &str) -> Result<crate::vm::VM, ()> {
    let tokens = match crate::lexer::lex(src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{label}:{e}");
            return Err(());
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{label}:{e}");
            return Err(());
        }
    };
    let chunk = match crate::compiler::compile_program(&program) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{label}: compile error: {e}");
            return Err(());
        }
    };
    let mut vm = crate::vm::VM::new();
    if let Err(e) = vm.run(&chunk) {
        eprintln!("{label}: runtime error: {e}");
        return Err(());
    }
    Ok(vm)
}

fn flush_vm_output(vm: &mut crate::vm::VM) {
    let s = vm.take_out();
    if !s.is_empty() {
        print!("{s}");
    }
}

/// Mirror of `update_key_state` for the bytecode VM. The `key`,
/// `key_press`, `mouse`, `mouse_held`, `mouse_press`, and `screen`
/// Objects are the same `Rc<RefCell<Object>>` instances the
/// stdlib installs, so writes via `.borrow_mut()` here reach the
/// running scene/state code through their globals.
fn update_vm_input(vm: &crate::vm::VM) {
    if let Some(__t) = (vm.get_global("key")).as_ref() {
        if __t.is_object() {
            let rc = __t.as_object();
            let mut o = rc.borrow_mut();
            for (name, code) in KEYS {
                o.insert_field(*name, Value::from_bool(is_key_down(*code)));
            }
        }
    }
    if let Some(__t) = (vm.get_global("key_press")).as_ref() {
        if __t.is_object() {
            let rc = __t.as_object();
            let mut o = rc.borrow_mut();
            for (name, code) in KEYS {
                o.insert_field(*name, Value::from_bool(is_key_pressed(*code)));
            }
        }
    }
    write_mouse_object(vm.get_global("mouse"));
    write_mouse_buttons(vm.get_global("mouse_held"), is_mouse_button_down);
    write_mouse_buttons(vm.get_global("mouse_press"), is_mouse_button_pressed);
    if let Some(__t) = (vm.get_global("screen")).as_ref() {
        if __t.is_object() {
            let rc = __t.as_object();
            let mut o = rc.borrow_mut();
            let w = screen_width() as f64;
            let h = screen_height() as f64;
            o.insert_field(
                "size".to_string(),
                Value::from_tuple(Rc::new(vec![Value::from_float(w), Value::from_float(h)])),
            );
            o.insert_field(
                "center".to_string(),
                Value::from_tuple(Rc::new(vec![
                    Value::from_float(w / 2.0),
                    Value::from_float(h / 2.0),
                ])),
            );
        }
    }
}

/// Write current mouse position + accumulated wheel delta into the
/// `mouse` ambient. v0.2 session 3.
fn write_mouse_object(mouse: Option<Value>) {
    let Some(t) = mouse else {
        return;
    };
    if !t.is_object() {
        return;
    }
    let rc = t.as_object();
    let (mx, my) = mouse_position();
    let (_wx, wy) = mouse_wheel();
    let mut o = rc.borrow_mut();
    o.insert_field("x", Value::from_float(mx as f64));
    o.insert_field("y", Value::from_float(my as f64));
    o.insert_field(
        "pos",
        Value::from_tuple(Rc::new(vec![
            Value::from_float(mx as f64),
            Value::from_float(my as f64),
        ])),
    );
    // y-axis wheel delta is the canonical "scroll" reading; macroquad
    // resets `mouse_wheel()` between frames so the value here is the
    // accumulated delta this frame.
    o.insert_field("wheel", Value::from_float(wy as f64));
}

/// Write per-button state into `mouse_held` or `mouse_press`. The
/// caller passes the button-state predicate (`is_mouse_button_down`
/// for `mouse_held`, `is_mouse_button_pressed` for edge-triggered
/// `mouse_press`). v0.2 session 3.
fn write_mouse_buttons(target: Option<Value>, mut pred: impl FnMut(MouseButton) -> bool) {
    let Some(t) = target else {
        return;
    };
    if !t.is_object() {
        return;
    }
    let rc = t.as_object();
    let mut o = rc.borrow_mut();
    for (name, btn) in MOUSE_BUTTONS {
        o.insert_field((*name).to_string(), Value::from_bool(pred(*btn)));
    }
}

fn current_mtime(path: &str) -> Option<SystemTime> {
    std::fs::metadata(Path::new(path))
        .ok()
        .and_then(|m| m.modified().ok())
}

fn initialize(path: &str) -> Result<Env, ()> {
    let src = match std::fs::read_to_string(Path::new(path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return Err(());
        }
    };
    initialize_from_source(&src, path)
}

/// Phase 12 session 4: shared init path for both file-backed and
/// embedded-bundle launches. Lexes / parses / runs top-level on the
/// given source string; `label` is the diagnostic prefix
/// (filesystem path or `<embedded>main.twe`).
fn initialize_from_source(src: &str, label: &str) -> Result<Env, ()> {
    let tokens = match crate::lexer::lex(src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{label}:{e}");
            return Err(());
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{label}:{e}");
            return Err(());
        }
    };
    let mut env = Env::new();
    crate::stdlib::install(&mut env);
    if let Err(e) = crate::eval::run_top_level(&mut env, &program) {
        eprintln!("{label}: runtime error: {e}");
        return Err(());
    }
    Ok(env)
}

fn flush_output(env: &mut Env) {
    if !env.out.is_empty() {
        print!("{}", env.out);
        env.out.clear();
    }
}

fn update_key_state(env: &mut Env) {
    if let Some(t) = env.get("key") {
        if t.is_object() {
            let rc = t.as_object();
            let mut o = rc.borrow_mut();
            for (name, code) in KEYS {
                o.insert_field(*name, Value::from_bool(is_key_down(*code)));
            }
        }
    }
    let kp = env.get("key_press");
    if kp.as_ref().is_some_and(|t| t.is_object()) {
        let rc = kp.unwrap().as_object();
        let mut o = rc.borrow_mut();
        for (name, code) in KEYS {
            o.insert_field(*name, Value::from_bool(is_key_pressed(*code)));
        }
    } else {
        // Lazily install key_press as a sibling object next to key.
        let mut press = Object {
            fields: HashMap::new(),
            kind: "input",
        };
        for (name, code) in KEYS {
            press.insert_field(*name, Value::from_bool(is_key_pressed(*code)));
        }
        env.set(
            "key_press".to_string(),
            Value::from_object(Rc::new(RefCell::new(press))),
        );
    }
    if let Some(t) = env.get("screen") {
        if t.is_object() {
            let rc = t.as_object();
            let mut o = rc.borrow_mut();
            let w = screen_width() as f64;
            let h = screen_height() as f64;
            o.insert_field(
                "size".to_string(),
                Value::from_tuple(Rc::new(vec![Value::from_float(w), Value::from_float(h)])),
            );
            o.insert_field(
                "center".to_string(),
                Value::from_tuple(Rc::new(vec![
                    Value::from_float(w / 2.0),
                    Value::from_float(h / 2.0),
                ])),
            );
        }
    }
    write_mouse_object(env.get("mouse"));
    write_mouse_buttons(env.get("mouse_held"), is_mouse_button_down);
    write_mouse_buttons(env.get("mouse_press"), is_mouse_button_pressed);
    poll_gamepad(env);
}

/// Phase 9 session 5: poll gilrs and write button + axis state to
/// the `gamepad`, `gamepad_press`, and `gamepad_axis` ambients. The
/// first connected gamepad wins; multi-gamepad routing is a follow-on.
/// `gamepad_press` is derived by diffing `PREV_GAMEPAD` against the
/// current state (gilrs's event stream is drained but we don't use
/// it directly — diffing keeps the surface symmetric with `key_press`).
fn poll_gamepad(env: &mut Env) {
    GILRS.with(|g| {
        // Lazy init the first time we're called inside the macroquad
        // window context. gilrs init failures (e.g. no input
        // subsystem) produce a single warning and disable polling.
        let mut state = g.borrow_mut();
        if matches!(*state, GilrsState::Uninit) {
            *state = match gilrs::Gilrs::new() {
                Ok(g) => GilrsState::Active(Box::new(g)),
                Err(e) => {
                    eprintln!("[twec] gamepad disabled: {e}");
                    GilrsState::Failed
                }
            };
        }
        let GilrsState::Active(gilrs) = &mut *state else {
            return;
        };
        let gilrs = gilrs.as_mut();
        // Drain pending events so gilrs's internal connection /
        // disconnection state stays current. We don't act on the
        // event payloads — buttons are sampled below by polling.
        while gilrs.next_event().is_some() {}

        let pad = gilrs.gamepads().next().map(|(_id, p)| p);
        let connected = pad.is_some();

        // Continuous + edge-triggered booleans.
        let cur: [bool; 14] = match pad {
            Some(p) => {
                let mut buf = [false; 14];
                for (i, (_name, btn)) in GAMEPAD_BUTTONS.iter().enumerate() {
                    buf[i] = p.is_pressed(*btn);
                }
                buf
            }
            None => [false; 14],
        };
        let prev = PREV_GAMEPAD.with(|p| *p.borrow());
        if let Some(t) = env.get("gamepad") {
            if t.is_object() {
                let rc = t.as_object();
                let mut o = rc.borrow_mut();
                for (i, (name, _)) in GAMEPAD_BUTTONS.iter().enumerate() {
                    o.insert_field(*name, Value::from_bool(cur[i]));
                }
                o.insert_field("connected", Value::from_bool(connected));
            }
        }
        if let Some(t) = env.get("gamepad_press") {
            if t.is_object() {
                let rc = t.as_object();
                let mut o = rc.borrow_mut();
                for (i, (name, _)) in GAMEPAD_BUTTONS.iter().enumerate() {
                    // Edge-trigger: true only on the frame the button
                    // transitions from up to down.
                    o.insert_field(*name, Value::from_bool(cur[i] && !prev[i]));
                }
            }
        }
        PREV_GAMEPAD.with(|p| *p.borrow_mut() = cur);

        // Analog axes.
        if let Some(t) = env.get("gamepad_axis") {
            if t.is_object() {
                let rc = t.as_object();
                let mut o = rc.borrow_mut();
                match pad {
                    Some(p) => {
                        for (name, axis) in GAMEPAD_AXES {
                            o.insert_field(*name, Value::from_float(p.value(*axis) as f64));
                        }
                    }
                    None => {
                        for (name, _) in GAMEPAD_AXES {
                            o.insert_field(*name, Value::from_float(0.0));
                        }
                    }
                }
            }
        }
    });
}
