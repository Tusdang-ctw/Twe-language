use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{
    AssignOp, AssignTarget, BinOp, DeclKind, DeclMember, Expr, Program, StateMember, Stmt, UnOp,
};
use crate::stdlib;
use crate::tagged_value::TaggedValue;
use crate::value::{Branch, ClassDef, Env, EveryClockDef, Frame, FrameKind, FunctionDef, Instance, MethodDef,
    Object, OnUpdateHandler, PathEntry, RuntimeError, StateDef, Value, LegacyValue, ToLegacyShim};

pub fn run(program: &Program) -> Result<String, RuntimeError> {
    run_with_frames(program, 0, 1.0 / 60.0)
}

pub fn run_with_frames(
    program: &Program,
    frames: u32,
    dt: f64,
) -> Result<String, RuntimeError> {
    let mut env = Env::new();
    stdlib::install(&mut env);
    run_top_level(&mut env, program)?;
    for _ in 0..frames {
        tick_frame(&mut env, dt)?;
        if env.returning.take().is_some() {
            break;
        }
    }
    Ok(env.out)
}

/// Run the program's top-level statements. After this returns, `env`
/// has any declared scenes / functions / globals bound. Callers use
/// `tick_frame` / `render_frame` to drive the interactive loop.
pub fn run_top_level(env: &mut Env, program: &Program) -> Result<(), RuntimeError> {
    run_block(env, &program.stmts)?;
    if env.returning.take().is_some() {
        // Top-level `return` is silently dropped.
    }
    Ok(())
}

/// Advance the active scene and any global on-update handler by `dt`
/// seconds. Side-effects (prints, field mutations, transitions) are
/// applied to `env`.
pub fn tick_frame(env: &mut Env, dt: f64) -> Result<(), RuntimeError> {
    update_time_ambient(env, dt);
    if let Some(handler) = env.on_update.clone() {
        env.set(handler.param.clone(), Value::from_float(dt));
        run_block(env, &handler.body)?;
        if env.returning.take().is_some() {
            return Ok(());
        }
    }
    if let Some(scene) = env.active_scene.clone() {
        dispatch_key_press(env, &scene)?;
        tick_scene(env, &scene, dt)?;
    }
    tick_entities(env, dt)?;
    prune_despawned(env);
    Ok(())
}

fn tick_entities(env: &mut Env, dt: f64) -> Result<(), RuntimeError> {
    let entities = env.active_entities.clone();
    for entity in entities {
        if entity.borrow().despawned {
            continue;
        }
        let class = entity.borrow().class.clone();
        if class.kind == "particles" {
            tick_particle_emitter(env, &entity, &class, dt)?;
            continue;
        }
        let method = match find_method(&class, "update") {
            Some(m) => m,
            None => continue,
        };
        call_method(
            env,
            Value::from_instance(entity),
            &method,
            &[Value::from_float(dt)],
            &[],
            0,
            0,
        )?;
    }
    Ok(())
}

fn prune_despawned(env: &mut Env) {
    env.active_entities.retain(|e| !e.borrow().despawned);
}

/// On `spawn EmitterClass at pos`, create the particle list as a hidden
/// `__particles` field on the emitter Instance, run `on_spawn(p)` for
/// each particle if defined. The emitter itself is then pushed to
/// `active_entities` by the caller.
fn seed_particle_emitter(
    env: &mut Env,
    emitter: &Rc<RefCell<Instance>>,
    at: Option<&Value>,
    line: u32,
    col: u32,
) -> Result<(), RuntimeError> {
    let (count, lifetime, class) = {
        let inst = emitter.borrow();
        let count = match inst.get_field("count").to_legacy() {
            Some(LegacyValue::Int(n)) if n >= 0 => n as usize,
            Some(other) => {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!(
                        "particles `count` must be a non-negative int, got {}",
                        other.type_name()
                    ),
                    help: None,
                });
            }
            None => 16,
        };
        let lifetime = match inst.get_field("lifetime").to_legacy() {
            Some(LegacyValue::Float(f)) => f,
            Some(LegacyValue::Int(n)) => n as f64,
            Some(LegacyValue::Quantity { value, .. }) => value,
            None => 1.0,
            Some(other) => {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!(
                        "particles `lifetime` must be a number or duration, got {}",
                        other.type_name()
                    ),
                    help: Some("e.g. `lifetime = 0.6` (seconds)".to_string()),
                });
            }
        };
        (count, lifetime, inst.class.clone())
    };
    let on_spawn = find_method(&class, "on_spawn");
    let mut particles: Vec<Value> = Vec::with_capacity(count);
    let initial_pos = at
        .cloned()
        .unwrap_or_else(|| Value::from_tuple(Rc::new(vec![Value::from_float(0.0), Value::from_float(0.0)])));
    for _ in 0..count {
        let p = make_particle(&initial_pos, lifetime);
        if let Some(method) = on_spawn.clone() {
            call_method(
                env,
                Value::from_instance(emitter.clone()),
                &method,
                std::slice::from_ref(&p),
                &[],
                line,
                col,
            )?;
        }
        particles.push(p);
    }
    emitter.borrow_mut().insert_field(
        "__particles",
        Value::from_list(Rc::new(RefCell::new(particles))),
    );
    Ok(())
}

fn make_particle(initial_pos: &Value, lifetime: f64) -> Value {
    let mut o = Object {
        fields: HashMap::new(),
        kind: "particle",
    };
    o.insert_field("pos", initial_pos.clone());
    o.insert_field(
        "velocity",
        Value::from_tuple(Rc::new(vec![Value::from_float(0.0), Value::from_float(0.0)])),
    );
    o.insert_field(
        "color",
        Value::from_tuple(Rc::new(vec![
            Value::from_float(1.0),
            Value::from_float(1.0),
            Value::from_float(1.0),
            Value::from_float(1.0),
        ])),
    );
    o.insert_field("size", Value::from_float(4.0));
    o.insert_field("age", Value::from_float(0.0));
    o.insert_field("age_ratio", Value::from_float(0.0));
    o.insert_field("lifetime", Value::from_float(lifetime));
    Value::from_object(Rc::new(RefCell::new(o)))
}

fn tick_particle_emitter(
    env: &mut Env,
    emitter: &Rc<RefCell<Instance>>,
    class: &Rc<ClassDef>,
    dt: f64,
) -> Result<(), RuntimeError> {
    let on_update = find_method(class, "on_update");
    let particles = match emitter
        .borrow()
        .get_field("__particles").to_legacy() {
        Some(LegacyValue::List(rc)) => rc,
        _ => return Ok(()),
    };
    let snapshot: Vec<Value> = particles.borrow().clone();
    for p in &snapshot {
        if let Some(method) = on_update.clone() {
            call_method(
                env,
                Value::from_instance(emitter.clone()),
                &method,
                &[p.clone(), Value::from_float(dt)],
                &[],
                0,
                0,
            )?;
        }
        if let LegacyValue::Object(rc) = p.to_legacy() {
            let mut o = rc.borrow_mut();
            let age = match o.get_field("age").to_legacy() {
                Some(LegacyValue::Float(a)) => a + dt,
                Some(LegacyValue::Int(a)) => a as f64 + dt,
                _ => dt,
            };
            let lifetime = match o.get_field("lifetime").to_legacy() {
                Some(LegacyValue::Float(l)) => l,
                _ => 1.0,
            };
            o.insert_field("age".to_string(), Value::from_float(age));
            let ratio = if lifetime > 0.0 {
                (age / lifetime).clamp(0.0, 1.0)
            } else {
                1.0
            };
            o.insert_field("age_ratio", Value::from_float(ratio));
        }
    }
    // Drop dead particles.
    particles.borrow_mut().retain(|p| match p.to_legacy() {
        LegacyValue::Object(rc) => match rc.borrow().get_field("age").to_legacy() {
            Some(LegacyValue::Float(age)) => match rc.borrow().get_field("lifetime").to_legacy() {
                Some(LegacyValue::Float(lt)) => age < lt,
                _ => true,
            },
            _ => true,
        },
        _ => true,
    });
    if particles.borrow().is_empty() {
        emitter.borrow_mut().despawned = true;
    }
    Ok(())
}

fn render_particle_emitter(
    env: &mut Env,
    emitter: &Rc<RefCell<Instance>>,
    class: &Rc<ClassDef>,
) -> Result<(), RuntimeError> {
    // If the user defined a custom `render()`, defer to it and skip the
    // built-in circle-per-particle path.
    if let Some(method) = find_method(class, "render") {
        return call_method(env, Value::from_instance(emitter.clone()), &method, &[], &[], 0, 0)
            .map(|_| ());
    }
    let particles = match emitter
        .borrow()
        .get_field("__particles").to_legacy() {
        Some(LegacyValue::List(rc)) => rc,
        _ => return Ok(()),
    };
    if !env.in_render {
        return Ok(());
    }
    for p in particles.borrow().iter() {
        if let LegacyValue::Object(rc) = p.to_legacy() {
            let o = rc.borrow();
            let (px, py) = match o.get_field("pos").to_legacy() {
                Some(LegacyValue::Tuple(elems)) if elems.len() >= 2 => (
                    number_or_zero(&elems[0]),
                    number_or_zero(&elems[1]),
                ),
                _ => (0.0, 0.0),
            };
            let radius = match o.get_field("size").to_legacy() {
                Some(LegacyValue::Float(f)) => f as f32,
                Some(LegacyValue::Int(n)) => n as f32,
                _ => 4.0,
            };
            let color = match o.get_field("color").to_legacy() {
                Some(LegacyValue::Tuple(elems)) if elems.len() >= 3 => {
                    let r = number_or_zero(&elems[0]) as f32;
                    let g = number_or_zero(&elems[1]) as f32;
                    let b = number_or_zero(&elems[2]) as f32;
                    let a = if elems.len() >= 4 {
                        number_or_zero(&elems[3]) as f32
                    } else {
                        1.0
                    };
                    macroquad::color::Color::new(r, g, b, a)
                }
                _ => macroquad::color::WHITE,
            };
            macroquad::shapes::draw_circle(px as f32, py as f32, radius, color);
        }
    }
    Ok(())
}

fn number_or_zero(v: &Value) -> f64 {
    match v.to_legacy() {
        LegacyValue::Int(n) => n as f64,
        LegacyValue::Float(f) => f,
        LegacyValue::Quantity { value, .. } => value,
        _ => 0.0,
    }
}

fn update_time_ambient(env: &mut Env, dt: f64) {
    if let Some(LegacyValue::Object(rc)) = env.get("time").to_legacy() {
        rc.borrow_mut().insert_field("dt", Value::from_float(dt));
    }
}

