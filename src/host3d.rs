//! web3d-M2: the interpreter side of 3D hosting, shared by every shell.
//!
//! Gathers a [`RenderSnapshot`] from interpreter state (the script's
//! `on render():` draw queue, the `camera` object, stdlib light / shadow
//! / post-FX settings) and hands it to the kernel renderer. The native
//! winit shell (`play3d.rs`) and the browser shell both call
//! [`render_frame`], so a frame means the same thing everywhere.

use crate::eval;
use crate::kernel::render::{
    AssetSource, Camera3d, PostFx, RenderSnapshot, Renderer, ShadowSettings,
};
use crate::value::{Env, Value};

/// Extract `camera.eye` / `camera.target` / `camera.up` from the
/// env's `camera` Object. Missing or malformed fields fall back to
/// the stdlib defaults (eye 3 units back + 1.5 up, looking at the
/// origin, +y up). Phase 5 task 5 session (d).
fn read_camera(env: &Env) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let eye_default = [0.0, 1.5, 3.0];
    let target_default = [0.0, 0.0, 0.0];
    let up_default = [0.0, 1.0, 0.0];
    let camera = {
        let __opt = env.get("camera");
        if let Some(__t) = (__opt).as_ref() {
            if __t.is_object() {
                let rc = __t.as_object();
                rc.clone()
            } else {
                return (eye_default, target_default, up_default);
            }
        } else {
            return (eye_default, target_default, up_default);
        }
    };
    let cam = camera.borrow();
    let eye = cam
        .get_field("eye")
        .as_ref()
        .and_then(value_as_vec3)
        .unwrap_or(eye_default);
    let target = cam
        .get_field("target")
        .as_ref()
        .and_then(value_as_vec3)
        .unwrap_or(target_default);
    let up = cam
        .get_field("up")
        .as_ref()
        .and_then(value_as_vec3)
        .unwrap_or(up_default);
    (eye, target, up)
}

fn value_as_vec3(v: &Value) -> Option<[f32; 3]> {
    if v.is_tuple() {
        let elems = v.as_tuple();
        if elems.len() == 3 {
            let x = number(&elems[0])?;
            let y = number(&elems[1])?;
            let z = number(&elems[2])?;
            return Some([x as f32, y as f32, z as f32]);
        }
    }
    None
}

fn number(v: &Value) -> Option<f64> {
    if v.is_int_or_boxed_int() {
        let n = v.as_int();
        Some(n as f64)
    } else if v.is_float() {
        let f = v.as_float();
        Some(f)
    } else {
        None
    }
}

/// Where one rendered frame's time went, in milliseconds (0 when the
/// host has no clock). Hosts show it (the web shell's `frame_stats()`,
/// web3d-M3's exit measurement).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTimes {
    /// The script's `on render():` filling the draw queue.
    pub script_ms: f64,
    /// Snapshot + kernel: culling, instance upload, command encoding.
    pub kernel_ms: f64,
}

/// Run the script's `on render():`, then hand the kernel a snapshot of
/// everything the frame needs from the interpreter. Script output (and
/// a render-body error) is left in `env.out` for the host to print.
pub fn render_frame(
    renderer: &mut Renderer,
    env: &mut Env,
    assets: &mut dyn AssetSource,
) -> Result<FrameTimes, String> {
    let start = crate::clock::now_secs();
    if let Err(e) = eval::render_frame3d(env) {
        // Surface the runtime error to stderr but keep rendering — a
        // broken render frame shouldn't tear down the window.
        env.out.push_str(&format!(
            "render error in `on render()`: {}:{}: {}
",
            e.line, e.col, e.message
        ));
    }
    let (eye, target, up) = read_camera(env);
    let draws = std::mem::take(&mut env.render_queue3d);
    let anim = |id: u32| crate::stdlib::mesh_anim_state(id);
    let snap = RenderSnapshot {
        camera: Camera3d { eye, target, up },
        lights: crate::stdlib::lights_snapshot(),
        shadow: ShadowSettings {
            enabled: crate::stdlib::shadow_enabled(),
            extent: crate::stdlib::shadow_extent(),
        },
        post: PostFx {
            tonemap_aces: crate::stdlib::tonemap_enabled(),
            vignette: crate::stdlib::vignette_strength(),
            vignette_color: crate::stdlib::vignette_color(),
            bloom_intensity: crate::stdlib::bloom_intensity(),
            bloom_threshold: crate::stdlib::bloom_threshold(),
            frustum_cull: crate::stdlib::frustum_culling_enabled(),
        },
        draws: &draws,
        mesh_paths: &env.mesh_paths,
        texture_paths: &env.texture_paths,
        time: env.sim_time as f32,
        materials: &env.material_sources,
        hud: &env.hud_queue,
        anim: &anim,
    };
    let scripted = crate::clock::now_secs();
    let result = renderer.render(&snap, assets);
    // Hand the (cleared) allocation back so the queue doesn't regrow.
    let mut draws = draws;
    draws.clear();
    env.render_queue3d = draws;
    result?;
    let ms = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (Some(a), Some(b)) => (b - a) * 1e3,
        _ => 0.0,
    };
    Ok(FrameTimes {
        script_ms: ms(start, scripted),
        kernel_ms: ms(scripted, crate::clock::now_secs()),
    })
}

