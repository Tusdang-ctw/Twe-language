//! web3d-M6: one frame of a 2D game, without a renderer.
//!
//! The 2D play loops (macroquad's today, the kernel shell next) and the
//! headless 2D renderer all advance a frame through these two calls, so
//! they can't drift apart:
//!
//! 1. [`tick`] runs the fixed-step simulation for the frame's wall-clock
//!    time (pause, persistent states and `fx.hit_stop` included);
//! 2. [`render`] advances the visual clocks (camera shake, `camera2d`
//!    animations, `fx`, `light2d`), runs the render code into
//!    `Env::draw2d`, adds the `light2d` and `fx` overlays, and returns
//!    the camera the list is drawn under.
//!
//! Audio and input stay with the caller: they belong to the shell.

use std::path::Path;

use crate::host3d::InputState;
use crate::kernel::canvas::Camera2d;
use crate::replay::InputCommand;
use crate::value::{Env, RuntimeError};

/// Load a 2D game: read `path`, run its top level (through the module
/// loader when it imports), and refuse `look:` blocks, which only the
/// 3D runtime draws. The error is the message to print.
pub fn load(path: &str) -> Result<Env, String> {
    refuse_looks(load_any(path)?, path)
}

/// [`load`] without refusing `look:` blocks — for the launcher that
/// dispatches a game to the 2D or 3D shell by [`detect_dimension`].
pub fn load_any(path: &str) -> Result<Env, String> {
    let src = std::fs::read_to_string(Path::new(path))
        .map_err(|e| format!("error: could not read '{path}': {e}"))?;
    // web3d-M1: a script that imports modules goes through the module
    // loader (it used to leave every import unbound in `twec play`).
    let imports = crate::lexer::lex(&src)
        .ok()
        .and_then(|t| crate::parser::parse(&t).ok())
        .is_some_and(|p| crate::module::has_imports(&p));
    if imports {
        return crate::module::prepare_entry(Path::new(path), &src);
    }
    load_source_any(&src, path)
}

/// [`load`] for source in hand (an embedded bundle). `label` names it
/// in errors.
pub fn load_source(src: &str, label: &str) -> Result<Env, String> {
    refuse_looks(load_source_any(src, label)?, label)
}

/// [`load_source`] without refusing `look:` blocks (for dispatch).
pub fn load_source_any(src: &str, label: &str) -> Result<Env, String> {
    let tokens = crate::lexer::lex(src).map_err(|e| format!("{label}:{e}"))?;
    let program = crate::parser::parse(&tokens).map_err(|e| format!("{label}:{e}"))?;
    // Lanternfall E5: a shipped game's modules travel in its bundle.
    if crate::module::has_imports(&program) && crate::bundle::has_active_bundle() {
        return crate::module::prepare_entry_bundled(src, &|_| {});
    }
    let mut env = Env::new();
    crate::stdlib::install(&mut env);
    crate::eval::run_top_level(&mut env, &program)
        .map_err(|e| format!("{label}: runtime error: {e}"))?;
    Ok(env)
}

/// web3d-M3: the 2D player can't draw `look:`; say so up front instead
/// of silently drawing nothing (see `eval::look_needs_3d`).
fn refuse_looks(env: Env, label: &str) -> Result<Env, String> {
    match crate::eval::first_look_class(&env) {
        Some(class) => {
            let e = crate::eval::look_needs_3d(&class);
            let mut msg = format!("{label}: {}", e.message);
            if let Some(help) = e.help {
                msg.push_str(&format!("\n  help: {help}"));
            }
            Err(msg)
        }
        None => Ok(env),
    }
}

/// Whether a game draws in 2D or 3D, decided once after its top level
/// has run. 3D when any entity class has a `look:` block, or the first
/// rendered frame enqueues a 3D draw (`cube`, `mesh`, `sphere`, …).
///
/// The check runs the top-level `on render():` once through the 3D path;
/// render code is pure drawing, so running it again on the first real
/// frame is harmless. It leaves `env` as it found it (both render queues
/// cleared, `in_render` restored). Shells that have separate 2D and 3D
/// entry points (native `twec play` / `play3d`) don't need it; the one
/// browser shell does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dimension {
    Two,
    Three,
}

pub fn detect_dimension(env: &mut Env) -> Dimension {
    if crate::eval::first_look_class(env).is_some() {
        return Dimension::Three;
    }
    let was = env.in_render;
    let three = crate::eval::render_frame3d(env).is_ok() && !env.render_queue3d.is_empty();
    env.render_queue3d.clear();
    env.draw2d.clear();
    env.in_render = was;
    if three {
        Dimension::Three
    } else {
        Dimension::Two
    }
}