/// Look at `env.key_press` (an Object whose fields are bool flags set
/// each frame by the host) and fire the active scene's matching
/// on_key_press handlers.
fn dispatch_key_press(
    env: &mut Env,
    scene: &Rc<RefCell<Instance>>,
) -> Result<(), RuntimeError> {
    let pressed = match env.get("key_press").to_legacy() {
        Some(LegacyValue::Object(rc)) => {
            let o = rc.borrow();
            o.fields
                .iter()
                .filter_map(|(k, v)| {
                    if matches!(v.clone().to_legacy(), LegacyValue::Bool(true)) {
                        Some(k.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        }
        _ => return Ok(()),
    };
    if pressed.is_empty() {
        return Ok(());
    }
    let bodies: Vec<Vec<Stmt>> = {
        let inst = scene.borrow();
        let state_name = match &inst.current_state {
            Some(n) => n.clone(),
            None => return Ok(()),
        };
        match inst.class.states.get(&state_name) {
            Some(state) => pressed
                .iter()
                .filter_map(|key| state.on_key_press.get(key).cloned())
                .collect(),
            None => return Ok(()),
        }
    };
    let prev_self = env.self_value.replace(Value::from_instance(scene.clone()));
    for body in bodies {
        run_block(env, &body)?;
        if env.returning.is_some() {
            break;
        }
        if let Some(target) = env.transitioning.take() {
            enter_state(env, scene, &target)?;
            break;
        }
    }
    env.self_value = prev_self;
    Ok(())
}

/// Run the top-level `on render():` body for the wgpu/3D path.
/// Clears the per-frame render queue first so `cube()` calls don't
/// pile up across frames; the caller drains `env.render_queue3d`
/// after this returns. `in_render` gates the drawing builtins so
/// they can't be called outside a render frame. Phase 5 task 5
/// session (d).
pub fn render_frame3d(env: &mut Env) -> Result<(), RuntimeError> {
    env.render_queue3d.clear();
    let body = match env.top_on_render.clone() {
        Some(b) => b,
        None => return Ok(()),
    };
    let prev_render = env.in_render;
    env.in_render = true;
    let result = run_block(env, &body);
    env.in_render = prev_render;
    // A `return` in the top-level on_render body just stops the
    // current frame's draw composition; clear the flag so
    // subsequent frames aren't affected.
    env.returning.take();
    result
}

/// Run the active scene's current state's on-render handler, plus
/// each active entity's `render()` method. Drawing primitives in
/// stdlib check `env.in_render`; the caller is responsible for
/// setting that flag (the macroquad `play` loop does it around this
/// call).
pub fn render_frame(env: &mut Env) -> Result<(), RuntimeError> {
    if let Some(scene) = env.active_scene.clone() {
        let body: Option<Vec<Stmt>> = {
            let inst = scene.borrow();
            inst.current_state
                .as_ref()
                .and_then(|n| inst.class.states.get(n))
                .and_then(|state| state.on_render.clone())
        };
        if let Some(body) = body {
            let prev_self = env.self_value.replace(Value::from_instance(scene));
            run_block(env, &body)?;
            env.self_value = prev_self;
        }
        env.transitioning.take();
    }
    let entities = env.active_entities.clone();
    for entity in entities {
        if entity.borrow().despawned {
            continue;
        }
        let class = entity.borrow().class.clone();
        if class.kind == "particles" {
            render_particle_emitter(env, &entity, &class)?;
            continue;
        }
        let method = match find_method(&class, "render") {
            Some(m) => m,
            None => continue,
        };
        call_method(env, Value::from_instance(entity), &method, &[], &[], 0, 0)?;
    }
    Ok(())
}

fn tick_scene(
    env: &mut Env,
    scene: &Rc<RefCell<Instance>>,
    dt: f64,
) -> Result<(), RuntimeError> {
    // Snapshot the state name + clock bodies before running, so a
    // transition during a clock body doesn't fire the wrong clock list.
    let (state_name, clocks): (Option<String>, Vec<(f64, Vec<Stmt>)>) = {
        let inst = scene.borrow();
        let name = inst.current_state.clone();
        let bodies: Vec<Vec<Stmt>> = if let Some(state) = name
            .as_ref()
            .and_then(|n| inst.class.states.get(n))
        {
            state.every_clocks.iter().map(|c| c.body.clone()).collect()
        } else {
            Vec::new()
        };
        let clocks: Vec<(f64, Vec<Stmt>)> = inst
            .every_intervals_secs
            .iter()
            .zip(bodies)
            .map(|(i, body)| (*i, body))
            .collect();
        (name, clocks)
    };
    if state_name.is_none() {
        return Ok(());
    }
    let prev_self = env.self_value.replace(Value::from_instance(scene.clone()));
    // Phase 5 fibers / v0.2 sessions 2a + 2b: if the state's
    // fiber is suspended on a `wait`, count down by `dt` and
    // either keep waiting (skip the rest of this state's
    // tick — the state is "asleep") or resume the topmost frame.
    let suspended = !scene.borrow().fiber_frames.is_empty();
    if suspended {
        let remaining = scene.borrow().entry_wait_remaining;
        let new_remaining = remaining - dt;
        if new_remaining > 0.0 {
            scene.borrow_mut().entry_wait_remaining = new_remaining;
            env.self_value = prev_self;
            return Ok(());
        }
        // Wait elapsed — resume the fiber. `resume_fiber` walks
        // frames from innermost (top of stack) back down to the
        // state-entry, completing or re-suspending as it goes.
        resume_fiber(env, scene)?;
        if let Some(target) = env.transitioning.take() {
            enter_state(env, scene, &target)?;
            env.self_value = prev_self;
            return Ok(());
        }
        // Resuming may have hit another `wait` — if so, the
        // instance still has frames on the fiber stack. Bail out
        // of the rest of this tick (clocks + on_update stay
        // paused while the entry is suspended).
        if !scene.borrow().fiber_frames.is_empty() {
            env.self_value = prev_self;
            return Ok(());
        }
        if env.returning.is_some() {
            env.self_value = prev_self;
            return Ok(());
        }
    }
    // State-scoped `on update(dt):` fires once per frame BEFORE the
    // every-clocks for this state. The top-level on_update has
    // already run (in tick_frame). A transition or return inside
    // the body skips the rest of this state's clocks.
    let state_on_update: Option<OnUpdateHandler> = {
        let inst = scene.borrow();
        inst.current_state
            .as_ref()
            .and_then(|n| inst.class.states.get(n))
            .and_then(|state| state.on_update.clone())
    };
    if let Some(handler) = state_on_update {
        env.set(handler.param.clone(), Value::from_float(dt));
        run_block(env, &handler.body)?;
        if env.returning.is_some() {
            env.self_value = prev_self;
            return Ok(());
        }
        if let Some(target) = env.transitioning.take() {
            enter_state(env, scene, &target)?;
            env.self_value = prev_self;
            return Ok(());
        }
    }
    // Phase 5 task 4: evaluate predicate hooks (`on hp < 20%:`,
    // `on player.within(8m):`, …). Each predicate's current
    // truthiness is compared against the last-seen value on the
    // instance; on a false → true transition we run the body.
    // Edge-triggered, so a predicate that stays true doesn't
    // re-fire. A transition inside a body cascades into the new
    // state immediately and skips the rest of this state's
    // predicates + clocks for this frame.
    let predicates: Vec<(crate::ast::Expr, Vec<Stmt>)> = {
        let inst = scene.borrow();
        inst.current_state
            .as_ref()
            .and_then(|n| inst.class.states.get(n))
            .map(|s| {
                s.on_predicates
                    .iter()
                    .map(|p| (p.predicate.clone(), p.body.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    for (idx, (pred, body)) in predicates.iter().enumerate() {
        let value = eval_expr(env, pred)?;
        let now_true = is_truthy(&value);
        let prev = scene
            .borrow()
            .predicate_last_values
            .get(idx)
            .copied()
            .unwrap_or(false);
        if idx < scene.borrow().predicate_last_values.len() {
            scene.borrow_mut().predicate_last_values[idx] = now_true;
        }
        if now_true && !prev {
            run_block(env, body)?;
            if env.returning.is_some() {
                env.self_value = prev_self;
                return Ok(());
            }
            if let Some(target) = env.transitioning.take() {
                enter_state(env, scene, &target)?;
                env.self_value = prev_self;
                return Ok(());
            }
        }
    }
    // Tick each clock with bounded catch-up: a clock whose accumulated
    // time covers N intervals fires up to MAX_CATCHUP_FIRES_PER_FRAME
    // times, then drops the residual. The cap prevents a long pause
    // (debugger, alt-tab, slow first frame) from causing a runaway
    // catch-up loop that stalls the next frame too. Closes Phase 2
    // frustration F4.
    'clocks: for (clock_idx, (interval, body)) in clocks.into_iter().enumerate() {
        {
            let mut inst = scene.borrow_mut();
            if clock_idx >= inst.every_timers.len() {
                continue;
            }
            inst.every_timers[clock_idx] += dt;
        }
        let mut fires: u32 = 0;
        while fires < MAX_CATCHUP_FIRES_PER_FRAME {
            let should_fire = {
                let inst = scene.borrow();
                inst.every_timers
                    .get(clock_idx)
                    .copied()
                    .unwrap_or(0.0)
                    >= interval
            };
            if !should_fire {
                break;
            }
            scene.borrow_mut().every_timers[clock_idx] -= interval;
            fires += 1;
            run_block(env, &body)?;
            if env.returning.is_some() {
                break 'clocks;
            }
            if let Some(target) = env.transitioning.take() {
                enter_state(env, scene, &target)?;
                break 'clocks;
            }
        }
        if fires >= MAX_CATCHUP_FIRES_PER_FRAME {
            // Drop residual so next frame starts fresh and doesn't
            // compound the backlog.
            scene.borrow_mut().every_timers[clock_idx] = 0.0;
        }
    }
    env.self_value = prev_self;
    Ok(())
}

/// Cap on how many times a single `every <duration>:` clock can fire in
/// one frame. Eight 16ms ticks ≈ 128ms of catch-up — comfortably enough
/// to absorb a slow first frame or a brief stall, while still bounded
/// so a long pause can't lock the runtime in catch-up forever.
const MAX_CATCHUP_FIRES_PER_FRAME: u32 = 8;

fn enter_state(
    env: &mut Env,
    scene: &Rc<RefCell<Instance>>,
    state_name: &str,
) -> Result<(), RuntimeError> {
    // Resolve the target state on the class.
    let state = {
        let inst = scene.borrow();
        match inst.class.states.get(state_name).cloned() {
            Some(s) => s,
            None => {
                let names: Vec<&String> = inst.class.states.keys().collect();
                let suggestion = crate::value::did_you_mean(state_name, &names).map(str::to_string);
                return Err(RuntimeError {
                    line: 0,
                    col: 0,
                    message: format!("no state named '{state_name}'"),
                    help: Some(match suggestion {
                        Some(s) => format!("did you mean `-> {s}`?"),
                        None => "transitions must target a `state <name>:` declared in the same scene"
                            .to_string(),
                    }),
                });
            }
        }
    };
    // Replace current_state, reset timers / intervals.
    {
        let mut inst = scene.borrow_mut();
        inst.current_state = Some(state.name.clone());
        inst.every_timers = vec![0.0; state.every_clocks.len()];
        inst.every_intervals_secs.clear();
        // Phase 5 task 4: reset predicate edge-detection state.
        // Initial value is `false` so a predicate that's already
        // true on the first tick after entry fires immediately —
        // matches game-state-machine intuition while keeping the
        // edge-triggered contract.
        inst.predicate_last_values = vec![false; state.on_predicates.len()];
    }
    // Resolve each every-clock interval (in seconds) by evaluating the
    // interval expression with self bound to the scene instance.
    let prev_self = env.self_value.replace(Value::from_instance(scene.clone()));
    let mut intervals = Vec::with_capacity(state.every_clocks.len());
    for clock in &state.every_clocks {
        let v = eval_expr(env, &clock.interval)?;
        intervals.push(quantity_to_seconds(&v, clock.interval.line(), clock.interval.col())?);
    }
    scene.borrow_mut().every_intervals_secs = intervals;
    // Reset suspension state — entering a new state restarts the
    // entry sequence from the top regardless of where the previous
    // state was paused.
    {
        let mut inst = scene.borrow_mut();
        inst.fiber_frames.clear();
        inst.entry_wait_remaining = 0.0;
    }
    // Run the on-entry body resumably. If the body (or any
    // function called from it) hits a `wait`, `run_state_entry`
    // pushes the relevant frame(s) onto `fiber_frames` and
    // returns normally — `tick_scene` picks up the work next
    // frame after the wait elapses.
    run_state_entry(env, scene, &state.on_entry)?;
    // A transition during on_entry is followed immediately.
    if let Some(next) = env.transitioning.take() {
        env.self_value = prev_self;
        return enter_state(env, scene, &next);
    }
    env.self_value = prev_self;
    Ok(())
}

/// What the resumable runner reports back at each level. Mirrors
/// the env-flag based control-flow signalling used elsewhere in
/// `eval`, but separated out as an explicit return value because
/// `Suspended` has no env-flag analogue (the env is otherwise
/// clean when a fiber suspends). v0.2 session 2a.
#[derive(Debug, Clone, Copy)]
enum FiberOutcome {
    /// Body finished normally.
    Completed,
    /// A `wait` fired somewhere in this body or its sub-blocks.
    /// The runner has built `out_path` bottom-up; the caller
    /// stores it on the instance for resume next frame.
    Suspended,
    /// `return <value>`. Propagated up the recursion via env.returning.
    Returning,
    /// `break`. Outer `while` consumes; caller propagates otherwise.
    Breaking,
    /// `continue`. Outer `while` consumes; caller propagates otherwise.
    Continuing,
    /// `-> <state>`. Caller (tick_scene / enter_state) handles the
    /// transition. Propagated via env.transitioning.
    Transitioning,
}

/// Drive a state's on-entry body, resumably. v0.2 session 2a.
///
/// Replaces the Phase 5 task 2 single-frame runner: the resume
/// state is now a path through nested blocks rather than a flat
/// statement index. `wait` works as a direct child of the entry
/// body (the original Phase 5 case) AND as a child of `if` /
/// `elif` / `else` / `while` blocks at any nesting depth within
/// the entry. `for` bodies still surface the wait-context error
/// (deferred to a follow-on session). v0.2 session 2b adds
/// function-body `wait` via the fiber stack (`Instance::fiber_frames`).
///
/// Frame ordering: `fiber_frames[0]` is the bottom of the call
/// stack (state-entry); `fiber_frames[len-1]` is the innermost
/// frame (the deepest function call that's currently suspended).
/// `Vec::push` / `Vec::pop` thus naturally manage the top.
fn run_state_entry(
    env: &mut Env,
    scene: &Rc<RefCell<Instance>>,
    stmts: &[Stmt],
) -> Result<(), RuntimeError> {
    // Push our state-entry frame upfront with an empty path.
    // Function frames pushed during the body's run land ABOVE
    // us. On suspend we update our frame's path in-place; on
    // complete we pop it. This keeps `fiber_frames` ordered
    // bottom-to-top no matter when in the call tree the wait
    // fires.
    scene.borrow_mut().fiber_frames.push(Frame {
        kind: FrameKind::StateEntry,
        resume_path: Vec::new(),
    });
    let our_idx = scene.borrow().fiber_frames.len() - 1;
    let mut out_path: Vec<PathEntry> = Vec::new();
    let outcome = run_block_resumable(env, scene, stmts, &[], &mut out_path)?;
    let mut inst = scene.borrow_mut();
    if matches!(outcome, FiberOutcome::Suspended) {
        out_path.reverse();
        inst.fiber_frames[our_idx].resume_path = out_path;
    } else {
        // Body finished. Inner frames should already be drained
        // (every push from a function call was paired with a
        // Suspended bubble-up — non-suspended completions don't
        // leave frames behind). Sanity-pop our frame at our_idx;
        // anything past it is a programmer error.
        debug_assert_eq!(inst.fiber_frames.len(), our_idx + 1);
        inst.fiber_frames.pop();
        if inst.fiber_frames.is_empty() {
            inst.entry_wait_remaining = 0.0;
        }
    }
    Ok(())
}

/// Resume a suspended fiber. Drives the topmost frame first; when
/// it completes, drains down to the parent. Frames stay on
/// `fiber_frames` while running so that any new function calls
/// inside the body land ABOVE the current frame in the natural
/// stack order. v0.2 session 2b.
fn resume_fiber(env: &mut Env, scene: &Rc<RefCell<Instance>>) -> Result<(), RuntimeError> {
    loop {
        let top_idx = match scene.borrow().fiber_frames.len().checked_sub(1) {
            Some(i) => i,
            None => return Ok(()),
        };
        // Snapshot what we need to drive the body. Frame stays
        // in place at `top_idx` while running.
        let (body, resume_path, is_function) = {
            let inst = scene.borrow();
            let f = &inst.fiber_frames[top_idx];
            let body = match &f.kind {
                FrameKind::StateEntry => inst
                    .current_state
                    .as_ref()
                    .and_then(|n| inst.class.states.get(n))
                    .map(|s| s.on_entry.clone())
                    .unwrap_or_default(),
                FrameKind::Function { def, .. } => def.body.clone(),
            };
            let is_function = matches!(f.kind, FrameKind::Function { .. });
            (body, f.resume_path.clone(), is_function)
        };

        if is_function {
            env.call_depth += 1;
        }
        let mut out_path: Vec<PathEntry> = Vec::new();
        let result = run_block_resumable(env, scene, &body, &resume_path, &mut out_path);
        if is_function {
            env.call_depth -= 1;
        }
        let outcome = result?;

        if matches!(outcome, FiberOutcome::Suspended) {
            // Re-suspended. Inner frames may have been pushed
            // above us during the run; our frame is still at
            // `top_idx`. Update its resume_path in place.
            out_path.reverse();
            scene.borrow_mut().fiber_frames[top_idx].resume_path = out_path;
            return Ok(());
        }

        // Body finished. Pop our frame; any inner frames pushed
        // during the run completed (no Suspended bubbled), so
        // our frame is at the top.
        let frame = {
            let mut inst = scene.borrow_mut();
            debug_assert_eq!(inst.fiber_frames.len(), top_idx + 1);
            inst.fiber_frames.pop().expect("frame was at top_idx")
        };
        // Restore saved env state for function frames whose body
        // is now done.
        if let FrameKind::Function {
            saved_returning,
            saved_params,
            ..
        } = frame.kind
        {
            // Discard the function's return value (Stmt::Expr
            // position; v0.2 session 2b doesn't pipe values
            // back into call-as-expression sites).
            let _ = env.returning.take();
            env.returning = saved_returning;
            for (name, prev) in saved_params {
                match prev {
                    Some(v) => env.set(name, v),
                    None => env.remove(&name),
                }
            }
        }
        match outcome {
            FiberOutcome::Returning => continue,
            FiberOutcome::Transitioning | FiberOutcome::Breaking | FiberOutcome::Continuing => {
                return Ok(());
            }
            FiberOutcome::Completed => continue,
            FiberOutcome::Suspended => unreachable!("handled above"),
        }
    }
}

/// Walk a body of statements, honouring an incoming resume path on
/// the first iteration and falling back to normal sequential
/// execution thereafter. On `wait` / sub-block suspension, builds
/// `out_path` from innermost to outermost (caller reverses once).
///
/// The runner only special-cases `Stmt::Wait` and the structured
/// statements that can host a nested `wait`: `Stmt::If` and
/// `Stmt::While`. Everything else is dispatched through `eval_stmt`
/// which handles `wait` inside its body via the existing error
/// path (function calls, `for` bodies, `every` clocks, etc.).
fn run_block_resumable(
    env: &mut Env,
    scene: &Rc<RefCell<Instance>>,
    body: &[Stmt],
    incoming: &[PathEntry],
    out_path: &mut Vec<PathEntry>,
) -> Result<FiberOutcome, RuntimeError> {
    let (start_idx, descent_branch) = match incoming.first() {
        Some(p) => (p.stmt_index, p.branch),
        None => (0, None),
    };
    let inner_incoming: &[PathEntry] = if incoming.is_empty() {
        &[]
    } else {
        &incoming[1..]
    };

    // The first iteration may need to drill into a sub-block
    // guided by `descent_branch`. Subsequent iterations are fresh
    // executions starting at stmt index `idx`.
    let mut idx = start_idx;
    let mut first_iter = true;
    while idx < body.len() {
        let stmt = &body[idx];
        let drilling = first_iter && descent_branch.is_some();
        first_iter = false;

        match stmt {
            Stmt::Wait { duration, line, col } => {
                if drilling {
                    return Err(RuntimeError {
                        line: *line,
                        col: *col,
                        message:
                            "internal: cannot drill into a `wait` statement (corrupted resume path)"
                                .to_string(),
                        help: None,
                    });
                }
                let v = eval_expr(env, duration)?;
                let secs = quantity_to_seconds(&v, *line, *col)?;
                scene.borrow_mut().entry_wait_remaining = secs;
                // Push the deepest entry: at this depth, the next
                // stmt to execute on resume is the one after the
                // wait. Caller(s) prepend their own entries.
                out_path.push(PathEntry {
                    stmt_index: idx + 1,
                    branch: None,
                });
                return Ok(FiberOutcome::Suspended);
            }
            Stmt::If {
                cond,
                then_body,
                elifs,
                else_body,
                ..
            } => {
                // Pick the branch: either resume the previously
                // chosen one (drilling) or evaluate fresh.
                let (target_body, branch) = if drilling {
                    let b = descent_branch.expect("drilling implies descent_branch is set");
                    let target: &[Stmt] = match b {
                        Branch::IfThen => then_body.as_slice(),
                        Branch::IfElif(arm) => elifs
                            .get(arm)
                            .map(|(_, body)| body.as_slice())
                            .unwrap_or(&[]),
                        Branch::IfElse => else_body
                            .as_ref()
                            .map(|v| v.as_slice())
                            .unwrap_or(&[]),
                        Branch::While => {
                            return Err(RuntimeError {
                                line: 0,
                                col: 0,
                                message: "internal: corrupted resume path (while branch on if)"
                                    .to_string(),
                                help: None,
                            });
                        }
                    };
                    (target, b)
                } else {
                    let v = eval_expr(env, cond)?;
                    if is_truthy(&v) {
                        (then_body.as_slice(), Branch::IfThen)
                    } else {
                        let mut chosen: Option<(&[Stmt], Branch)> = None;
                        for (arm_idx, (elif_cond, elif_body)) in elifs.iter().enumerate() {
                            let v = eval_expr(env, elif_cond)?;
                            if is_truthy(&v) {
                                chosen = Some((elif_body.as_slice(), Branch::IfElif(arm_idx)));
                                break;
                            }
                        }
                        match chosen {
                            Some(c) => c,
                            None => match else_body.as_ref() {
                                Some(eb) => (eb.as_slice(), Branch::IfElse),
                                None => {
                                    // No branch matched — skip this stmt entirely.
                                    idx += 1;
                                    continue;
                                }
                            },
                        }
                    }
                };

                let inner = if drilling { inner_incoming } else { &[] };
                let inner_outcome =
                    run_block_resumable(env, scene, target_body, inner, out_path)?;
                match inner_outcome {
                    FiberOutcome::Suspended => {
                        out_path.push(PathEntry {
                            stmt_index: idx,
                            branch: Some(branch),
                        });
                        return Ok(FiberOutcome::Suspended);
                    }
                    FiberOutcome::Returning
                    | FiberOutcome::Breaking
                    | FiberOutcome::Continuing
                    | FiberOutcome::Transitioning => {
                        return Ok(inner_outcome);
                    }
                    FiberOutcome::Completed => {
                        idx += 1;
                    }
                }
            }
            Stmt::While {
                cond,
                body: while_body,
                ..
            } => {
                env.loop_depth += 1;
                let result = run_while_resumable(
                    env,
                    scene,
                    cond,
                    while_body,
                    drilling,
                    inner_incoming,
                    out_path,
                );
                env.loop_depth -= 1;
                let outcome = result?;
                match outcome {
                    FiberOutcome::Suspended => {
                        out_path.push(PathEntry {
                            stmt_index: idx,
                            branch: Some(Branch::While),
                        });
                        return Ok(FiberOutcome::Suspended);
                    }
                    FiberOutcome::Returning | FiberOutcome::Transitioning => {
                        return Ok(outcome);
                    }
                    // `break` / `continue` were consumed inside.
                    // `Completed` just falls through to next stmt.
                    _ => {
                        idx += 1;
                    }
                }
            }
            Stmt::Expr(expr) if is_top_level_user_call(env, expr) => {
                if drilling {
                    return Err(RuntimeError {
                        line: 0,
                        col: 0,
                        message: "internal: corrupted resume path (drilling into expression statement)"
                            .to_string(),
                        help: None,
                    });
                }
                // v0.2 session 2b: top-level function-call
                // statements are run resumably so a `wait`
                // reached from the function's body can suspend
                // the entire fiber stack rather than error.
                // Built-in calls / methods / call-as-expression
                // still go through `eval_stmt` (call_function's
                // run_block path).
                let outcome = run_user_call_resumable(env, scene, expr, out_path)?;
                match outcome {
                    FiberOutcome::Suspended => {
                        // Suspension: record OUR position
                        // (post-call) so the parent runner
                        // knows to skip this stmt on resume.
                        out_path.push(PathEntry {
                            stmt_index: idx + 1,
                            branch: None,
                        });
                        return Ok(FiberOutcome::Suspended);
                    }
                    FiberOutcome::Returning
                    | FiberOutcome::Breaking
                    | FiberOutcome::Continuing
                    | FiberOutcome::Transitioning => {
                        return Ok(outcome);
                    }
                    FiberOutcome::Completed => {
                        idx += 1;
                    }
                }
            }
            _ => {
                if drilling {
                    return Err(RuntimeError {
                        line: 0,
                        col: 0,
                        message: format!(
                            "internal: corrupted resume path (drilling into {})",
                            stmt_kind_name(stmt)
                        ),
                        help: None,
                    });
                }
                // Other statements — let `eval_stmt` handle them.
                // Any `wait` reachable from here goes through
                // `run_block`, which still surfaces the original
                // "wait only supported at..." error. Function
                // calls inside expressions (`let x = f()`),
                // `for` bodies, methods, etc., are follow-ons.
                eval_stmt(env, stmt)?;
                if env.returning.is_some() {
                    return Ok(FiberOutcome::Returning);
                }
                if env.breaking {
                    return Ok(FiberOutcome::Breaking);
                }
                if env.continuing {
                    return Ok(FiberOutcome::Continuing);
                }
                if env.transitioning.is_some() {
                    return Ok(FiberOutcome::Transitioning);
                }
                idx += 1;
            }
        }
    }
    Ok(FiberOutcome::Completed)
}

/// Run a `while` loop resumably. On the first iteration, may
/// resume mid-body (when `drilling` is true and `inner_incoming`
/// is the path into the body). After completing the body —
/// whether on first iteration or resumed — re-evaluates `cond`
/// and loops normally. v0.2 session 2a.
fn run_while_resumable(
    env: &mut Env,
    scene: &Rc<RefCell<Instance>>,
    cond: &Expr,
    body: &[Stmt],
    mut drilling: bool,
    inner_incoming: &[PathEntry],
    out_path: &mut Vec<PathEntry>,
) -> Result<FiberOutcome, RuntimeError> {
    loop {
        if !drilling {
            let v = eval_expr(env, cond)?;
            if !is_truthy(&v) {
                return Ok(FiberOutcome::Completed);
            }
        }
        let inner: &[PathEntry] = if drilling { inner_incoming } else { &[] };
        drilling = false; // only the very first iteration drills.
        let outcome = run_block_resumable(env, scene, body, inner, out_path)?;
        match outcome {
            FiberOutcome::Suspended => return Ok(FiberOutcome::Suspended),
            FiberOutcome::Returning | FiberOutcome::Transitioning => return Ok(outcome),
            FiberOutcome::Breaking => {
                env.breaking = false;
                return Ok(FiberOutcome::Completed);
            }
            FiberOutcome::Continuing => {
                env.continuing = false;
                // fall through to re-eval cond
            }
            FiberOutcome::Completed => {
                // fall through to re-eval cond
            }
        }
    }
}

/// Decide whether a `Stmt::Expr(expr)` is a call site that the
/// resumable runner should drive. Currently restricted to
/// bare-name calls whose target resolves to a user-defined
/// `Value::Function`. Methods (resolved via `find_method` on
/// `self`'s class, or via `Expr::Field` callees), builtins, and
/// calls through any other callee shape fall through to the
/// existing `eval_stmt` path. v0.2 session 2b.
fn is_top_level_user_call(env: &Env, expr: &Expr) -> bool {
    let Expr::Call { callee, .. } = expr else {
        return false;
    };
    let Expr::Ident { name, .. } = callee.as_ref() else {
        return false;
    };
    // Self-method takes precedence — those don't suspend in
    // session 2b (method-body wait deferred to a follow-on).
    if let Some(LegacyValue::Instance(rc)) = &env.self_value.to_legacy() {
        let class = rc.borrow().class.clone();
        if find_method(&class, name).is_some() {
            return false;
        }
    }
    matches!(env.get(name).to_legacy(), Some(LegacyValue::Function(_)))
}

/// Run a `Stmt::Expr(Call)` whose callee is a user function,
/// resumably. Mirrors `call_function`'s param-binding logic but
/// drives the body through `run_block_resumable`. On suspension,
/// pushes a `FrameKind::Function` onto the fiber stack with the
/// saved env state needed to restore on completion. v0.2 session
/// 2b.
fn run_user_call_resumable(
    env: &mut Env,
    scene: &Rc<RefCell<Instance>>,
    expr: &Expr,
    out_path: &mut Vec<PathEntry>,
) -> Result<FiberOutcome, RuntimeError> {
    let _ = out_path; // suspend-path handling is at the parent level
    let (name, args, kwargs, line, col) = match expr {
        Expr::Call {
            callee,
            args,
            kwargs,
            line,
            col,
        } => {
            let n = match callee.as_ref() {
                Expr::Ident { name, .. } => name.clone(),
                _ => unreachable!("guarded by is_top_level_user_call"),
            };
            (n, args.as_slice(), kwargs.as_slice(), *line, *col)
        }
        _ => unreachable!("guarded by is_top_level_user_call"),
    };
    let def: Rc<FunctionDef> = match env.get(&name).to_legacy() {
        Some(LegacyValue::Function(d)) => d.clone(),
        _ => unreachable!("guarded by is_top_level_user_call"),
    };

    // Argument evaluation runs in the caller's scope before any
    // params are shadowed.
    let arg_vals = eval_args(env, args)?;
    let kwarg_vals = eval_kwargs(env, kwargs)?;

    // Mirror call_function's parameter-binding: positionals only
    // when no kwargs, otherwise reorder via bind_kwargs.
    let bound = if kwarg_vals.is_empty() {
        if arg_vals.len() != def.params.len() {
            return Err(RuntimeError {
                line,
                col,
                message: format!(
                    "function '{}' expected {} arguments, got {}",
                    def.name,
                    def.params.len(),
                    arg_vals.len()
                ),
                help: None,
            });
        }
        arg_vals
    } else {
        let param_refs: Vec<&str> = def.params.iter().map(|s| s.as_str()).collect();
        bind_kwargs(&param_refs, &def.name, arg_vals, kwarg_vals, line, col)?
    };

    let saved_returning = env.returning.take();
    let saved_params: Vec<(String, Option<Value>)> = def
        .params
        .iter()
        .map(|p| (p.clone(), env.get(p)))
        .collect();
    for (param, arg) in def.params.iter().zip(bound.iter()) {
        env.set(param.clone(), arg.clone());
    }

    // Push the function frame upfront. Saved env state lives on
    // the frame so it can be restored on completion (whether
    // immediate or deferred via a wait + resume cycle). Function
    // frames pushed by deeper-still calls land ABOVE us, keeping
    // the natural call-stack order.
    scene.borrow_mut().fiber_frames.push(Frame {
        kind: FrameKind::Function {
            def: def.clone(),
            saved_returning,
            saved_params,
        },
        resume_path: Vec::new(),
    });
    let our_idx = scene.borrow().fiber_frames.len() - 1;

    env.call_depth += 1;
    let mut inner_out: Vec<PathEntry> = Vec::new();
    let result = run_block_resumable(env, scene, &def.body, &[], &mut inner_out);
    env.call_depth -= 1;
    let outcome = match result {
        Ok(o) => o,
        Err(e) => {
            // Pop our frame on error and restore env state from
            // it so the caller sees a sane scope chain.
            let frame = scene.borrow_mut().fiber_frames.remove(our_idx);
            if let FrameKind::Function {
                saved_returning,
                saved_params,
                ..
            } = frame.kind
            {
                env.returning = saved_returning;
                for (n, prev) in saved_params {
                    match prev {
                        Some(v) => env.set(n, v),
                        None => env.remove(&n),
                    }
                }
            }
            return Err(e);
        }
    };

    if matches!(outcome, FiberOutcome::Suspended) {
        // Update our frame's resume_path in place. Inner frames
        // pushed by deeper calls (if any) sit above us at higher
        // indices and stay there.
        inner_out.reverse();
        scene.borrow_mut().fiber_frames[our_idx].resume_path = inner_out;
        return Ok(FiberOutcome::Suspended);
    }

    // Body finished. Pop our frame; restore env state from its
    // saved fields.
    let frame = {
        let mut inst = scene.borrow_mut();
        debug_assert_eq!(inst.fiber_frames.len(), our_idx + 1);
        inst.fiber_frames.pop().expect("frame at our_idx")
    };
    let _ = env.returning.take(); // discard return value (Stmt::Expr position)
    if let FrameKind::Function {
        saved_returning,
        saved_params,
        ..
    } = frame.kind
    {
        env.returning = saved_returning;
        for (n, prev) in saved_params {
            match prev {
                Some(v) => env.set(n, v),
                None => env.remove(&n),
            }
        }
    }

    match outcome {
        // A `return` inside the function body is observed at
        // *function-body level* — for the parent runner the
        // function call simply completed.
        FiberOutcome::Returning => Ok(FiberOutcome::Completed),
        other => Ok(other),
    }
}

/// Name a `Stmt` for diagnostic messages. Only used by the
/// "corrupted resume path" internal error so the user sees
/// *which* kind of stmt the runner tried to drill into. Limited
/// to the variants the resumable runner can encounter.
fn stmt_kind_name(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::Let { .. } => "let",
        Stmt::Assign { .. } => "assign",
        Stmt::If { .. } => "if",
        Stmt::OnUpdate { .. } => "on update",
        Stmt::OnRender { .. } => "on render",
        Stmt::Decl { .. } => "decl",
        Stmt::FunctionDecl { .. } => "function",
        Stmt::Return { .. } => "return",
        Stmt::While { .. } => "while",
        Stmt::For { .. } => "for",
        Stmt::Break { .. } => "break",
        Stmt::Continue { .. } => "continue",
        Stmt::Transition { .. } => "transition",
        Stmt::Spawn { .. } => "spawn",
        Stmt::Despawn { .. } => "despawn",
        Stmt::Wait { .. } => "wait",
        Stmt::DialogueDecl { .. } => "dialogue",
        Stmt::Say { .. } => "say",
        Stmt::Choice { .. } => "choice",
        Stmt::Expr(_) => "expression",
    }
}

/// Resolve a bare name. Scope chain: the active `self` instance's fields
/// shadow env globals, so `ticks += 1` inside a scene state mutates
/// `self.ticks`. Without this, scene fields would only be reachable via
/// explicit `self.x` syntax — verbose and unusual.
fn lookup_name(env: &Env, name: &str) -> Option<Value> {
    if let Some(LegacyValue::Instance(rc)) = &env.self_value.to_legacy() {
        if let Some(v) = rc.borrow().get_field(name) {
            return Some(v);
        }
    }
    env.get(name)
}

fn quantity_to_seconds(v: &Value, line: u32, col: u32) -> Result<f64, RuntimeError> {
    match v.to_legacy() {
        LegacyValue::Quantity { value: value, unit: unit } => match unit.as_str() {
            "s" => Ok(value),
            "ms" => Ok(value / 1000.0),
            "min" => Ok(value * 60.0),
            "h" => Ok(value * 3600.0),
            other => Err(RuntimeError {
                line,
                col,
                message: format!(
                    "every <duration> needs a time unit (s, ms, min, h), got '{other}'"
                ),
                help: Some(
                    "duration literals carry a unit suffix: `100ms`, `0.5s`, `2min`, `1h`"
                        .to_string(),
                ),
            }),
        },
        LegacyValue::Float(f) => Ok(f),
        LegacyValue::Int(n) => Ok(n as f64),
        other => Err(RuntimeError {
            line,
            col,
            message: format!(
                "every <duration> expects a duration quantity, got {}",
                other.type_name()
            ),
            help: Some("e.g. `every 100ms:` or `every 0.5s:`".to_string()),
        }),
    }
}

fn run_block(env: &mut Env, stmts: &[Stmt]) -> Result<(), RuntimeError> {
    for stmt in stmts {
        eval_stmt(env, stmt)?;
        if env.returning.is_some()
            || env.breaking
            || env.continuing
            || env.transitioning.is_some()
        {
            return Ok(());
        }
    }
    Ok(())
}

fn eval_stmt(env: &mut Env, stmt: &Stmt) -> Result<(), RuntimeError> {
    match stmt {
        Stmt::Let { name, value, .. } => {
            let v = eval_expr(env, value)?;
            env.set(name.clone(), v);
            Ok(())
        }
        Stmt::Assign {
            target,
            op,
            value,
            line,
            col,
        } => eval_assign(env, target, *op, value, *line, *col),
        Stmt::If {
            cond,
            then_body,
            elifs,
            else_body,
            ..
        } => {
            let cond_val = eval_expr(env, cond)?;
            if is_truthy(&cond_val) {
                return run_block(env, then_body);
            }
            for (elif_cond, elif_body) in elifs {
                let v = eval_expr(env, elif_cond)?;
                if is_truthy(&v) {
                    return run_block(env, elif_body);
                }
            }
            if let Some(eb) = else_body {
                run_block(env, eb)?;
            }
            Ok(())
        }
        Stmt::FunctionDecl { name, params, body, .. } => {
            // Annotations don't affect runtime semantics; the
            // tree-walker still binds bare-name params. Strict
            // mode (Phase 6 session 2) uses the annotations
            // statically in `infer.rs`.
            let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
            env.set(
                name.clone(),
                Value::from_function(Rc::new(FunctionDef {
                    name: name.clone(),
                    params: param_names,
                    body: body.clone(),
                })),
            );
            Ok(())
        }
        Stmt::Return { value, line, col } => {
            if env.call_depth == 0 {
                return Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: "`return` is only valid inside a function or method body"
                        .to_string(),
                    help: Some(
                        "to exit early from a state body, use `-> <state>` to transition; \
                         to exit a dialogue, the dialogue's body ends naturally"
                            .to_string(),
                    ),
                });
            }
            let v = match value {
                Some(e) => eval_expr(env, e)?,
                None => Value::NIL,
            };
            env.returning = Some(v);
            Ok(())
        }
        Stmt::While { cond, body, .. } => {
            env.loop_depth += 1;
            let result = run_while(env, cond, body);
            env.loop_depth -= 1;
            result
        }
        Stmt::For {
            var, iter, body, line, col,
        } => {
            env.loop_depth += 1;
            let result = run_for(env, var, iter, body, *line, *col);
            env.loop_depth -= 1;
            result
        }
        Stmt::Break { line, col } => {
            if env.loop_depth == 0 {
                return Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: "`break` is only valid inside a loop".to_string(),
                    help: Some(
                        "loops are `while <cond>:` and `for <var> in <iter>:` — `break` exits the nearest enclosing one"
                            .to_string(),
                    ),
                });
            }
            env.breaking = true;
            Ok(())
        }
        Stmt::Continue { line, col } => {
            if env.loop_depth == 0 {
                return Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: "`continue` is only valid inside a loop".to_string(),
                    help: Some(
                        "loops are `while <cond>:` and `for <var> in <iter>:` — `continue` skips to the next iteration of the nearest enclosing one"
                            .to_string(),
                    ),
                });
            }
            env.continuing = true;
            Ok(())
        }
        Stmt::Transition { target, .. } => {
            env.transitioning = Some(target.clone());
            Ok(())
        }
        Stmt::Spawn { class, at, line, col } => {
            let class_val = env.get(class).ok_or_else(|| RuntimeError {
                line: *line,
                col: *col,
                message: format!("class '{class}' is not defined"),
                help: Some(format!("declare it with `entity {class}:` first")),
            })?;
            let class_rc = match class_val.to_legacy() {
                LegacyValue::Class(c) => c,
                other => {
                    return Err(RuntimeError {
                        line: *line,
                        col: *col,
                        message: format!(
                            "`spawn {class}` expects a class, but {class} is a {}",
                            other.type_name()
                        ),
                        help: None,
                    });
                }
            };
            let at_value = match at {
                Some(expr) => Some(eval_expr(env, expr)?),
                None => None,
            };
            let inst_val = instantiate(class_rc.clone());
            if let Some(av) = &at_value {
                if inst_val.is_instance() {
                    let rc = inst_val.as_instance();
                    rc.borrow_mut().insert_field("pos", av.clone());
                }
            }
            if inst_val.is_instance() {
                let rc = inst_val.as_instance();
                if class_rc.kind == "particles" {
                    seed_particle_emitter(env, &rc, at_value.as_ref(), *line, *col)?;
                }
                env.active_entities.push(rc.clone());
            }
            Ok(())
        }
        Stmt::Despawn { target, line, col } => {
            let v = eval_expr(env, target)?;
            match v.to_legacy() {
                LegacyValue::Instance(rc) => {
                    rc.borrow_mut().despawned = true;
                    Ok(())
                }
                other => Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: format!(
                        "`despawn` expects an instance, got {}",
                        other.type_name()
                    ),
                    help: None,
                }),
            }
        }
        Stmt::DialogueDecl { name, body, .. } => {
            // Register the dialogue as a parameterless callable. We
            // reuse `Value::Function` so the existing call-site
            // machinery (function-call lookup, scoping, return
            // semantics) Just Works. The dialogue's body runs to
            // completion when invoked; v0.1 does not pause on
            // `wait` inside a dialogue (the wait runtime error
            // surfaces if a user tries it — see the
            // `wait`-context error in the Stmt::Wait arm). A
            // per-dialogue scheduler is a Phase 5 task 3 follow-on.
            let dialogue = Value::from_function(Rc::new(FunctionDef {
                name: name.clone(),
                params: Vec::new(),
                body: body.clone(),
            }));
            env.set(name.clone(), dialogue);
            Ok(())
        }
        Stmt::Say { actor, text, line, col } => {
            let text_value = eval_expr(env, text)?;
            let text_str = text_value.display();
            let actor_str = match actor {
                Some(a) => Some(eval_expr(env, a)?),
                None => None,
            };
            match actor_str {
                Some(av) => {
                    // Render the actor with whatever's most natural
                    // for the value: instances show their class name
                    // (Wren-style), strings show themselves, anything
                    // else falls back to `display`. Output is a
                    // single line per `say`.
                    let label = match &av.to_legacy() {
                        LegacyValue::Instance(inst) => inst.borrow().class.name.clone(),
                        LegacyValue::Str(s) => s.as_ref().clone(),
                        other => other.display(),
                    };
                    env.out.push_str(&format!("{label}: {text_str}\n"));
                }
                None => {
                    env.out.push_str(&text_str);
                    env.out.push('\n');
                }
            }
            // Suppress the line/col warning when neither actor nor
            // text errors — they're carried for diagnostics if a
            // future strict pass cares.
            let _ = (*line, *col);
            Ok(())
        }
        Stmt::Choice { branches, line, col } => {
            // Print each label so a transcript shows the user what
            // was on offer. v0.1 always picks the first branch — the
            // deterministic surface is enough to ship dialogue;
            // interactive selection is a Phase 5 task 3 follow-on.
            for (i, (label, _)) in branches.iter().enumerate() {
                let label_value = eval_expr(env, label)?;
                env.out.push_str(&format!(
                    "  [{}] {}\n",
                    i + 1,
                    label_value.display()
                ));
            }
            // Pick branch 0. Empty branches list was rejected at
            // parse time, so unwrap is safe.
            let (_, body) = branches.first().expect("choice has at least one branch");
            run_block(env, body)?;
            let _ = (*line, *col);
            Ok(())
        }
        Stmt::Wait { line, col, .. } => {
            // Phase 5 task 2: `wait` is intercepted directly by
            // `run_state_entry` before reaching this arm. Any path
            // that lands here ran through `run_block`, which means
            // `wait` was used somewhere we don't yet support
            // (function body, every-clock, on update, …). Surface
            // the limitation explicitly rather than silently sleep
            // for zero seconds.
            Err(RuntimeError {
                line: *line,
                col: *col,
                message: "`wait` is only supported as a direct statement of a state body in v0.1"
                    .to_string(),
                help: Some(
                    "move the `wait` to the top level of a `state <name>:` body — fiber-aware contexts (functions, every, dialogue) ship in later Phase 5 sessions".to_string(),
                ),
            })
        }
        Stmt::OnUpdate { param, body, .. } => {
            env.on_update = Some(OnUpdateHandler {
                param: param.clone(),
                body: body.clone(),
            });
            Ok(())
        }
        Stmt::OnRender { body, .. } => {
            env.top_on_render = Some(body.clone());
            Ok(())
        }
        Stmt::Decl {
            kind,
            name,
            parent,
            members,
            line,
            col,
        } => eval_decl(env, *kind, name, parent.as_deref(), members, *line, *col),
        Stmt::Expr(e) => {
            eval_expr(env, e)?;
            Ok(())
        }
    }
}

