//! `twec play3d` — wgpu-driven 3D backend.
//!
//! Phase 5 task 5 closed at v0.1-minimum-viable on 2026-04-29 across
//! six sessions, all landing here:
//!
//! - **(a)** Clear-color window via wgpu + winit
//!   (`docs/changes/2026-04-29-phase-5-task-5-session-1-wgpu-scaffold.md`).
//! - **(b) + (c)** Vertex / index / camera buffers, WGSL flat-shading
//!   pipeline, depth buffer, hand-rolled column-major matrix math
//!   (`docs/changes/2026-04-29-phase-5-task-5-sessions-bc-cube-and-camera.md`).
//! - **(d) + (e)** Twe-driven scene: top-level `on render():` queues
//!   cubes via `cube(at:, color:, size:)`; `camera.eye`/`.target`/`.up`
//!   are mutable ambient fields. `vec3(x, y, z)` constructor + math
//!   primitives. One instanced draw call per frame, up to 4096 cubes
//!   (`docs/changes/2026-04-29-phase-5-task-5-sessions-de-twe-driven-3d.md`).
//! - **Carry-over** (this module's final shape): winit `KeyboardInput`
//!   → Twe `key.*` / `key_press.*`, mtime-poll hot reload, per-vertex
//!   normals + Lambertian directional shading. `tick_frame` runs the
//!   script's `on update(dt):` before each render so input-driven
//!   logic actually fires.
//!
//! Architecture matches `src/play.rs`'s split — startup runs the
//! script's top-level once (registering globals + the `on update`
//! / `on render` handlers), then the platform render loop drives
//! per-frame invocation. Each `RedrawRequested` does, in order:
//! hot-reload poll, key-state push into env, `tick_frame` (which
//! fires `on update(dt):`), `render_frame3d` (which fires
//! `on render():` and drains the cube queue), GPU submit, present.
//!
//! v0.2 task-5 follow-ons (per `docs/changes/2026-04-29-phase-5-closeout.md`):
//! `.glb` / `.obj` mesh import (session 1, this module's `mesh()` plus
//! `load_glb`), generic primitives (`sphere`, `plane`, `mesh`), bytecode
//! VM 3D path, mouse input, proper lighting (point / area / shadows),
//! `mat4` / `quat` stdlib types.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::kernel::render::{
    parse_glb_bytes, AssetKind, AssetReady, AssetSource, LoadedGlb, Renderer,
};
use crate::value::Env;
use crate::{eval, lexer, parser, stdlib};

