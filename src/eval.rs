use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{
    AssignOp, AssignTarget, BinOp, DeclKind, DeclMember, Expr, Program, StateMember, Stmt, UnOp,
};
use crate::stdlib;
use crate::tagged_value::TaggedValue;
use crate::value::{
    Branch, ClassDef, Env, EveryClockDef, Frame, FrameKind, FunctionDef, Instance, MethodDef,
    Object, OnUpdateHandler, PathEntry, RuntimeError, StateDef, Value,
};

/// Phase 29 session 1: fixed-timestep simulation rate. Glenn Fiedler
/// "Fix Your Timestep!" pattern — the play loop accumulates wall-clock
/// frame time and calls `tick_frame` zero-or-more times per render
/// frame at this rate, so simulation state advances deterministically
/// regardless of display refresh rate. 60 Hz is the rate every
/// shipped Twe example was already authored against.
pub const PHYSICS_DT: f64 = 1.0 / 60.0;

/// Cap on a single render frame's contribution to the accumulator.
/// Without this, a hot-reload pause or breakpoint would generate a
/// huge `frame_dt`, the accumulator would overflow, and the next
/// frame would tick the simulation thousands of times trying to
/// catch up — the "spiral of death." 0.25s is Glenn Fiedler's
/// recommended cap.
pub const MAX_FRAME_DT: f64 = 0.25;

/// Hard cap on substeps per render frame. Belt-and-suspenders for
/// `MAX_FRAME_DT`: if a single tick takes longer than `PHYSICS_DT`
/// to compute, the accumulator never drains and we'd still spiral.
/// 8 substeps per frame at 60 Hz = up to 8/60 ≈ 133ms of simulation
/// per render. Hitting the cap drops the residual accumulator.
pub const MAX_SUBSTEPS: u32 = 8;

pub fn run(program: &Program) -> Result<String, RuntimeError> {
    run_with_frames(program, 0, PHYSICS_DT)
}

pub fn run_with_frames(program: &Program, frames: u32, dt: f64) -> Result<String, RuntimeError> {
    let mut env = Env::new();
    stdlib::install(&mut env);
    headless(&mut env);
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
    // web3d-M1: names resolve lexically; scope errors (undefined names,
    // another function's locals, assignment to undeclared names) are
    // reported before anything runs, not when a branch finally executes.
    crate::resolve::check_before_run(program, env)?;
    run_block(env, &program.stmts)?;
    if env.returning.take().is_some() {
        // Top-level `return` is silently dropped.
    }
    Ok(())
}

/// Advance the active scene and any global on-update handler by `dt`
/// seconds. Side-effects (prints, field mutations, transitions) are
/// applied to `env`.
/// web3d-M7 session 16: mark `env` as running with no renderer (see
/// [`Env::particles_unseen`]).
pub fn headless(env: &mut Env) {
    env.gpu_particles = true;
    env.particles_unseen = true;
}

pub fn tick_frame(env: &mut Env, dt: f64) -> Result<(), RuntimeError> {
    let _profile = crate::profile::scope("tick");
    // web3d-M0: guaranteed per-tick safepoint. Nothing lives on the
    // Rust stack here, and entity `update` methods (call depth ≥ 1)
    // never collect, so without this a game whose work is all in
    // entity methods would only collect at scene-level statements.
    safepoint(env);
    env.sim_time += dt;
    update_time_ambient(env, dt);
    // v1.0.1 session 6: when paused, the top-level `on update()` is
    // never persistent — it isn't bound to any state, so the global
    // pause flag halts it. State-scoped logic (scene / entity
    // updates) flows through `tick_scene` / `tick_entities`, which
    // consult the persistent-state registry per-instance.
    let paused = crate::stdlib::is_paused();
    if !paused {
        if let Some(handler) = env.on_update.clone() {
            run_frame_body(
                env,
                vec![(Rc::from(handler.param.as_str()), Value::from_float(dt))],
                &handler.body,
            )?;
            if env.returning.take().is_some() {
                return Ok(());
            }
        }
    }
    if let Some(scene) = env.active_scene.clone() {
        // Key-press dispatch + state tick filter per-scene. A paused
        // game whose active scene is currently in a persistent state
        // (e.g. `pause_menu`) still receives input and runs its
        // update; non-persistent states no-op until pause clears.
        let scene_state_persistent = scene
            .borrow()
            .current_state
            .as_ref()
            .map(|s| crate::stdlib::is_persistent_state(s))
            .unwrap_or(false);
        if !paused || scene_state_persistent {
            dispatch_key_press(env, &scene)?;
            tick_scene(env, &scene, dt)?;
        }
    }
    tick_entities(env, dt)?;
    prune_despawned(env);
    // Phase 29 session 5: advance the audio simulation clock and
    // dispatch any scheduled one-shots whose deadline has passed.
    // Done last so a sound the script just queued can fire on the
    // same tick if its `when` is at or before the new clock value.
    crate::stdlib::tick_audio_schedule(dt);
    Ok(())
}