fn eval_assign(
    env: &mut Env,
    target: &AssignTarget,
    op: AssignOp,
    value: &Expr,
    line: u32,
    col: u32,
) -> Result<(), RuntimeError> {
    let new_value = eval_expr(env, value)?;
    match target {
        AssignTarget::Name(name) => {
            if matches!(op, AssignOp::Set) {
                // Mutate the instance field if `name` is one (scope chain),
                // else fall back to env. New `let` bindings are introduced
                // by Stmt::Let, not by plain `name = value`.
                if let Some(LegacyValue::Instance(rc)) = &env.self_value.to_legacy() {
                    let mut inst = rc.borrow_mut();
                    if inst.fields.contains_key(name) {
                        inst.insert_field(name.clone(), new_value);
                        return Ok(());
                    }
                }
                env.set(name.clone(), new_value);
                return Ok(());
            }
            let current = lookup_name(env, name).ok_or_else(|| RuntimeError {
                line,
                col,
                message: format!("name '{name}' is not defined"),
                help: Some(format!("declare it with `let {name} = ...` before use")),
            })?;
            let combined = compound(op, &current, &new_value, line, col)?;
            if let Some(LegacyValue::Instance(rc)) = &env.self_value.to_legacy() {
                let mut inst = rc.borrow_mut();
                if inst.fields.contains_key(name) {
                    inst.insert_field(name.clone(), combined);
                    return Ok(());
                }
            }
            env.set(name.clone(), combined);
            Ok(())
        }
        AssignTarget::Field { object, name } => {
            let obj_val = eval_expr(env, object)?;
            match obj_val.to_legacy() {
                LegacyValue::Object(rc) => {
                    let final_value = if matches!(op, AssignOp::Set) {
                        new_value
                    } else {
                        let current = rc.borrow().get_field(name).ok_or_else(|| {
                            RuntimeError {
                                line,
                                col,
                                message: format!("field '{name}' is not defined on this object"),
                                help: Some(format!("set it first with `obj.{name} = ...`")),
                            }
                        })?;
                        compound(op, &current, &new_value, line, col)?
                    };
                    // Special case: `.pos = (x, y)` on a sprite-shaped object
                    // also updates `.x` and `.y`. Mirrors Example 1's
                    // tuple-as-Vector2 behavior.
                    if name == "pos" {
                        if let LegacyValue::Tuple(elems) = &final_value.to_legacy() {
                            if elems.len() >= 2 {
                                let mut o = rc.borrow_mut();
                                o.insert_field("x".to_string(), elems[0].clone());
                                o.insert_field("y".to_string(), elems[1].clone());
                            }
                        }
                    }
                    rc.borrow_mut().insert_field(name.clone(), final_value);
                    if name == "x" || name == "y" {
                        refresh_pos(&rc);
                    }
                    Ok(())
                }
                LegacyValue::Instance(rc) => {
                    let final_value = if matches!(op, AssignOp::Set) {
                        new_value
                    } else {
                        let current = rc
                            .borrow()
                            .get_field(name)
                            .ok_or_else(|| {
                                let inst = rc.borrow();
                                let names: Vec<&String> = inst.fields.keys().collect();
                                let suggestion = crate::value::did_you_mean(name, &names)
                                    .map(str::to_string);
                                RuntimeError {
                                    line,
                                    col,
                                    message: format!(
                                        "field '{name}' is not defined on instance of {}",
                                        inst.class.name
                                    ),
                                    help: match suggestion {
                                        Some(s) => Some(format!("did you mean `{s}`?")),
                                        None => Some(
                                            "use `<instance>.<field> = <value>` only for fields declared on the class"
                                                .to_string(),
                                        ),
                                    },
                                }
                            })?;
                        compound(op, &current, &new_value, line, col)?
                    };
                    rc.borrow_mut().insert_field(name.clone(), final_value);
                    Ok(())
                }
                other => Err(RuntimeError {
                    line,
                    col,
                    message: format!(
                        "cannot assign field on value of type {}",
                        other.type_name()
                    ),
                    help: Some(
                        "only objects and class instances support field assignment".to_string(),
                    ),
                }),
            }
        }
    }
}