/// Twe-side key names ↔ winit physical key codes. Same name set
/// `src/play.rs` exposes for the macroquad path; the user's
/// `key.right` reads the same way no matter which loop is driving.
// Phase 27: full key namespace for the 3D winit path — matches
// the field set registered by `stdlib::register_keys` and the
// 2D macroquad mirror in `play.rs`.
const KEYS: &[(&str, KeyCode)] = &[
    // Movement / arrows.
    ("right", KeyCode::ArrowRight),
    ("left", KeyCode::ArrowLeft),
    ("up", KeyCode::ArrowUp),
    ("down", KeyCode::ArrowDown),
    // Common control keys.
    ("space", KeyCode::Space),
    ("escape", KeyCode::Escape),
    ("enter", KeyCode::Enter),
    ("tab", KeyCode::Tab),
    ("backspace", KeyCode::Backspace),
    ("shift", KeyCode::ShiftLeft),
    ("ctrl", KeyCode::ControlLeft),
    ("alt", KeyCode::AltLeft),
    // Letters a–z.
    ("a", KeyCode::KeyA),
    ("b", KeyCode::KeyB),
    ("c", KeyCode::KeyC),
    ("d", KeyCode::KeyD),
    ("e", KeyCode::KeyE),
    ("f", KeyCode::KeyF),
    ("g", KeyCode::KeyG),
    ("h", KeyCode::KeyH),
    ("i", KeyCode::KeyI),
    ("j", KeyCode::KeyJ),
    ("k", KeyCode::KeyK),
    ("l", KeyCode::KeyL),
    ("m", KeyCode::KeyM),
    ("n", KeyCode::KeyN),
    ("o", KeyCode::KeyO),
    ("p", KeyCode::KeyP),
    ("q", KeyCode::KeyQ),
    ("r", KeyCode::KeyR),
    ("s", KeyCode::KeyS),
    ("t", KeyCode::KeyT),
    ("u", KeyCode::KeyU),
    ("v", KeyCode::KeyV),
    ("w", KeyCode::KeyW),
    ("x", KeyCode::KeyX),
    ("y", KeyCode::KeyY),
    ("z", KeyCode::KeyZ),
    // Digits 0–9.
    ("0", KeyCode::Digit0),
    ("1", KeyCode::Digit1),
    ("2", KeyCode::Digit2),
    ("3", KeyCode::Digit3),
    ("4", KeyCode::Digit4),
    ("5", KeyCode::Digit5),
    ("6", KeyCode::Digit6),
    ("7", KeyCode::Digit7),
    ("8", KeyCode::Digit8),
    ("9", KeyCode::Digit9),
    // Function row F1–F12.
    ("f1", KeyCode::F1),
    ("f2", KeyCode::F2),
    ("f3", KeyCode::F3),
    ("f4", KeyCode::F4),
    ("f5", KeyCode::F5),
    ("f6", KeyCode::F6),
    ("f7", KeyCode::F7),
    ("f8", KeyCode::F8),
    ("f9", KeyCode::F9),
    ("f10", KeyCode::F10),
    ("f11", KeyCode::F11),
    ("f12", KeyCode::F12),
];

/// Map a winit `MouseButton` to its Twe-side name. Buttons beyond
/// left / middle / right (Back / Forward / Other) aren't surfaced
/// in v0.2 — match what macroquad exposes for cross-backend parity.
fn mouse_button_name(b: MouseButton) -> Option<&'static str> {
    match b {
        MouseButton::Left => Some("left"),
        MouseButton::Middle => Some("middle"),
        MouseButton::Right => Some("right"),
        _ => None,
    }
}

/// `twec play3d <file>` entry. Parses + runs the file's top-level
/// code once, then enters the wgpu render loop until the window
/// closes. Returns the process exit code.
pub fn launch(path: String) -> i32 {
    // web3d-M4: this shell plays `sound.*` (see `NativeAudio`).
    crate::audio_host::enable();
    let env = match initialize(&path) {
        Ok(env) => env,
        Err(()) => return 1,
    };
    let last_mtime = current_mtime(&path);

    let event_loop = match EventLoop::new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("error: could not create event loop: {e}");
            return 1;
        }
    };
    let mut app = App::new(env, path, last_mtime);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("error: event loop: {e}");
        return 1;
    }
    app.exit_code
}

fn current_mtime(path: &str) -> Option<SystemTime> {
    std::fs::metadata(Path::new(path))
        .ok()
        .and_then(|m| m.modified().ok())
}

/// Lex + parse + run the Twe file's top-level statements once. Any
/// error during this phase prints the diagnostic and returns Err so
/// the caller doesn't open a window. Returns the live Env for the
/// render loop to call `on render():` against on every frame.
fn initialize(path: &str) -> Result<Env, ()> {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read {path}: {e}");
            return Err(());
        }
    };
    let tokens = match lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}:{}:{}: {}", e.line, e.col, e.message);
            return Err(());
        }
    };
    let program = match parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}:{}:{}: {}", e.line, e.col, e.message);
            if let Some(help) = &e.help {
                eprintln!("  help: {help}");
            }
            return Err(());
        }
    };
    // web3d-M1: scripts that import modules go through the module loader.
    let mut env = if crate::module::has_imports(&program) {
        crate::module::prepare_entry(std::path::Path::new(path), &src).map_err(|msg| {
            eprintln!("{msg}");
        })?
    } else {
        let mut env = Env::new();
        stdlib::install(&mut env);
        if let Err(e) = eval::run_top_level(&mut env, &program) {
            eprintln!("{path}:{}:{}: {}", e.line, e.col, e.message);
            if let Some(help) = &e.help {
                eprintln!("  help: {help}");
            }
            return Err(());
        }
        env
    };
    if !env.out.is_empty() {
        // Drain any startup `print` output to stdout so the user
        // sees it before the window opens.
        print!("{}", env.out);
        env.out.clear();
    }
    Ok(env)
}

