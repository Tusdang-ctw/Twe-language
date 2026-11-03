//! web3d-M2: the interpreter side of 3D hosting, shared by every shell.
//!
//! Gathers a [`RenderSnapshot`] from interpreter state (the script's
//! `on render():` draw queue, the `camera` object, stdlib light / shadow
//! / post-FX settings) and hands it to the kernel renderer. The native
//! winit shell (`play3d.rs`) and the browser shell both call
//! [`render_frame`], so a frame means the same thing everywhere.

use crate::eval;
use crate::kernel::render::{AssetSource, PostFx, RenderSnapshot, Renderer, ShadowSettings};
use crate::replay::InputCommand;
use crate::value::{Env, RuntimeError, Value};
use std::collections::BTreeSet;

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
    render_frame_dt(renderer, env, assets, crate::eval::PHYSICS_DT)
}

/// [`render_frame`] for a frame `frame_dt` seconds after the last one.
/// Lanternfall E2: the visual clocks (screen shake, `fx.*`) advance by
/// it, as the 2D loop's do; `render_frame` advances one tick's worth, so
/// captures and tests are deterministic.
pub fn render_frame_dt(
    renderer: &mut Renderer,
    env: &mut Env,
    assets: &mut dyn AssetSource,
    frame_dt: f64,
) -> Result<FrameTimes, String> {
    let start = crate::clock::now_secs();
    crate::stdlib::camera_tick(frame_dt);
    crate::fx::fx_tick(frame_dt);
    // This frame's shake, as a world offset of the camera: the script's
    // `camera.project` and the renderer both see it.
    {
        let (sx, sy) = crate::stdlib::camera_shake_offset();
        let cam = crate::camera3d::read_camera(env);
        crate::camera3d::set_shake(crate::camera3d::shake_offset(&cam, sx as f32, sy as f32));
    }
    // web3d-M7: from here on, particles that can run on the GPU do.
    env.gpu_particles = true;
    // web3d-M7 follow-up: the looks the renderer already holds.
    env.retained_looks = renderer.retained_generation();
    if let Err(e) = eval::render_frame3d(env) {
        // Surface the runtime error to stderr but keep rendering — a
        // broken render frame shouldn't tear down the window.
        env.out.push_str(&format!(
            "render error in `on render()`: {}:{}: {}
",
            e.line, e.col, e.message
        ));
    }
    let camera = crate::camera3d::shaken(&crate::camera3d::read_camera(env));
    // Lanternfall E2: `fx.*` effects over the HUD, world-anchored ones
    // where their point projects this frame.
    {
        let (w, h) = crate::camera3d::viewport();
        let hud = crate::camera3d::hud_size(w, h);
        let aspect = crate::camera3d::aspect();
        crate::fx::fx_draw_overlay_3d(&mut env.draw2d, &|p| {
            crate::camera3d::project(&camera, aspect, hud, p)
        });
    }
    let draws = std::mem::take(&mut env.render_queue3d);
    let anim = |id: u32| crate::stdlib::mesh_anim_state(id);
    let lut = crate::stdlib::color_lut();
    let environment = crate::stdlib::environment_settings();
    let point_lights = crate::stdlib::point_lights_snapshot();
    let emissions = std::mem::take(&mut env.particle_emissions);
    let cpu_particles = eval::cpu_particles_3d(env);
    let snap = RenderSnapshot {
        particles: crate::kernel::particles::ParticleFrame {
            programs: &env.particle_programs,
            emissions: &emissions,
            cpu: &cpu_particles,
        },
        point_lights: &point_lights,
        fog: crate::stdlib::fog_settings().map(|(density, falloff, color)| crate::kernel::render::FogSettings {
            density,
            falloff,
            color,
            volumetric: crate::stdlib::fog_volumetric(),
        }),
        camera,
        lut: lut.as_ref().map(|(path, strength)| crate::kernel::render::LutSettings {
            path,
            strength: *strength,
        }),
        environment: environment.as_ref().map(|(path, intensity)| crate::kernel::render::EnvironmentSettings {
            path,
            intensity: *intensity,
            backdrop: false,
        }),
        background: crate::stdlib::background_color(),
        lights: crate::stdlib::lights_snapshot(),
        shadow: ShadowSettings {
            enabled: crate::stdlib::shadow_enabled(),
            extent: crate::stdlib::shadow_extent(),
        },
        post: PostFx {
            tonemapper: crate::stdlib::tonemap_curve(),
            taa: crate::stdlib::taa_enabled(),
            vignette: crate::stdlib::vignette_strength(),
            vignette_color: crate::stdlib::vignette_color(),
            bloom_intensity: crate::stdlib::bloom_intensity(),
            bloom_threshold: crate::stdlib::bloom_threshold(),
            frustum_cull: crate::stdlib::frustum_culling_enabled(),
            exposure: crate::stdlib::exposure_stops(),
            auto_exposure: crate::stdlib::auto_exposure_enabled(),
            ao: crate::stdlib::ao_settings().0,
            ao_radius: crate::stdlib::ao_settings().1,
            dof_focus: crate::stdlib::dof_settings().0,
            dof_f_stop: crate::stdlib::dof_settings().1,
            motion_blur: crate::stdlib::motion_blur_shutter(),
            ssr: crate::stdlib::ssr_strength(),
        },
        draws: &draws,
        draws_generation: env.draws_generation,
        mesh_paths: &env.mesh_paths,
        texture_paths: &env.texture_paths,
        time: env.sim_time as f32,
        materials: &env.material_sources,
        hud: &env.draw2d,
        anim: &anim,
        instance_anims: &env.instance_anims,
    };
    let scripted = crate::clock::now_secs();
    renderer.set_quality(crate::stdlib::graphics_quality());
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

/// Mouse buttons the shells report, as Twe `mouse_held.<name>` /
/// `mouse_press.<name>` names.
pub const MOUSE_BUTTONS: &[&str] = &["left", "middle", "right"];

/// web3d-M4: the input a shell has seen since the last simulation tick.
///
/// Shells feed it events as they arrive; each tick takes one
/// [`InputCommand`] from it with [`sim_tick`]. A press is seen by
/// exactly one tick however frames and ticks interleave: at 144 Hz
/// most frames run no tick, and a press applied per frame (as before)
/// was lost; a frame that ran two ticks saw it twice. Motion and wheel
/// accumulate the same way.
#[derive(Default)]
pub struct InputState {
    keys_held: BTreeSet<&'static str>,
    keys_pressed: BTreeSet<&'static str>,
    buttons_held: BTreeSet<&'static str>,
    buttons_pressed: BTreeSet<&'static str>,
    mouse: (f64, f64),
    motion: (f64, f64),
    wheel: f64,
    pad_axes: Option<[f64; 6]>,
    pad_held: BTreeSet<&'static str>,
    pad_pressed: BTreeSet<&'static str>,
    /// Lanternfall E1: the view in pixels, as the 3D host last reported
    /// it ([`InputState::set_view`]); `(0, 0)` in hosts that don't.
    view: (u32, u32),
}

impl InputState {
    /// Lanternfall E1: the window (or canvas) is now `width × height`
    /// pixels. The simulation sees it through the next command
    /// (`camera.project`, `screen.aspect`, the HUD's width), and the
    /// cursor keeps its place in the view.
    pub fn set_view(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (width, height) == self.view {
            return;
        }
        let old = self.hud_size();
        self.view = (width, height);
        let new = self.hud_size();
        self.mouse = (
            self.mouse.0 * new[0] / old[0],
            self.mouse.1 * new[1] / old[1],
        );
    }

    /// The HUD canvas the cursor is measured in: 640×480 until a view
    /// is set, then 480 units tall and as wide as the view's aspect
    /// (`camera3d::hud_size`).
    pub fn hud_size(&self) -> [f64; 2] {
        let [w, h] = crate::camera3d::hud_size(self.view.0, self.view.1);
        [f64::from(w), f64::from(h)]
    }

    /// The cursor is at fraction `(fx, fy)` of the view (0..1 from the
    /// top-left), stored in HUD canvas units.
    pub fn pointer(&mut self, fx: f64, fy: f64) {
        let [w, h] = self.hud_size();
        self.mouse = (fx * w, fy * h);
    }

    /// A key went down. Auto-repeat (already held) is not a new press.
    pub fn key_down(&mut self, name: &'static str) {
        if self.keys_held.insert(name) {
            self.keys_pressed.insert(name);
        }
    }

    pub fn key_up(&mut self, name: &str) {
        self.keys_held.remove(name);
    }

    pub fn button_down(&mut self, name: &'static str) {
        if self.buttons_held.insert(name) {
            self.buttons_pressed.insert(name);
        }
    }

    pub fn button_up(&mut self, name: &str) {
        self.buttons_held.remove(name);
    }

    /// Cursor position, in the 640×480 canvas coordinates the HUD uses.
    pub fn mouse_move(&mut self, x: f64, y: f64) {
        self.mouse = (x, y);
    }

    /// Raw pointer motion (unaffected by cursor lock).
    pub fn mouse_motion(&mut self, dx: f64, dy: f64) {
        self.motion.0 += dx;
        self.motion.1 += dy;
    }

    pub fn wheel(&mut self, dy: f64) {
        self.wheel += dy;
    }

    /// The first gamepad as the shell polled it this frame: `buttons`
    /// in `stdlib::GAMEPAD_BUTTON_NAMES` order, `axes` in
    /// `GAMEPAD_AXIS_NAMES` order; `None` when no pad is connected.
    /// A button that went down since the last poll is a press.
    pub fn set_gamepad(&mut self, pad: Option<(&[bool], [f64; 6])>) {
        let names = crate::stdlib::GAMEPAD_BUTTON_NAMES;
        match pad {
            Some((buttons, axes)) => {
                self.pad_axes = Some(axes);
                for (name, down) in names.iter().zip(buttons) {
                    if *down {
                        if self.pad_held.insert(name) {
                            self.pad_pressed.insert(name);
                        }
                    } else {
                        self.pad_held.remove(name);
                    }
                }
            }
            None => {
                self.pad_axes = None;
                self.pad_held.clear();
            }
        }
    }

    /// The window lost focus: its key-up events will never arrive, so
    /// let go of everything rather than leave keys stuck down.
    pub fn release_all(&mut self) {
        self.keys_held.clear();
        self.buttons_held.clear();
    }

    /// The command for the next tick; presses, motion and wheel start
    /// over.
    pub fn take_command(&mut self) -> InputCommand {
        let names = |s: &BTreeSet<&'static str>| s.iter().map(|n| n.to_string()).collect();
        let cmd = InputCommand {
            keys_held: names(&self.keys_held),
            keys_pressed: names(&self.keys_pressed),
            mouse_x: self.mouse.0,
            mouse_y: self.mouse.1,
            mb_held: names(&self.buttons_held),
            mb_press: names(&self.buttons_pressed),
            mouse_dx: self.motion.0,
            mouse_dy: self.motion.1,
            wheel: self.wheel,
            pad_axes: self.pad_axes,
            pad_held: names(&self.pad_held),
            pad_pressed: names(&self.pad_pressed),
            view_w: self.view.0,
            view_h: self.view.1,
        };
        self.keys_pressed.clear();
        self.pad_pressed.clear();
        self.buttons_pressed.clear();
        self.motion = (0.0, 0.0);
        self.wheel = 0.0;
        cmd
    }
}

/// Write a command into the input ambients: `key` / `key_press` for
/// every name in [`KEY_CODES`], `mouse` (`x`, `y`, `pos`, `dx`, `dy`,
/// `wheel`), `mouse_held` / `mouse_press` for [`MOUSE_BUTTONS`], and
/// `gamepad` / `gamepad_press` / `gamepad_axis`.
pub fn apply_command(env: &mut Env, cmd: &InputCommand) {
    let has = |list: &[String], name: &str| list.iter().any(|n| n == name);
    let names: Vec<&str> = KEY_CODES.iter().map(|(n, _)| *n).collect();
    apply_key_state(env, &names, &|n| has(&cmd.keys_held, n), &|n| {
        has(&cmd.keys_pressed, n)
    });
    let pads = crate::stdlib::GAMEPAD_BUTTON_NAMES;
    for (ambient, list, names) in [
        ("mouse_held", &cmd.mb_held, MOUSE_BUTTONS),
        ("mouse_press", &cmd.mb_press, MOUSE_BUTTONS),
        ("gamepad", &cmd.pad_held, pads),
        ("gamepad_press", &cmd.pad_pressed, pads),
    ] {
        if let Some(t) = env.get(ambient) {
            if t.is_object() {
                let rc = t.as_object();
                let mut o = rc.borrow_mut();
                for name in names {
                    o.insert_field(*name, Value::from_bool(has(list, name)));
                }
            }
        }
    }
    crate::replay::write_pad(env, cmd.pad_axes);
    if cmd.view_w > 0 && cmd.view_h > 0 {
        apply_view(env, cmd.view_w, cmd.view_h);
    }
    if let Some(t) = env.get("mouse") {
        if t.is_object() {
            let rc = t.as_object();
            let mut o = rc.borrow_mut();
            o.insert_field("x", Value::from_float(cmd.mouse_x));
            o.insert_field("y", Value::from_float(cmd.mouse_y));
            o.insert_field(
                "pos",
                Value::from_tuple(vec![
                    Value::from_float(cmd.mouse_x),
                    Value::from_float(cmd.mouse_y),
                ]),
            );
            o.insert_field("dx", Value::from_float(cmd.mouse_dx));
            o.insert_field("dy", Value::from_float(cmd.mouse_dy));
            o.insert_field("wheel", Value::from_float(cmd.wheel));
        }
    }
}

/// Lanternfall E1: the view is `width × height` pixels. `camera.project`
/// and friends use its aspect; `screen.size` / `screen.center` report
/// the HUD canvas it gives (480 units tall) and `screen.aspect` the
/// ratio.
fn apply_view(env: &mut Env, width: u32, height: u32) {
    crate::camera3d::set_viewport(width, height);
    let [w, h] = crate::camera3d::hud_size(width, height);
    let (w, h) = (f64::from(w), f64::from(h));
    if let Some(t) = env.get("screen").filter(|t| t.is_object()) {
        let rc = t.as_object();
        let mut o = rc.borrow_mut();
        o.insert_field(
            "size",
            Value::from_tuple(vec![Value::from_float(w), Value::from_float(h)]),
        );
        o.insert_field(
            "center",
            Value::from_tuple(vec![Value::from_float(w * 0.5), Value::from_float(h * 0.5)]),
        );
        o.insert_field("aspect", Value::from_float(f64::from(width) / f64::from(height)));
    }
}

/// One fixed simulation tick: step the physics world, take the tick's
/// input command, let the replay recorder / player see it
/// (`replay::step`), write it into the ambients, then run
/// `eval::tick_frame`. Every 3D shell — native `play3d`, the browser
/// shell, the soak harness — ticks through here, so both input and the
/// physics step reach the simulation one way only.
///
/// web3d-M11 follow-on: the `physics3d::step` used to live in the
/// native-only `play3d` loop, so browser 3D games never advanced
/// physics. Stepping here (before the script's `on update(dt)` body)
/// gives native and web the same authoritative integrator. The step is
/// a near-instant no-op until a script spawns a body, so non-physics
/// scenes pay nothing.
pub fn sim_tick(env: &mut Env, input: &mut InputState, dt: f64) -> Result<(), RuntimeError> {
    // Lanternfall E2: `fx.hit_stop` freezes the game for whole ticks, as
    // the 2D loop does: no physics, no input taken (presses wait for the
    // next real tick), no script. Counted in ticks, so replays agree.
    if crate::fx::consume_hit_stop_tick() {
        return Ok(());
    }
    // Physics holds still while paused, unless a persistent state keeps
    // the world running (what `eval::tick_frame` ticks then).
    // Lanternfall G6: `time.set_scale` slows (or speeds) the game: the
    // tick advances physics and the script by the scaled step.
    let dt = dt * crate::stdlib::time_scale();
    if !crate::stdlib::is_paused() || crate::stdlib::has_persistent_states() {
        crate::physics3d::step(dt as f32);
    }
    let cmd = crate::replay::step(input.take_command());
    apply_command(env, &cmd);
    eval::tick_frame(env, dt)
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

#[cfg(test)]
mod input_tests {
    use super::InputState;

    #[test]
    fn a_press_reaches_exactly_one_tick() {
        let mut input = InputState::default();
        input.key_down("space");
        // Frames that run no tick don't consume it...
        input.key_down("space"); // auto-repeat
        let first = input.take_command();
        assert_eq!(first.keys_pressed, vec!["space".to_string()]);
        assert_eq!(first.keys_held, vec!["space".to_string()]);
        // ...and the next tick doesn't see it again.
        let second = input.take_command();
        assert!(second.keys_pressed.is_empty());
        assert_eq!(second.keys_held, vec!["space".to_string()]);
        // A tap shorter than a tick still counts.
        input.key_up("space");
        input.key_down("x");
        input.key_up("x");
        let third = input.take_command();
        assert_eq!(third.keys_pressed, vec!["x".to_string()]);
        assert!(third.keys_held.is_empty());
    }

    #[test]
    fn motion_and_wheel_accumulate_per_tick_and_blur_releases() {
        let mut input = InputState::default();
        input.mouse_motion(2.0, 1.0);
        input.mouse_motion(3.0, -1.0);
        input.wheel(1.0);
        input.button_down("left");
        let cmd = input.take_command();
        assert_eq!((cmd.mouse_dx, cmd.mouse_dy, cmd.wheel), (5.0, 0.0, 1.0));
        assert_eq!(cmd.mb_press, vec!["left".to_string()]);
        let next = input.take_command();
        assert_eq!((next.mouse_dx, next.wheel), (0.0, 0.0));
        input.key_down("w");
        input.release_all();
        let after = input.take_command();
        assert!(after.keys_held.is_empty() && after.mb_held.is_empty());
    }
}

/// Low-end track (G5): one live frame of the automatic quality governor.
/// When the game hasn't fixed a budget (`graphics.quality`,
/// `graphics.render_scale`, `TWE_QUALITY`), feed the governor the
/// interval since the last frame and publish its budget for
/// `render_frame`; otherwise switch it off. The governor is created on
/// the first such frame, from the GPU's kind. Live hosts only: tests,
/// benchmarks and `?capture` never call this, so they render on `Ultra`.
/// Returns the new budget when it changed (hosts log it).
pub fn govern_quality(
    governor: &mut Option<crate::kernel::quality::AutoQuality>,
    device: wgpu::DeviceType,
    interval_secs: f32,
    target_fps: f32,
) -> Option<crate::kernel::quality::Quality> {
    use crate::kernel::quality::{starting_tier, AutoQuality};
    if crate::stdlib::graphics_quality_is_explicit() {
        crate::stdlib::set_auto_quality(None);
        return None;
    }
    let mut changed = governor.is_none();
    let g = governor.get_or_insert_with(|| AutoQuality::new(starting_tier(device), target_fps));
    changed |= g.update(interval_secs);
    let q = g.quality();
    crate::stdlib::set_auto_quality(Some(q));
    changed.then_some(q)
}