fn refresh_pos(rc: &Rc<std::cell::RefCell<crate::value::Object>>) {
    let (x, y) = {
        let o = rc.borrow();
        (
            o.get_field("x").unwrap_or(Value::NIL),
            o.get_field("y").unwrap_or(Value::NIL),
        )
    };
    rc.borrow_mut()
        .insert_field("pos", Value::from_tuple(Rc::new(vec![x, y])));
}

fn compound(
    op: AssignOp,
    current: &Value,
    rhs: &Value,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    let bop = match op {
        AssignOp::Set => unreachable!("handled above"),
        AssignOp::AddAssign => BinOp::Add,
        AssignOp::SubAssign => BinOp::Sub,
        AssignOp::MulAssign => BinOp::Mul,
        AssignOp::DivAssign => BinOp::Div,
    };
    apply_arith(bop, current, rhs, line, col)
}

fn eval_expr(env: &mut Env, expr: &Expr) -> Result<Value, RuntimeError> {
    match expr {
        Expr::Str { value, .. } => Ok(Value::from_string(value.clone())),
        Expr::Interp { parts, exprs, .. } => {
            // parts.len() == exprs.len() + 1 by construction in lex_string.
            let mut out = String::new();
            for (i, p) in parts.iter().enumerate() {
                out.push_str(p);
                if let Some(e) = exprs.get(i) {
                    let v = eval_expr(env, e)?;
                    out.push_str(&v.display());
                }
            }
            Ok(Value::from_string(out))
        }
        Expr::Int { value, .. } => Ok(Value::from_int(*value)),
        Expr::Float { value, .. } => Ok(Value::from_float(*value)),
        Expr::Bool { value, .. } => Ok(Value::from_bool(*value)),
        Expr::Percent { value, .. } => Ok(Value::from_percent(*value)),
        Expr::Quantity { value, unit, .. } => Ok(Value::from_quantity(*value, Rc::new(unit.clone()))),
        Expr::Ident { name, line, col } => lookup_name(env, name).ok_or_else(|| RuntimeError {
            line: *line,
            col: *col,
            message: format!("name '{name}' is not defined"),
            help: Some(format!("declare it with `let {name} = ...` before use")),
        }),
        Expr::SelfRef { line, col } => env.self_value.clone().ok_or_else(|| RuntimeError {
            line: *line,
            col: *col,
            message: "`self` is only valid inside a method body".to_string(),
            help: Some(
                "method bodies inside `entity` / `item` / `scene` blocks bind `self` to the instance; outside that, refer to values by name"
                    .to_string(),
            ),
        }),
        Expr::Tuple { elems, .. } => {
            let mut vals = Vec::with_capacity(elems.len());
            for e in elems {
                vals.push(eval_expr(env, e)?);
            }
            Ok(Value::from_tuple(Rc::new(vals)))
        }
        Expr::List { elems, .. } => {
            let mut vals = Vec::with_capacity(elems.len());
            for e in elems {
                vals.push(eval_expr(env, e)?);
            }
            Ok(Value::from_list(Rc::new(RefCell::new(vals))))
        }
        Expr::Index { object, index, line, col } => {
            let obj = eval_expr(env, object)?;
            let idx = eval_expr(env, index)?;
            index_get(&obj, &idx, *line, *col)
        }
        Expr::Range {
            start,
            end,
            exclusive,
            line,
            col,
        } => {
            let s = eval_expr(env, start)?;
            let e = eval_expr(env, end)?;
            match (&s, &e).to_legacy() {
                (LegacyValue::Int(a), LegacyValue::Int(b)) => Ok(Value::from_range(a, b, *exclusive)),
                _ => Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: format!(
                        "range bounds must be ints, got {} and {}",
                        s.type_name(),
                        e.type_name()
                    ),
                    help: Some("v0.1 supports only integer ranges; float ranges ship later".to_string()),
                }),
            }
        }
        Expr::Field {
            object,
            name,
            line,
            col,
        } => {
            let obj = eval_expr(env, object)?;
            field_get(&obj, name, *line, *col)
        }
        Expr::Call {
            callee,
            args,
            kwargs,
            line,
            col,
        } => eval_call(env, callee, args, kwargs, *line, *col),
        Expr::Unary {
            op,
            operand,
            line,
            col,
        } => {
            let v = eval_expr(env, operand)?;
            match (op, v.to_legacy()) {
                (UnOp::Neg, LegacyValue::Int(n)) => Ok(Value::from_int(-n)),
                (UnOp::Neg, LegacyValue::Float(x)) => Ok(Value::from_float(-x)),
                (UnOp::Neg, _) => Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: format!("cannot negate value of type {}", v.type_name()),
                    help: Some("`-` is defined on int and float".to_string()),
                }),
                (UnOp::Not, _) => Ok(Value::from_bool(!is_truthy(&v))),
            }
        }
        Expr::Binary {
            op,
            left,
            right,
            line,
            col,
        } => eval_binary(env, *op, left, right, *line, *col),
    }
}

