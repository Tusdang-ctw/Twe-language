use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{
    AssignOp, AssignTarget, BinOp, DeclKind, DeclMember, Expr, Program, StateMember, Stmt, UnOp,
};
use crate::stdlib;
use crate::value::{
    ClassDef, Env, EveryClockDef, FunctionDef, Instance, MethodDef, Object, OnUpdateHandler,
    RuntimeError, StateDef, Value,
};

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
        env.set(handler.param.clone(), Value::Float(dt));
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
            Value::Instance(entity),
            &method,
            &[Value::Float(dt)],
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
        let count = match inst.fields.get("count") {
            Some(Value::Int(n)) if *n >= 0 => *n as usize,
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
        let lifetime = match inst.fields.get("lifetime") {
            Some(Value::Float(f)) => *f,
            Some(Value::Int(n)) => *n as f64,
            Some(Value::Quantity { value, .. }) => *value,
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
        .unwrap_or_else(|| Value::Tuple(Rc::new(vec![Value::Float(0.0), Value::Float(0.0)])));
    for _ in 0..count {
        let p = make_particle(&initial_pos, lifetime);
        if let Some(method) = on_spawn.clone() {
            call_method(
                env,
                Value::Instance(emitter.clone()),
                &method,
                std::slice::from_ref(&p),
                &[],
                line,
                col,
            )?;
        }
        particles.push(p);
    }
    emitter.borrow_mut().fields.insert(
        "__particles".to_string(),
        Value::List(Rc::new(RefCell::new(particles))),
    );
    Ok(())
}

fn make_particle(initial_pos: &Value, lifetime: f64) -> Value {
    let mut fields = HashMap::new();
    fields.insert("pos".to_string(), initial_pos.clone());
    fields.insert(
        "velocity".to_string(),
        Value::Tuple(Rc::new(vec![Value::Float(0.0), Value::Float(0.0)])),
    );
    fields.insert(
        "color".to_string(),
        Value::Tuple(Rc::new(vec![
            Value::Float(1.0),
            Value::Float(1.0),
            Value::Float(1.0),
            Value::Float(1.0),
        ])),
    );
    fields.insert("size".to_string(), Value::Float(4.0));
    fields.insert("age".to_string(), Value::Float(0.0));
    fields.insert("age_ratio".to_string(), Value::Float(0.0));
    fields.insert("lifetime".to_string(), Value::Float(lifetime));
    Value::Object(Rc::new(RefCell::new(Object {
        fields,
        kind: "particle",
    })))
}

fn tick_particle_emitter(
    env: &mut Env,
    emitter: &Rc<RefCell<Instance>>,
    class: &Rc<ClassDef>,
    dt: f64,
) -> Result<(), RuntimeError> {
    let on_update = find_method(class, "on_update");
    let particles = match emitter.borrow().fields.get("__particles").cloned() {
        Some(Value::List(rc)) => rc,
        _ => return Ok(()),
    };
    let snapshot: Vec<Value> = particles.borrow().clone();
    for p in &snapshot {
        if let Some(method) = on_update.clone() {
            call_method(
                env,
                Value::Instance(emitter.clone()),
                &method,
                &[p.clone(), Value::Float(dt)],
                &[],
                0,
                0,
            )?;
        }
        if let Value::Object(rc) = p {
            let mut o = rc.borrow_mut();
            let age = match o.fields.get("age") {
                Some(Value::Float(a)) => *a + dt,
                Some(Value::Int(a)) => *a as f64 + dt,
                _ => dt,
            };
            let lifetime = match o.fields.get("lifetime") {
                Some(Value::Float(l)) => *l,
                _ => 1.0,
            };
            o.fields.insert("age".to_string(), Value::Float(age));
            let ratio = if lifetime > 0.0 {
                (age / lifetime).clamp(0.0, 1.0)
            } else {
                1.0
            };
            o.fields
                .insert("age_ratio".to_string(), Value::Float(ratio));
        }
    }
    // Drop dead particles.
    particles.borrow_mut().retain(|p| match p {
        Value::Object(rc) => match rc.borrow().fields.get("age") {
            Some(Value::Float(age)) => match rc.borrow().fields.get("lifetime") {
                Some(Value::Float(lt)) => *age < *lt,
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
        return call_method(env, Value::Instance(emitter.clone()), &method, &[], &[], 0, 0)
            .map(|_| ());
    }
    let particles = match emitter.borrow().fields.get("__particles").cloned() {
        Some(Value::List(rc)) => rc,
        _ => return Ok(()),
    };
    if !env.in_render {
        return Ok(());
    }
    for p in particles.borrow().iter() {
        if let Value::Object(rc) = p {
            let o = rc.borrow();
            let (px, py) = match o.fields.get("pos") {
                Some(Value::Tuple(elems)) if elems.len() >= 2 => (
                    number_or_zero(&elems[0]),
                    number_or_zero(&elems[1]),
                ),
                _ => (0.0, 0.0),
            };
            let radius = match o.fields.get("size") {
                Some(Value::Float(f)) => *f as f32,
                Some(Value::Int(n)) => *n as f32,
                _ => 4.0,
            };
            let color = match o.fields.get("color") {
                Some(Value::Tuple(elems)) if elems.len() >= 3 => {
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
    match v {
        Value::Int(n) => *n as f64,
        Value::Float(f) => *f,
        Value::Quantity { value, .. } => *value,
        _ => 0.0,
    }
}

fn update_time_ambient(env: &mut Env, dt: f64) {
    if let Some(Value::Object(rc)) = env.get("time").cloned() {
        rc.borrow_mut()
            .fields
            .insert("dt".to_string(), Value::Float(dt));
    }
}

/// Look at `env.key_press` (an Object whose fields are bool flags set
/// each frame by the host) and fire the active scene's matching
/// on_key_press handlers.
fn dispatch_key_press(
    env: &mut Env,
    scene: &Rc<RefCell<Instance>>,
) -> Result<(), RuntimeError> {
    let pressed = match env.get("key_press").cloned() {
        Some(Value::Object(rc)) => {
            let o = rc.borrow();
            o.fields
                .iter()
                .filter_map(|(k, v)| {
                    if matches!(v, Value::Bool(true)) {
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
    let prev_self = env.self_value.replace(Value::Instance(scene.clone()));
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
            let prev_self = env.self_value.replace(Value::Instance(scene));
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
        call_method(env, Value::Instance(entity), &method, &[], &[], 0, 0)?;
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
    let prev_self = env.self_value.replace(Value::Instance(scene.clone()));
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
        env.set(handler.param.clone(), Value::Float(dt));
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
        inst.class
            .states
            .get(state_name)
            .cloned()
            .ok_or_else(|| RuntimeError {
                line: 0,
                col: 0,
                message: format!("no state named '{state_name}'"),
                help: Some(
                    "transitions must target a `state <name>:` declared in the same scene"
                        .to_string(),
                ),
            })?
    };
    // Replace current_state, reset timers / intervals.
    {
        let mut inst = scene.borrow_mut();
        inst.current_state = Some(state.name.clone());
        inst.every_timers = vec![0.0; state.every_clocks.len()];
        inst.every_intervals_secs.clear();
    }
    // Resolve each every-clock interval (in seconds) by evaluating the
    // interval expression with self bound to the scene instance.
    let prev_self = env.self_value.replace(Value::Instance(scene.clone()));
    let mut intervals = Vec::with_capacity(state.every_clocks.len());
    for clock in &state.every_clocks {
        let v = eval_expr(env, &clock.interval)?;
        intervals.push(quantity_to_seconds(&v, clock.interval.line(), clock.interval.col())?);
    }
    scene.borrow_mut().every_intervals_secs = intervals;
    // Run the on-entry body.
    run_block(env, &state.on_entry)?;
    // A transition during on_entry is followed immediately.
    if let Some(next) = env.transitioning.take() {
        env.self_value = prev_self;
        return enter_state(env, scene, &next);
    }
    env.self_value = prev_self;
    Ok(())
}

/// Resolve a bare name. Scope chain: the active `self` instance's fields
/// shadow env globals, so `ticks += 1` inside a scene state mutates
/// `self.ticks`. Without this, scene fields would only be reachable via
/// explicit `self.x` syntax — verbose and unusual.
fn lookup_name(env: &Env, name: &str) -> Option<Value> {
    if let Some(Value::Instance(rc)) = &env.self_value {
        if let Some(v) = rc.borrow().fields.get(name) {
            return Some(v.clone());
        }
    }
    env.get(name).cloned()
}

fn quantity_to_seconds(v: &Value, line: u32, col: u32) -> Result<f64, RuntimeError> {
    match v {
        Value::Quantity { value, unit } => match unit.as_str() {
            "s" => Ok(*value),
            "ms" => Ok(*value / 1000.0),
            "min" => Ok(*value * 60.0),
            "h" => Ok(*value * 3600.0),
            other => Err(RuntimeError {
                line,
                col,
                message: format!(
                    "every <duration> needs a time unit (s, ms, min, h), got '{other}'"
                ),
                help: None,
            }),
        },
        Value::Float(f) => Ok(*f),
        Value::Int(n) => Ok(*n as f64),
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
            env.set(
                name.clone(),
                Value::Function(Rc::new(FunctionDef {
                    name: name.clone(),
                    params: params.clone(),
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
                    help: None,
                });
            }
            let v = match value {
                Some(e) => eval_expr(env, e)?,
                None => Value::Nil,
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
                    help: None,
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
                    help: None,
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
            let class_val = env.get(class).cloned().ok_or_else(|| RuntimeError {
                line: *line,
                col: *col,
                message: format!("class '{class}' is not defined"),
                help: Some(format!("declare it with `entity {class}:` first")),
            })?;
            let class_rc = match class_val {
                Value::Class(c) => c,
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
            if let (Some(at_value), Value::Instance(rc)) = (&at_value, &inst_val) {
                rc.borrow_mut().fields.insert("pos".to_string(), at_value.clone());
            }
            if let Value::Instance(rc) = &inst_val {
                if class_rc.kind == "particles" {
                    seed_particle_emitter(env, rc, at_value.as_ref(), *line, *col)?;
                }
                env.active_entities.push(rc.clone());
            }
            Ok(())
        }
        Stmt::Despawn { target, line, col } => {
            let v = eval_expr(env, target)?;
            match v {
                Value::Instance(rc) => {
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
        Stmt::OnUpdate { param, body, .. } => {
            env.on_update = Some(OnUpdateHandler {
                param: param.clone(),
                body: body.clone(),
            });
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
                if let Some(Value::Instance(rc)) = &env.self_value {
                    let mut inst = rc.borrow_mut();
                    if inst.fields.contains_key(name) {
                        inst.fields.insert(name.clone(), new_value);
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
            if let Some(Value::Instance(rc)) = &env.self_value {
                let mut inst = rc.borrow_mut();
                if inst.fields.contains_key(name) {
                    inst.fields.insert(name.clone(), combined);
                    return Ok(());
                }
            }
            env.set(name.clone(), combined);
            Ok(())
        }
        AssignTarget::Field { object, name } => {
            let obj_val = eval_expr(env, object)?;
            match obj_val {
                Value::Object(rc) => {
                    let final_value = if matches!(op, AssignOp::Set) {
                        new_value
                    } else {
                        let current = rc.borrow().fields.get(name).cloned().ok_or_else(|| {
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
                        if let Value::Tuple(elems) = &final_value {
                            if elems.len() >= 2 {
                                let mut o = rc.borrow_mut();
                                o.fields.insert("x".to_string(), elems[0].clone());
                                o.fields.insert("y".to_string(), elems[1].clone());
                            }
                        }
                    }
                    rc.borrow_mut().fields.insert(name.clone(), final_value);
                    if name == "x" || name == "y" {
                        refresh_pos(&rc);
                    }
                    Ok(())
                }
                Value::Instance(rc) => {
                    let final_value = if matches!(op, AssignOp::Set) {
                        new_value
                    } else {
                        let current = rc.borrow().fields.get(name).cloned().ok_or_else(|| {
                            RuntimeError {
                                line,
                                col,
                                message: format!(
                                    "field '{name}' is not defined on instance of {}",
                                    rc.borrow().class.name
                                ),
                                help: None,
                            }
                        })?;
                        compound(op, &current, &new_value, line, col)?
                    };
                    rc.borrow_mut().fields.insert(name.clone(), final_value);
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
            o.fields.get("x").cloned().unwrap_or(Value::Nil),
            o.fields.get("y").cloned().unwrap_or(Value::Nil),
        )
    };
    rc.borrow_mut()
        .fields
        .insert("pos".to_string(), Value::Tuple(Rc::new(vec![x, y])));
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
        Expr::Str { value, .. } => Ok(Value::Str(Rc::new(value.clone()))),
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
            Ok(Value::Str(Rc::new(out)))
        }
        Expr::Int { value, .. } => Ok(Value::Int(*value)),
        Expr::Float { value, .. } => Ok(Value::Float(*value)),
        Expr::Bool { value, .. } => Ok(Value::Bool(*value)),
        Expr::Percent { value, .. } => Ok(Value::Percent(*value)),
        Expr::Quantity { value, unit, .. } => Ok(Value::Quantity {
            value: *value,
            unit: Rc::new(unit.clone()),
        }),
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
            help: None,
        }),
        Expr::Tuple { elems, .. } => {
            let mut vals = Vec::with_capacity(elems.len());
            for e in elems {
                vals.push(eval_expr(env, e)?);
            }
            Ok(Value::Tuple(Rc::new(vals)))
        }
        Expr::List { elems, .. } => {
            let mut vals = Vec::with_capacity(elems.len());
            for e in elems {
                vals.push(eval_expr(env, e)?);
            }
            Ok(Value::List(Rc::new(RefCell::new(vals))))
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
            match (&s, &e) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Range {
                    start: *a,
                    end: *b,
                    exclusive: *exclusive,
                }),
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
            match (op, &v) {
                (UnOp::Neg, Value::Int(n)) => Ok(Value::Int(-n)),
                (UnOp::Neg, Value::Float(x)) => Ok(Value::Float(-x)),
                (UnOp::Neg, _) => Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: format!("cannot negate value of type {}", v.type_name()),
                    help: Some("`-` is defined on int and float".to_string()),
                }),
                (UnOp::Not, _) => Ok(Value::Bool(!is_truthy(&v))),
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
    match (obj, idx) {
        (Value::List(rc), Value::Int(i)) => {
            let v = rc.borrow();
            let len = v.len() as i64;
            let actual = if *i < 0 { *i + len } else { *i };
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
        (Value::Tuple(elems), Value::Int(i)) => {
            let len = elems.len() as i64;
            let actual = if *i < 0 { *i + len } else { *i };
            if actual < 0 || actual >= len {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!("tuple index {i} out of bounds (length {len})"),
                    help: None,
                });
            }
            Ok(elems[actual as usize].clone())
        }
        (Value::List(_) | Value::Tuple(_), other) => Err(RuntimeError {
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
    match obj {
        Value::Tuple(elems) => match name {
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
        Value::List(rc) => match name {
            "length" => Ok(Value::Int(rc.borrow().len() as i64)),
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
        Value::Object(rc) => rc.borrow().fields.get(name).cloned().ok_or_else(|| {
            RuntimeError {
                line,
                col,
                message: format!("field '{name}' is not defined on this object"),
                help: Some(format!("set it first with `obj.{name} = ...`")),
            }
        }),
        Value::Instance(rc) => {
            let inst = rc.borrow();
            if let Some(v) = inst.fields.get(name) {
                return Ok(v.clone());
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
        if let Some(Value::Instance(rc)) = env.self_value.clone() {
            let class = rc.borrow().class.clone();
            if let Some(method) = find_method(&class, name) {
                let arg_vals = eval_args(env, args)?;
                let kwarg_vals = eval_kwargs(env, kwargs)?;
                return call_method(
                    env,
                    Value::Instance(rc),
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
        if let Value::List(rc) = &recv {
            if !kwargs.is_empty() {
                return Err(no_kwargs_error(&format!("list.{name}"), line, col));
            }
            if let Some(v) = list_method_call(env, rc, name, args, line, col)? {
                return Ok(v);
            }
        }
        if let Value::Range { start, end, exclusive } = &recv {
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
        if let Value::Instance(rc) = &recv {
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
            Ok(Some(Value::Nil))
        }
        "prepend" => {
            arity_check(1)?;
            let v = eval_expr(env, &args[0])?;
            rc.borrow_mut().insert(0, v);
            Ok(Some(Value::Nil))
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
            Ok(Some(Value::Bool(found)))
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
            Ok(Some(Value::Int(start + (n % span) as i64)))
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
            let result = match v {
                Value::Int(n) => n >= start && n < upper,
                _ => false,
            };
            Ok(Some(Value::Bool(result)))
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
    match f {
        Value::Builtin { name, params, func } => {
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
        Value::Function(def) => call_function(env, &def, args, kwargs, line, col),
        Value::Class(class) => {
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
        .map(|p| (p.clone(), env.get(p).cloned()))
        .collect();
    for (param, arg) in def.params.iter().zip(args.iter()) {
        env.set(param.clone(), arg.clone());
    }
    env.call_depth += 1;
    let body_result = run_block(env, &def.body);
    env.call_depth -= 1;
    let return_value = env.returning.take().unwrap_or(Value::Nil);
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
    let mut fields = HashMap::new();
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
    Value::Instance(Rc::new(RefCell::new(Instance {
        class,
        fields,
        current_state: None,
        every_timers: Vec::new(),
        every_intervals_secs: Vec::new(),
        despawned: false,
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
        .map(|p| (p.clone(), env.get(p).cloned()))
        .collect();
    for (param, arg) in method.params.iter().zip(args.iter()) {
        env.set(param.clone(), arg.clone());
    }
    env.call_depth += 1;
    let body_result = run_block(env, &method.body);
    env.call_depth -= 1;
    let return_value = env.returning.take().unwrap_or(Value::Nil);
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
    let saved = env.get(var).cloned();
    let result = match iter_val {
        Value::Range { start, end, exclusive } => {
            let limit = if exclusive { end } else { end + 1 };
            run_for_iter(env, var, body, (start..limit).map(Value::Int))
        }
        Value::List(rc) => {
            let snapshot: Vec<Value> = rc.borrow().clone();
            run_for_iter(env, var, body, snapshot.into_iter())
        }
        Value::Tuple(elems) => {
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
        match env.get(p) {
            Some(Value::Class(c)) => Some(c.clone()),
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
                methods.insert(
                    mname.clone(),
                    Rc::new(MethodDef {
                        params: params.clone(),
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
    env.set(name.to_string(), Value::Class(class.clone()));

    // Scenes auto-instantiate at declaration time and become the active
    // scene. There's only one active scene per program in v0.1.
    if matches!(kind, DeclKind::Scene) {
        let inst = match instantiate(class.clone()) {
            Value::Instance(rc) => rc,
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
        BinOp::Eq => Ok(Value::Bool(values_equal(&l, &r))),
        BinOp::Neq => Ok(Value::Bool(!values_equal(&l, &r))),
        BinOp::Lt => cmp_int(&l, &r, |a, b| a < b, |a, b| a < b, "<", line, col),
        BinOp::Gt => cmp_int(&l, &r, |a, b| a > b, |a, b| a > b, ">", line, col),
        BinOp::Lte => cmp_int(&l, &r, |a, b| a <= b, |a, b| a <= b, "<=", line, col),
        BinOp::Gte => cmp_int(&l, &r, |a, b| a >= b, |a, b| a >= b, ">=", line, col),
        BinOp::In => Ok(Value::Bool(value_in(&l, &r, line, col)?)),
        BinOp::NotIn => Ok(Value::Bool(!value_in(&l, &r, line, col)?)),
        BinOp::And | BinOp::Or => unreachable!("handled above"),
    }
}

fn value_in(
    needle: &Value,
    haystack: &Value,
    line: u32,
    col: u32,
) -> Result<bool, RuntimeError> {
    match haystack {
        Value::List(rc) => Ok(rc.borrow().iter().any(|v| values_equal(v, needle))),
        Value::Tuple(elems) => Ok(elems.iter().any(|v| values_equal(v, needle))),
        Value::Range { start, end, exclusive } => match needle {
            Value::Int(n) => {
                let upper = if *exclusive { *end } else { *end + 1 };
                Ok(*n >= *start && *n < upper)
            }
            _ => Ok(false),
        },
        Value::Str(s) => match needle {
            Value::Str(sub) => Ok(s.contains(sub.as_ref())),
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
        if let (Value::Str(a), Value::Str(b)) = (l, r) {
            let mut s = String::with_capacity(a.len() + b.len());
            s.push_str(a);
            s.push_str(b);
            return Ok(Value::Str(Rc::new(s)));
        }
    }
    // Tuple arithmetic — element-wise add/sub between same-length tuples
    // (Snake's `snake[0] + direction` shape) and tuple * scalar (Snake's
    // `cell * cell_size`).
    if let (Value::Tuple(a), Value::Tuple(b)) = (l, r) {
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
            return Ok(Value::Tuple(Rc::new(out_elems)));
        }
    }
    if let Value::Tuple(elems) = l {
        if matches!(op, BinOp::Mul | BinOp::Div) && is_scalar(r) {
            let mut out_elems = Vec::with_capacity(elems.len());
            for x in elems.iter() {
                out_elems.push(apply_arith(op, x, r, line, col)?);
            }
            return Ok(Value::Tuple(Rc::new(out_elems)));
        }
    }
    if let Value::Tuple(elems) = r {
        if matches!(op, BinOp::Mul) && is_scalar(l) {
            let mut out_elems = Vec::with_capacity(elems.len());
            for y in elems.iter() {
                out_elems.push(apply_arith(op, l, y, line, col)?);
            }
            return Ok(Value::Tuple(Rc::new(out_elems)));
        }
    }
    let pair = match (l, r) {
        (Value::Int(a), Value::Int(b)) => NumPair::Ints(*a, *b),
        (Value::Float(a), Value::Float(b)) => NumPair::Floats(*a, *b),
        (Value::Int(a), Value::Float(b)) => NumPair::Floats(*a as f64, *b),
        (Value::Float(a), Value::Int(b)) => NumPair::Floats(*a, *b as f64),
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
        (BinOp::Add, NumPair::Ints(a, b)) => Ok(Value::Int(a + b)),
        (BinOp::Sub, NumPair::Ints(a, b)) => Ok(Value::Int(a - b)),
        (BinOp::Mul, NumPair::Ints(a, b)) => Ok(Value::Int(a * b)),
        (BinOp::Div, NumPair::Ints(_, 0)) => Err(RuntimeError {
            line,
            col,
            message: "division by zero".to_string(),
            help: Some("guard the divisor with `if b != 0:` before dividing".to_string()),
        }),
        (BinOp::Div, NumPair::Ints(a, b)) => Ok(Value::Int(a / b)),
        (BinOp::Add, NumPair::Floats(a, b)) => Ok(Value::Float(a + b)),
        (BinOp::Sub, NumPair::Floats(a, b)) => Ok(Value::Float(a - b)),
        (BinOp::Mul, NumPair::Floats(a, b)) => Ok(Value::Float(a * b)),
        (BinOp::Div, NumPair::Floats(a, b)) => Ok(Value::Float(a / b)),
        _ => unreachable!(),
    }
}

enum NumPair {
    Ints(i64, i64),
    Floats(f64, f64),
}

fn is_scalar(v: &Value) -> bool {
    matches!(v, Value::Int(_) | Value::Float(_))
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
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(int_cmp(*a, *b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(float_cmp(*a, *b))),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Bool(float_cmp(*a as f64, *b))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(float_cmp(*a, *b as f64))),
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
    match (l, r) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
        (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
        (Value::Percent(a), Value::Percent(b)) => a == b,
        (
            Value::Quantity { value: a, unit: u1 },
            Value::Quantity { value: b, unit: u2 },
        ) => a == b && u1 == u2,
        (
            Value::Range {
                start: s1,
                end: e1,
                exclusive: x1,
            },
            Value::Range {
                start: s2,
                end: e2,
                exclusive: x2,
            },
        ) => s1 == s2 && e1 == e2 && x1 == x2,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Tuple(a), Value::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| values_equal(x, y))
        }
        _ => false,
    }
}

fn is_truthy(v: &Value) -> bool {
    !matches!(v, Value::Bool(false))
}
