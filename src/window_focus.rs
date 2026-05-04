// Phase 11 follow-on (deeper): close the auto-pause-on-window-blur gap
// the Phase-11 closeout flagged. macroquad 0.4 / miniquad don't expose
// desktop focus events, so the play loop polls `is_focused()` once per
// frame and the `BlurAutoPause` state machine drives the pause flag on
// the focused→unfocused / unfocused→focused transitions.
//
// On Windows we read `GetForegroundWindow` and compare its owning PID to
// our own (rather than tracking our HWND, which macroquad doesn't
// expose). False-positive risk: spawning a child window from the same
// process — we don't.
//
// On non-Windows targets `is_focused` returns `true` unconditionally —
// the macOS (`NSApplication.isActive`) and X11/Wayland paths are
// captured as a follow-on. That keeps the cross-platform builds green
// and turns auto-pause-on-blur into a no-op on those platforms until a
// later session lands the per-OS code.
#![allow(unsafe_code)]

/// True when the foreground window belongs to our process. On platforms
/// without a focus-detection implementation, returns `true` so the
/// pause flag is never spuriously flipped.
pub fn is_focused() -> bool {
    is_focused_impl()
}

#[cfg(windows)]
fn is_focused_impl() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };
    // SAFETY: `GetForegroundWindow` is a side-effect-free Win32 call
    // returning a pointer that is either null or a valid HWND. We
    // never dereference it; we only pass it back to
    // `GetWindowThreadProcessId`, which writes into a stack-allocated
    // u32 and ignores invalid handles. Both functions are documented
    // as safe to call from any thread.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return true;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        pid == std::process::id()
    }
}

#[cfg(not(windows))]
fn is_focused_impl() -> bool {
    true
}