fn index_get(obj: &Value, idx: &Value, line: u32, col: u32) -> Result<Value, RuntimeError> {
    match (obj, idx).to_legacy() {
        (LegacyValue::List(rc), LegacyValue::Int(i)) => {
            let v = rc.borrow();
            let len = v.len() as i64;
            let actual = if i < 0 { i + len } else { i };
            if actual < 0 || actual >= len {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!("list index {i} out of bounds (length {len})"),
                    help: Some(
                        "lists are 0-indexed; negative indices count from the end".to_string(),
                    ),
                });
            }
            Ok(v[actual as usize].clone())
        }
        (LegacyValue::Tuple(elems), LegacyValue::Int(i)) => {
            let len = elems.len() as i64;
            let actual = if i < 0 { i + len } else { i };
            if actual < 0 || actual >= len {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!("tuple index {i} out of bounds (length {len})"),
                    help: Some(format!(
                        "tuple indices are 0-based, so a length-{len} tuple uses indices 0..{}",
                        len.saturating_sub(1)
                    )),
                });
            }
            Ok(elems[actual as usize].clone())
        }
        (LegacyValue::List(_) | LegacyValue::Tuple(_), other) => Err(RuntimeError {
            line,
            col,
            message: format!("index must be int, got {}", other.type_name()),
            help: None,
        }),
        (other, _) => Err(RuntimeError {
            line,
            col,
            message: format!("cannot index value of type {}", other.type_name()),
            help: Some("indexing works on lists and tuples".to_string()),
        }),
    }
}