/// Run as many fixed `PHYSICS_DT` ticks as `frame_dt` of wall-clock
/// time allows (at most `MAX_SUBSTEPS`). `accumulator` carries the
/// remainder between frames.
///
/// With `input` (a shell's [`InputState`]), each tick takes one input
/// command (recorded or replayed by `replay::step`), and afterwards the
/// input ambients hold the whole frame's input for the render code:
/// the latest held keys and cursor, every press of every tick, the
/// summed motion and wheel. A frame that runs no tick (paused, or
/// between ticks at a high refresh rate) takes a fresh command for the
/// render code alone, so a pause menu's buttons still see clicks.
/// Without `input` the shell has written the ambients itself.
pub fn tick(
    env: &mut Env,
    mut input: Option<&mut InputState>,
    accumulator: &mut f64,
    frame_dt: f64,
) -> Result<(), RuntimeError> {
    use crate::eval::{MAX_SUBSTEPS, PHYSICS_DT};
    let mut frame: Option<InputCommand> = None;
    // v1.0.1 session 6: persistent states keep ticking through pause;
    // `eval::tick_frame` skips the rest.
    let running = !crate::stdlib::is_paused() || crate::stdlib::has_persistent_states();
    let mut result = Ok(());
    if running {
        *accumulator += frame_dt;
        let mut substeps: u32 = 0;
        while *accumulator >= PHYSICS_DT && substeps < MAX_SUBSTEPS {
            // v1.0.1 session 1: `fx.hit_stop` spends wall-clock time
            // without advancing the game.
            if !crate::fx::consume_hit_stop_tick() {
                if let Some(input) = input.as_deref_mut() {
                    let cmd = crate::replay::step(input.take_command());
                    crate::host3d::apply_command(env, &cmd);
                    frame = Some(match frame.take() {
                        Some(earlier) => merge(earlier, cmd),
                        None => cmd,
                    });
                }
                result = crate::eval::tick_frame(env, PHYSICS_DT * crate::stdlib::time_scale());
                if result.is_err() {
                    break;
                }
            }
            *accumulator -= PHYSICS_DT;
            substeps += 1;
        }
        if substeps >= MAX_SUBSTEPS {
            *accumulator = 0.0;
        }
    }
    if let Some(input) = input {
        let frame = frame.unwrap_or_else(|| {
            let cmd = input.take_command();
            // A paused frame's input is recorded and replayed too, one
            // line per frame, or a pause menu's "Resume" click would be
            // missing from a replay. (Running frames that fall between
            // ticks aren't: how many there are depends on the display.)
            if running {
                cmd
            } else {
                crate::replay::step(cmd)
            }
        });
        crate::host3d::apply_command(env, &frame);
    }
    result
}

/// Two ticks' commands as one frame's: the later state, every press.
fn merge(a: InputCommand, b: InputCommand) -> InputCommand {
    let union = |mut x: Vec<String>, y: Vec<String>| {
        for n in y {
            if !x.contains(&n) {
                x.push(n);
            }
        }
        x
    };
    InputCommand {
        keys_pressed: union(a.keys_pressed, b.keys_pressed),
        mb_press: union(a.mb_press, b.mb_press),
        pad_pressed: union(a.pad_pressed, b.pad_pressed),
        mouse_dx: a.mouse_dx + b.mouse_dx,
        mouse_dy: a.mouse_dy + b.mouse_dy,
        wheel: a.wheel + b.wheel,
        ..b
    }
}

/// Render the frame into `env.draw2d`, for a view `view_w × view_h`
/// pixels. Returns the camera to draw the list under (`None`: canvas
/// coordinates as they are) and the render code's result; the list
/// holds whatever was drawn before an error.
pub fn render(
    env: &mut Env,
    frame_dt: f64,
    view_w: f32,
    view_h: f32,
) -> (Option<Camera2d>, Result<(), RuntimeError>) {
    env.in_render = true;
    // Visual clocks run on wall-clock time, once per displayed frame.
    crate::stdlib::camera_tick(frame_dt);
    crate::stdlib::camera2d_tick(env, frame_dt);
    crate::fx::fx_tick(frame_dt);
    crate::light2d::tick(frame_dt);
    // Phase 9 session 2: `camera.pos` is the world point at the view
    // centre. Left at the origin, with zoom 1 and no shake, the script
    // draws in canvas pixels (origin top-left, +y down).
    let ((px, py), zoom) = crate::stdlib::camera_view(env);
    let (sx, sy) = crate::stdlib::camera_shake_offset();
    let active = px != 0.0 || py != 0.0 || zoom != 1.0 || sx != 0.0 || sy != 0.0;
    let camera = active.then_some(Camera2d {
        center: [(px + sx) as f32, (py + sy) as f32],
        zoom: zoom as f32,
    });
    env.draw2d.clear();
    let result = crate::eval::render_frame(env);
    // v1.0.1 session 3: the light overlay sits over the world and under
    // the fx layer. Its ambient darkness covers the view, generously
    // (4× in each direction) under a camera so zooming out stays dark.
    match camera {
        Some(cam) => {
            let z = cam.zoom.max(0.0001);
            let (half_w, half_h) = (view_w / 2.0 / z * 4.0, view_h / 2.0 / z * 4.0);
            crate::light2d::draw_overlay(
                &mut env.draw2d,
                cam.center[0] - half_w,
                cam.center[1] - half_h,
                half_w * 2.0,
                half_h * 2.0,
            );
        }
        None => crate::light2d::draw_overlay(&mut env.draw2d, 0.0, 0.0, view_w, view_h),
    }
    // v1.0.1 session 1: bursts and damage numbers, under the camera too.
    crate::fx::fx_draw_overlay(&mut env.draw2d);
    crate::light2d::clear_frame_lights();
    env.in_render = false;
    (camera, result)
}
