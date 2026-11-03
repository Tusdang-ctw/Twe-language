//! web3d (2026-10-08): the `mesh_anim.*` builtins (clip playback,
//! crossfade, the 1-D blend space) split out of the stdlib god-module.
//! The MESH_ANIM_STATE thread-local stays in the parent (it sits in the
//! central thread_local! block); this module reaches it via `use super::*`.

use super::*;

use crate::render3d_types::AnimEntry;

/// web3d-M11 session 1: default auto-crossfade when `play()` changes the
/// clip — smooths walk↔idle↔attack transitions instead of popping.
const DEFAULT_CROSSFADE: f32 = 0.15;

/// What a `mesh_anim.*` call animates.
pub(super) enum AnimTarget {
    /// web3d-M4: a mesh, named by the path its draws use (`look: mesh:
    /// "hero.glb"` / `mesh("hero.glb", ...)`), which is the renderer's
    /// mesh id. Every entity drawing it without an animation of its own
    /// shares it. (An integer handle is still accepted.)
    Mesh(u32),
    /// Lanternfall E4: one entity (`mesh_anim.play(self, ...)`): its look
    /// is posed by its own animation, apart from every other entity.
    Entity(Rc<RefCell<crate::value::Instance>>),
}

pub(super) fn anim_handle(env: &mut Env, v: &Value, what: &str) -> Result<AnimTarget, RuntimeError> {
    if v.is_instance() {
        return Ok(AnimTarget::Entity(v.as_instance()));
    }
    if v.is_str() {
        return Ok(AnimTarget::Mesh(env.intern_mesh_path(&v.as_string())));
    }
    handle_int(v, what).map(AnimTarget::Mesh).map_err(|mut e| {
        e.help = Some(format!(
            "pass an entity (`{what}(self, ...)`) or the mesh's path (`{what}(\"assets/hero.glb\", ...)`)"
        ));
        e
    })
}

/// Run `f` on the target's animation, creating it the first time. An
/// entity's first animation moves its look onto the per-entity path, so
/// the gathered looks are refreshed (`bump_look_epoch`).
fn with_entry<R>(target: &AnimTarget, f: impl FnOnce(&mut AnimEntry) -> R) -> R {
    match target {
        AnimTarget::Mesh(handle) => {
            MESH_ANIM_STATE.with(|s| f(s.borrow_mut().entry(*handle).or_default()))
        }
        AnimTarget::Entity(inst) => {
            let mut inst = inst.borrow_mut();
            if inst.anim.is_none() {
                crate::value::bump_look_epoch();
            }
            f(inst.anim.get_or_insert_with(Box::default))
        }
    }
}

/// The target's animation as it stands (stopped when it has none).
fn current_entry(target: &AnimTarget) -> AnimEntry {
    match target {
        AnimTarget::Mesh(handle) => {
            MESH_ANIM_STATE.with(|s| s.borrow().get(handle).cloned().unwrap_or_default())
        }
        AnimTarget::Entity(inst) => inst.borrow().anim.as_deref().cloned().unwrap_or_default(),
    }
}

pub(super) fn mesh_play_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "mesh.play")?;
    let target = anim_handle(env, &args[0], "mesh_anim.play")?;
    let clip = string_arg(&args[1], "mesh.play", "clip")?;
    let looping = if args[2].is_bool() {
        args[2].as_bool()
    } else {
        false
    };
    with_entry(&target, |entry| {
        let switching = entry.clip != clip;
        if switching {
            let cf = entry.crossfade_config.unwrap_or(DEFAULT_CROSSFADE);
            // web3d-M11 s1: crossfade from the current clip to the new one
            // (the old clip becomes the blend target, fully weighted, and
            // ramps out over `cf`). A hard switch when there's no prior
            // clip or the crossfade is disabled.
            if cf > 0.0 && !entry.clip.is_empty() {
                entry.blend_clip = Some(std::mem::take(&mut entry.clip));
                entry.blend_time = entry.time;
                entry.blend_t = 1.0;
                entry.crossfade_remaining = cf;
                entry.crossfade_duration = cf;
            } else {
                entry.blend_clip = None;
                entry.blend_t = 0.0;
                entry.crossfade_remaining = 0.0;
            }
            entry.time = 0.0;
        }
        entry.clip = clip;
        entry.looping = looping;
    });
    Ok(Value::NIL)
}