fn field_get(obj: &Value, name: &str, line: u32, col: u32) -> Result<Value, RuntimeError> {
    match obj.to_legacy() {
        LegacyValue::Tuple(elems) => match name {
            "x" if !elems.is_empty() => Ok(elems[0].clone()),
            "y" if elems.len() >= 2 => Ok(elems[1].clone()),
            "z" if elems.len() >= 3 => Ok(elems[2].clone()),
            _ => Err(RuntimeError {
                line,
                col,
                message: format!("tuple has no field '{name}'"),
                help: Some(
                    "tuples expose .x, .y, .z (and only those for the leading components)"
                        .to_string(),
                ),
            }),
        },
        LegacyValue::List(rc) => match name {
            "length" => Ok(Value::from_int(rc.borrow().len() as i64)),
            _ => Err(RuntimeError {
                line,
                col,
                message: format!("list has no field '{name}'"),
                help: Some(
                    "lists expose .length; methods are .append, .prepend, .pop_back, \
                     .pop_front, .contains"
                        .to_string(),
                ),
            }),
        },
        LegacyValue::Object(rc) => rc.borrow().get_field(name).ok_or_else(|| {
            RuntimeError {
                line,
                col,
                message: format!("field '{name}' is not defined on this object"),
                help: Some(format!("set it first with `obj.{name} = ...`")),
            }
        }),
        LegacyValue::Instance(rc) => {
            let inst = rc.borrow();
            if let Some(v) = inst.get_field(name) {
                return Ok(v);
            }
            // Methods are not values yet — `obj.method` outside a call site is
            // not supported in this commit. The Call path resolves them
            // directly. Falling through to "field not defined" is correct.
            Err(RuntimeError {
                line,
                col,
                message: format!(
                    "field '{name}' is not defined on instance of {}",
                    inst.class.name
                ),
                help: None,
            })
        }
        _ => Err(RuntimeError {
            line,
            col,
            message: format!("cannot read field on value of type {}", obj.type_name()),
            help: None,
        }),
    }
}

fn eval_call(
    env: &mut Env,
    callee: &Expr,
    args: &[Expr],
    kwargs: &[(String, Expr)],
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    // Bare-name call inside a method or scene state body: dispatch to a
    // method on self if the name resolves there. Mirrors the bare-name
    // read/assign behaviour in `lookup_name` / `eval_assign`. Without
    // this, scene methods would only be reachable via `self.method()`
    // — verbose, and Snake-style code uses bare calls.
    if let Expr::Ident { name, .. } = callee {
        if let Some(LegacyValue::Instance(rc)) = env.self_value.clone().to_legacy() {
            let class = rc.borrow().class.clone();
            if let Some(method) = find_method(&class, name) {
                let arg_vals = eval_args(env, args)?;
                let kwarg_vals = eval_kwargs(env, kwargs)?;
                return call_method(
                    env,
                    Value::from_instance(rc),
                    &method,
                    &arg_vals,
                    &kwarg_vals,
                    line,
                    col,
                );
            }
        }
    }
    // Method call: `recv.method(args)`. Resolved here (not via field_get)
    // because methods aren't first-class values yet.
    if let Expr::Field { object, name, .. } = callee {
        let recv = eval_expr(env, object)?;
        // List built-in methods.
        if let LegacyValue::List(rc) = &recv.to_legacy() {
            if !kwargs.is_empty() {
                return Err(no_kwargs_error(&format!("list.{name}"), line, col));
            }
            if let Some(v) = list_method_call(env, rc, name, args, line, col)? {
                return Ok(v);
            }
        }
        if let LegacyValue::Range { start: start, end: end, exclusive: exclusive } = &recv.to_legacy() {
            if !kwargs.is_empty() {
                return Err(no_kwargs_error(&format!("range.{name}"), line, col));
            }
            if let Some(v) = range_method_call(
                env,
                *start,
                *end,
                *exclusive,
                name,
                args,
                line,
                col,
            )? {
                return Ok(v);
            }
        }
        if let LegacyValue::Instance(rc) = &recv.to_legacy() {
            let class = rc.borrow().class.clone();
            if let Some(method) = find_method(&class, name) {
                let arg_vals = eval_args(env, args)?;
                let kwarg_vals = eval_kwargs(env, kwargs)?;
                return call_method(env, recv, &method, &arg_vals, &kwarg_vals, line, col);
            }
            // Fall through to a normal field_get -> Call path, which will
            // produce a "field not defined" error below.
        }
        // Re-create the field-get + call path for non-instance receivers.
        let arg_vals = eval_args(env, args)?;
        let kwarg_vals = eval_kwargs(env, kwargs)?;
        let f = field_get(&recv, name, line, col)?;
        return apply_call(env, f, &arg_vals, &kwarg_vals, line, col);
    }
    let f = eval_expr(env, callee)?;
    let arg_vals = eval_args(env, args)?;
    let kwarg_vals = eval_kwargs(env, kwargs)?;
    apply_call(env, f, &arg_vals, &kwarg_vals, line, col)
}

fn eval_args(env: &mut Env, args: &[Expr]) -> Result<Vec<Value>, RuntimeError> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        out.push(eval_expr(env, a)?);
    }
    Ok(out)
}

fn eval_kwargs(
    env: &mut Env,
    kwargs: &[(String, Expr)],
) -> Result<Vec<(String, Value)>, RuntimeError> {
    let mut out = Vec::with_capacity(kwargs.len());
    for (n, e) in kwargs {
        out.push((n.clone(), eval_expr(env, e)?));
    }
    Ok(out)
}

fn no_kwargs_error(callee: &str, line: u32, col: u32) -> RuntimeError {
    RuntimeError {
        line,
        col,
        message: format!("{callee} doesn't accept keyword arguments"),
        help: Some("call it with positional arguments only".to_string()),
    }
}