// ---------- Vertex / instance / uniform layouts ----------

struct App {
    /// Live `Env` from `initialize`. Held across the event loop so
    /// the per-frame `on render():` invocation reaches the same
    /// globals the script registered at startup. Replaced wholesale
    /// on hot reload.
    env: Env,
    state: Option<ShellState>,
    /// web3d-M2: loads meshes / textures for the kernel renderer.
    assets: NativeAssets,
    last_frame_at: Instant,
    /// web3d-M2: monotonic clock for the tap detector. (It called
    /// macroquad's `time::get_time()`, which panics outside a macroquad
    /// window — every `twec play3d` run crashed on its first frame from
    /// 2026-06-01 until this fix.)
    started_at: Instant,
    /// Source path + last-seen mtime for hot reload polling.
    path: String,
    last_mtime: Option<SystemTime>,
    /// web3d-M4: keys, buttons, cursor, motion and wheel seen since
    /// the last simulation tick; each tick takes one command from it
    /// (`host3d::sim_tick`).
    input: crate::host3d::InputState,
    /// Phase 29 session 1: fixed-timestep accumulator. Same Glenn
    /// Fiedler pattern as the 2D macroquad path in `src/play.rs`.
    /// We drain `eval::PHYSICS_DT`-sized slices through
    /// `physics3d::step` + `eval::tick_frame` before each render.
    sim_accumulator: f64,
    exit_code: i32,
    /// web3d-M4: plays the script's queued `sound.*` commands.
    audio: NativeAudio,
    /// web3d-M4: the gamepad, polled each frame into `input`. `None`
    /// once gilrs failed to start (no input subsystem).
    gilrs: Option<gilrs::Gilrs>,
    /// web3d-M4: `auto_pause_on_blur(true)` pauses when the window
    /// loses focus.
    blur: crate::host3d::BlurAutoPause,
}

/// web3d-M4: the native 3D shell's sound player. `sound.*` queues
/// commands (`audio_host`) since there is no macroquad window here;
/// this plays them through `quad-snd`, macroquad's own audio backend.
/// The device opens on the first sound, so silent games never touch it.
#[derive(Default)]
struct NativeAudio {
    ctx: Option<quad_snd::AudioContext>,
    sounds: HashMap<String, quad_snd::Sound>,
    /// Paths that failed to load: reported once, not every play.
    failed: HashSet<String>,
}

impl NativeAudio {
    fn play_queued(&mut self) {
        for cmd in crate::audio_host::drain() {
            match cmd {
                crate::audio_host::AudioCmd::Play {
                    path,
                    volume,
                    looped,
                } => {
                    if self.load(&path) {
                        if let (Some(ctx), Some(sound)) = (&self.ctx, self.sounds.get(&path)) {
                            sound.play(ctx, quad_snd::PlaySoundParams { looped, volume });
                        }
                    }
                }
                crate::audio_host::AudioCmd::Stop { path } => {
                    if let (Some(ctx), Some(sound)) = (&self.ctx, self.sounds.get(&path)) {
                        sound.stop(ctx);
                    }
                }
                crate::audio_host::AudioCmd::SetVolume { path, volume } => {
                    if let (Some(ctx), Some(sound)) = (&self.ctx, self.sounds.get(&path)) {
                        sound.set_volume(ctx, volume);
                    }
                }
            }
        }
    }

