//! `twec play` — the native 2D/3D launcher.
//!
//! web3d-M6: macroquad is gone. `twec play` draws through the kernel's
//! 2D canvas in a winit window ([`crate::play2d`]); a 3D game (embedded
//! in a shipped `.exe`) runs on the kernel's 3D renderer
//! ([`crate::play3d`]). This module keeps the shared plumbing both
//! shells use — the gamepad (gilrs) and the hot-reload debounce gate —
//! and the public `launch` / `launch_embedded` entry points.

#[cfg(not(target_arch = "wasm32"))]
use std::cell::RefCell;
use std::time::SystemTime;

/// Twe gamepad-button name → gilrs::Button. The order and names mirror
/// `stdlib::GAMEPAD_BUTTON_NAMES` (scripts read fields by name, so the
/// Rust list is the source of truth for which buttons we poll).
#[cfg(not(target_arch = "wasm32"))]
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

/// Twe axis name → gilrs::Axis. Triggers `lt` / `rt` use `LeftZ` /
/// `RightZ` (gilrs's analog trigger axes); the boolean `lt` / `rt`
/// fields on `gamepad` come from thresholding these.
#[cfg(not(target_arch = "wasm32"))]
const GAMEPAD_AXES: &[(&str, gilrs::Axis)] = &[
    ("lx", gilrs::Axis::LeftStickX),
    ("ly", gilrs::Axis::LeftStickY),
    ("rx", gilrs::Axis::RightStickX),
    ("ry", gilrs::Axis::RightStickY),
    ("lt", gilrs::Axis::LeftZ),
    ("rt", gilrs::Axis::RightZ),
];

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// Lazily-initialised gilrs context, held so `shutdown_gilrs` can
    /// drop it before `process::exit`. The shells own their own
    /// `gilrs::Gilrs` for polling; this is only the clean-shutdown slot.
    static GILRS: RefCell<GilrsState> = const { RefCell::new(GilrsState::Uninit) };
    /// Previous-frame button states for edge-triggered detection.
    /// Cleared on hot reload by `clear_gamepad_state`.
    static PREV_GAMEPAD: RefCell<[bool; 14]> = const { RefCell::new([false; 14]) };
}

#[cfg(not(target_arch = "wasm32"))]
enum GilrsState {
    Uninit,
    // Boxed: gilrs::Gilrs is ~230 bytes; without the Box, clippy's
    // large_enum_variant fires because Uninit / Failed carry no data.
    #[allow(dead_code)]
    Active(Box<gilrs::Gilrs>),
    #[allow(dead_code)]
    Failed,
}

/// web3d-M4: the first connected pad's buttons (in
/// `stdlib::GAMEPAD_BUTTON_NAMES` order) and axes (in
/// `GAMEPAD_AXIS_NAMES` order), for a shell's input state. Drains
/// gilrs's events first so its connection state is current.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn read_first_pad(gilrs: &mut gilrs::Gilrs) -> Option<([bool; 14], [f64; 6])> {
    while gilrs.next_event().is_some() {}
    let (_, pad) = gilrs.gamepads().next()?;
    let mut buttons = [false; 14];
    for (slot, (_, btn)) in buttons.iter_mut().zip(GAMEPAD_BUTTONS) {
        *slot = pad.is_pressed(*btn);
    }
    let mut axes = [0.0; 6];
    for (slot, (_, axis)) in axes.iter_mut().zip(GAMEPAD_AXES) {
        *slot = f64::from(pad.value(*axis));
    }
    Some((buttons, axes))
}

/// Reset the previous-frame gamepad state so a hot reload doesn't
/// produce phantom press events.
#[cfg(not(target_arch = "wasm32"))]
pub fn clear_gamepad_state() {
    PREV_GAMEPAD.with(|p| *p.borrow_mut() = [false; 14]);
}
#[cfg(target_arch = "wasm32")]
pub fn clear_gamepad_state() {}

/// Drop any held gilrs context before `process::exit`. gilrs 0.10 on
/// Windows panics in `Gilrs::drop` when `process::exit` triggers TLS
/// cleanup while the background polling thread handle is already
/// consumed; resetting the slot here avoids that.
#[cfg(not(target_arch = "wasm32"))]
pub fn shutdown_gilrs() {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        GILRS.with(|g| {
            *g.borrow_mut() = GilrsState::Uninit;
        });
    }));
}
#[cfg(target_arch = "wasm32")]
pub fn shutdown_gilrs() {}

/// Phase 11 session 4: hot-reload reliability gate.
///
/// Editor save sequences are typically `truncate → write → close` on
/// POSIX or `write-temp → rename` on Windows. A naïve mtime-poll races
/// both: a poll between truncate and the final write reads a half-empty
/// file and produces a parse error. The gate debounces — when mtime
/// first changes it starts a countdown; the reload only fires after the
/// mtime has been stable for `RELOAD_DEBOUNCE_FRAMES` polls. A
/// mid-debounce change resets the countdown.
pub struct ReloadGate {
    /// Last mtime that was successfully loaded (or the initial mtime).
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
            // the file came back to the same mtime (touch then undo).
            self.pending = None;
            return false;
        }
        match self.pending {
            None => {
                self.pending = Some((cur, RELOAD_DEBOUNCE_FRAMES));
                false
            }
            Some((p, _)) if p != cur => {
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

/// `twec play <file>`: run the game in a window. A 2D game draws on the
/// kernel canvas ([`crate::play2d`]); a 3D game runs on the kernel's 3D
/// renderer ([`crate::play3d`]). The dimension is detected from the
/// game ([`crate::frame2d::detect_dimension`]).
pub fn launch(path: String) -> i32 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if dimension_of_file(&path) == crate::frame2d::Dimension::Three {
            return crate::play3d::launch(path);
        }
        crate::play2d::launch(path)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        0
    }
}

/// Phase 12 session 4: launch an embedded `main.twe` (a shipped `.exe`,
/// no hot reload). `cli::run` detects the embedded bundle, mounts it,
/// then calls this; web3d-M6 routes a 3D game to the 3D renderer.
pub fn launch_embedded(source: String) -> i32 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if dimension_of_source(&source) == crate::frame2d::Dimension::Three {
            return crate::play3d::launch_embedded(source);
        }
        crate::play2d::launch_embedded(source)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = source;
        0
    }
}

/// web3d-M6: 2D or 3D, by running the game's top level and inspecting
/// its first frame. A load error resolves to 2D, so the 2D shell shows
/// the real diagnostic.
#[cfg(not(target_arch = "wasm32"))]
fn dimension_of_file(path: &str) -> crate::frame2d::Dimension {
    crate::frame2d::load_any(path)
        .map(|mut env| crate::frame2d::detect_dimension(&mut env))
        .unwrap_or(crate::frame2d::Dimension::Two)
}

#[cfg(not(target_arch = "wasm32"))]
fn dimension_of_source(source: &str) -> crate::frame2d::Dimension {
    crate::frame2d::load_source_any(source, "<embedded>main.twe")
        .map(|mut env| crate::frame2d::detect_dimension(&mut env))
        .unwrap_or(crate::frame2d::Dimension::Two)
}