fn tick_entities(env: &mut Env, dt: f64) -> Result<(), RuntimeError> {
    // v1.0.1 session 6: when the global pause flag is set, only
    // entities whose current state is registered as persistent
    // continue updating. Entities with no explicit state (the
    // pre-state-machine default) freeze with the rest of the world.
    let paused = crate::stdlib::is_paused();
    // web3d-M3: consecutive entities are usually the same class; reuse
    // its `update` lookup instead of hashing the method name per entity.
    let mut last: Option<(*const ClassDef, Option<Rc<MethodDef>>)> = None;
    // web3d-M7 follow-up: walk only the entities with something to run
    // (100k static blocks cost ~1.9 ms a tick natively when every
    // entity was visited), by index rather than cloning the list;
    // entities spawned during the walk start next tick, as before.
    let count = env.tickable_entities.len();
    for index in 0..count {
        let idle = {
            let e = env.tickable_entities[index].borrow();
            if e.despawned {
                true
            } else if e.class.kind == "particles" || paused {
                false
            } else {
                let key = Rc::as_ptr(&e.class);
                match &last {
                    Some((k, m)) if *k == key => m.is_none(),
                    _ => {
                        let m = find_method(&e.class, "update");
                        let none = m.is_none();
                        last = Some((key, m));
                        none
                    }
                }
            }
        };
        if idle {
            continue;
        }
        let entity = env.tickable_entities[index].clone();
        let class = entity.borrow().class.clone();
        if class.kind == "particles" {
            tick_particle_emitter(env, &entity, &class, dt)?;
            continue;
        }
        if paused {
            let persistent = entity
                .borrow()
                .current_state
                .as_ref()
                .map(|s| crate::stdlib::is_persistent_state(s))
                .unwrap_or(false);
            if !persistent {
                continue;
            }
        }
        let key = Rc::as_ptr(&class);
        let method = match &last {
            Some((k, m)) if *k == key => m.clone(),
            _ => {
                let m = find_method(&class, "update");
                last = Some((key, m.clone()));
                m
            }
        };
        let Some(method) = method else {
            continue;
        };
        call_method(
            env,
            instance_value(&entity),
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
    // web3d-M7 follow-up: nothing to prune unless something despawned
    // since the last prune (a 100k-entity world paid for the walk every
    // tick).
    if !std::mem::take(&mut env.despawned_since_prune) {
        return;
    }
    // Phase 9 session 7b: fire `on <Class>.death(e):` handlers for any
    // entity whose `despawned` flag was set this frame and whose death
    // hasn't fired yet. The dying entity is still in `active_entities`
    // at this point so the handler body can read its fields (`e.pos`,
    // etc.) before pruning. Handlers run in registration order; spawn
    // calls inside a handler push to `active_entities` and won't be
    // re-pruned this frame.
    if !env.death_handlers.is_empty() {
        let snapshot = env.active_entities.clone();
        for entity in snapshot {
            let (despawned, fired, class_name) = {
                let i = entity.borrow();
                (i.despawned, i.death_fired, i.class.name.clone())
            };
            if !despawned || fired {
                continue;
            }
            entity.borrow_mut().death_fired = true;
            let handlers = env.death_handlers.get(&class_name).cloned();
            if let Some(handlers) = handlers {
                let entity_value = Value::from_instance(entity);
                for handler in handlers {
                    if let Err(e) = run_death_handler(env, &handler, entity_value) {
                        eprintln!("[twec] error in `on {class_name}.death`: {e}");
                    }
                }
            }
        }
    }
    env.active_entities.retain(|e| !e.borrow().despawned);
    env.tickable_entities.retain(|e| !e.borrow().despawned);
}

/// Bind the handler's `param` to the dying entity and run the body.
/// The param is a local of the handler's frame (web3d-M1; it used to
/// be left behind as a global).
fn run_death_handler(
    env: &mut Env,
    handler: &crate::value::OnDeathHandler,
    entity: Value,
) -> Result<(), RuntimeError> {
    run_frame_body(
        env,
        vec![(Rc::from(handler.param.as_str()), entity)],
        &handler.body,
    )
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
        let count = match inst.get_field("count") {
            Some(t) if t.is_int_or_boxed_int() && t.as_int() >= 0 => t.as_int() as usize,
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
        let lifetime = {
            let __opt = inst.get_field("lifetime");
            if let Some(__t) = (__opt).as_ref() {
                if __t.is_float() {
                    __t.as_float()
                } else if __t.is_int_or_boxed_int() {
                    let n = __t.as_int();
                    n as f64
                } else if __t.is_quantity() {
                    let (value, _) = __t.as_quantity();
                    value
                } else {
                    let other = *__t;
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
            } else {
                1.0
            }
        };
        (count, lifetime, inst.class.clone())
    };
    // web3d-M7: each emitter draws its particles' random numbers from
    // its own stream, so particles never change the script's (and the
    // CPU and GPU paths leave the simulation identical).
    env.particle_seed = env.particle_seed.wrapping_add(1);
    let seed = env.particle_seed;
    // web3d-M7: on a 3D host, a block that compiled runs on the GPU:
    // record the emission; the emitter only counts down its lifetime.
    let gpu_at = at
        .and_then(|v| crate::stdlib::xyz_of(v, "at").ok())
        .or(if at.is_none() { Some([0.0; 3]) } else { None });
    if env.gpu_particles {
        if let (Some(at), Some(program)) = (gpu_at, env.intern_particle_program(&class.name)) {
            if !env.particles_unseen {
                env.particle_emissions.push(crate::kernel::particles::ParticleEmission {
                    program,
                    at,
                    count: count.min(u32::MAX as usize) as u32,
                    lifetime: lifetime as f32,
                    seed,
                });
            }
            let mut e = emitter.borrow_mut();
            e.insert_field("__gpu_age", Value::from_float(0.0));
            e.insert_field("__gpu_lifetime", Value::from_float(if count == 0 { f64::NEG_INFINITY } else { lifetime }));
            return Ok(());
        }
    }
    let on_spawn = find_method(&class, "on_spawn");
    let mut particles: Vec<Value> = Vec::with_capacity(count);
    let initial_pos = at
        .cloned()
        .unwrap_or_else(|| Value::from_tuple(vec![Value::from_float(0.0), Value::from_float(0.0)]));
    let mut rng = particle_stream(seed);
    // In 3D a particle's size is a radius in world units.
    let default_size = if env.gpu_particles { 0.1 } else { 4.0 };
    for _ in 0..count {
        let p = make_particle(&initial_pos, lifetime, default_size);
        if let Some(method) = on_spawn.clone() {
            let script_rng = env.swap_rng(rng);
            let result = call_method(
                env,
                Value::from_instance(emitter.clone()),
                &method,
                std::slice::from_ref(&p),
                &[],
                line,
                col,
            );
            rng = env.swap_rng(script_rng);
            result?;
        }
        particles.push(p);
    }
    let mut e = emitter.borrow_mut();
    e.insert_field("__particles", Value::from_list(Rc::new(RefCell::new(particles))));
    e.insert_field("__rng", Value::from_int(rng as i64));
    Ok(())
}

/// web3d-M7: the start of emitter `seed`'s random stream (SplitMix64).
fn particle_stream(seed: u32) -> u64 {
    let mut z = u64::from(seed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn make_particle(initial_pos: &Value, lifetime: f64, size: f64) -> Value {
    let mut o = Object {
        fields: HashMap::new(),
        kind: "particle",
    };
    o.insert_field("pos", *initial_pos);
    o.insert_field(
        "velocity",
        Value::from_tuple(vec![Value::from_float(0.0), Value::from_float(0.0)]),
    );
    o.insert_field(
        "color",
        Value::from_tuple(vec![
            Value::from_float(1.0),
            Value::from_float(1.0),
            Value::from_float(1.0),
            Value::from_float(1.0),
        ]),
    );
    o.insert_field("size", Value::from_float(size));
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
    // web3d-M7: a GPU emitter lives exactly as long as its particles
    // would on the CPU (same ageing, same comparison).
    let gpu = {
        let e = emitter.borrow();
        e.get_field("__gpu_age").zip(e.get_field("__gpu_lifetime"))
    };
    if let Some((age, lifetime)) = gpu {
        let age = age.as_float() + dt;
        let mut e = emitter.borrow_mut();
        e.insert_field("__gpu_age", Value::from_float(age));
        // Alive while `age < lifetime`, exactly the CPU's test.
        if age.partial_cmp(&lifetime.as_float()) != Some(std::cmp::Ordering::Less) {
            e.despawned = true;
            crate::value::bump_look_epoch();
            drop(e);
            env.despawned_since_prune = true;
        }
        return Ok(());
    }
    let on_update = find_method(class, "on_update");
    let particles = {
        let __opt = emitter.borrow().get_field("__particles");
        if let Some(__t) = (__opt).as_ref() {
            if __t.is_list() {
                __t.as_list()
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        }
    };
    let snapshot: Vec<Value> = particles.borrow().clone();
    let stream = emitter.borrow().get_field("__rng").filter(|v| v.is_int_or_boxed_int());
    let mut rng = stream.map_or(1, |v| v.as_int() as u64);
    for p in &snapshot {
        if let Some(method) = on_update.clone() {
            let script_rng = env.swap_rng(rng);
            let result = call_method(
                env,
                Value::from_instance(emitter.clone()),
                &method,
                &[*p, Value::from_float(dt)],
                &[],
                0,
                0,
            );
            rng = env.swap_rng(script_rng);
            result?;
        }
        if p.is_object() {
            let rc = p.as_object();
            let mut o = rc.borrow_mut();
            let age = {
                let __opt = o.get_field("age");
                if let Some(__t) = (__opt).as_ref() {
                    if __t.is_float() {
                        let a = __t.as_float();
                        a + dt
                    } else if __t.is_int_or_boxed_int() {
                        let a = __t.as_int();
                        a as f64 + dt
                    } else {
                        dt
                    }
                } else {
                    dt
                }
            };
            let lifetime = {
                let __opt = o.get_field("lifetime");
                if let Some(__t) = (__opt).as_ref() {
                    if __t.is_float() {
                        __t.as_float()
                    } else {
                        1.0
                    }
                } else {
                    1.0
                }
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
    emitter.borrow_mut().insert_field("__rng", Value::from_int(rng as i64));
    // Drop dead particles.
    particles.borrow_mut().retain(|p| {
        if p.is_object() {
            let rc = p.as_object();
            let age_opt = rc.borrow().get_field("age");
            let lifetime_opt = rc.borrow().get_field("lifetime");
            if let (Some(age_v), Some(lt_v)) = (age_opt, lifetime_opt) {
                if age_v.is_float() && lt_v.is_float() {
                    return age_v.as_float() < lt_v.as_float();
                }
            }
            true
        } else {
            true
        }
    });
    if particles.borrow().is_empty() {
        emitter.borrow_mut().despawned = true;
        crate::value::bump_look_epoch();
        env.despawned_since_prune = true;
    }
    Ok(())
}

/// web3d-M7: the live CPU-simulated particles, for a 3D host to draw
/// (blocks that couldn't compile for the GPU). A 2D position lies in
/// the z = 0 plane.
pub fn cpu_particles_3d(env: &Env) -> Vec<crate::kernel::particles::GpuParticle> {
    let mut out = Vec::new();
    let num = |v: &Value| {
        if v.is_float() {
            Some(v.as_float() as f32)
        } else if v.is_int_or_boxed_int() {
            Some(v.as_int() as f32)
        } else {
            None
        }
    };
    let floats = |v: Option<Value>| -> Vec<f32> {
        v.and_then(|v| v.with_tuple(|e| e.iter().filter_map(num).collect()))
            .unwrap_or_default()
    };
    // Emitters are tickable entities: walk those, not every entity
    // (web3d-M7 follow-up: 1.3 ms a frame with 100k static blocks).
    for entity in &env.tickable_entities {
        let e = entity.borrow();
        if e.despawned || e.class.kind != "particles" {
            continue;
        }
        let Some(list) = e.get_field("__particles").filter(|v| v.is_list()) else {
            continue;
        };
        for p in list.as_list().borrow().iter().filter(|p| p.is_object()) {
            let o = p.as_object();
            let o = o.borrow();
            let pos = floats(o.get_field("pos"));
            let color = floats(o.get_field("color"));
            let scalar = |name| o.get_field(name).as_ref().and_then(num).unwrap_or(0.0);
            if pos.len() < 2 || color.len() != 4 {
                continue;
            }
            out.push(crate::kernel::particles::GpuParticle {
                pos: [pos[0], pos[1], pos.get(2).copied().unwrap_or(0.0)],
                age: scalar("age"),
                velocity: [0.0; 3],
                lifetime: scalar("lifetime"),
                color: [color[0], color[1], color[2], color[3]],
                size: scalar("size"),
                age_ratio: scalar("age_ratio"),
                program: 0,
                seed: 0,
            });
        }
    }
    out
}

fn render_particle_emitter(
    env: &mut Env,
    emitter: &Rc<RefCell<Instance>>,
    class: &Rc<ClassDef>,
) -> Result<(), RuntimeError> {
    // If the user defined a custom `render()`, defer to it and skip the
    // built-in circle-per-particle path.
    if let Some(method) = find_method(class, "render") {
        return call_method(
            env,
            Value::from_instance(emitter.clone()),
            &method,
            &[],
            &[],
            0,
            0,
        )
        .map(|_| ());
    }
    let particles = {
        let __opt = emitter.borrow().get_field("__particles");
        if let Some(__t) = (__opt).as_ref() {
            if __t.is_list() {
                __t.as_list()
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        }
    };
    if !env.in_render {
        return Ok(());
    }
    for p in particles.borrow().iter() {
        if p.is_object() {
            let rc = p.as_object();
            let o = rc.borrow();
            let (px, py) = match o.get_field("pos") {
                Some(t) if t.is_tuple() => {
                    let elems = t.as_tuple();
                    if elems.len() >= 2 {
                        (number_or_zero(&elems[0]), number_or_zero(&elems[1]))
                    } else {
                        (0.0, 0.0)
                    }
                }
                _ => (0.0, 0.0),
            };
            let radius = {
                let __opt = o.get_field("size");
                if let Some(__t) = (__opt).as_ref() {
                    if __t.is_float() {
                        let f = __t.as_float();
                        f as f32
                    } else if __t.is_int_or_boxed_int() {
                        let n = __t.as_int();
                        n as f32
                    } else {
                        4.0
                    }
                } else {
                    4.0
                }
            };
            let color = match o.get_field("color") {
                Some(t) if t.is_tuple() => {
                    let elems = t.as_tuple();
                    if elems.len() >= 3 {
                        let r = number_or_zero(&elems[0]) as f32;
                        let g = number_or_zero(&elems[1]) as f32;
                        let b = number_or_zero(&elems[2]) as f32;
                        let a = if elems.len() >= 4 {
                            number_or_zero(&elems[3]) as f32
                        } else {
                            1.0
                        };
                        macroquad::color::Color::new(r, g, b, a)
                    } else {
                        macroquad::color::WHITE
                    }
                }
                _ => macroquad::color::WHITE,
            };
            macroquad::shapes::draw_circle(px as f32, py as f32, radius, color);
        }
    }
    Ok(())
}

fn number_or_zero(v: &Value) -> f64 {
    if v.is_int_or_boxed_int() {
        let n = v.as_int();
        n as f64
    } else if v.is_float() {
        v.as_float()
    } else if v.is_quantity() {
        let (value, _) = v.as_quantity();
        value
    } else {
        0.0
    }
}

fn update_time_ambient(env: &mut Env, dt: f64) {
    if let Some(__t) = (env.get("time")).as_ref() {
        if __t.is_object() {
            let rc = __t.as_object();
            rc.borrow_mut().insert_field("dt", Value::from_float(dt));
        }
    }
}

/// Look at `env.key_press` (an Object whose fields are bool flags set
/// each frame by the host) and fire the active scene's matching
/// on_key_press handlers.
fn dispatch_key_press(env: &mut Env, scene: &Rc<RefCell<Instance>>) -> Result<(), RuntimeError> {
    let pressed = {
        let __opt = env.get("key_press");
        if let Some(__t) = (__opt).as_ref() {
            if __t.is_object() {
                let rc = __t.as_object();
                let o = rc.borrow();
                o.fields
                    .iter()
                    .filter_map(|(k, v)| {
                        if v.is_bool() && v.as_bool() {
                            Some(k.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        }
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
    let prev_self = env.self_value.replace(instance_value(scene));
    let _self_root = root_saved_self(prev_self);
    for body in bodies {
        run_frame_body(env, Vec::new(), &body)?;
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
    env.hud_queue.clear();
    let prev_render = env.in_render;
    env.in_render = true;
    let mut result = Ok(());
    if let Some(body) = env.top_on_render.clone() {
        result = run_frame_body(env, Vec::new(), &body);
        // A `return` in the top-level on_render body just stops the
        // current frame's draw composition; clear the flag so
        // subsequent frames aren't affected.
        env.returning.take();
    }
    // web3d-M4: the active scene state's `on render():` too — per-state
    // HUDs and menus (level-up picker, game over).
    if result.is_ok() {
        result = render_scene_state(env);
        env.returning.take();
    }
    env.in_render = prev_render;
    result?;
    draw_looks(env)
}

/// web3d-M3: `look:` is drawn by the 3D kernel only; the 2D macroquad
/// player retires in web3d-M6. Rather than drawing nothing, it refuses.
pub fn look_needs_3d(class: &str) -> RuntimeError {
    RuntimeError {
        line: 0,
        col: 0,
        message: format!("`{class}` has a look:, which only the 3D runtime draws"),
        help: Some(
            "run it with `twec play3d` or build it with `twec build --target web`; the 2D player gains look: in web3d-M6"
                .to_string(),
        ),
    }
}

/// The first class defined in `env` whose (merged) look is set, for
/// hosts that can't draw looks to refuse at startup.
pub fn first_look_class(env: &Env) -> Option<String> {
    let mut names: Vec<String> = env
        .iter_bindings()
        .filter(|(_, v)| v.is_class() && v.as_class().look.is_some())
        .map(|(n, _)| n)
        .collect();
    names.sort();
    names.into_iter().next()
}

/// What one look draws, after evaluating its keys.
#[derive(Clone, Copy)]
struct LookValues {
    primitive: crate::value::Primitive,
    color: [f32; 4],
    size: f32,
    yaw: f32,
    material: u32,
}

/// web3d-M3: queue a draw for every live entity whose class has a
/// `look:` (`docs/06` §4.9a). Shared keys are evaluated once per class;
/// per-entity keys once per entity, with `self` bound to it.
fn draw_looks(env: &mut Env) -> Result<(), RuntimeError> {
    // web3d-M7 session 16: gather the entities with looks only when
    // something that could change them happened (`look_epoch`).
    if env.look_cache.as_ref().is_none_or(|c| c.epoch != crate::value::look_epoch()) {
        env.look_cache = Some(gather_looks(env)?);
    }
    // Taken out while look keys run (they may spawn, or read `env`).
    let cache = env.look_cache.take().expect("gathered above");
    // web3d-M7 follow-up: a frame of nothing but looks that read nothing
    // of their entities is the same frame while their gathering and
    // each class's shared values stand still. Fingerprint it; if the
    // renderer already holds those draws, don't queue 100k of them again.
    env.draws_generation = None;
    let result = if env.render_queue3d.is_empty() && cache.own.is_empty() {
        match looks_fingerprint(env, &cache) {
            Ok(fp) => {
                env.draws_generation = Some(fp);
                if env.retained_looks == Some(fp) {
                    Ok(())
                } else {
                    emit_looks(env, &cache)
                }
            }
            Err(e) => Err(e),
        }
    } else {
        emit_looks(env, &cache)
    };
    if env.look_cache.is_none() {
        env.look_cache = Some(cache);
    }
    result
}

/// Every live entity with a look: those whose keys read nothing of the
/// entity grouped by class with their positions, the rest listed.
fn gather_looks(env: &mut Env) -> Result<crate::value::LookCache, RuntimeError> {
    let mut cache = crate::value::LookCache {
        epoch: crate::value::look_epoch(),
        ..Default::default()
    };
    // Per-class slot in `cache.shared`, keyed by class identity. Few
    // classes have looks, so a linear scan beats hashing.
    let mut slots: Vec<(*const ClassDef, usize)> = Vec::new();
    for entity in &env.active_entities {
        let inst = entity.borrow();
        let Some(look) = inst.class.look.as_ref() else {
            continue;
        };
        if inst.despawned {
            continue;
        }
        if look_has_own_keys(look) {
            cache.own.push(entity.clone());
            continue;
        }
        let at = look_pos(&inst)?;
        let key = Rc::as_ptr(&inst.class);
        match slots.iter().find(|(k, _)| *k == key) {
            Some((_, i)) => cache.shared[*i].1.push(at),
            None => {
                slots.push((key, cache.shared.len()));
                cache.shared.push((entity.clone(), vec![at]));
            }
        }
    }
    Ok(cache)
}

fn look_has_own_keys(look: &crate::value::LookDef) -> bool {
    [&look.mesh, &look.tint, &look.scale, &look.facing, &look.material]
        .iter()
        .any(|s| s.as_ref().is_some_and(|s| s.per_entity))
}

/// An entity's `pos` as the point its look is drawn at.
fn look_pos(inst: &Instance) -> Result<[f32; 3], RuntimeError> {
    let pos = inst.get_field("pos");
    match pos.map(|p| crate::stdlib::xyz_of(&p, "pos")) {
        Some(Ok(at)) => Ok(at),
        _ => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "`{}` has a look: but its `pos` is not a vec3 (it is {})",
                inst.class.name,
                pos.map(|p| p.type_name()).unwrap_or("missing")
            ),
            help: Some(format!(
                "give `{}` a field `var pos = vec3(0, 0, 0)`, or spawn it with `spawn {} at vec3(...)`",
                inst.class.name, inst.class.name
            )),
        }),
    }
}

/// web3d-M7 follow-up: a fingerprint of the draws `emit_looks` would
/// queue for a cache of shared-look classes only: the gathering (its
/// epoch) and each class's shared values this frame.
fn looks_fingerprint(env: &mut Env, cache: &crate::value::LookCache) -> Result<u64, RuntimeError> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cache.epoch.hash(&mut h);
    for (entity, positions) in &cache.shared {
        let look = entity.borrow().class.look.clone().expect("gathered with a look");
        let v = look_values(env, entity, &look, false, LookValues::default_look())?;
        match v.primitive {
            crate::value::Primitive::Cube => 0u32.hash(&mut h),
            crate::value::Primitive::Sphere => 1u32.hash(&mut h),
            crate::value::Primitive::Mesh(id) => (2u32, id).hash(&mut h),
        }
        for x in v.color {
            x.to_bits().hash(&mut h);
        }
        (v.size.to_bits(), v.yaw.to_bits(), v.material, positions.len()).hash(&mut h);
    }
    Ok(h.finish())
}

/// Queue this frame's draws: each class's shared keys once, then its
/// cached positions; per-entity keys for the entities that have them.
fn emit_looks(env: &mut Env, cache: &crate::value::LookCache) -> Result<(), RuntimeError> {
    let mut shared: Vec<(*const ClassDef, LookValues)> = Vec::new();
    let mut class_values = |env: &mut Env, entity: &Rc<RefCell<Instance>>| -> Result<LookValues, RuntimeError> {
        let class = entity.borrow().class.clone();
        let key = Rc::as_ptr(&class);
        if let Some((_, v)) = shared.iter().find(|(k, _)| *k == key) {
            return Ok(*v);
        }
        let look = class.look.clone().expect("gathered with a look");
        let v = look_values(env, entity, &look, false, LookValues::default_look())?;
        shared.push((key, v));
        Ok(v)
    };
    for (entity, positions) in &cache.shared {
        let values = class_values(env, entity)?;
        env.render_queue3d.extend(positions.iter().map(|&at| crate::value::DrawCall3d {
            primitive: values.primitive,
            at,
            color: values.color,
            size: values.size,
            texture: 0,
            yaw: values.yaw,
            material: values.material,
        }));
    }
    for entity in &cache.own {
        if entity.borrow().despawned {
            continue;
        }
        let base = class_values(env, entity)?;
        let look = entity.borrow().class.look.clone().expect("gathered with a look");
        let values = look_values(env, entity, &look, true, base)?;
        let at = look_pos(&entity.borrow())?;
        env.render_queue3d.push(crate::value::DrawCall3d {
            primitive: values.primitive,
            at,
            color: values.color,
            size: values.size,
            texture: 0,
            yaw: values.yaw,
            material: values.material,
        });
    }
    Ok(())
}

impl LookValues {
    fn default_look() -> Self {
        LookValues {
            primitive: crate::value::Primitive::Cube,
            color: [1.0, 1.0, 1.0, 1.0],
            size: 1.0,
            yaw: 0.0,
            material: 0,
        }
    }
}

/// Evaluate the look keys whose `per_entity` flag equals `per_entity`
/// over `base` (keys of the other kind keep `base`'s values).
fn look_values(
    env: &mut Env,
    entity: &Rc<RefCell<Instance>>,
    look: &crate::value::LookDef,
    per_entity: bool,
    base: LookValues,
) -> Result<LookValues, RuntimeError> {
    let mut v = base;
    fn wanted(
        s: &Option<crate::value::LookSlot>,
        per_entity: bool,
    ) -> Option<&crate::value::LookSlot> {
        s.as_ref().filter(|s| s.per_entity == per_entity)
    }
    if let Some(slot) = wanted(&look.mesh, per_entity) {
        let val = eval_look_slot(env, entity, slot)?;
        v.primitive = look_mesh(env, val, slot)?;
    }
    if let Some(slot) = wanted(&look.tint, per_entity) {
        let val = eval_look_slot(env, entity, slot)?;
        v.color = look_tint(val, slot)?;
    }
    if let Some(slot) = wanted(&look.scale, per_entity) {
        let val = eval_look_slot(env, entity, slot)?;
        v.size = crate::stdlib::number(&val, "look scale").map_err(|e| at_slot(e, slot))? as f32;
    }
    if let Some(slot) = wanted(&look.facing, per_entity) {
        let val = eval_look_slot(env, entity, slot)?;
        v.yaw = crate::stdlib::number(&val, "look facing").map_err(|e| at_slot(e, slot))? as f32;
    }
    if let Some(slot) = wanted(&look.material, per_entity) {
        let val = eval_look_slot(env, entity, slot)?;
        v.material = look_material(env, val, slot)?;
    }
    Ok(v)
}

/// Evaluate one look key in the entity's scope, as a method body would.
fn eval_look_slot(
    env: &mut Env,
    entity: &Rc<RefCell<Instance>>,
    slot: &crate::value::LookSlot,
) -> Result<Value, RuntimeError> {
    let saved_self = env.self_value.replace(instance_value(entity));
    let home = frame_home(env, &slot.home);
    push_call_frame(env, &[], &[], home);
    // Nonzero call depth: no GC safepoint while look values are held.
    env.call_depth += 1;
    let result = eval_expr(env, &slot.expr);
    env.call_depth -= 1;
    pop_frame(env);
    env.self_value = saved_self;
    result.map_err(|e| at_slot(e, slot))
}

/// Builtin errors carry no position; point them at the look key.
fn at_slot(mut e: RuntimeError, slot: &crate::value::LookSlot) -> RuntimeError {
    if e.line == 0 {
        e.line = slot.line;
        e.col = slot.col;
    }
    e
}

fn look_mesh(
    env: &mut Env,
    v: Value,
    slot: &crate::value::LookSlot,
) -> Result<crate::value::Primitive, RuntimeError> {
    let bad = |got: String| RuntimeError {
        line: slot.line,
        col: slot.col,
        message: format!("look mesh must be \"cube\", \"sphere\" or a .glb path, got {got}"),
        help: Some("e.g. `mesh: \"cube\"` or `mesh: \"models/ship.glb\"`".to_string()),
    };
    if !v.is_str() {
        return Err(bad(v.type_name().to_string()));
    }
    let s = v.as_string();
    Ok(match s.as_str() {
        "cube" => crate::value::Primitive::Cube,
        "sphere" => crate::value::Primitive::Sphere,
        path if path.ends_with(".glb") => crate::value::Primitive::Mesh(env.intern_mesh_path(path)),
        other => return Err(bad(format!("\"{other}\""))),
    })
}

/// A look's `material:` — a `visual` block, used as the mesh surface.
fn look_material(
    env: &mut Env,
    v: Value,
    slot: &crate::value::LookSlot,
) -> Result<u32, RuntimeError> {
    let err = |message: String, help: &str| RuntimeError {
        line: slot.line,
        col: slot.col,
        message,
        help: Some(help.to_string()),
    };
    if !(v.is_class() && v.as_class().kind == "visual") {
        return Err(err(
            format!("look material must be a visual block, got {}", v.type_name()),
            "declare one with `visual Name:` and a `pixel(uv, time) -> color` or `surface(uv, time, pos, normal) -> material` method, then `material: Name`",
        ));
    }
    let name = v.as_class().name.clone();
    env.intern_material(&name).map_err(|e| {
        err(
            format!("visual `{name}` can't be used as a material: {e}"),
            "a material's methods must pass the visual-block checks (`twec verify`)",
        )
    })
}

fn look_tint(v: Value, slot: &crate::value::LookSlot) -> Result<[f32; 4], RuntimeError> {
    if v.is_tuple() && v.as_tuple().len() == 3 {
        let e = v.as_tuple();
        let c = |x: &Value| crate::stdlib::number(x, "look tint").map(|n| n as f32);
        return Ok([c(&e[0])?, c(&e[1])?, c(&e[2])?, 1.0]);
    }
    crate::stdlib::rgba_of(&v, "look tint").map_err(|e| at_slot(e, slot))
}

/// Run the active scene's current state's on-render handler, plus
/// each active entity's `render()` method. Drawing primitives in
/// stdlib check `env.in_render`; the caller is responsible for
/// setting that flag (the macroquad `play` loop does it around this
/// call).
pub fn render_frame(env: &mut Env) -> Result<(), RuntimeError> {
    let _profile = crate::profile::scope("render");
    render_scene_state(env)?;
    render_entities_2d(env)
}

/// The active scene's current state's `on render():`, then any state
/// transition it raised (a modal's "Resume" / "pick upgrade"). Shared
/// by the 2D and 3D render paths (web3d-M4: `survive3d` draws its HUD
/// and menus per state).
fn render_scene_state(env: &mut Env) -> Result<(), RuntimeError> {
    if let Some(scene) = env.active_scene.clone() {
        let body: Option<Vec<Stmt>> = {
            let inst = scene.borrow();
            inst.current_state
                .as_ref()
                .and_then(|n| inst.class.states.get(n))
                .and_then(|state| state.on_render.clone())
        };
        if let Some(body) = body {
            let prev_self = env.self_value.replace(instance_value(&scene));
            let _self_root = root_saved_self(prev_self);
            run_frame_body(env, Vec::new(), &body)?;
            env.self_value = prev_self;
        }
        // Apply state transitions raised during on_render — modal
        // states like a level-up picker put their button widgets
        // in render and rely on `-> playing` to dismiss themselves
        // (same pattern as the pause menu's Resume button).
        if let Some(target) = env.transitioning.take() {
            enter_state(env, &scene, &target)?;
        }
    }
    Ok(())
}

/// Each live entity's 2D `render()` (and particle emitters).
fn render_entities_2d(env: &mut Env) -> Result<(), RuntimeError> {
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
        if class.look.is_some() {
            return Err(look_needs_3d(&class.name));
        }
        let method = match find_method(&class, "render") {
            Some(m) => m,
            None => continue,
        };
        call_method(env, Value::from_instance(entity), &method, &[], &[], 0, 0)?;
    }
    Ok(())
}

fn tick_scene(env: &mut Env, scene: &Rc<RefCell<Instance>>, dt: f64) -> Result<(), RuntimeError> {
    // Snapshot the state name + clock bodies before running, so a
    // transition during a clock body doesn't fire the wrong clock list.
    let (state_name, clocks): (Option<String>, Vec<(f64, Vec<Stmt>)>) = {
        let inst = scene.borrow();
        let name = inst.current_state.clone();
        let bodies: Vec<Vec<Stmt>> =
            if let Some(state) = name.as_ref().and_then(|n| inst.class.states.get(n)) {
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
    let prev_self = env.self_value.replace(instance_value(scene));
    let _self_root = root_saved_self(prev_self);
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
        run_frame_body(
            env,
            vec![(Rc::from(handler.param.as_str()), Value::from_float(dt))],
            &handler.body,
        )?;
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
            run_frame_body(env, Vec::new(), body)?;
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
                inst.every_timers.get(clock_idx).copied().unwrap_or(0.0) >= interval
            };
            if !should_fire {
                break;
            }
            scene.borrow_mut().every_timers[clock_idx] -= interval;
            fires += 1;
            run_frame_body(env, Vec::new(), &body)?;
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
                        None => {
                            "transitions must target a `state <name>:` declared in the same scene"
                                .to_string()
                        }
                    }),
                });
            }
        }
    };
    // Snake NP9: run the outgoing state's `on exit:` hook before we
    // switch, so cleanup observes the state it's leaving still active.
    // Skipped on the initial entry (no prior state). on_exit is
    // synchronous cleanup — it runs to completion here, and any
    // transition it raises is discarded so it can't hijack the
    // transition already in flight.
    let outgoing = scene.borrow().current_state.clone();
    if let Some(old_name) = outgoing {
        let exit_body = scene
            .borrow()
            .class
            .states
            .get(&old_name)
            .and_then(|s| s.on_exit.clone());
        if let Some(body) = exit_body {
            let prev_self = env.self_value.replace(instance_value(scene));
            let _self_root = root_saved_self(prev_self);
            run_frame_body(env, Vec::new(), &body)?;
            env.transitioning = None;
            env.self_value = prev_self;
        }
    }
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
    let prev_self = env.self_value.replace(instance_value(scene));
    let _self_root = root_saved_self(prev_self);
    let mut intervals = Vec::with_capacity(state.every_clocks.len());
    for clock in &state.every_clocks {
        let v = eval_expr(env, &clock.interval)?;
        intervals.push(quantity_to_seconds(
            &v,
            clock.interval.line(),
            clock.interval.col(),
        )?);
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
        locals: crate::value::LocalFrame::default(),
    });
    let our_idx = scene.borrow().fiber_frames.len() - 1;
    let mut out_path: Vec<PathEntry> = Vec::new();
    // web3d-M1: the entry body runs in its own frame.
    env.frames.push(crate::value::LocalFrame::default());
    let result = run_block_resumable(env, scene, stmts, &[], &mut out_path);
    let entry_frame = env.frames.pop().unwrap_or_default();
    let outcome = result?;
    let mut inst = scene.borrow_mut();
    if matches!(outcome, FiberOutcome::Suspended) {
        out_path.reverse();
        inst.fiber_frames[our_idx].resume_path = out_path;
        // Park the entry body's locals until the fiber resumes.
        inst.fiber_frames[our_idx].locals = entry_frame;
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
        let (body, resume_path, is_function, parked) = {
            let mut inst = scene.borrow_mut();
            // web3d-M1: take the frame's parked locals to run with.
            let parked = std::mem::take(&mut inst.fiber_frames[top_idx].locals);
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
            (body, f.resume_path.clone(), is_function, parked)
        };

        if is_function {
            env.call_depth += 1;
        }
        env.frames.push(parked);
        let mut out_path: Vec<PathEntry> = Vec::new();
        let result = run_block_resumable(env, scene, &body, &resume_path, &mut out_path);
        let live = env.frames.pop().unwrap_or_default();
        if is_function {
            env.call_depth -= 1;
        }
        let outcome = result?;

        if matches!(outcome, FiberOutcome::Suspended) {
            // Re-suspended. Inner frames may have been pushed
            // above us during the run; our frame is still at
            // `top_idx`. Update its resume_path in place and park
            // its locals again.
            out_path.reverse();
            let mut inst = scene.borrow_mut();
            inst.fiber_frames[top_idx].resume_path = out_path;
            inst.fiber_frames[top_idx].locals = live;
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
        // Restore the caller's return slot for a function frame whose
        // body is now done. Its locals were dropped with `live`.
        if let FrameKind::Function {
            saved_returning, ..
        } = frame.kind
        {
            // Discard the function's return value (Stmt::Expr
            // position; v0.2 session 2b doesn't pipe values
            // back into call-as-expression sites).
            let _ = env.returning.take();
            env.returning = saved_returning;
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
            Stmt::Wait {
                duration,
                line,
                col,
            } => {
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
            Stmt::Then {
                action,
                body,
                line,
                col,
            } => {
                if drilling {
                    // Resuming after the action's wait elapsed — run the
                    // body resumably (it may itself `wait` / `then`).
                    let outcome = run_block_resumable(env, scene, body, inner_incoming, out_path)?;
                    match outcome {
                        FiberOutcome::Suspended => {
                            out_path.push(PathEntry {
                                stmt_index: idx,
                                branch: Some(Branch::Then),
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
                } else {
                    // First hit: run the action (side effects + its
                    // duration value), then suspend for that long — the
                    // `wait`-equivalent, gated on the action's result.
                    let v = eval_expr(env, action)?;
                    let secs = quantity_to_seconds(&v, *line, *col)?;
                    scene.borrow_mut().entry_wait_remaining = secs;
                    out_path.push(PathEntry {
                        stmt_index: idx,
                        branch: Some(Branch::Then),
                    });
                    return Ok(FiberOutcome::Suspended);
                }
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
                        Branch::IfElse => else_body.as_ref().map(|v| v.as_slice()).unwrap_or(&[]),
                        Branch::While => {
                            return Err(RuntimeError {
                                line: 0,
                                col: 0,
                                message: "internal: corrupted resume path (while branch on if)"
                                    .to_string(),
                                help: None,
                            });
                        }
                        Branch::Then => {
                            return Err(RuntimeError {
                                line: 0,
                                col: 0,
                                message: "internal: corrupted resume path (then branch on if)"
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
                let inner_outcome = run_block_resumable(env, scene, target_body, inner, out_path)?;
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
                        message:
                            "internal: corrupted resume path (drilling into expression statement)"
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
    if let Some(__t) = (env.self_value).as_ref() {
        if __t.is_instance() {
            let rc = __t.as_instance();
            let class = rc.borrow().class.clone();
            if find_method(&class, name).is_some() {
                return false;
            }
        }
    }
    lookup_name(env, name)
        .as_ref()
        .is_some_and(|t| t.is_function())
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
    let def: Rc<FunctionDef> = {
        let __opt = lookup_name(env, &name);
        if let Some(__t) = (__opt).as_ref() {
            if __t.is_function() {
                let d = __t.as_function();
                d.clone()
            } else {
                unreachable!("guarded by is_top_level_user_call")
            }
        } else {
            unreachable!("guarded by is_top_level_user_call")
        }
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
        let param_refs: Vec<&str> = def.params.iter().map(|s| &**s).collect();
        bind_kwargs(&param_refs, &def.name, arg_vals, kwarg_vals, line, col)?
    };

    let saved_returning = env.returning.take();

    // Push the function frame upfront. Saved env state lives on
    // the frame so it can be restored on completion (whether
    // immediate or deferred via a wait + resume cycle). Function
    // frames pushed by deeper-still calls land ABOVE us, keeping
    // the natural call-stack order.
    scene.borrow_mut().fiber_frames.push(Frame {
        kind: FrameKind::Function {
            def: def.clone(),
            saved_returning,
        },
        resume_path: Vec::new(),
        locals: crate::value::LocalFrame::default(),
    });
    let our_idx = scene.borrow().fiber_frames.len() - 1;

    // web3d-M1: parameters are locals of the call's own frame.
    let home = frame_home(env, &def.home);
    push_call_frame(env, &def.params, &bound, home);
    env.call_depth += 1;
    let mut inner_out: Vec<PathEntry> = Vec::new();
    let result = run_block_resumable(env, scene, &def.body, &[], &mut inner_out);
    env.call_depth -= 1;
    let live = env.frames.pop().unwrap_or_default();
    let outcome = match result {
        Ok(o) => o,
        Err(e) => {
            // Pop our fiber frame on error and restore the caller's
            // return slot.
            let frame = scene.borrow_mut().fiber_frames.remove(our_idx);
            if let FrameKind::Function {
                saved_returning, ..
            } = frame.kind
            {
                env.returning = saved_returning;
            }
            return Err(e);
        }
    };

    if matches!(outcome, FiberOutcome::Suspended) {
        // Update our frame's resume_path in place and park the call's
        // locals. Inner frames pushed by deeper calls (if any) sit
        // above us at higher indices and stay there.
        inner_out.reverse();
        let mut inst = scene.borrow_mut();
        inst.fiber_frames[our_idx].resume_path = inner_out;
        inst.fiber_frames[our_idx].locals = live;
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
        saved_returning, ..
    } = frame.kind
    {
        env.returning = saved_returning;
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
        Stmt::OnClassEvent { .. } => "on class event",
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
        Stmt::Then { .. } => "then",
        Stmt::DialogueDecl { .. } => "dialogue",
        Stmt::Say { .. } => "say",
        Stmt::Choice { .. } => "choice",
        Stmt::Import { .. } => "import",
        Stmt::Expr(_) => "expression",
    }
}

/// Resolve a bare name. Scope chain: the active `self` instance's fields
/// shadow env globals, so `ticks += 1` inside a scene state mutates
/// `self.ticks`. Without this, scene fields would only be reachable via
/// explicit `self.x` syntax — verbose and unusual.
fn lookup_name(env: &Env, name: &str) -> Option<Value> {
    // web3d-M1: the innermost frame's locals come first.
    let home = match env.frames.last() {
        Some(f) => {
            if let Some(v) = f.get(name) {
                return Some(v);
            }
            f.home
        }
        None => None,
    };
    if let Some(v) = lookup_self_field(env, name) {
        return Some(v);
    }
    // A function defined in another module resolves its free names in
    // that module's globals.
    if let Some(h) = home {
        if h.is_object() {
            if let Some(v) = h.as_object().borrow().fields.get(name) {
                return Some(*v);
            }
        }
    }
    env.get(name)
}

fn lookup_self_field(env: &Env, name: &str) -> Option<Value> {
    match env.self_value.as_ref() {
        Some(t) if t.is_instance() => t.with_instance(|inst| inst.borrow().get_field(name)),
        _ => None,
    }
}

// ---- web3d-M3: names by resolution ----
//
// The resolver annotates each name with where it lives (`ast::Res`).
// These go straight to that binding; whenever the runtime disagrees
// with the annotation they fall back to the by-name functions above,
// so a wrong annotation costs speed, not correctness. `slot_misses`
// counts the fallbacks — the corpus runs with none
// (`tests/examples_run.rs`).

thread_local! {
    static SLOT_MISSES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Name of an unused slot (padding, or a loop variable after its
    /// loop). Never equal to a real name.
    static NO_NAME: Rc<str> = Rc::from("");
}

/// How many resolved names fell back to a by-name lookup on this
/// thread. Zero means the resolver's frames matched the runtime's.
pub fn slot_misses() -> u64 {
    SLOT_MISSES.with(|c| c.get())
}

fn slot_miss() {
    SLOT_MISSES.with(|c| c.set(c.get() + 1));
}

#[inline]
fn slot_is(held: &Rc<str>, want: &Rc<str>) -> bool {
    Rc::ptr_eq(held, want) || **held == **want
}

/// A module global or env global: `lookup_name` without the frame and
/// `self` steps.
fn lookup_global(env: &Env, name: &str) -> Option<Value> {
    if let Some(h) = env.frames.last().and_then(|f| f.home) {
        if h.is_object() {
            if let Some(v) = h.as_object().borrow().fields.get(name) {
                return Some(*v);
            }
        }
    }
    env.get(name)
}

/// Read `name` using its resolution.
#[inline]
fn read_name(env: &Env, name: &str, res: &crate::ast::ResCell) -> Option<Value> {
    use crate::ast::Res;
    match res.get() {
        Some(Res::Local { slot, name: want }) => {
            if let Some((held, v)) = env
                .frames
                .last()
                .and_then(|f| f.locals.get(usize::from(*slot)))
            {
                if slot_is(held, want) {
                    return Some(*v);
                }
            }
            slot_miss();
            lookup_name(env, name)
        }
        Some(Res::Field) => {
            let hit = env.self_value.as_ref().and_then(|t| {
                t.try_with_instance(|inst| {
                    let inst = inst.borrow();
                    inst.fields.at(res.hint(), name).or_else(|| {
                        let i = inst.fields.index_of(name)?;
                        res.set_hint(i as u32);
                        inst.fields.at(i as u32, name)
                    })
                })
                .flatten()
            });
            hit.or_else(|| lookup_name(env, name))
        }
        Some(Res::Global) => {
            // Entry-program globals by cached index; module globals
            // (a frame with a `home`) keep the by-name path.
            if env.frames.last().is_none_or(|f| f.home.is_none()) {
                if let Some(v) = env.global_at(res.hint(), name) {
                    return Some(v);
                }
                if let Some(i) = env.global_slot(name) {
                    res.set_hint(i);
                }
            }
            lookup_global(env, name).or_else(|| lookup_name(env, name))
        }
        None => lookup_name(env, name),
    }
}

/// `let` / loop-variable binding using its resolution.
fn declare_resolved(env: &mut Env, name: &str, res: &crate::ast::ResCell, v: Value) {
    if let Some(crate::ast::Res::Local { slot, name: want }) = res.get() {
        if let Some(f) = env.frames.last_mut() {
            let i = usize::from(*slot);
            if i < f.locals.len() {
                let (held, val) = &mut f.locals[i];
                if slot_is(held, want) || held.is_empty() {
                    *held = want.clone();
                    *val = v;
                    return;
                }
            } else {
                while f.locals.len() < i {
                    f.locals.push((NO_NAME.with(Rc::clone), Value::NIL));
                }
                f.locals.push((want.clone(), v));
                return;
            }
        }
        slot_miss();
    }
    declare_name(env, name, v);
}

/// `name = value` using its resolution; false if `name` isn't bound.
fn assign_resolved(env: &mut Env, name: &str, res: &crate::ast::ResCell, v: Value) -> bool {
    use crate::ast::Res;
    match res.get() {
        Some(Res::Local { slot, name: want }) => {
            if let Some((held, val)) = env
                .frames
                .last_mut()
                .and_then(|f| f.locals.get_mut(usize::from(*slot)))
            {
                if slot_is(held, want) {
                    *val = v;
                    return true;
                }
            }
            slot_miss();
        }
        Some(Res::Field) => {
            if let Some(t) = env.self_value.as_ref() {
                if t.is_instance()
                    && t.with_instance(|inst| {
                        let mut inst = inst.borrow_mut();
                        inst.fields.set_at(res.hint(), name, v) || {
                            match inst.fields.index_of(name) {
                                Some(i) => {
                                    res.set_hint(i as u32);
                                    inst.fields.set_at(i as u32, name, v)
                                }
                                None => false,
                            }
                        }
                    })
                {
                    return true;
                }
            }
        }
        Some(Res::Global) => {
            if env.frames.last().is_none_or(|f| f.home.is_none()) {
                if env.assign_global_at(res.hint(), name, v) {
                    return true;
                }
                if let Some(i) = env.global_slot(name) {
                    res.set_hint(i);
                }
            }
        }
        None => {}
    }
    assign_name(env, name, v)
}

/// [`loop_var_save`] using the loop variable's resolution.
fn loop_var_save_resolved(env: &Env, var: &str, res: &crate::ast::ResCell) -> Option<Value> {
    if let Some(crate::ast::Res::Local { slot, name: want }) = res.get() {
        if let Some(f) = env.frames.last() {
            return match f.locals.get(usize::from(*slot)) {
                Some((held, v)) if slot_is(held, want) => Some(*v),
                _ => None,
            };
        }
    }
    loop_var_save(env, var)
}

/// [`loop_var_restore`] using the loop variable's resolution: put the
/// shadowed value back, or free the slot.
fn loop_var_restore_resolved(
    env: &mut Env,
    var: &str,
    res: &crate::ast::ResCell,
    saved: Option<Value>,
) {
    if let Some(crate::ast::Res::Local { slot, name: want }) = res.get() {
        if let Some(entry) = env
            .frames
            .last_mut()
            .and_then(|f| f.locals.get_mut(usize::from(*slot)))
        {
            if slot_is(&entry.0, want) {
                match saved {
                    Some(v) => entry.1 = v,
                    None => {
                        entry.0 = NO_NAME.with(Rc::clone);
                        entry.1 = Value::NIL;
                    }
                }
                return;
            }
        }
    }
    loop_var_restore(env, var, saved);
}

fn quantity_to_seconds(v: &Value, line: u32, col: u32) -> Result<f64, RuntimeError> {
    if v.is_quantity() {
        let (value, unit) = v.as_quantity();
        match unit.as_str() {
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
        }
    } else if v.is_float() {
        let f = v.as_float();
        Ok(f)
    } else if v.is_int_or_boxed_int() {
        let n = v.as_int();
        Ok(n as f64)
    } else {
        let other = *v;
        Err(RuntimeError {
            line,
            col,
            message: format!(
                "every <duration> expects a duration quantity, got {}",
                other.type_name()
            ),
            help: Some("e.g. `every 100ms:` or `every 0.5s:`".to_string()),
        })
    }
}

/// Root the `self` value a scene-level runner swapped out, until the
/// returned scope drops. web3d-M0: a transition's on-entry body runs
/// statements (safepoints) at depth 0 while the previous `self`
/// wrapper lives only on the Rust stack; restoring a swept wrapper
/// afterwards left `self` dangling.
fn root_saved_self(prev: Option<Value>) -> crate::heap::RootScope {
    let roots = crate::heap::RootScope::new();
    if let Some(v) = prev {
        roots.push(v);
    }
    roots
}

// ---------- web3d-M1: lexical frames ----------
//
// Every function, method and event-handler body runs in its own
// `LocalFrame` on `env.frames`: parameters and `let` / `var` bindings
// live there and vanish when the body ends. Name lookup inside a body
// is: frame locals → `self` fields → the defining module's globals (for
// a function called from another module) → env globals. Top-level
// statements run with no frame and bind globals. Block visibility is
// enforced statically by `crate::resolve` before a program runs.

/// Run `body` in a fresh frame holding `locals`. The frame is popped
/// whether the body succeeds or errors.
fn run_frame_body(
    env: &mut Env,
    locals: Vec<(Rc<str>, Value)>,
    body: &[Stmt],
) -> Result<(), RuntimeError> {
    env.frames
        .push(crate::value::LocalFrame { locals, home: None });
    let result = run_block(env, body);
    pop_frame(env);
    result
}

/// Push a frame binding `params` to `args` (lengths already checked),
/// reusing a pooled locals vector when one is available.
fn push_call_frame(env: &mut Env, params: &[Rc<str>], args: &[Value], home: Option<Value>) {
    let mut locals = env.frame_pool.pop().unwrap_or_default();
    locals.extend(params.iter().cloned().zip(args.iter().copied()));
    env.frames.push(crate::value::LocalFrame { locals, home });
}

/// Pop the innermost frame and return its locals vector to the pool.
fn pop_frame(env: &mut Env) {
    if let Some(mut f) = env.frames.pop() {
        f.locals.clear();
        if env.frame_pool.len() < 64 {
            env.frame_pool.push(f.locals);
        }
    }
}

/// The frame `home` for a body defined in module `home`: that module,
/// unless it is the module this env is currently initialising (whose
/// globals *are* the env's globals).
fn frame_home(env: &Env, home: &Option<Value>) -> Option<Value> {
    match home {
        Some(h) => {
            let running = env
                .current_module
                .as_ref()
                .is_some_and(|m| m.heap_ptr() == h.heap_ptr());
            if running {
                None
            } else {
                Some(*h)
            }
        }
        None => None,
    }
}

/// Bind `name` in the innermost frame, or as a global at top level.
fn declare_name(env: &mut Env, name: &str, v: Value) {
    match env.frames.last_mut() {
        Some(f) => f.declare(name, v),
        // Re-binding (e.g. a top-level loop variable each iteration)
        // updates in place instead of allocating a new key.
        None => {
            if !env.assign_existing(name, v) {
                env.set(name.to_string(), v);
            }
        }
    }
}

/// Assign to an *existing* binding — local, `self` field, the frame's
/// home-module global, or env global, in that order. Returns false when
/// `name` isn't bound anywhere (assigning an undeclared name is an
/// error; `let` / `var` introduce names).
fn assign_name(env: &mut Env, name: &str, v: Value) -> bool {
    let home = match env.frames.last_mut() {
        Some(f) => {
            if f.assign(name, v) {
                return true;
            }
            f.home
        }
        None => None,
    };
    if let Some(t) = env.self_value.as_ref() {
        if t.is_instance() && t.with_instance(|inst| inst.borrow_mut().set_existing_field(name, v))
        {
            return true;
        }
    }
    if let Some(h) = home {
        if h.is_object() {
            let rc = h.as_object();
            let mut obj = rc.borrow_mut();
            if obj.fields.contains_key(name) {
                obj.fields.insert(name.to_string(), v);
                return true;
            }
        }
    }
    env.assign_existing(name, v)
}

/// A `for` / comprehension variable shadows any binding of the same
/// name in the current scope (frame locals, or globals at top level)
/// for the loop's duration. Returns the shadowed value to restore.
fn loop_var_save(env: &Env, var: &str) -> Option<Value> {
    match env.frames.last() {
        Some(f) => f.get(var),
        None => env.get(var),
    }
}

/// Undo `loop_var_save`: put the shadowed binding back, or drop the
/// loop variable if nothing was shadowed.
fn loop_var_restore(env: &mut Env, var: &str, saved: Option<Value>) {
    match (env.frames.last_mut(), saved) {
        (Some(f), Some(v)) => f.declare(var, v),
        (Some(f), None) => f.remove(var),
        (None, Some(v)) => env.set(var.to_string(), v),
        (None, None) => env.remove(var),
    }
}

fn undeclared_assign_error(name: &str, line: u32, col: u32) -> RuntimeError {
    RuntimeError {
        line,
        col,
        message: format!("assignment to undeclared name '{name}'"),
        help: Some(format!(
            "declare it first with `var {name} = ...` (at top level for a global)"
        )),
    }
}

/// The instance's GC value, allocated once and cached on the instance
/// (see `Instance::cached_value`).
fn instance_value(rc: &Rc<RefCell<Instance>>) -> Value {
    if let Some(v) = rc.borrow().cached_value {
        return v;
    }
    let v = Value::from_instance(rc.clone());
    rc.borrow_mut().cached_value = Some(v);
    v
}

/// Collect if the heap is over threshold (or in stress mode). Only
/// call where no unrooted `TaggedValue` lives on the Rust stack.
fn safepoint(env: &mut Env) {
    if crate::heap::gc_should_collect() {
        crate::heap::gc_collect_with(|| env.scan_roots());
    }
}

fn run_block(env: &mut Env, stmts: &[Stmt]) -> Result<(), RuntimeError> {
    for stmt in stmts {
        // GC safepoint between statements — but only at call depth 0.
        // web3d-M0: a function body reached from inside an expression
        // (`f(a, g())`, `x + h()`, a comprehension element) runs with
        // the caller's half-evaluated temporaries (arg vectors, the
        // left operand, …) living only on the Rust stack, so
        // collecting there freed live values. At depth 0 the only
        // Rust-stack values across a statement are the few the
        // interpreter roots explicitly via `heap::RootScope` (`for`
        // snapshots, a swapped-out `self`). Game code still collects
        // once per tick via `safepoint` in `tick_frame`.
        if env.call_depth == 0 && crate::heap::gc_should_collect() {
            crate::heap::gc_collect_with(|| env.scan_roots());
        }
        eval_stmt(env, stmt)?;
        if env.returning.is_some() || env.breaking || env.continuing || env.transitioning.is_some()
        {
            return Ok(());
        }
    }
    Ok(())
}

fn eval_stmt(env: &mut Env, stmt: &Stmt) -> Result<(), RuntimeError> {
    match stmt {
        Stmt::Let {
            name, value, res, ..
        } => {
            let v = eval_expr(env, value)?;
            declare_resolved(env, name, res, v);
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
        Stmt::FunctionDecl {
            name, params, body, ..
        } => {
            // Annotations don't affect runtime semantics; the
            // tree-walker still binds bare-name params. Strict
            // mode (Phase 6 session 2) uses the annotations
            // statically in `infer.rs`.
            let param_names: Vec<Rc<str>> =
                params.iter().map(|p| Rc::from(p.name.as_str())).collect();
            let f = Value::from_function(Rc::new(FunctionDef {
                name: name.clone(),
                params: param_names,
                body: body.clone(),
                home: env.current_module,
            }));
            declare_name(env, name, f);
            Ok(())
        }
        Stmt::Return { value, line, col } => {
            if env.call_depth == 0 {
                return Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: "`return` is only valid inside a function or method body".to_string(),
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
            var,
            iter,
            body,
            line,
            col,
            var_res,
        } => {
            env.loop_depth += 1;
            let result = run_for(env, var, var_res, iter, body, *line, *col);
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
        Stmt::Spawn {
            class,
            at,
            line,
            col,
        } => {
            let class_val = lookup_name(env, class).ok_or_else(|| RuntimeError {
                line: *line,
                col: *col,
                message: format!("class '{class}' is not defined"),
                help: Some(format!("declare it with `entity {class}:` first")),
            })?;
            let class_rc = if class_val.is_class() {
                class_val.as_class()
            } else {
                let other = class_val;
                return Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: format!(
                        "`spawn {class}` expects a class, but {class} is a {}",
                        other.type_name()
                    ),
                    help: None,
                });
            };
            let at_value = match at {
                Some(expr) => Some(eval_expr(env, expr)?),
                None => None,
            };
            let inst_val = instantiate(class_rc.clone());
            if let Some(av) = &at_value {
                if inst_val.is_instance() {
                    let rc = inst_val.as_instance();
                    rc.borrow_mut().insert_field("pos", *av);
                }
            }
            if inst_val.is_instance() {
                let rc = inst_val.as_instance();
                if class_rc.kind == "particles" {
                    seed_particle_emitter(env, &rc, at_value.as_ref(), *line, *col)?;
                }
                env.active_entities.push(rc.clone());
                if class_rc.kind == "particles" || find_method(&class_rc, "update").is_some() {
                    env.tickable_entities.push(rc.clone());
                }
                crate::value::bump_look_epoch();
            }
            Ok(())
        }
        Stmt::Despawn { target, line, col } => {
            let v = eval_expr(env, target)?;
            if v.is_instance() {
                let rc = v.as_instance();
                rc.borrow_mut().despawned = true;
                crate::value::bump_look_epoch();
                env.despawned_since_prune = true;
                Ok(())
            } else {
                let other = v;
                Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: format!("`despawn` expects an instance, got {}", other.type_name()),
                    help: None,
                })
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
                home: env.current_module,
            }));
            declare_name(env, name, dialogue);
            Ok(())
        }
        Stmt::Say {
            actor,
            text,
            line,
            col,
        } => {
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
                    let label = if av.is_instance() {
                        let inst = av.as_instance();
                        let n = inst.borrow().class.name.clone();
                        n
                    } else if av.is_str() {
                        av.as_string()
                    } else {
                        av.display()
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
        Stmt::Choice {
            branches,
            line,
            col,
        } => {
            // Print each label so a transcript shows the user what
            // was on offer. v0.1 always picks the first branch — the
            // deterministic surface is enough to ship dialogue;
            // interactive selection is a Phase 5 task 3 follow-on.
            for (i, (label, _)) in branches.iter().enumerate() {
                let label_value = eval_expr(env, label)?;
                env.out
                    .push_str(&format!("  [{}] {}\n", i + 1, label_value.display()));
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
        Stmt::Then { line, col, .. } => {
            // Like `wait`, `then` only suspends inside a state on-entry
            // body (it's `wait <action>` + body). Reaching here means it
            // was used in a non-fiber context (every-clock, on update,
            // function body).
            Err(RuntimeError {
                line: *line,
                col: *col,
                message: "`then` is only supported as a direct statement of a state body in v0.1"
                    .to_string(),
                help: Some(
                    "`<action> then <body>` waits like `wait`; use it at the top level of a `state <name>:` body".to_string(),
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
        Stmt::OnClassEvent {
            class,
            event,
            param,
            body,
            ..
        } => {
            // Phase 9 session 7b. Only `event = "death"` is recognized
            // by the parser; we keep the runtime keyed by event-name
            // so future events ride the same field with no schema bump.
            // Multiple handlers per (class, event) registered in source
            // order; the death-fire site iterates that list.
            let _ = event;
            env.death_handlers.entry(class.clone()).or_default().push(
                crate::value::OnDeathHandler {
                    param: param.clone(),
                    body: body.clone(),
                },
            );
            Ok(())
        }
        Stmt::Decl {
            kind,
            name,
            parent,
            members,
            line,
            col,
            ..
        } => eval_decl(env, *kind, name, parent.as_deref(), members, *line, *col),
        Stmt::Import {
            path,
            alias,
            line,
            col,
        } => {
            // Phase 13 session 3: resolve the import against the
            // current source file's directory, look up the
            // pre-evaluated module value, and bind it under the
            // chosen name. When `current_source` is `None` (ad-hoc
            // `eval::run` callers, REPL, tests that don't go through
            // `module::run_with_modules`) the statement degrades to
            // a no-op so the surface still parses for those callers.
            if let Some(src) = env.current_source.clone() {
                let target = crate::module::resolve(&src, path).map_err(|m| RuntimeError {
                    line: *line,
                    col: *col,
                    message: m,
                    help: None,
                })?;
                let key = crate::module::canonical_key(&target);
                let module_value = env.module_cache.get(&key).copied().ok_or(RuntimeError {
                    line: *line,
                    col: *col,
                    message: format!(
                        "module `{path}` was not pre-evaluated; this points at a loader bug"
                    ),
                    help: Some(
                        "imports are evaluated in topological order before the entry runs; if you reach this branch the module graph is missing a dep".to_string(),
                    ),
                })?;
                let bind_name = crate::module::import_binding_name(path, alias.as_deref());
                env.set(bind_name, module_value);
            }
            Ok(())
        }
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
        AssignTarget::Name(name, res) => {
            // web3d-M1: `name = value` updates an existing binding —
            // local, `self` field, module global, or global (see
            // `assign_name`). New names come only from `let` / `var`;
            // assigning an undeclared name used to create a global
            // silently.
            let final_value = if matches!(op, AssignOp::Set) {
                new_value
            } else {
                let current = read_name(env, name, res).ok_or_else(|| RuntimeError {
                    line,
                    col,
                    message: format!("name '{name}' is not defined"),
                    help: Some(format!("declare it with `let {name} = ...` before use")),
                })?;
                compound(op, &current, &new_value, line, col)?
            };
            if assign_resolved(env, name, res, final_value) {
                Ok(())
            } else {
                Err(undeclared_assign_error(name, line, col))
            }
        }
        AssignTarget::Field { object, name } => {
            let obj_val = eval_expr(env, object)?;
            if obj_val.is_object() {
                let rc = obj_val.as_object();
                let final_value = if matches!(op, AssignOp::Set) {
                    new_value
                } else {
                    let current = rc.borrow().get_field(name).ok_or_else(|| RuntimeError {
                        line,
                        col,
                        message: format!("field '{name}' is not defined on this object"),
                        help: Some(format!("set it first with `obj.{name} = ...`")),
                    })?;
                    compound(op, &current, &new_value, line, col)?
                };
                // Special case: `.pos = (x, y)` on a sprite-shaped object
                // also updates `.x` and `.y`. Mirrors Example 1's
                // tuple-as-Vector2 behavior.
                if name == "pos" && final_value.is_tuple() {
                    let elems = &final_value.as_tuple();
                    if elems.len() >= 2 {
                        let mut o = rc.borrow_mut();
                        o.insert_field("x".to_string(), elems[0]);
                        o.insert_field("y".to_string(), elems[1]);
                    }
                }
                rc.borrow_mut().insert_field(name.clone(), final_value);
                if name == "x" || name == "y" {
                    refresh_pos(&rc);
                }
                Ok(())
            } else if obj_val.is_instance() {
                let rc = obj_val.as_instance();
                let final_value = if matches!(op, AssignOp::Set) {
                    new_value
                } else {
                    let current = rc
                            .borrow()
                            .get_field(name)
                            .ok_or_else(|| {
                                let inst = rc.borrow();
                                let names: Vec<&str> = inst.fields.keys().collect();
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
            } else {
                let other = obj_val;
                Err(RuntimeError {
                    line,
                    col,
                    message: format!("cannot assign field on value of type {}", other.type_name()),
                    help: Some(
                        "only objects and class instances support field assignment".to_string(),
                    ),
                })
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
        .insert_field("pos", Value::from_tuple(vec![x, y]));
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
        // Phase 33 session 9: typed hole. Verify reports holes as
        // Warnings; *running* a program with an unfilled hole is a
        // runtime error so the model knows the program isn't ready
        // to ship.
        Expr::Hole { line, col } => Err(RuntimeError {
            line: *line,
            col: *col,
            message: "encountered unfilled hole `???`".to_string(),
            help: Some(
                "fill in this expression. `twec verify` reports the inferred expected type at each hole — use it to ground the next edit"
                    .to_string(),
            ),
        }),
        Expr::IfExpr {
            cond,
            then_expr,
            elifs,
            else_expr,
            ..
        } => {
            let c = eval_expr(env, cond)?;
            if is_truthy(&c) {
                return eval_expr(env, then_expr);
            }
            for (elif_cond, elif_expr) in elifs {
                let v = eval_expr(env, elif_cond)?;
                if is_truthy(&v) {
                    return eval_expr(env, elif_expr);
                }
            }
            eval_expr(env, else_expr)
        }
        Expr::Percent { value, .. } => Ok(Value::from_percent(*value)),
        Expr::Quantity { value, unit, .. } => Ok(Value::from_quantity(*value, Rc::new(unit.clone()))),
        Expr::Ident {
            name,
            line,
            col,
            res,
        } => read_name(env, name, res).ok_or_else(|| RuntimeError {
            line: *line,
            col: *col,
            message: format!("name '{name}' is not defined"),
            help: Some(format!("declare it with `let {name} = ...` before use")),
        }),
        Expr::SelfRef { line, col } => env.self_value.ok_or_else(|| RuntimeError {
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
            Ok(Value::from_tuple(vals))
        }
        Expr::List { elems, .. } => {
            let mut vals = Vec::with_capacity(elems.len());
            for e in elems {
                vals.push(eval_expr(env, e)?);
            }
            Ok(Value::from_list(Rc::new(RefCell::new(vals))))
        }
        Expr::ListComp {
            element,
            var,
            iterable,
            condition,
            line,
            col,
            var_res,
        } => {
            let iter_val = eval_expr(env, iterable)?;
            let items = iterable_snapshot(&iter_val, *line, *col)?;
            // Shadow the loop variable for the duration of the
            // comprehension, then restore it (matches `run_for`).
            let saved = loop_var_save_resolved(env, var, var_res);
            let mut out = Vec::new();
            let mut result = Ok(());
            for item in items {
                declare_resolved(env, var, var_res, item);
                if let Some(cond) = condition {
                    match eval_expr(env, cond) {
                        Ok(c) => {
                            if !is_truthy(&c) {
                                continue;
                            }
                        }
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    }
                }
                match eval_expr(env, element) {
                    Ok(v) => out.push(v),
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            loop_var_restore_resolved(env, var, var_res, saved);
            result?;
            Ok(Value::from_list(Rc::new(RefCell::new(out))))
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
            if s.is_int_or_boxed_int() && e.is_int_or_boxed_int() {
                Ok(Value::from_range(s.as_int(), e.as_int(), *exclusive))
            } else {
                Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: format!(
                        "range bounds must be ints, got {} and {}",
                        s.type_name(),
                        e.type_name()
                    ),
                    help: Some("v0.1 supports only integer ranges; float ranges ship later".to_string()),
                })
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
            match op {
                UnOp::Neg => {
                    if v.is_int_or_boxed_int() {
                        Ok(Value::from_int(-v.as_int()))
                    } else if v.is_float() {
                        Ok(Value::from_float(-v.as_float()))
                    } else {
                        Err(RuntimeError {
                            line: *line,
                            col: *col,
                            message: format!("cannot negate value of type {}", v.type_name()),
                            help: Some("`-` is defined on int and float".to_string()),
                        })
                    }
                }
                UnOp::Not => Ok(Value::from_bool(!is_truthy(&v))),
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
    if obj.is_list() {
        if !idx.is_int_or_boxed_int() {
            return Err(RuntimeError {
                line,
                col,
                message: format!("index must be int, got {}", idx.type_name()),
                help: None,
            });
        }
        let rc = obj.as_list();
        let i = idx.as_int();
        let v = rc.borrow();
        let len = v.len() as i64;
        let actual = if i < 0 { i + len } else { i };
        if actual < 0 || actual >= len {
            return Err(RuntimeError {
                line,
                col,
                message: format!("list index {i} out of bounds (length {len})"),
                help: Some("lists are 0-indexed; negative indices count from the end".to_string()),
            });
        }
        Ok(v[actual as usize])
    } else if obj.is_tuple() {
        if !idx.is_int_or_boxed_int() {
            return Err(RuntimeError {
                line,
                col,
                message: format!("index must be int, got {}", idx.type_name()),
                help: None,
            });
        }
        let elems = obj.as_tuple();
        let i = idx.as_int();
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
        Ok(elems[actual as usize])
    } else {
        Err(RuntimeError {
            line,
            col,
            message: format!("cannot index value of type {}", obj.type_name()),
            help: Some("indexing works on lists and tuples".to_string()),
        })
    }
}

fn field_get(obj: &Value, name: &str, line: u32, col: u32) -> Result<Value, RuntimeError> {
    // One decode for the common tuple case (`.x` / `.y` / `.z`).
    if let Some(component) = obj.with_tuple(|elems| match name {
        "x" => elems.first().copied(),
        "y" => elems.get(1).copied(),
        "z" => elems.get(2).copied(),
        _ => None,
    }) {
        match component {
            Some(v) => Ok(v),
            None => Err(RuntimeError {
                line,
                col,
                message: format!("tuple has no field '{name}'"),
                help: Some(
                    "tuples expose .x, .y, .z (and only those for the leading components)"
                        .to_string(),
                ),
            }),
        }
    } else if obj.is_list() {
        let rc = obj.as_list();
        match name {
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
        }
    } else if obj.is_object() {
        let rc = obj.as_object();
        let result = rc.borrow().get_field(name).ok_or_else(|| RuntimeError {
            line,
            col,
            message: format!("field '{name}' is not defined on this object"),
            help: Some(format!("set it first with `obj.{name} = ...`")),
        });
        result
    } else if obj.is_instance() {
        let rc = obj.as_instance();
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
    } else {
        Err(RuntimeError {
            line,
            col,
            message: format!("cannot read field on value of type {}", obj.type_name()),
            help: None,
        })
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
    if let Expr::Ident { name, res, .. } = callee {
        // web3d-M3: a name the resolver placed among the globals
        // (`vec3`, a top-level function) can't be a method of `self`.
        let may_be_method = !matches!(res.get(), Some(crate::ast::Res::Global));
        if let Some(__t) = (env.self_value).as_ref().filter(|_| may_be_method) {
            if __t.is_instance() {
                let rc = __t.as_instance();
                let class = rc.borrow().class.clone();
                if let Some(method) = find_method(&class, name) {
                    let arg_vals = eval_args(env, args)?;
                    let kwarg_vals = eval_kwargs(env, kwargs)?;
                    let r = call_method(
                        env,
                        Value::from_instance(rc),
                        &method,
                        &arg_vals,
                        &kwarg_vals,
                        line,
                        col,
                    );
                    recycle_args(env, arg_vals);
                    return r;
                }
            }
        }
    }
    // Method call: `recv.method(args)`. Resolved here (not via field_get)
    // because methods aren't first-class values yet.
    if let Expr::Field { object, name, .. } = callee {
        let recv = eval_expr(env, object)?;
        // List built-in methods.
        if recv.is_list() {
            let rc = &recv.as_list();
            if !kwargs.is_empty() {
                return Err(no_kwargs_error(&format!("list.{name}"), line, col));
            }
            if let Some(v) = list_method_call(env, rc, name, args, line, col)? {
                return Ok(v);
            }
        }
        if recv.is_range() {
            let (start, end, exclusive) = &recv.as_range();
            if !kwargs.is_empty() {
                return Err(no_kwargs_error(&format!("range.{name}"), line, col));
            }
            if let Some(v) =
                range_method_call(env, *start, *end, *exclusive, name, args, line, col)?
            {
                return Ok(v);
            }
        }
        if recv.is_instance() {
            let rc = &recv.as_instance();
            let class = rc.borrow().class.clone();
            if let Some(method) = find_method(&class, name) {
                let arg_vals = eval_args(env, args)?;
                let kwarg_vals = eval_kwargs(env, kwargs)?;
                let r = call_method(env, recv, &method, &arg_vals, &kwarg_vals, line, col);
                recycle_args(env, arg_vals);
                return r;
            }
            // Fall through to a normal field_get -> Call path, which will
            // produce a "field not defined" error below.
        }
        // Re-create the field-get + call path for non-instance receivers.
        let arg_vals = eval_args(env, args)?;
        let kwarg_vals = eval_kwargs(env, kwargs)?;
        let f = field_get(&recv, name, line, col)?;
        let r = apply_call(env, f, &arg_vals, &kwarg_vals, line, col);
        recycle_args(env, arg_vals);
        return r;
    }
    let f = eval_expr(env, callee)?;
    let arg_vals = eval_args(env, args)?;
    let kwarg_vals = eval_kwargs(env, kwargs)?;
    let r = apply_call(env, f, &arg_vals, &kwarg_vals, line, col);
    recycle_args(env, arg_vals);
    r
}

/// Evaluate call arguments into a pooled vector; hand it back with
/// [`recycle_args`] once the call returns.
fn eval_args(env: &mut Env, args: &[Expr]) -> Result<Vec<Value>, RuntimeError> {
    let mut out = env.arg_pool.pop().unwrap_or_default();
    for a in args {
        match eval_expr(env, a) {
            Ok(v) => out.push(v),
            Err(e) => {
                recycle_args(env, out);
                return Err(e);
            }
        }
    }
    Ok(out)
}

fn recycle_args(env: &mut Env, mut v: Vec<Value>) {
    v.clear();
    if env.arg_pool.len() < 64 {
        env.arg_pool.push(v);
    }
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
                    message: format!("{callee} has no parameter named `{kname}`"),
                    help: Some(format!("expected parameters: {}", params.join(", "))),
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
                    message: format!("{callee}: missing argument for parameter `{}`", params[i]),
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
            rc.borrow_mut()
                .pop()
                .ok_or_else(|| RuntimeError {
                    line,
                    col,
                    message: "pop_back on an empty list".to_string(),
                    help: Some("guard with `if list.length > 0:` before popping".to_string()),
                })
                .map(Some)
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
        // Phase 27: indexed mutation. Twe's `AssignTarget` enum
        // doesn't support `arr[i] = x` syntactically (only Name +
        // Field). `list.set(i, v)` is the canonical way to mutate
        // a slot in place. Out-of-bounds indices error instead of
        // silently growing the list — that surface choice
        // matches list.append's "explicit growth" stance.
        "set" => {
            arity_check(2)?;
            let idx_v = eval_expr(env, &args[0])?;
            let val = eval_expr(env, &args[1])?;
            let i = if idx_v.is_int() {
                idx_v.as_int()
            } else {
                return Err(RuntimeError {
                    line,
                    col,
                    message: "list.set expects an int index".to_string(),
                    help: None,
                });
            };
            let mut v = rc.borrow_mut();
            let len = v.len() as i64;
            if i < 0 || i >= len {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!("list.set index {i} out of bounds (length {len})"),
                    help: Some("guard with `if i >= 0 and i < list.length:`".to_string()),
                });
            }
            v[i as usize] = val;
            Ok(Some(Value::NIL))
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
            let result = if v.is_int_or_boxed_int() {
                let n = v.as_int();
                n >= start && n < upper
            } else {
                false
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
    if f.is_builtin() {
        let (name, params, func) = f.as_builtin();
        if params.is_empty() {
            if !kwargs.is_empty() {
                return Err(no_kwargs_error(name, line, col));
            }
            func(env, args)
        } else if kwargs.is_empty() && args.len() == params.len() {
            // All positional, one per parameter: nothing to bind, so
            // don't copy the arguments (the common case — `vec3(x, y, z)`,
            // `math.sqrt(d)`).
            func(env, args)
        } else {
            let bound = bind_kwargs(params, name, args.to_vec(), kwargs.to_vec(), line, col)?;
            func(env, &bound)
        }
    } else if f.is_function() {
        let def = f.as_function();
        call_function(env, &def, args, kwargs, line, col)
    } else if f.is_class() {
        let class = f.as_class();
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
    } else {
        let other = f;
        Err(RuntimeError {
            line,
            col,
            message: format!("cannot call value of type {}", other.type_name()),
            help: Some("only functions, builtins, and class constructors are callable".to_string()),
        })
    }
}

pub(crate) fn call_function(
    env: &mut Env,
    def: &FunctionDef,
    args: &[Value],
    kwargs: &[(String, Value)],
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    let _profile = crate::profile::scope(&def.name);
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
        std::borrow::Cow::Borrowed(args)
    } else {
        let param_refs: Vec<&str> = def.params.iter().map(|s| &**s).collect();
        std::borrow::Cow::Owned(bind_kwargs(
            &param_refs,
            &def.name,
            args.to_vec(),
            kwargs.to_vec(),
            line,
            col,
        )?)
    };
    // web3d-M1: parameters are locals of a fresh frame; free names
    // resolve lexically (never in the caller's frame).
    let saved_returning = env.returning.take();
    let home = frame_home(env, &def.home);
    push_call_frame(env, &def.params, &bound, home);
    env.call_depth += 1;
    let body_result = run_block(env, &def.body);
    env.call_depth -= 1;
    pop_frame(env);
    let return_value = env.returning.take().unwrap_or(Value::NIL);
    env.returning = saved_returning;
    body_result?;
    Ok(return_value)
}

/// web3d-M3: a class's [`ClassDef::field_layout`]: the parent's
/// layout, then this class's own fields in sorted order (new names
/// appended; defaults for inherited names overwrite in place).
fn field_layout(
    parent: Option<&Rc<ClassDef>>,
    own: &HashMap<String, TaggedValue>,
) -> Vec<(Rc<str>, TaggedValue)> {
    let mut layout = parent.map(|p| p.field_layout.clone()).unwrap_or_default();
    let mut names: Vec<&String> = own.keys().collect();
    names.sort();
    for name in names {
        let v = own[name];
        match layout.iter_mut().find(|(n, _)| **n == **name) {
            Some(slot) => slot.1 = v,
            None => layout.push((Rc::from(name.as_str()), v)),
        }
    }
    layout
}

fn instantiate(class: Rc<ClassDef>) -> Value {
    let fields = crate::value::Fields::from_layout(&class.field_layout);
    let rc = Rc::new(RefCell::new(Instance {
        class,
        fields,
        current_state: None,
        every_timers: Vec::new(),
        every_intervals_secs: Vec::new(),
        despawned: false,
        death_fired: false,
        fiber_frames: Vec::new(),
        entry_wait_remaining: 0.0,
        predicate_last_values: Vec::new(),
        cached_value: None,
    }));
    // Seed the cached wrapper so later per-frame calls reuse it.
    instance_value(&rc)
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
    let _profile = crate::profile::scope("method");
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
        std::borrow::Cow::Borrowed(args)
    } else {
        let param_refs: Vec<&str> = method.params.iter().map(|s| &**s).collect();
        std::borrow::Cow::Owned(bind_kwargs(
            &param_refs,
            "method",
            args.to_vec(),
            kwargs.to_vec(),
            line,
            col,
        )?)
    };
    let saved_self = env.self_value.replace(recv);
    let saved_returning = env.returning.take();
    let home = frame_home(env, &method.home);
    push_call_frame(env, &method.params, &bound, home);
    env.call_depth += 1;
    let body_result = run_block(env, &method.body);
    env.call_depth -= 1;
    pop_frame(env);
    let return_value = env.returning.take().unwrap_or(Value::NIL);
    env.returning = saved_returning;
    env.self_value = saved_self;
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

/// Snapshot the values of an iterable (range / list / tuple) into a Vec.
/// Shared by list comprehensions; mirrors the per-kind dispatch in
/// `run_for`. Lists/tuples are cloned so mutation during iteration can't
/// invalidate the walk.
fn iterable_snapshot(val: &Value, line: u32, col: u32) -> Result<Vec<Value>, RuntimeError> {
    if val.is_range() {
        let (start, end, exclusive) = val.as_range();
        let limit = if exclusive { end } else { end + 1 };
        Ok((start..limit).map(Value::from_int).collect())
    } else if val.is_list() {
        Ok(val.as_list().borrow().clone())
    } else if val.is_tuple() {
        Ok(val.as_tuple().iter().cloned().collect())
    } else {
        Err(RuntimeError {
            line,
            col,
            message: format!(
                "comprehension iterable must be a range, list, or tuple, got {}",
                val.type_name()
            ),
            help: Some("iterate over `0..n`, a list, or a tuple".to_string()),
        })
    }
}

fn run_for(
    env: &mut Env,
    var: &str,
    var_res: &crate::ast::ResCell,
    iter: &Expr,
    body: &[Stmt],
    line: u32,
    col: u32,
) -> Result<(), RuntimeError> {
    let iter_val = eval_expr(env, iter)?;
    let saved = loop_var_save_resolved(env, var, var_res);
    // web3d-M0: the iterable, its element snapshot, and the shadowed
    // loop variable live only on the Rust stack while the body runs
    // statements (safepoints). Root them for the loop's duration — the
    // snapshot elements individually, since the body may remove them
    // from the underlying list.
    let roots = crate::heap::RootScope::new();
    roots.push(iter_val);
    if let Some(v) = saved {
        roots.push(v);
    }
    let result = if iter_val.is_range() {
        let (start, end, exclusive) = iter_val.as_range();
        let limit = if exclusive { end } else { end + 1 };
        run_for_iter(env, var, var_res, body, (start..limit).map(Value::from_int))
    } else if iter_val.is_list() {
        let rc = iter_val.as_list();
        let snapshot: Vec<Value> = rc.borrow().clone();
        roots.push_all(&snapshot);
        run_for_iter(env, var, var_res, body, snapshot.into_iter())
    } else if iter_val.is_tuple() {
        let elems = iter_val.as_tuple();
        let snapshot: Vec<Value> = elems.iter().cloned().collect();
        roots.push_all(&snapshot);
        run_for_iter(env, var, var_res, body, snapshot.into_iter())
    } else {
        let other = iter_val;
        Err(RuntimeError {
            line,
            col,
            message: format!(
                "for-loop iterable must be a range, list, or tuple, got {}",
                other.type_name()
            ),
            help: None,
        })
    };
    loop_var_restore_resolved(env, var, var_res, saved);
    result
}

fn run_for_iter<I: Iterator<Item = Value>>(
    env: &mut Env,
    var: &str,
    var_res: &crate::ast::ResCell,
    body: &[Stmt],
    items: I,
) -> Result<(), RuntimeError> {
    for item in items {
        declare_resolved(env, var, var_res, item);
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

/// web3d-M3: a class's merged `look:` — its own keys over its parent's
/// (`docs/06` §4.9a). Rejects unknown and duplicate keys, and marks
/// each key per-entity or shared (see [`look_reads_entity`]).
fn build_look(
    env: &Env,
    parent: Option<&Rc<ClassDef>>,
    keys: Option<&[crate::ast::LookKey]>,
    own_fields: &HashMap<String, TaggedValue>,
) -> Result<Option<Rc<crate::value::LookDef>>, RuntimeError> {
    let inherited = parent.and_then(|p| p.look.clone());
    let Some(keys) = keys else {
        return Ok(inherited);
    };
    let mut look = inherited.as_deref().cloned().unwrap_or_default();
    let is_field = |name: &str| {
        if own_fields.contains_key(name) {
            return true;
        }
        let mut c = parent.cloned();
        while let Some(class) = c {
            if class.field_defaults.contains_key(name) {
                return true;
            }
            c = class.parent.clone();
        }
        false
    };
    let mut seen: Vec<&str> = Vec::new();
    for k in keys {
        if let Some((message, help)) = crate::ast::look_key_problem(&k.key) {
            return Err(RuntimeError {
                line: k.line,
                col: k.col,
                message,
                help: Some(help),
            });
        }
        if seen.contains(&k.key.as_str()) {
            return Err(RuntimeError {
                line: k.line,
                col: k.col,
                message: format!("look key `{}` is set twice", k.key),
                help: Some("keep one line per key".to_string()),
            });
        }
        seen.push(&k.key);
        let slot = crate::value::LookSlot {
            expr: k.value.clone(),
            per_entity: look_reads_entity(&k.value, &is_field),
            home: env.current_module,
            line: k.line,
            col: k.col,
        };
        match k.key.as_str() {
            "mesh" => look.mesh = Some(slot),
            "tint" => look.tint = Some(slot),
            "scale" => look.scale = Some(slot),
            "facing" => look.facing = Some(slot),
            "material" => look.material = Some(slot),
            other => unreachable!("look_key_problem accepted `{other}`"),
        }
    }
    Ok(Some(Rc::new(look)))
}

/// Whether a look key must be evaluated per entity: it reads `self` or
/// a field, or calls something (a call may be impure — `random.float()`
/// must differ between entities). Anything else is the same for every
/// entity of the class in a frame, so it is evaluated once per class.
fn look_reads_entity(e: &Expr, is_field: &dyn Fn(&str) -> bool) -> bool {
    let any = |xs: &[Expr]| xs.iter().any(|x| look_reads_entity(x, is_field));
    match e {
        Expr::Str { .. }
        | Expr::Int { .. }
        | Expr::Float { .. }
        | Expr::Bool { .. }
        | Expr::Percent { .. }
        | Expr::Quantity { .. } => false,
        Expr::Ident { name, .. } => is_field(name),
        Expr::Interp { exprs, .. } => any(exprs),
        Expr::Tuple { elems, .. } | Expr::List { elems, .. } => any(elems),
        Expr::Field { object, .. } => look_reads_entity(object, is_field),
        Expr::Index { object, index, .. } => {
            look_reads_entity(object, is_field) || look_reads_entity(index, is_field)
        }
        Expr::Unary { operand, .. } => look_reads_entity(operand, is_field),
        Expr::Binary { left, right, .. } => {
            look_reads_entity(left, is_field) || look_reads_entity(right, is_field)
        }
        Expr::Range { start, end, .. } => {
            look_reads_entity(start, is_field) || look_reads_entity(end, is_field)
        }
        Expr::IfExpr {
            cond,
            then_expr,
            elifs,
            else_expr,
            ..
        } => {
            look_reads_entity(cond, is_field)
                || look_reads_entity(then_expr, is_field)
                || elifs
                    .iter()
                    .any(|(c, v)| look_reads_entity(c, is_field) || look_reads_entity(v, is_field))
                || look_reads_entity(else_expr, is_field)
        }
        Expr::SelfRef { .. } | Expr::Call { .. } | Expr::ListComp { .. } | Expr::Hole { .. } => {
            true
        }
    }
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
        {
            let __opt = lookup_name(env, p);
            if let Some(__t) = (__opt).as_ref() {
                if __t.is_class() {
                    let c = __t.as_class();
                    Some(c.clone())
                } else {
                    let other = *__t;
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
            } else {
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
    let mut methods = crate::value::NameMap::default();
    let mut states = HashMap::new();
    let mut initial_state: Option<String> = None;
    let mut own_look: Option<&[crate::ast::LookKey]> = None;
    for member in members {
        match member {
            DeclMember::Field {
                name: fname, value, ..
            } => {
                let v = eval_expr(env, value)?;
                field_defaults.insert(fname.clone(), v);
            }
            DeclMember::Look { keys, .. } => own_look = Some(keys),
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
                let param_names: Vec<Rc<str>> =
                    params.iter().map(|p| Rc::from(p.name.as_str())).collect();
                methods.insert(
                    mname.clone(),
                    Rc::new(MethodDef {
                        params: param_names,
                        body: body.clone(),
                        home: env.current_module,
                    }),
                );
            }
            DeclMember::InitialState { name: sname, .. } => {
                initial_state = Some(sname.clone());
            }
            DeclMember::State {
                name: sname,
                members: smembers,
                ..
            } => {
                let mut on_entry = Vec::new();
                let mut every_clocks = Vec::new();
                let mut on_render: Option<Vec<Stmt>> = None;
                let mut on_key_press: HashMap<String, Vec<Stmt>> = HashMap::new();
                let mut on_update: Option<OnUpdateHandler> = None;
                let mut on_predicates: Vec<crate::value::PredicateHandlerDef> = Vec::new();
                let mut on_exit: Option<Vec<Stmt>> = None;
                for sm in smembers {
                    match sm {
                        StateMember::Stmt(stmt) => on_entry.push(stmt.clone()),
                        // `on enter:` folds into the same on-entry stream
                        // as the bare body — one entry mechanism.
                        StateMember::OnEnter { body, .. } => {
                            on_entry.extend(body.iter().cloned());
                        }
                        StateMember::OnExit { body, .. } => {
                            on_exit = Some(body.clone());
                        }
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
                        StateMember::OnPredicate {
                            predicate, body, ..
                        } => {
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
                        on_exit,
                    }),
                );
            }
        }
    }

    if matches!(kind, DeclKind::Visual) {
        // web3d-M3: ready this visual for use as a mesh material. A
        // failure is kept, and reported only if a look uses it.
        let decl = Program {
            stmts: vec![Stmt::Decl {
                kind,
                name: name.to_string(),
                parent: None,
                members: members.to_vec(),
                deprecation: None,
                line: 0,
                col: 0,
            }],
        };
        let checked = crate::visual_check::check_program(&decl);
        let compiled = match checked.first() {
            Some(e) => Err(e.message.clone()),
            None => crate::visual_wgsl::compile_material(name, members).map_err(|e| e.message),
        };
        env.visual_materials.insert(name.to_string(), compiled);
    }
    if matches!(kind, DeclKind::Particles) {
        // web3d-M7: ready this block for the GPU; if it can't run there,
        // it runs on the CPU.
        let compiled = crate::particles_wgsl::compile(name, members);
        env.particle_classes.insert(name.to_string(), compiled);
    }
    let look = build_look(env, parent_class.as_ref(), own_look, &field_defaults)?;
    let field_layout = field_layout(parent_class.as_ref(), &field_defaults);
    let class = Rc::new(ClassDef {
        kind: kind.as_str(),
        name: name.to_string(),
        parent: parent_class,
        field_defaults,
        methods,
        states,
        initial_state,
        look,
        field_layout,
    });
    env.set(name.to_string(), Value::from_class(class.clone()));

    // Scenes auto-instantiate at declaration time and become the active
    // scene. There's only one active scene per program in v0.1.
    if matches!(kind, DeclKind::Scene) {
        let inst = {
            let __t = instantiate(class.clone());
            if __t.is_instance() {
                __t.as_instance()
            } else {
                unreachable!("instantiate always returns Instance")
            }
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
        return if is_truthy(&l) {
            eval_expr(env, right)
        } else {
            Ok(l)
        };
    }
    if matches!(op, BinOp::Or) {
        let l = eval_expr(env, left)?;
        return if is_truthy(&l) {
            Ok(l)
        } else {
            eval_expr(env, right)
        };
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

fn value_in(needle: &Value, haystack: &Value, line: u32, col: u32) -> Result<bool, RuntimeError> {
    if haystack.is_list() {
        let rc = haystack.as_list();
        let answer = rc.borrow().iter().any(|v| values_equal(v, needle));
        Ok(answer)
    } else if haystack.is_tuple() {
        let elems = haystack.as_tuple();
        Ok(elems.iter().any(|v| values_equal(v, needle)))
    } else if haystack.is_range() {
        let (start, end, exclusive) = haystack.as_range();
        if needle.is_int_or_boxed_int() {
            let n = needle.as_int();
            let upper = if exclusive { end } else { end + 1 };
            Ok(n >= start && n < upper)
        } else {
            Ok(false)
        }
    } else if haystack.is_str() {
        let s = haystack.as_string();
        if needle.is_str() {
            let sub = needle.as_string();
            Ok(s.contains(sub.as_str()))
        } else {
            Ok(false)
        }
    } else {
        let other = *haystack;
        Err(RuntimeError {
            line,
            col,
            message: format!(
                "`in` expects a list, tuple, range, or string, got {}",
                other.type_name()
            ),
            help: None,
        })
    }
}

fn apply_arith(
    op: BinOp,
    l: &Value,
    r: &Value,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    // web3d-M3: float with float first — the common case in game code,
    // and the same result the general path below computes.
    if l.is_float() && r.is_float() {
        let (a, b) = (l.as_float(), r.as_float());
        return Ok(Value::from_float(match op {
            BinOp::Add => a + b,
            BinOp::Sub => a - b,
            BinOp::Mul => a * b,
            BinOp::Div => a / b,
            _ => unreachable!(),
        }));
    }
    let op_str = match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        _ => unreachable!(),
    };
    // String concatenation via `+`.
    if matches!(op, BinOp::Add) && l.is_str() && r.is_str() {
        let a = l.as_string();
        let b = r.as_string();
        let mut s = String::with_capacity(a.len() + b.len());
        s.push_str(a.as_str());
        s.push_str(b.as_str());
        return Ok(Value::from_string(s));
    }
    // Tuple arithmetic — element-wise add/sub between same-length tuples
    // (Snake's `snake[0] + direction` shape) and tuple * scalar (Snake's
    // `cell * cell_size`).
    if l.is_tuple() && r.is_tuple() {
        let a = l.as_tuple();
        let b = r.as_tuple();
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
            return Ok(Value::from_tuple(out_elems));
        }
    }
    if l.is_tuple() {
        let elems = l.as_tuple();
        if matches!(op, BinOp::Mul | BinOp::Div) && is_scalar(r) {
            let mut out_elems = Vec::with_capacity(elems.len());
            for x in elems.iter() {
                out_elems.push(apply_arith(op, x, r, line, col)?);
            }
            return Ok(Value::from_tuple(out_elems));
        }
    }
    if r.is_tuple() {
        let elems = r.as_tuple();
        if matches!(op, BinOp::Mul) && is_scalar(l) {
            let mut out_elems = Vec::with_capacity(elems.len());
            for y in elems.iter() {
                out_elems.push(apply_arith(op, l, y, line, col)?);
            }
            return Ok(Value::from_tuple(out_elems));
        }
    }
    let pair = if l.is_int_or_boxed_int() && r.is_int_or_boxed_int() {
        NumPair::Ints(l.as_int(), r.as_int())
    } else if l.is_float() && r.is_float() {
        NumPair::Floats(l.as_float(), r.as_float())
    } else if l.is_int_or_boxed_int() && r.is_float() {
        NumPair::Floats(l.as_int() as f64, r.as_float())
    } else if l.is_float() && r.is_int_or_boxed_int() {
        NumPair::Floats(l.as_float(), r.as_int() as f64)
    } else {
        return Err(RuntimeError {
            line,
            col,
            message: format!(
                "operator '{op_str}' is not defined on {} and {}",
                l.type_name(),
                r.type_name()
            ),
            help: None,
        });
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
    v.is_number()
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
    if l.is_int_or_boxed_int() && r.is_int_or_boxed_int() {
        Ok(Value::from_bool(int_cmp(l.as_int(), r.as_int())))
    } else if l.is_float() && r.is_float() {
        Ok(Value::from_bool(float_cmp(l.as_float(), r.as_float())))
    } else if l.is_int_or_boxed_int() && r.is_float() {
        Ok(Value::from_bool(float_cmp(l.as_int() as f64, r.as_float())))
    } else if l.is_float() && r.is_int_or_boxed_int() {
        Ok(Value::from_bool(float_cmp(l.as_float(), r.as_int() as f64)))
    } else {
        Err(RuntimeError {
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
}

fn values_equal(l: &Value, r: &Value) -> bool {
    l.equals(r)
}

fn is_truthy(v: &Value) -> bool {
    v.is_truthy()
}