/// web3d-M4: every key a Twe script can name, with its code — the
/// DOM `KeyboardEvent.code`, which is also winit's `KeyCode` variant
/// name. Both 3D shells report exactly this set (the native shell's
/// table is checked against it by a test).
pub const KEY_CODES: &[(&str, &str)] = &[
    ("right", "ArrowRight"),
    ("left", "ArrowLeft"),
    ("up", "ArrowUp"),
    ("down", "ArrowDown"),
    ("space", "Space"),
    ("escape", "Escape"),
    ("enter", "Enter"),
    ("tab", "Tab"),
    ("backspace", "Backspace"),
    ("shift", "ShiftLeft"),
    ("ctrl", "ControlLeft"),
    ("alt", "AltLeft"),
    ("a", "KeyA"),
    ("b", "KeyB"),
    ("c", "KeyC"),
    ("d", "KeyD"),
    ("e", "KeyE"),
    ("f", "KeyF"),
    ("g", "KeyG"),
    ("h", "KeyH"),
    ("i", "KeyI"),
    ("j", "KeyJ"),
    ("k", "KeyK"),
    ("l", "KeyL"),
    ("m", "KeyM"),
    ("n", "KeyN"),
    ("o", "KeyO"),
    ("p", "KeyP"),
    ("q", "KeyQ"),
    ("r", "KeyR"),
    ("s", "KeyS"),
    ("t", "KeyT"),
    ("u", "KeyU"),
    ("v", "KeyV"),
    ("w", "KeyW"),
    ("x", "KeyX"),
    ("y", "KeyY"),
    ("z", "KeyZ"),
    ("0", "Digit0"),
    ("1", "Digit1"),
    ("2", "Digit2"),
    ("3", "Digit3"),
    ("4", "Digit4"),
    ("5", "Digit5"),
    ("6", "Digit6"),
    ("7", "Digit7"),
    ("8", "Digit8"),
    ("9", "Digit9"),
    ("f1", "F1"),
    ("f2", "F2"),
    ("f3", "F3"),
    ("f4", "F4"),
    ("f5", "F5"),
    ("f6", "F6"),
    ("f7", "F7"),
    ("f8", "F8"),
    ("f9", "F9"),
    ("f10", "F10"),
    ("f11", "F11"),
    ("f12", "F12"),
];

/// Write the host's keyboard state into the `key` (held) and
/// `key_press` (pressed this frame) ambients for every name in `names`.
/// Shells map their own key codes (winit / DOM `KeyboardEvent.code`) to
/// Twe key names and call this once per frame.
pub fn apply_key_state(
    env: &mut Env,
    names: &[&str],
    held: &dyn Fn(&str) -> bool,
    pressed: &dyn Fn(&str) -> bool,
) {
    for (ambient, state) in [("key", held), ("key_press", pressed)] {
        if let Some(t) = env.get(ambient) {
            if t.is_object() {
                let rc = t.as_object();
                let mut o = rc.borrow_mut();
                for name in names {
                    o.insert_field(*name, Value::from_bool(state(name)));
                }
            }
        }
    }
}

// Phase 11 follow-on (deeper): the real auto-pause-on-window-blur
// machinery the Phase-11 closeout punted on. Each shell reports focus once per
// frame (the 2D loop polls `window_focus::is_focused()`; the native 3D
// shell reads winit focus events; the browser reads
// `document.visibilityState`) and this drives the pause flag on
// transitions. State-machine summary:
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
pub struct BlurAutoPause {
    /// Was the window focused last frame? Initial state is `true` so
    /// startup-while-unfocused doesn't fire a spurious pause.
    last_focused: bool,
    /// True when we drove `pause(true)` — focus return will then drive
    /// `pause(false)`. Manually set pause stays paused.
    paused_by_us: bool,
}

impl BlurAutoPause {
    pub fn new() -> Self {
        Self {
            last_focused: true,
            paused_by_us: false,
        }
    }

    pub fn tick(&mut self, focused: bool) {
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

impl Default for BlurAutoPause {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod blur_tests {
    use super::BlurAutoPause;
    use crate::stdlib::{is_paused, set_paused};

    #[test]
    fn blur_pauses_only_when_asked_and_resumes_its_own_pause() {
        set_paused(false);
        let mut blur = BlurAutoPause::new();
        blur.tick(false);
        assert!(!is_paused(), "off by default");
        blur.tick(true);

        crate::stdlib::set_auto_pause_on_blur(true);
        blur.tick(false);
        assert!(is_paused(), "focus lost: paused");
        blur.tick(true);
        assert!(!is_paused(), "focus back: resumed");

        // A pause the game set itself survives a focus round trip.
        set_paused(true);
        blur.tick(false);
        blur.tick(true);
        assert!(is_paused());
        set_paused(false);
        crate::stdlib::set_auto_pause_on_blur(false);
    }
}