    /// Decode `path` on first use (opening the device if needed);
    /// false if it can't be loaded.
    fn load(&mut self, path: &str) -> bool {
        if self.failed.contains(path) {
            return false;
        }
        if !self.sounds.contains_key(path) {
            let ctx = self.ctx.get_or_insert_with(quad_snd::AudioContext::new);
            match crate::bundle::read_asset_bytes(path) {
                Ok(bytes) => {
                    let sound = quad_snd::Sound::load(ctx, &bytes);
                    self.sounds.insert(path.to_string(), sound);
                }
                Err(e) => {
                    eprintln!("sound: cannot read '{path}': {e}");
                    self.failed.insert(path.to_string());
                    return false;
                }
            }
        }
        true
    }
}

impl App {
    fn new(env: Env, path: String, last_mtime: Option<SystemTime>) -> Self {
        Self {
            assets: NativeAssets::default(),
            env,
            state: None,
            last_frame_at: Instant::now(),
            started_at: Instant::now(),
            path,
            last_mtime,
            input: crate::host3d::InputState::default(),
            sim_accumulator: 0.0,
            exit_code: 0,
            audio: NativeAudio::default(),
            blur: crate::host3d::BlurAutoPause::new(),
            gilrs: gilrs::Gilrs::new()
                .map_err(|e| eprintln!("[twec] gamepad disabled: {e}"))
                .ok(),
        }
    }