/// Distribute kwargs into a positional Vec given the callee's declared
/// param names. Returns `Vec<Value>` of length `params.len()` with every
/// slot filled, or an error for: extra positionals, unknown kw, duplicate
/// binding (kw collides with positional or another kw), or missing params.
fn bind_kwargs(
    params: &[&str],
    callee: &str,
    positional: Vec<Value>,
    kwargs: Vec<(String, Value)>,
    line: u32,
    col: u32,
) -> Result<Vec<Value>, RuntimeError> {
    if kwargs.is_empty() && positional.len() == params.len() {
        return Ok(positional);
    }
    if positional.len() > params.len() {
        return Err(RuntimeError {
            line,
            col,
            message: format!(
                "{callee} expected {} arguments, got {} positional",
                params.len(),
                positional.len()
            ),
            help: None,
        });
    }
    let mut slots: Vec<Option<Value>> = vec![None; params.len()];
    for (i, v) in positional.into_iter().enumerate() {
        slots[i] = Some(v);
    }
    for (kname, kval) in kwargs {
        let idx = match params.iter().position(|p| *p == kname) {
            Some(i) => i,
            None => {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!(
                        "{callee} has no parameter named `{kname}`"
                    ),
                    help: Some(format!(
                        "expected parameters: {}",
                        params.join(", ")
                    )),
                });
            }
        };
        if slots[idx].is_some() {
            return Err(RuntimeError {
                line,
                col,
                message: format!(
                    "{callee}: parameter `{kname}` already bound by an earlier argument"
                ),
                help: None,
            });
        }
        slots[idx] = Some(kval);
    }
    let mut result = Vec::with_capacity(slots.len());
    for (i, slot) in slots.into_iter().enumerate() {
        match slot {
            Some(v) => result.push(v),
            None => {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!(
                        "{callee}: missing argument for parameter `{}`",
                        params[i]
                    ),
                    help: None,
                });
            }
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn list_method_call(
    env: &mut Env,
    rc: &Rc<RefCell<Vec<Value>>>,
    name: &str,
    args: &[Expr],
    line: u32,
    col: u32,
) -> Result<Option<Value>, RuntimeError> {
    let arity_check = |expected: usize| -> Result<(), RuntimeError> {
        if args.len() != expected {
            Err(RuntimeError {
                line,
                col,
                message: format!(
                    "list.{name} expected {expected} argument{}, got {}",
                    if expected == 1 { "" } else { "s" },
                    args.len()
                ),
                help: None,
            })
        } else {
            Ok(())
        }
    };
    match name {
        "append" => {
            arity_check(1)?;
            let v = eval_expr(env, &args[0])?;
            rc.borrow_mut().push(v);
            Ok(Some(Value::NIL))
        }
        "prepend" => {
            arity_check(1)?;
            let v = eval_expr(env, &args[0])?;
            rc.borrow_mut().insert(0, v);
            Ok(Some(Value::NIL))
        }
        "pop_back" => {
            arity_check(0)?;
            rc.borrow_mut().pop().ok_or_else(|| RuntimeError {
                line,
                col,
                message: "pop_back on an empty list".to_string(),
                help: Some("guard with `if list.length > 0:` before popping".to_string()),
            }).map(Some)
        }
        "pop_front" => {
            arity_check(0)?;
            let mut v = rc.borrow_mut();
            if v.is_empty() {
                return Err(RuntimeError {
                    line,
                    col,
                    message: "pop_front on an empty list".to_string(),
                    help: Some("guard with `if list.length > 0:` before popping".to_string()),
                });
            }
            Ok(Some(v.remove(0)))
        }
        "contains" => {
            arity_check(1)?;
            let needle = eval_expr(env, &args[0])?;
            let found = rc.borrow().iter().any(|v| values_equal(v, &needle));
            Ok(Some(Value::from_bool(found)))
        }
        _ => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
fn range_method_call(
    env: &mut Env,
    start: i64,
    end: i64,
    exclusive: bool,
    name: &str,
    args: &[Expr],
    line: u32,
    col: u32,
) -> Result<Option<Value>, RuntimeError> {
    match name {
        "roll" => {
            if !args.is_empty() {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!("range.roll expected 0 arguments, got {}", args.len()),
                    help: None,
                });
            }
            let upper = if exclusive { end } else { end + 1 };
            if upper <= start {
                return Err(RuntimeError {
                    line,
                    col,
                    message: "range.roll on an empty range".to_string(),
                    help: None,
                });
            }
            let n = env.next_random_u64();
            let span = (upper - start) as u64;
            Ok(Some(Value::from_int(start + (n % span) as i64)))
        }
        "contains" => {
            if args.len() != 1 {
                return Err(RuntimeError {
                    line,
                    col,
                    message: "range.contains expected 1 argument".to_string(),
                    help: None,
                });
            }
            let v = eval_expr(env, &args[0])?;
            let upper = if exclusive { end } else { end + 1 };
            let result = match v.to_legacy() {
                LegacyValue::Int(n) => n >= start && n < upper,
                _ => false,
            };
            Ok(Some(Value::from_bool(result)))
        }
        _ => Ok(None),
    }
}

fn apply_call(
    env: &mut Env,
    f: Value,
    args: &[Value],
    kwargs: &[(String, Value)],
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    match f.to_legacy() {
        LegacyValue::Builtin { name: name, params: params, func: func } => {
            if params.is_empty() {
                if !kwargs.is_empty() {
                    return Err(no_kwargs_error(name, line, col));
                }
                func(env, args)
            } else {
                let bound = bind_kwargs(
                    params,
                    name,
                    args.to_vec(),
                    kwargs.to_vec(),
                    line,
                    col,
                )?;
                func(env, &bound)
            }
        }
        LegacyValue::Function(def) => call_function(env, &def, args, kwargs, line, col),
        LegacyValue::Class(class) => {
            if !args.is_empty() || !kwargs.is_empty() {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!(
                        "constructor for {} takes no arguments yet (got {})",
                        class.name,
                        args.len() + kwargs.len()
                    ),
                    help: Some(
                        "v0.1 constructors initialise from field defaults; \
                         positional/keyword args ship later"
                            .to_string(),
                    ),
                });
            }
            Ok(instantiate(class))
        }
        other => Err(RuntimeError {
            line,
            col,
            message: format!("cannot call value of type {}", other.type_name()),
            help: Some(
                "only functions, builtins, and class constructors are callable".to_string(),
            ),
        }),
    }
}

fn call_function(
    env: &mut Env,
    def: &FunctionDef,
    args: &[Value],
    kwargs: &[(String, Value)],
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    let bound = if kwargs.is_empty() {
        if args.len() != def.params.len() {
            return Err(RuntimeError {
                line,
                col,
                message: format!(
                    "function '{}' expected {} arguments, got {}",
                    def.name,
                    def.params.len(),
                    args.len()
                ),
                help: None,
            });
        }
        args.to_vec()
    } else {
        let param_refs: Vec<&str> = def.params.iter().map(|s| s.as_str()).collect();
        bind_kwargs(
            &param_refs,
            &def.name,
            args.to_vec(),
            kwargs.to_vec(),
            line,
            col,
        )?
    };
    let args = &bound;
    let saved_returning = env.returning.take();
    let saved_params: Vec<(String, Option<Value>)> = def
        .params
        .iter()
        .map(|p| (p.clone(), env.get(p)))
        .collect();
    for (param, arg) in def.params.iter().zip(args.iter()) {
        env.set(param.clone(), arg.clone());
    }
    env.call_depth += 1;
    let body_result = run_block(env, &def.body);
    env.call_depth -= 1;
    let return_value = env.returning.take().unwrap_or(Value::NIL);
    env.returning = saved_returning;
    for (name, prev) in saved_params {
        match prev {
            Some(v) => env.set(name, v),
            None => env.remove(&name),
        }
    }
    body_result?;
    Ok(return_value)
}

fn instantiate(class: Rc<ClassDef>) -> Value {
    let mut fields: HashMap<String, TaggedValue> = HashMap::new();
    // Walk the parent chain, oldest first, so child overrides win.
    let mut chain: Vec<Rc<ClassDef>> = Vec::new();
    let mut cur = Some(class.clone());
    while let Some(c) = cur {
        chain.push(c.clone());
        cur = c.parent.clone();
    }
    for c in chain.iter().rev() {
        for (k, v) in &c.field_defaults {
            fields.insert(k.clone(), v.clone());
        }
    }
    Value::from_instance(Rc::new(RefCell::new(Instance {
        class,
        fields,
        current_state: None,
        every_timers: Vec::new(),
        every_intervals_secs: Vec::new(),
        despawned: false,
        fiber_frames: Vec::new(),
        entry_wait_remaining: 0.0,
        predicate_last_values: Vec::new(),
    })))
}

fn find_method(class: &ClassDef, name: &str) -> Option<Rc<MethodDef>> {
    if let Some(m) = class.methods.get(name) {
        return Some(m.clone());
    }
    class.parent.as_ref().and_then(|p| find_method(p, name))
}

fn call_method(
    env: &mut Env,
    recv: Value,
    method: &MethodDef,
    args: &[Value],
    kwargs: &[(String, Value)],
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    let bound = if kwargs.is_empty() {
        if args.len() != method.params.len() {
            return Err(RuntimeError {
                line,
                col,
                message: format!(
                    "method expected {} arguments, got {}",
                    method.params.len(),
                    args.len()
                ),
                help: None,
            });
        }
        args.to_vec()
    } else {
        let param_refs: Vec<&str> = method.params.iter().map(|s| s.as_str()).collect();
        bind_kwargs(
            &param_refs,
            "method",
            args.to_vec(),
            kwargs.to_vec(),
            line,
            col,
        )?
    };
    let args = &bound;
    let saved_self = env.self_value.replace(recv);
    let saved_returning = env.returning.take();
    let saved_params: Vec<(String, Option<Value>)> = method
        .params
        .iter()
        .map(|p| (p.clone(), env.get(p)))
        .collect();
    for (param, arg) in method.params.iter().zip(args.iter()) {
        env.set(param.clone(), arg.clone());
    }
    env.call_depth += 1;
    let body_result = run_block(env, &method.body);
    env.call_depth -= 1;
    let return_value = env.returning.take().unwrap_or(Value::NIL);
    env.returning = saved_returning;
    env.self_value = saved_self;
    for (name, prev) in saved_params {
        match prev {
            Some(v) => env.set(name, v),
            None => env.remove(&name),
        }
    }
    body_result?;
    Ok(return_value)
}

fn run_while(env: &mut Env, cond: &Expr, body: &[Stmt]) -> Result<(), RuntimeError> {
    loop {
        let v = eval_expr(env, cond)?;
        if !is_truthy(&v) {
            break;
        }
        run_block(env, body)?;
        if env.returning.is_some() {
            break;
        }
        if env.breaking {
            env.breaking = false;
            break;
        }
        if env.continuing {
            env.continuing = false;
        }
    }
    Ok(())
}

fn run_for(
    env: &mut Env,
    var: &str,
    iter: &Expr,
    body: &[Stmt],
    line: u32,
    col: u32,
) -> Result<(), RuntimeError> {
    let iter_val = eval_expr(env, iter)?;
    let saved = env.get(var);
    let result = match iter_val.to_legacy() {
        LegacyValue::Range { start: start, end: end, exclusive: exclusive } => {
            let limit = if exclusive { end } else { end + 1 };
            run_for_iter(env, var, body, (start..limit).map(Value::from_int))
        }
        LegacyValue::List(rc) => {
            let snapshot: Vec<Value> = rc.borrow().clone();
            run_for_iter(env, var, body, snapshot.into_iter())
        }
        LegacyValue::Tuple(elems) => {
            let snapshot: Vec<Value> = elems.iter().cloned().collect();
            run_for_iter(env, var, body, snapshot.into_iter())
        }
        other => Err(RuntimeError {
            line,
            col,
            message: format!(
                "for-loop iterable must be a range, list, or tuple, got {}",
                other.type_name()
            ),
            help: None,
        }),
    };
    match saved {
        Some(v) => env.set(var.to_string(), v),
        None => env.remove(var),
    }
    result
}

fn run_for_iter<I: Iterator<Item = Value>>(
    env: &mut Env,
    var: &str,
    body: &[Stmt],
    items: I,
) -> Result<(), RuntimeError> {
    for item in items {
        env.set(var.to_string(), item);
        run_block(env, body)?;
        if env.returning.is_some() {
            break;
        }
        if env.breaking {
            env.breaking = false;
            break;
        }
        if env.continuing {
            env.continuing = false;
        }
    }
    Ok(())
}