pub(super) fn mesh_stop_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mesh.stop")?;
    match anim_handle(env, &args[0], "mesh_anim.stop")? {
        AnimTarget::Mesh(handle) => MESH_ANIM_STATE.with(|s| {
            s.borrow_mut().remove(&handle);
        }),
        // The entity goes back to the mesh's shared animation.
        AnimTarget::Entity(inst) => {
            if inst.borrow_mut().anim.take().is_some() {
                crate::value::bump_look_epoch();
            }
        }
    }
    Ok(Value::NIL)
}

pub(super) fn mesh_blend_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "mesh.blend")?;
    let target = anim_handle(env, &args[0], "mesh_anim.blend")?;
    let clip_a = string_arg(&args[1], "mesh.blend", "clip_a")?;
    let clip_b = string_arg(&args[2], "mesh.blend", "clip_b")?;
    let t = (number(&args[3], "mesh.blend.t")? as f32).clamp(0.0, 1.0);
    with_entry(&target, |entry| {
        if entry.clip != clip_a {
            entry.clip = clip_a;
            entry.time = 0.0;
            entry.blend_time = 0.0;
        }
        entry.blend_clip = Some(clip_b);
        entry.blend_t = t;
        // Explicit blend takes over from any auto-crossfade; the partner
        // clip plays in sync with the primary.
        entry.crossfade_remaining = 0.0;
        entry.blend_time = entry.time;
    });
    Ok(Value::NIL)
}

/// Read a Twe list argument as `Vec<String>` for the anim builtins.
pub(super) fn anim_string_list(v: &Value, what: &str, field: &str) -> Result<Vec<String>, RuntimeError> {
    if !v.is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: `{field}` must be a list of clip names, got {}", v.type_name()),
            help: Some(format!("e.g. `{what}(hero, [\"idle\", \"walk\", \"run\"], [0, 3, 6], speed)`")),
        });
    }
    let list = v.as_list();
    let borrowed = list.borrow();
    let mut out = Vec::with_capacity(borrowed.len());
    for item in borrowed.iter() {
        if !item.is_str() {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("{what}: every `{field}` entry must be a clip name (string), got {}", item.type_name()),
                help: None,
            });
        }
        out.push(item.as_string());
    }
    Ok(out)
}

/// Read a Twe list argument as `Vec<f32>` for the anim builtins.
pub(super) fn anim_f32_list(v: &Value, what: &str, field: &str) -> Result<Vec<f32>, RuntimeError> {
    if !v.is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: `{field}` must be a list of numbers, got {}", v.type_name()),
            help: Some(format!("e.g. `{what}(hero, [\"idle\", \"walk\", \"run\"], [0, 3, 6], speed)`")),
        });
    }
    let list = v.as_list();
    let borrowed = list.borrow();
    let mut out = Vec::with_capacity(borrowed.len());
    for item in borrowed.iter() {
        out.push(number(item, &format!("{what}.{field}"))? as f32);
    }
    Ok(out)
}