    /// Map a winit physical-key code to the Twe-side `&'static str`
    /// name (`"right"`, `"space"`, …). Returns `None` for keys we
    /// don't surface — the input set matches the macroquad path's
    /// `KEYS` table so scripts behave the same on both backends.
    fn key_name(code: KeyCode) -> Option<&'static str> {
        KEYS.iter()
            .find_map(|(name, c)| if *c == code { Some(*name) } else { None })
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title("Twec play3d")
            .with_inner_size(LogicalSize::new(640.0, 480.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("error: window create: {e}");
                self.exit_code = 1;
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let created = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())
            .and_then(|surface| {
                pollster::block_on(Renderer::new(&instance, surface, size.width, size.height))
            });
        match created {
            Ok(renderer) => self.state = Some(ShellState { window, renderer }),
            Err(e) => {
                eprintln!("error: wgpu init: {e}");
                self.exit_code = 1;
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let state = match self.state.as_mut() {
            Some(s) => s,
            None => return,
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(focused) => {
                if !focused {
                    self.input.release_all();
                }
                self.blur.tick(focused);
            }
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    state.renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: key_state,
                        repeat,
                        ..
                    },
                ..
            } => {
                if let Some(name) = Self::key_name(code) {
                    match key_state {
                        // OS auto-repeat is not a new press.
                        ElementState::Pressed if !repeat => self.input.key_down(name),
                        ElementState::Pressed => {}
                        ElementState::Released => self.input.key_up(name),
                    }
                }
                // Esc closes the window — same convention as the
                // macroquad path, including the `quit_on_escape(false)`
                // opt-out (web3d-M0).
                if matches!(code, KeyCode::Escape)
                    && key_state == ElementState::Pressed
                    && crate::stdlib::quit_on_escape()
                {
                    event_loop.exit();
                }
            }
            // v0.2 session 3: mouse events. CursorMoved tracks
            // position; MouseInput tracks button held + edge-press;
            // MouseWheel accumulates the per-frame wheel delta.
            WindowEvent::CursorMoved { position, .. } => {
                // web3d-M4: in the HUD's 640×480 canvas units, as in 2D.
                let size = state.window.inner_size();
                self.input.mouse_move(
                    position.x * f64::from(crate::kernel::hud::CANVAS_W) / f64::from(size.width.max(1)),
                    position.y * f64::from(crate::kernel::hud::CANVAS_H) / f64::from(size.height.max(1)),
                );
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                if let Some(name) = mouse_button_name(button) {
                    match btn_state {
                        ElementState::Pressed => self.input.button_down(name),
                        ElementState::Released => self.input.button_up(name),
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Two delta shapes: line-based (most desktop mice)
                // and pixel-based (trackpads). Normalize both into
                // a single y-axis scroll value summed for the frame.
                // Pixel-deltas tend to be ~10–20px per "tick"; the
                // 1/120 factor approximates the macroquad path's
                // tick-count semantics.
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_x, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32) / 120.0,
                };
                self.input.wheel(f64::from(dy));
            }
            WindowEvent::RedrawRequested => {
                // Hot reload: poll the source's mtime, re-init env
                // on change. Mirrors `src/play.rs::run_loop`. A
                // failed re-init keeps the current env so the
                // window doesn't tear down on a transient typo.
                let cur_mtime = current_mtime(&self.path);
                if cur_mtime.is_some() && cur_mtime != self.last_mtime {
                    if let Ok(new_env) = initialize(&self.path) {
                        eprintln!("[twec] hot reload: {}", self.path);
                        crate::stdlib::clear_asset_caches();
                        // The new env's `mesh_paths` indices are
                        // independent of the old env's, so cached
                        // GpuMesh entries by id are stale. Drop
                        // them and let the next frame re-load by
                        // path; on-disk `.glb` edits also pick up
                        // because of this.
                        // Phase 28 session 5: in-flight async loads
                        // also reference the old env's path/id pairs
                        // — drop them so the new env's first frame
                        // re-requests from current paths.
                        state.renderer.clear_asset_caches();
                        self.assets = NativeAssets::default();
                        // Phase 18: drop all rigid bodies; the new
                        // env will recreate them on its first
                        // `on update(dt)` tick. Otherwise stale
                        // handles from the prior run leak.
                        crate::physics3d::reset();
                        self.env = new_env;
                    }
                    // A failed re-init keeps the current env so the
                    // window doesn't tear down on a transient typo.
                    self.last_mtime = cur_mtime;
                }

                // v1.0.2 Session 7: tap-event diff. The 3D path can
                // be a mobile target too (Phase 39 reference scene
                // composes touch + virtual joystick); same hook as
                // every `run_loop_*` in play.rs.
                crate::stdlib::tick_touch_taps(self.started_at.elapsed().as_secs_f64());
                if let Some(g) = self.gilrs.as_mut() {
                    let pad = crate::play::read_first_pad(g);
                    self.input
                        .set_gamepad(pad.as_ref().map(|(b, a)| (&b[..], *a)));
                }

                // Phase 17 session 3: drain any pending cursor-mode
                // request from the script side. cursor.lock() /
                // cursor.unlock() write a CursorMode here; we apply
                // it to the window once per frame so the request
                // takes effect even if the script is in a render
                // handler when it fires.
                if let Some(mode) = crate::stdlib::take_pending_cursor_mode() {
                    apply_cursor_mode(&state.window, mode);
                }

                let now = Instant::now();
                let frame_dt = now.duration_since(self.last_frame_at).as_secs_f32();
                self.last_frame_at = now;
                // Phase 29 session 1: drain fixed-step substeps before
                // composing the frame. `step_simulation_3d` runs
                // physics3d::step + eval::tick_frame at PHYSICS_DT;
                // `render` does GPU work only.
                let frame_dt_clamped = (frame_dt as f64).min(crate::eval::MAX_FRAME_DT);
                self.sim_accumulator += frame_dt_clamped;
                let mut substeps: u32 = 0;
                while self.sim_accumulator >= crate::eval::PHYSICS_DT
                    && substeps < crate::eval::MAX_SUBSTEPS
                {
                    step_simulation_3d(&mut self.env, &mut self.input, crate::eval::PHYSICS_DT as f32);
                    self.sim_accumulator -= crate::eval::PHYSICS_DT;
                    substeps += 1;
                }
                if substeps >= crate::eval::MAX_SUBSTEPS {
                    self.sim_accumulator = 0.0;
                }
                let rendered = crate::host3d::render_frame(
                    &mut state.renderer,
                    &mut self.env,
                    &mut self.assets,
                );
                if !self.env.out.is_empty() {
                    print!("{}", self.env.out);
                    self.env.out.clear();
                }
                self.audio.play_queued();
                if let Err(e) = rendered {
                    eprintln!("render error: {e}");
                }
                // web3d-M0: a script's `quit()` ends the game after
                // this frame, same as the 2D loops.
                if crate::stdlib::take_quit_request() {
                    event_loop.exit();
                    return;
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }

    /// Phase 17 session 3: raw mouse delta from the OS, independent
    /// of cursor position / wraparound. Required for FPS-style
    /// camera control.  `WindowEvent::CursorMoved` only gives
    /// absolute window coords, which jumps when the cursor is locked
    /// or wraps; `DeviceEvent::MouseMotion` is the raw integrated
    /// pointer velocity.
    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            self.input.mouse_motion(dx, dy);
        }
    }
}

/// Phase 17 session 3: apply a pending cursor-mode request from
/// the script. Locked grab is the FPS-style "infinite cursor"
/// mode; if the platform doesn't support Locked, we fall back to
/// Confined (which keeps the cursor inside the window). Visibility
/// is also toggled — locked games hide the cursor by convention.
fn apply_cursor_mode(window: &Window, locked: bool) {
    use winit::window::CursorGrabMode;
    if locked {
        // Try Locked first (raw input, no cursor movement). Some
        // platforms (older X11) only support Confined; fall back
        // gracefully rather than failing the call.
        if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
            let _ = window.set_cursor_grab(CursorGrabMode::Confined);
        }
        window.set_cursor_visible(false);
    } else {
        let _ = window.set_cursor_grab(CursorGrabMode::None);
        window.set_cursor_visible(true);
    }
}