fn eval_decl(
    env: &mut Env,
    kind: DeclKind,
    name: &str,
    parent: Option<&str>,
    members: &[DeclMember],
    line: u32,
    col: u32,
) -> Result<(), RuntimeError> {
    let parent_class = if let Some(p) = parent {
        match env.get(p).to_legacy() {
            Some(LegacyValue::Class(c)) => Some(c.clone()),
            Some(other) => {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!(
                        "cannot extend `{p}`: it is a {}, not a class",
                        other.type_name()
                    ),
                    help: None,
                });
            }
            None => {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!("parent `{p}` is not defined"),
                    help: Some(format!(
                        "declare `{p}` with `entity {p}:` or `item {p}:` before extending it"
                    )),
                });
            }
        }
    } else {
        None
    };

    let mut field_defaults = HashMap::new();
    let mut methods = HashMap::new();
    let mut states = HashMap::new();
    let mut initial_state: Option<String> = None;
    for member in members {
        match member {
            DeclMember::Field { name: fname, value, .. } => {
                let v = eval_expr(env, value)?;
                field_defaults.insert(fname.clone(), v);
            }
            DeclMember::Method {
                name: mname,
                params,
                body,
                ..
            } => {
                // Annotations don't affect runtime semantics; the
                // tree-walker still binds bare-name params. Strict
                // mode (Phase 6 session 4) consumes the annotations
                // statically in `infer.rs`.
                let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
                methods.insert(
                    mname.clone(),
                    Rc::new(MethodDef {
                        params: param_names,
                        body: body.clone(),
                    }),
                );
            }
            DeclMember::InitialState { name: sname, .. } => {
                initial_state = Some(sname.clone());
            }
            DeclMember::State { name: sname, members: smembers, .. } => {
                let mut on_entry = Vec::new();
                let mut every_clocks = Vec::new();
                let mut on_render: Option<Vec<Stmt>> = None;
                let mut on_key_press: HashMap<String, Vec<Stmt>> = HashMap::new();
                let mut on_update: Option<OnUpdateHandler> = None;
                let mut on_predicates: Vec<crate::value::PredicateHandlerDef> = Vec::new();
                for sm in smembers {
                    match sm {
                        StateMember::Stmt(stmt) => on_entry.push(stmt.clone()),
                        StateMember::Every { interval, body, .. } => {
                            every_clocks.push(EveryClockDef {
                                interval: interval.clone(),
                                body: body.clone(),
                            });
                        }
                        StateMember::OnRender { body, .. } => {
                            on_render = Some(body.clone());
                        }
                        StateMember::OnKeyPress { key, body, .. } => {
                            on_key_press.insert(key.clone(), body.clone());
                        }
                        StateMember::OnUpdate { param, body, .. } => {
                            on_update = Some(OnUpdateHandler {
                                param: param.clone(),
                                body: body.clone(),
                            });
                        }
                        StateMember::OnPredicate { predicate, body, .. } => {
                            on_predicates.push(crate::value::PredicateHandlerDef {
                                predicate: predicate.clone(),
                                body: body.clone(),
                            });
                        }
                    }
                }
                states.insert(
                    sname.clone(),
                    Rc::new(StateDef {
                        name: sname.clone(),
                        on_entry,
                        every_clocks,
                        on_render,
                        on_key_press,
                        on_update,
                        on_predicates,
                    }),
                );
            }
        }
    }

    let class = Rc::new(ClassDef {
        kind: kind.as_str(),
        name: name.to_string(),
        parent: parent_class,
        field_defaults,
        methods,
        states,
        initial_state,
    });
    env.set(name.to_string(), Value::from_class(class.clone()));

    // Scenes auto-instantiate at declaration time and become the active
    // scene. There's only one active scene per program in v0.1.
    if matches!(kind, DeclKind::Scene) {
        let inst = match instantiate(class.clone()).to_legacy() {
            LegacyValue::Instance(rc) => rc,
            _ => unreachable!("instantiate always returns Instance"),
        };
        env.active_scene = Some(inst.clone());
        if let Some(start) = class.initial_state.clone() {
            enter_state(env, &inst, &start)?;
        }
    }
    Ok(())
}

fn eval_binary(
    env: &mut Env,
    op: BinOp,
    left: &Expr,
    right: &Expr,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    if matches!(op, BinOp::And) {
        let l = eval_expr(env, left)?;
        return if is_truthy(&l) { eval_expr(env, right) } else { Ok(l) };
    }
    if matches!(op, BinOp::Or) {
        let l = eval_expr(env, left)?;
        return if is_truthy(&l) { Ok(l) } else { eval_expr(env, right) };
    }
    let l = eval_expr(env, left)?;
    let r = eval_expr(env, right)?;
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => apply_arith(op, &l, &r, line, col),
        BinOp::Eq => Ok(Value::from_bool(values_equal(&l, &r))),
        BinOp::Neq => Ok(Value::from_bool(!values_equal(&l, &r))),
        BinOp::Lt => cmp_int(&l, &r, |a, b| a < b, |a, b| a < b, "<", line, col),
        BinOp::Gt => cmp_int(&l, &r, |a, b| a > b, |a, b| a > b, ">", line, col),
        BinOp::Lte => cmp_int(&l, &r, |a, b| a <= b, |a, b| a <= b, "<=", line, col),
        BinOp::Gte => cmp_int(&l, &r, |a, b| a >= b, |a, b| a >= b, ">=", line, col),
        BinOp::In => Ok(Value::from_bool(value_in(&l, &r, line, col)?)),
        BinOp::NotIn => Ok(Value::from_bool(!value_in(&l, &r, line, col)?)),
        BinOp::And | BinOp::Or => unreachable!("handled above"),
    }
}

fn value_in(
    needle: &Value,
    haystack: &Value,
    line: u32,
    col: u32,
) -> Result<bool, RuntimeError> {
    match haystack.to_legacy() {
        LegacyValue::List(rc) => Ok(rc.borrow().iter().any(|v| values_equal(v, needle))),
        LegacyValue::Tuple(elems) => Ok(elems.iter().any(|v| values_equal(v, needle))),
        LegacyValue::Range { start: start, end: end, exclusive: exclusive } => match needle.to_legacy() {
            LegacyValue::Int(n) => {
                let upper = if exclusive { end } else { end + 1 };
                Ok(n >= start && n < upper)
            }
            _ => Ok(false),
        },
        LegacyValue::Str(s) => match needle.to_legacy() {
            LegacyValue::Str(sub) => Ok(s.contains(sub.as_ref())),
            _ => Ok(false),
        },
        other => Err(RuntimeError {
            line,
            col,
            message: format!(
                "`in` expects a list, tuple, range, or string, got {}",
                other.type_name()
            ),
            help: None,
        }),
    }
}

fn apply_arith(
    op: BinOp,
    l: &Value,
    r: &Value,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    let op_str = match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        _ => unreachable!(),
    };
    // String concatenation via `+`.
    if matches!(op, BinOp::Add) {
        if let (LegacyValue::Str(a), LegacyValue::Str(b)) = (l, r).to_legacy() {
            let mut s = String::with_capacity(a.len() + b.len());
            s.push_str(a.as_str());
            s.push_str(b.as_str());
            return Ok(Value::from_string(s));
        }
    }
    // Tuple arithmetic — element-wise add/sub between same-length tuples
    // (Snake's `snake[0] + direction` shape) and tuple * scalar (Snake's
    // `cell * cell_size`).
    if let (LegacyValue::Tuple(a), LegacyValue::Tuple(b)) = (l, r).to_legacy() {
        if matches!(op, BinOp::Add | BinOp::Sub) {
            if a.len() != b.len() {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!(
                        "tuple {} requires equal-length operands ({} vs {})",
                        op_str,
                        a.len(),
                        b.len()
                    ),
                    help: None,
                });
            }
            let mut out_elems = Vec::with_capacity(a.len());
            for (x, y) in a.iter().zip(b.iter()) {
                out_elems.push(apply_arith(op, x, y, line, col)?);
            }
            return Ok(Value::from_tuple(Rc::new(out_elems)));
        }
    }
    if let LegacyValue::Tuple(elems) = l.to_legacy() {
        if matches!(op, BinOp::Mul | BinOp::Div) && is_scalar(r) {
            let mut out_elems = Vec::with_capacity(elems.len());
            for x in elems.iter() {
                out_elems.push(apply_arith(op, x, r, line, col)?);
            }
            return Ok(Value::from_tuple(Rc::new(out_elems)));
        }
    }
    if let LegacyValue::Tuple(elems) = r.to_legacy() {
        if matches!(op, BinOp::Mul) && is_scalar(l) {
            let mut out_elems = Vec::with_capacity(elems.len());
            for y in elems.iter() {
                out_elems.push(apply_arith(op, l, y, line, col)?);
            }
            return Ok(Value::from_tuple(Rc::new(out_elems)));
        }
    }
    let pair = match (l, r).to_legacy() {
        (LegacyValue::Int(a), LegacyValue::Int(b)) => NumPair::Ints(a, b),
        (LegacyValue::Float(a), LegacyValue::Float(b)) => NumPair::Floats(a, b),
        (LegacyValue::Int(a), LegacyValue::Float(b)) => NumPair::Floats(a as f64, b),
        (LegacyValue::Float(a), LegacyValue::Int(b)) => NumPair::Floats(a, b as f64),
        _ => {
            return Err(RuntimeError {
                line,
                col,
                message: format!(
                    "operator '{op_str}' is not defined on {} and {}",
                    l.type_name(),
                    r.type_name()
                ),
                help: None,
            })
        }
    };
    match (op, pair) {
        (BinOp::Add, NumPair::Ints(a, b)) => Ok(Value::from_int(a + b)),
        (BinOp::Sub, NumPair::Ints(a, b)) => Ok(Value::from_int(a - b)),
        (BinOp::Mul, NumPair::Ints(a, b)) => Ok(Value::from_int(a * b)),
        (BinOp::Div, NumPair::Ints(_, 0)) => Err(RuntimeError {
            line,
            col,
            message: "division by zero".to_string(),
            help: Some("guard the divisor with `if b != 0:` before dividing".to_string()),
        }),
        (BinOp::Div, NumPair::Ints(a, b)) => Ok(Value::from_int(a / b)),
        (BinOp::Add, NumPair::Floats(a, b)) => Ok(Value::from_float(a + b)),
        (BinOp::Sub, NumPair::Floats(a, b)) => Ok(Value::from_float(a - b)),
        (BinOp::Mul, NumPair::Floats(a, b)) => Ok(Value::from_float(a * b)),
        (BinOp::Div, NumPair::Floats(a, b)) => Ok(Value::from_float(a / b)),
        _ => unreachable!(),
    }
}

enum NumPair {
    Ints(i64, i64),
    Floats(f64, f64),
}

fn is_scalar(v: &Value) -> bool {
    matches!(v.to_legacy(), LegacyValue::Int(_) | LegacyValue::Float(_))
}

fn cmp_int(
    l: &Value,
    r: &Value,
    int_cmp: fn(i64, i64) -> bool,
    float_cmp: fn(f64, f64) -> bool,
    op_str: &str,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    match (l, r).to_legacy() {
        (LegacyValue::Int(a), LegacyValue::Int(b)) => Ok(Value::from_bool(int_cmp(a, b))),
        (LegacyValue::Float(a), LegacyValue::Float(b)) => Ok(Value::from_bool(float_cmp(a, b))),
        (LegacyValue::Int(a), LegacyValue::Float(b)) => Ok(Value::from_bool(float_cmp(a as f64, b))),
        (LegacyValue::Float(a), LegacyValue::Int(b)) => Ok(Value::from_bool(float_cmp(a, b as f64))),
        _ => Err(RuntimeError {
            line,
            col,
            message: format!(
                "operator '{op_str}' is not defined on {} and {}",
                l.type_name(),
                r.type_name()
            ),
            help: None,
        }),
    }
}

fn values_equal(l: &Value, r: &Value) -> bool {
    match (l, r).to_legacy() {
        (LegacyValue::Nil, LegacyValue::Nil) => true,
        (LegacyValue::Bool(a), LegacyValue::Bool(b)) => a == b,
        (LegacyValue::Int(a), LegacyValue::Int(b)) => a == b,
        (LegacyValue::Float(a), LegacyValue::Float(b)) => a == b,
        (LegacyValue::Int(a), LegacyValue::Float(b)) => (a as f64) == b,
        (LegacyValue::Float(a), LegacyValue::Int(b)) => a == (b as f64),
        (LegacyValue::Percent(a), LegacyValue::Percent(b)) => a == b,
        (
            LegacyValue::Quantity { value: a, unit: u1 },
            LegacyValue::Quantity { value: b, unit: u2 },
        ) => a == b && u1 == u2,
        (
            LegacyValue::Range { start: s1, end: e1, exclusive: x1 },
            LegacyValue::Range { start: s2, end: e2, exclusive: x2 },
        ) => s1 == s2 && e1 == e2 && x1 == x2,
        (LegacyValue::Str(a), LegacyValue::Str(b)) => a == b,
        (LegacyValue::Tuple(a), LegacyValue::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| values_equal(x, y))
        }
        _ => false,
    }
}

fn is_truthy(v: &Value) -> bool {
    v.is_truthy()
}