/// web3d-M11 follow-on: a 1-D locomotion blend space. `clips` are clip
/// names at ascending `stops` (e.g. idle@0, walk@3.2, run@6.5); `value`
/// (say the ground speed) picks the two bracketing clips and the weight
/// between them. Below the first stop it plays the first clip; above the
/// last, the last. The two active clips share one clock, so gaits stay
/// foot-synced, and nothing resets when `value` crosses a stop — it is a
/// continuous weighted blend, not a discrete `play()` switch. It reduces
/// to the existing 2-clip blend (`clip` ⊕ `blend_clip` by `blend_t`), so
/// the renderer is unchanged.
pub(super) fn mesh_blend_space_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "mesh.blend_space")?;
    let target = anim_handle(env, &args[0], "mesh_anim.blend_space")?;
    let clips = anim_string_list(&args[1], "mesh_anim.blend_space", "clips")?;
    let stops = anim_f32_list(&args[2], "mesh_anim.blend_space", "stops")?;
    let value = number(&args[3], "mesh.blend_space.value")? as f32;
    if clips.len() < 2 || clips.len() != stops.len() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "mesh_anim.blend_space: `clips` and `stops` must be the same length and at least 2 (got {} clips, {} stops)",
                clips.len(),
                stops.len()
            ),
            help: Some("e.g. `mesh_anim.blend_space(hero, [\"idle\", \"walk\", \"run\"], [0, 3.2, 6.5], speed)`".to_string()),
        });
    }
    if stops.windows(2).any(|w| w[1] < w[0]) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "mesh_anim.blend_space: `stops` must be in ascending order".to_string(),
            help: None,
        });
    }
    // Find the bracketing pair and the weight toward the upper clip.
    let last = stops.len() - 1;
    let (lo_i, hi_i, t) = if value <= stops[0] {
        (0, 0, 0.0)
    } else if value >= stops[last] {
        (last, last, 0.0)
    } else {
        let mut i = 0;
        while i < last && value > stops[i + 1] {
            i += 1;
        }
        let span = stops[i + 1] - stops[i];
        let t = if span > 0.0 { (value - stops[i]) / span } else { 0.0 };
        (i, i + 1, t.clamp(0.0, 1.0))
    };
    with_entry(&target, |entry| {
        entry.clip = clips[lo_i].clone();
        entry.looping = true;
        if lo_i == hi_i {
            entry.blend_clip = None;
            entry.blend_t = 0.0;
        } else {
            entry.blend_clip = Some(clips[hi_i].clone());
            entry.blend_t = t;
        }
        // The blend space owns the weight — cancel any timed crossfade,
        // and keep the two active clips phase-synced on the shared clock
        // (so neither foot-slides relative to the other).
        entry.crossfade_remaining = 0.0;
        entry.blend_time = entry.time;
    });
    Ok(Value::NIL)
}

/// web3d-M11 session 1: set the auto-crossfade length (seconds) `play()`
/// uses when it changes this mesh's clip. 0 = hard switch.
pub(super) fn mesh_set_crossfade_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "mesh.set_crossfade")?;
    let target = anim_handle(env, &args[0], "mesh_anim.set_crossfade")?;
    let secs = (number(&args[1], "mesh.set_crossfade.seconds")? as f32).max(0.0);
    with_entry(&target, |entry| entry.crossfade_config = Some(secs));
    Ok(Value::NIL)
}

pub(super) fn mesh_current_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mesh.current")?;
    let target = anim_handle(env, &args[0], "mesh_anim.current")?;
    let entry = current_entry(&target);
    let mut fields = HashMap::new();
    fields.insert("clip".to_string(), Value::from_string(entry.clip.clone()));
    fields.insert("time".to_string(), Value::from_float(entry.time as f64));
    fields.insert("looping".to_string(), Value::from_bool(entry.looping));
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "anim_state",
    }))))
}

/// Advance every animation by `dt`: each mesh's shared one and
/// (Lanternfall E4) each live entity's own.
pub(super) fn mesh_advance_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mesh.advance")?;
    let dt = number(&args[0], "mesh.advance.dt")? as f32;
    MESH_ANIM_STATE.with(|s| {
        for entry in s.borrow_mut().values_mut() {
            entry.advance(dt);
        }
    });
    for entity in &env.active_entities {
        if let Some(entry) = entity.borrow_mut().anim.as_deref_mut() {
            entry.advance(dt);
        }
    }
    Ok(Value::NIL)
}

/// Phase 24: expose the per-mesh animation state to the renderer
/// in a form decoupled from `Value` ownership. Returns an empty
/// snapshot (clip=""), which the renderer treats as "rest pose,
/// no animation," when the script never called `mesh_anim.play`
/// for this handle.
pub(crate) fn mesh_anim_state(handle: u32) -> crate::render3d_types::AnimSnapshot {
    MESH_ANIM_STATE.with(|s| {
        s.borrow()
            .get(&handle)
            .map(AnimEntry::snapshot)
            .unwrap_or_default()
    })
}

