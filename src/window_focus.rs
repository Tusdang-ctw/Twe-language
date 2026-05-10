// Phase 11 follow-on (deeper): close the auto-pause-on-window-blur gap
// the Phase-11 closeout flagged. macroquad 0.4 / miniquad don't expose
// desktop focus events, so the play loop polls `is_focused()` once per
// frame and the `BlurAutoPause` state machine drives the pause flag on
// the focused→unfocused / unfocused→focused transitions.
//
// Phase 34 fills in the macOS + Linux X11 paths the Phase-11 closeout
// stubbed. Each platform owns a small branch:
//
//   Windows:  `GetForegroundWindow` + `GetWindowThreadProcessId` and
//             compare PID to ours. Already shipped by Phase 11.
//   macOS:    `[[NSApplication sharedApplication] isActive]`. Phase 34.
//   X11:      Parallel X11 connection polling `_NET_ACTIVE_WINDOW` on
//             the root window, then reading that window's
//             `_NET_WM_PID` and comparing to ours. Phase 34.
//   Wayland:  Documented stub returning `true`. Wayland focus is
//             per-input-device and only delivered to the focused
//             client; no portable way to query "am I focused" from
//             outside the windowing system client (miniquad). Phase 34
//             ships the stub honestly.
//   wasm32:   Returns `true` unconditionally. Browsers handle blur via
//             the page-visibility API; the Phase 30 web target uses
//             that path inside the macroquad WASM event loop, not
//             this module.
//
// On any platform without focus detection, returns `true` so the pause
// flag is never spuriously flipped.
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

#[cfg(target_os = "macos")]
fn is_focused_impl() -> bool {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    // `NSApplication.sharedApplication` is the singleton that owns
    // `isActive`. We never retain or release it; the singleton lives
    // for the lifetime of the process. Calling from a non-main thread
    // is documented as safe for the read accessors. macroquad's main
    // loop is on the main thread, which is where we poll from.
    //
    // SAFETY: both `msg_send!` calls invoke methods on a valid
    // singleton pointer. `sharedApplication` returns a non-null
    // pointer for any AppKit-linked process; if AppKit fails to load
    // (we're not in a windowed context), `class!(NSApplication)`
    // would have already panicked at module init time, so reaching
    // this point implies the class is registered. `isActive` returns
    // a `BOOL` (i.e. `bool` via objc2's repr-aware bridging) with
    // no side effects.
    unsafe {
        let cls = class!(NSApplication);
        let app: *mut AnyObject = msg_send![cls, sharedApplication];
        if app.is_null() {
            return true;
        }
        let active: bool = msg_send![app, isActive];
        active
    }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
fn is_focused_impl() -> bool {
    // Wayland sessions report themselves with `WAYLAND_DISPLAY` set;
    // X11 sessions with `DISPLAY`. When both are set (XWayland), prefer
    // X11 (we'll be running through XWayland and the X11 query works).
    // When only WAYLAND_DISPLAY is set, no portable query exists —
    // fall through to `true`.
    if std::env::var_os("DISPLAY").is_some() {
        return x11_is_focused().unwrap_or(true);
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        // Wayland focus is per-input-device and only delivered as
        // events to the focused client. A separate Wayland connection
        // (which we'd open here) would not be told who is focused;
        // only the windowing-system client (miniquad) can answer this.
        // The honest stub returns `true` until miniquad surfaces
        // focus events upstream.
        return true;
    }
    // Headless / unknown — pause-on-blur is meaningless without a
    // window manager. Stay focused.
    true
}

#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
fn x11_is_focused() -> Option<bool> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    // Open a parallel X11 connection. This is independent of
    // miniquad's connection — slight smell (typically one X11
    // connection per process), but the alternative is plumbing
    // focus events out through macroquad → miniquad → us, which
    // is upstream work not gated on this phase.
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let setup = conn.setup();
    let root = setup.roots.get(screen_num)?.root;

    let active_atom = conn
        .intern_atom(true, b"_NET_ACTIVE_WINDOW")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let pid_atom = conn
        .intern_atom(true, b"_NET_WM_PID")
        .ok()?
        .reply()
        .ok()?
        .atom;

    // `_NET_ACTIVE_WINDOW` (cardinal[1]) on the root window is the
    // freedesktop-spec way to ask "which window currently has input
    // focus." Returns 0 (None) if no window is focused — we treat
    // that as focused (don't pause when the WM is doing something).
    let active_reply = conn
        .get_property(false, root, active_atom, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    let active_win = active_reply
        .value32()?
        .next()
        .filter(|w| *w != 0)?;

    // `_NET_WM_PID` on that window holds the owning process id as a
    // cardinal[1]. If unset (some apps don't set it), we can't
    // compare — return None and the caller falls back to `true`.
    let pid_reply = conn
        .get_property(false, active_win, pid_atom, AtomEnum::CARDINAL, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    let active_pid = pid_reply.value32()?.next()?;

    Some(active_pid == std::process::id())
}

#[cfg(any(
    target_arch = "wasm32",
    all(
        not(windows),
        not(target_os = "macos"),
        not(unix),
    ),
))]
fn is_focused_impl() -> bool {
    // wasm32 + unknown targets: pause-on-blur is either handled by
    // the host environment (browser page-visibility) or doesn't apply.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // The implementation can only be exercised against the running
    // platform — there's no portable way to assert "yes the focus
    // logic returned true because we have focus" inside a unit test
    // (the test runner often has no window). We assert the function
    // is callable and the return value is well-formed; integration
    // testing happens in the play loop's BlurAutoPause harness.
    #[test]
    fn is_focused_returns_a_bool() {
        let _ = is_focused();
    }
}