// ---------- wgpu setup ----------

/// Decode a `.glb` (or `.gltf`) at `path`. Returns interleaved
/// `Vertex` array + u32 index list + optionally the base color
/// texture from the first primitive's material + optionally the
/// skin/animation data when the document carries a skinned mesh.
/// Errors are stringified at the boundary because the upstream
/// `gltf::Error` carries lifetimes we don't want to leak.
pub(crate) fn load_glb(path: &str) -> Result<LoadedGlb, String> {
    // Phase 12 session 3: bundle-first lookup, filesystem fallback.
    let bytes = crate::bundle::read_asset_bytes(path).map_err(|e| e.to_string())?;
    parse_glb_bytes(&bytes)
}

/// Phase 29 session 1: one fixed-timestep simulation slice. Steps
/// rapier3d at the same rate as the script's `on update(dt)` body so
/// physics state and script state advance in lockstep. Called zero
/// or more times per render frame from the App event loop.
fn step_simulation_3d(env: &mut Env, input: &mut crate::host3d::InputState, dt: f32) {
    // Phase 18: step the rapier3d world before the Twe `on update`
    // body runs, so script logic reads authoritative positions.
    // Scripts own intent (velocity / impulse), the integrator owns
    // truth. No-op if the script never created any bodies — the
    // world's empty body set means step() returns near-instantly.
    crate::physics3d::step(dt);

    // Tick the per-frame logic — top-level `on update(dt):`
    // plus any active scene / entity tick. Without this the script's
    // `on update(dt):` never fires, so anything that reads
    // `key.*` to drive state stays frozen.
    if let Err(e) = crate::host3d::sim_tick(env, input, dt as f64) {
        eprintln!(
            "render error in `on update(dt)`: {}:{}: {}",
            e.line, e.col, e.message
        );
    }
    if !env.out.is_empty() {
        print!("{}", env.out);
        env.out.clear();
    }
}

// ---------- Hand-rolled column-major matrix math ----------

/// web3d-M2: the native host's window + kernel renderer.
struct ShellState {
    window: Arc<Window>,
    renderer: Renderer,
}

/// web3d-M2: native asset loading for the kernel. `.glb` files are read
/// and parsed on worker threads (Phase 28 session 5 — a big mesh no
/// longer stalls a frame); textures are read synchronously and decoded
/// by the kernel. Both go through the bundle-aware loader, so shipped
/// builds read from their bundle.
#[derive(Default)]
pub struct NativeAssets {
    mesh_jobs: HashMap<u32, std::thread::JoinHandle<Result<LoadedGlb, String>>>,
    ready: Vec<AssetReady>,
}

impl NativeAssets {
    /// Loads requested but not yet handed to the renderer.
    pub fn pending(&self) -> usize {
        self.mesh_jobs.len() + self.ready.len()
    }
}

impl AssetSource for NativeAssets {
    fn request(&mut self, kind: AssetKind, id: u32, path: &str) {
        match kind {
            AssetKind::Mesh => {
                let path = path.to_string();
                let spawned = std::thread::Builder::new()
                    .name(format!("twec-glb-load-{id}"))
                    .spawn(move || load_glb(&path));
                match spawned {
                    Ok(handle) => {
                        self.mesh_jobs.insert(id, handle);
                    }
                    Err(e) => self
                        .ready
                        .push(AssetReady::Mesh(id, Err(format!("spawn loader: {e}")))),
                }
            }
            AssetKind::Texture | AssetKind::Environment | AssetKind::Lut => {
                let bytes =
                    crate::bundle::read_asset_bytes(path).map_err(|e| format!("`{path}`: {e}"));
                self.ready.push(match kind {
                    AssetKind::Texture => AssetReady::Texture(id, bytes),
                    AssetKind::Environment => AssetReady::Environment(id, bytes),
                    _ => AssetReady::Lut(id, bytes),
                });
            }
        }
    }

    fn poll(&mut self) -> Vec<AssetReady> {
        let done: Vec<u32> = self
            .mesh_jobs
            .iter()
            .filter(|(_, h)| h.is_finished())
            .map(|(id, _)| *id)
            .collect();
        for id in done {
            let handle = self.mesh_jobs.remove(&id).expect("just listed");
            let result = handle
                .join()
                .unwrap_or_else(|_| Err("mesh-load worker thread panicked".to_string()));
            self.ready.push(AssetReady::Mesh(id, result));
        }
        std::mem::take(&mut self.ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// web3d-M4: native and web report the same keys under the same
    /// names; the native table must match `host3d::KEY_CODES`.
    #[test]
    fn native_keys_match_the_shared_table() {
        let native: Vec<(&str, String)> =
            KEYS.iter().map(|(n, k)| (*n, format!("{k:?}"))).collect();
        let shared: Vec<(&str, String)> = crate::host3d::KEY_CODES
            .iter()
            .map(|(n, c)| (*n, c.to_string()))
            .collect();
        assert_eq!(native, shared);
    }

    #[test]
    fn the_3d_shell_never_calls_into_macroquad() {
        // play3d runs on winit + wgpu; macroquad is never initialised
        // there, and any macroquad call asserts and aborts the process
        // (`time::get_time()` crashed every play3d run for months).
        let needle = ["macro", "quad::"].concat();
        assert!(
            !include_str!("play3d.rs").contains(&needle),
            "src/play3d.rs references macroquad"
        );
    }

    #[test]
    fn load_glb_missing_file_errors() {
        // Path that should never exist on a sane test machine.
        let result = load_glb(".twec_no_such_glb_at_test_time.glb");
        assert!(result.is_err());
    }
}
