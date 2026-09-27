//! web3d-M0: experimental stdlib namespaces, compiled only with
//! `--features experimental`.
//!
//! These namespaces shipped as scaffolding: author-facing APIs whose
//! runtimes are stubs, bookkeeping the renderer never consumes, or
//! no-op platform traits. Per `docs/changes/2026-09-27-web3d-pivot.md`
//! they stay out of the default build, the stdlib manifest, and the
//! LLM primer until a milestone makes them real:
//!
//! - `console.*`, `achievements.*` / `cloud_save.*` / `friends.*`
//!   (Phase 40 platform-trait stubs)
//! - `mmo.*`, `workshop.*` (Phase 41 single-player stubs)
//! - `rollback.*` (Phase 37 snapshot ring; no rewind engine)
//! - `world.*`, `terrain.*` (Phase 32 spatial / streaming / LOD / cull /
//!   instance bookkeeping; not consumed by the renderer)
//!
//! A child module of `stdlib`, so it reuses the parent's private
//! helpers (`arity`, `string_arg`, `as_f64`, …) via `use super::*`.

use super::*;

// ---------------------------------------------------------------
// Phase 40 sessions 2 + 3: console.* abstract controller + glyphs.
//
// Per `docs/changes/2026-05-11-console-targets-rfc.md` the public
// surface ships platform-agnostic abstractions; SDK-specific
// implementations live in partner private forks. `console.controller(i)`
// wraps the gamepad ambient (Phase 9 / gilrs on PC + Steam Deck);
// partner forks replace the wiring per platform.
//
// Button names use the **Xbox layout as canonical** (a / b / x / y).
// `console.glyph(button, style)` returns the per-style glyph string
// for UI rendering.
// ---------------------------------------------------------------

pub(super) fn install_console(env: &mut Env) {
    let mut c = HashMap::new();
    c.insert(
        "controller".to_string(),
        Value::from_builtin("console.controller", &["i"], console_controller),
    );
    c.insert(
        "controller_count".to_string(),
        Value::from_builtin("console.controller_count", &[], console_controller_count),
    );
    c.insert(
        "glyph".to_string(),
        Value::from_builtin("console.glyph", &["button", "style"], console_glyph),
    );
    c.insert(
        "glyph_asset".to_string(),
        Value::from_builtin(
            "console.glyph_asset",
            &["button", "style"],
            console_glyph_asset,
        ),
    );
    c.insert(
        "detect_style".to_string(),
        Value::from_builtin("console.detect_style", &[], console_detect_style),
    );
    env.set(
        "console".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: c,
            kind: "module",
        }))),
    );
}

/// Returns a controller record for the i-th connected gamepad.
/// `i = 0` reads from the existing `gamepad` / `gamepad_axis`
/// ambients (Phase 9 wires gilrs to controller 0). Higher indices
/// return `connected = false` today; multi-controller support is a
/// partner-fork extension per the Phase 40 RFC.
fn console_controller(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "console.controller")?;
    let i = as_i64(&args[0], "console.controller")?;
    if i < 0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("console.controller: i must be >= 0 (got {i})"),
            help: None,
        });
    }
    let mut fields: HashMap<String, Value> = HashMap::new();
    if i == 0 {
        // Read from Phase 9's gamepad ambient.
        let (a, b, x, y, lb, rb, lt_button, rt_button, start, select, dup, ddown, dleft, dright,
             connected) = read_gamepad_buttons(env);
        let (lx, ly, rx, ry, lt_axis, rt_axis) = read_gamepad_axes(env);
        fields.insert("connected".to_string(), Value::from_bool(connected));
        fields.insert("a".to_string(), Value::from_bool(a));
        fields.insert("b".to_string(), Value::from_bool(b));
        fields.insert("x".to_string(), Value::from_bool(x));
        fields.insert("y".to_string(), Value::from_bool(y));
        fields.insert("left_shoulder".to_string(), Value::from_bool(lb));
        fields.insert("right_shoulder".to_string(), Value::from_bool(rb));
        fields.insert("left_trigger".to_string(), Value::from_float(lt_axis));
        fields.insert("right_trigger".to_string(), Value::from_float(rt_axis));
        // Boolean trigger forms thresholded at 0.5 — convenience for
        // scripts that just want "is the trigger pulled".
        fields.insert(
            "left_trigger_pressed".to_string(),
            Value::from_bool(lt_button || lt_axis > 0.5),
        );
        fields.insert(
            "right_trigger_pressed".to_string(),
            Value::from_bool(rt_button || rt_axis > 0.5),
        );
        fields.insert("dpad_up".to_string(), Value::from_bool(dup));
        fields.insert("dpad_down".to_string(), Value::from_bool(ddown));
        fields.insert("dpad_left".to_string(), Value::from_bool(dleft));
        fields.insert("dpad_right".to_string(), Value::from_bool(dright));
        fields.insert("start".to_string(), Value::from_bool(start));
        fields.insert("select".to_string(), Value::from_bool(select));
        // Sticks as nested records — scripts read `pad.left_stick.x`.
        let mut ls: HashMap<String, Value> = HashMap::new();
        ls.insert("x".to_string(), Value::from_float(lx));
        ls.insert("y".to_string(), Value::from_float(ly));
        fields.insert(
            "left_stick".to_string(),
            Value::from_object(Rc::new(RefCell::new(Object {
                fields: ls,
                kind: "stick",
            }))),
        );
        let mut rs: HashMap<String, Value> = HashMap::new();
        rs.insert("x".to_string(), Value::from_float(rx));
        rs.insert("y".to_string(), Value::from_float(ry));
        fields.insert(
            "right_stick".to_string(),
            Value::from_object(Rc::new(RefCell::new(Object {
                fields: rs,
                kind: "stick",
            }))),
        );
        // L3 / R3 / Home are honest-deferred until the partner fork
        // (or a follow-on gilrs upgrade) wires them. Today report
        // false so scripts reading them get a definite answer.
        fields.insert("left_stick_button".to_string(), Value::from_bool(false));
        fields.insert("right_stick_button".to_string(), Value::from_bool(false));
        fields.insert("home".to_string(), Value::from_bool(false));
    } else {
        // Higher indices: scaffolding only — partner forks wire
        // multi-pad gilrs (or platform-native input) here.
        fill_disconnected_controller(&mut fields);
    }
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "controller",
    }))))
}

fn fill_disconnected_controller(fields: &mut HashMap<String, Value>) {
    fields.insert("connected".to_string(), Value::from_bool(false));
    for name in [
        "a",
        "b",
        "x",
        "y",
        "left_shoulder",
        "right_shoulder",
        "left_trigger_pressed",
        "right_trigger_pressed",
        "dpad_up",
        "dpad_down",
        "dpad_left",
        "dpad_right",
        "start",
        "select",
        "left_stick_button",
        "right_stick_button",
        "home",
    ] {
        fields.insert(name.to_string(), Value::from_bool(false));
    }
    fields.insert("left_trigger".to_string(), Value::from_float(0.0));
    fields.insert("right_trigger".to_string(), Value::from_float(0.0));
    for stick_name in ["left_stick", "right_stick"] {
        let mut s: HashMap<String, Value> = HashMap::new();
        s.insert("x".to_string(), Value::from_float(0.0));
        s.insert("y".to_string(), Value::from_float(0.0));
        fields.insert(
            stick_name.to_string(),
            Value::from_object(Rc::new(RefCell::new(Object {
                fields: s,
                kind: "stick",
            }))),
        );
    }
}

/// Tuple of 14 button bools + connected flag, returned from
/// `read_gamepad_buttons`. Keyed positionally so the caller binds
/// fields by name; the type alias keeps clippy's complex-type lint
/// happy.
type GamepadButtonState = (
    bool, // a
    bool, // b
    bool, // x
    bool, // y
    bool, // lb
    bool, // rb
    bool, // lt (boolean threshold)
    bool, // rt (boolean threshold)
    bool, // start
    bool, // select
    bool, // dpad up
    bool, // dpad down
    bool, // dpad left
    bool, // dpad right
    bool, // connected
);

fn read_gamepad_buttons(env: &Env) -> GamepadButtonState {
    let opt = env.get("gamepad");
    let v = match opt.as_ref() {
        Some(v) if v.is_object() => *v,
        _ => {
            return (
                false, false, false, false, false, false, false, false, false, false, false,
                false, false, false, false,
            )
        }
    };
    let rc = v.as_object();
    let o = rc.borrow();
    let g = |k: &str| {
        o.fields
            .get(k)
            .filter(|v| v.is_bool())
            .map(|v| v.as_bool())
            .unwrap_or(false)
    };
    (
        g("a"),
        g("b"),
        g("x"),
        g("y"),
        g("lb"),
        g("rb"),
        g("lt"),
        g("rt"),
        g("start"),
        g("select"),
        g("dup"),
        g("ddown"),
        g("dleft"),
        g("dright"),
        g("connected"),
    )
}

fn read_gamepad_axes(env: &Env) -> (f64, f64, f64, f64, f64, f64) {
    let opt = env.get("gamepad_axis");
    let v = match opt.as_ref() {
        Some(v) if v.is_object() => *v,
        _ => return (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
    };
    let rc = v.as_object();
    let o = rc.borrow();
    let g = |k: &str| {
        o.fields
            .get(k)
            .and_then(|v| {
                if v.is_float() {
                    Some(v.as_float())
                } else if v.is_int_or_boxed_int() {
                    Some(v.as_int() as f64)
                } else {
                    None
                }
            })
            .unwrap_or(0.0)
    };
    (g("lx"), g("ly"), g("rx"), g("ry"), g("lt"), g("rt"))
}

fn console_controller_count(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "console.controller_count")?;
    // Today: 1 if controller 0 reports connected, 0 otherwise.
    // Partner forks override with real multi-pad enumeration.
    let opt = env.get("gamepad");
    let connected = match opt.as_ref() {
        Some(v) if v.is_object() => {
            let rc = v.as_object();
            let o = rc.borrow();
            o.fields
                .get("connected")
                .filter(|f| f.is_bool())
                .map(|f| f.as_bool())
                .unwrap_or(false)
        }
        _ => false,
    };
    Ok(Value::from_int(if connected { 1 } else { 0 }))
}

/// Per-style glyph string for a canonical (Xbox-named) button.
/// Empty string for unknown buttons.
fn console_glyph(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "console.glyph")?;
    let button = string_arg(&args[0], "console.glyph", "button")?;
    let style = string_arg(&args[1], "console.glyph", "style")?;
    let resolved_style = if style == "auto" {
        // Auto-detect from connected controller — today always falls
        // back to xbox since the gilrs path doesn't surface the
        // controller's vendor. Partner forks override this.
        "xbox".to_string()
    } else {
        style
    };
    Ok(Value::from_string(
        glyph_lookup(&button, &resolved_style).to_string(),
    ))
}

fn glyph_lookup(button: &str, style: &str) -> &'static str {
    match (style, button) {
        ("xbox", "a") => "(A)",
        ("xbox", "b") => "(B)",
        ("xbox", "x") => "(X)",
        ("xbox", "y") => "(Y)",
        ("xbox", "left_shoulder") => "[LB]",
        ("xbox", "right_shoulder") => "[RB]",
        ("xbox", "left_trigger") => "[LT]",
        ("xbox", "right_trigger") => "[RT]",
        ("xbox", "left_stick_button") => "[L3]",
        ("xbox", "right_stick_button") => "[R3]",
        ("xbox", "start") => "[Menu]",
        ("xbox", "select") => "[View]",
        ("playstation", "a") => "✕",
        ("playstation", "b") => "◯",
        ("playstation", "x") => "□",
        ("playstation", "y") => "△",
        ("playstation", "left_shoulder") => "[L1]",
        ("playstation", "right_shoulder") => "[R1]",
        ("playstation", "left_trigger") => "[L2]",
        ("playstation", "right_trigger") => "[R2]",
        ("playstation", "left_stick_button") => "[L3]",
        ("playstation", "right_stick_button") => "[R3]",
        ("playstation", "start") => "[Options]",
        ("playstation", "select") => "[Share]",
        ("switch", "a") => "(A)",
        ("switch", "b") => "(B)",
        ("switch", "x") => "(X)",
        ("switch", "y") => "(Y)",
        ("switch", "left_shoulder") => "[L]",
        ("switch", "right_shoulder") => "[R]",
        ("switch", "left_trigger") => "[ZL]",
        ("switch", "right_trigger") => "[ZR]",
        ("switch", "left_stick_button") => "[LS]",
        ("switch", "right_stick_button") => "[RS]",
        ("switch", "start") => "[+]",
        ("switch", "select") => "[-]",
        _ => "",
    }
}

/// Asset key for a glyph sprite. Returns a key like
/// `"glyph/xbox/a.png"` that scripts pass to `image()`. The asset
/// itself ships with partner forks (signed glyphs from the platform
/// SDK); the open-source repo does not bundle the platform-owned
/// glyphs. Returns empty string when no asset is available.
fn console_glyph_asset(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "console.glyph_asset")?;
    let button = string_arg(&args[0], "console.glyph_asset", "button")?;
    let style = string_arg(&args[1], "console.glyph_asset", "style")?;
    if button.is_empty() || style.is_empty() {
        return Ok(Value::from_string(String::new()));
    }
    Ok(Value::from_string(format!("glyph/{style}/{button}.png")))
}

/// Returns the detected glyph style for the connected controller.
/// Today always returns `"xbox"` (matches the gilrs PC path); partner
/// forks override per-platform.
fn console_detect_style(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "console.detect_style")?;
    Ok(Value::from_string("xbox".to_string()))
}

// ---------------------------------------------------------------
// Phase 40 session 4: platform-service traits.
//
// `achievements.unlock` / `cloud_save.save` / `friends.list` — trait
// stubs that route through `crate::steam::*` on Steam builds, no-op
// on every other build. Partner forks provide platform-specific
// implementations behind feature flags (analogous to `--features
// steam` / `--features steam-net`).
// ---------------------------------------------------------------

pub(super) fn install_platform_services(env: &mut Env) {
    let mut a = HashMap::new();
    a.insert(
        "unlock".to_string(),
        Value::from_builtin("achievements.unlock", &["id"], achievements_unlock),
    );
    a.insert(
        "is_unlocked".to_string(),
        Value::from_builtin(
            "achievements.is_unlocked",
            &["id"],
            achievements_is_unlocked,
        ),
    );
    env.set(
        "achievements".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: a,
            kind: "module",
        }))),
    );

    let mut c = HashMap::new();
    c.insert(
        "save".to_string(),
        Value::from_builtin("cloud_save.save", &["slot", "value"], cloud_save_save),
    );
    c.insert(
        "load".to_string(),
        Value::from_builtin("cloud_save.load", &["slot"], cloud_save_load),
    );
    env.set(
        "cloud_save".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: c,
            kind: "module",
        }))),
    );

    let mut f = HashMap::new();
    f.insert(
        "list".to_string(),
        Value::from_builtin("friends.list", &[], friends_list),
    );
    f.insert(
        "is_friend".to_string(),
        Value::from_builtin("friends.is_friend", &["id"], friends_is_friend),
    );
    env.set(
        "friends".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: f,
            kind: "module",
        }))),
    );
}

fn achievements_unlock(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "achievements.unlock")?;
    // Route through the Phase 15 Steam achievement path. On non-Steam
    // builds this is a no-op (the Steam stub returns nil). Partner
    // forks add platform-specific routes alongside.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = crate::steam::achievement_unlock(env, args);
    }
    let _ = env;
    Ok(Value::NIL)
}

fn achievements_is_unlocked(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "achievements.is_unlocked")?;
    // No-op for the open-source repo. Partner forks query the
    // platform-specific achievement state.
    Ok(Value::from_bool(false))
}

fn cloud_save_save(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "cloud_save.save")?;
    let slot = string_arg(&args[0], "cloud_save.save", "slot")?;
    let payload = args[1].display();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = crate::steam::cloud_save(
            env,
            &[
                Value::from_string(slot),
                Value::from_string(payload),
            ],
        );
    }
    // No cloud backend in the browser build.
    #[cfg(target_arch = "wasm32")]
    let _ = (slot, payload);
    let _ = env;
    Ok(Value::NIL)
}

fn cloud_save_load(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "cloud_save.load")?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::steam::cloud_load(env, args)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = env;
        let _ = args;
        Ok(Value::NIL)
    }
}

fn friends_list(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "friends.list")?;
    // Empty list on the open-source repo. Partner forks return the
    // platform-specific friend list (Steam Friends, PSN, etc).
    Ok(Value::from_list(Rc::new(RefCell::new(Vec::new()))))
}

fn friends_is_friend(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "friends.is_friend")?;
    Ok(Value::from_bool(false))
}

// ---------------------------------------------------------------
// Phase 41: mmo.* + workshop.* — MMO architecture stubs.
//
// Per `docs/changes/2026-05-11-mmo-rfc.md` the runtime is honest-
// deferred to a future-implementer-with-bandwidth opening the phase
// properly. This module ships the *author-facing API* that compiles
// + runs today as single-player no-ops, so scripts written against
// the contract keep working when (if) a server runtime appears.
//
// Naming convention:
//   - `mmo.replicate(name, value)` — declare replicated state.
//     Today no-op locally; future runtime broadcasts to peers.
//   - `mmo.persist(key, value)` / `mmo.load(key)` — server-side DB
//     stub. Today saves to a thread-local Map; future runtime
//     routes to SQL / Redis.
//   - `mmo.broadcast(channel, payload)` + `mmo.next_event()` —
//     event queue. Today scripts observe their own broadcasts;
//     future runtime routes to peers in the same shard.
//   - `mmo.entities_near(x, y, z, r)` — AOI query. Composes with
//     `world.spatial_query_radius` directly; the server-side
//     version filters by what the player can see.
//   - `mmo.shard_id()` / `mmo.transfer_to(shard)` — sharding
//     lifecycle. Today returns "default"; future runtime returns
//     the active zone name and handles cross-shard handoff.
// ---------------------------------------------------------------

thread_local! {
    /// Persistent-world database stub. Future runtime replaces this
    /// with a SQL / Redis route. (Non-const init because
    /// `HashMap::new()` with the default RandomState hasher isn't
    /// const-callable.)
    #[allow(clippy::missing_const_for_thread_local)]
    static MMO_DB: RefCell<HashMap<String, crate::json::Value>> =
        RefCell::new(HashMap::new());
    /// Event queue stub. `mmo.broadcast` pushes onto this; the local
    /// script's `mmo.next_event` drains it. Future runtime routes
    /// to peers in the same shard.
    static MMO_EVENTS: RefCell<std::collections::VecDeque<(String, String, String)>> =
        const { RefCell::new(std::collections::VecDeque::new()) };
    /// Active shard id. Future runtime sets this on handoff. Stored
    /// behind a `RefCell<Option<String>>` so the init is const; `None`
    /// is the default-shard sentinel, surfaced as `"default"`.
    static MMO_SHARD_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub(super) fn install_mmo(env: &mut Env) {
    let mut m = HashMap::new();
    m.insert(
        "replicate".to_string(),
        Value::from_builtin("mmo.replicate", &["name", "value"], mmo_replicate),
    );
    m.insert(
        "persist".to_string(),
        Value::from_builtin("mmo.persist", &["key", "value"], mmo_persist),
    );
    m.insert(
        "load".to_string(),
        Value::from_builtin("mmo.load", &["key"], mmo_load),
    );
    m.insert(
        "broadcast".to_string(),
        Value::from_builtin(
            "mmo.broadcast",
            &["channel", "payload"],
            mmo_broadcast,
        ),
    );
    m.insert(
        "next_event".to_string(),
        Value::from_builtin("mmo.next_event", &[], mmo_next_event),
    );
    m.insert(
        "entities_near".to_string(),
        Value::from_builtin(
            "mmo.entities_near",
            &["x", "y", "z", "radius"],
            mmo_entities_near,
        ),
    );
    m.insert(
        "shard_id".to_string(),
        Value::from_builtin("mmo.shard_id", &[], mmo_shard_id),
    );
    m.insert(
        "transfer_to".to_string(),
        Value::from_builtin("mmo.transfer_to", &["shard"], mmo_transfer_to),
    );
    env.set(
        "mmo".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: m,
            kind: "module",
        }))),
    );
}

/// Declare a replicated state slot. Today no-op — the value is
/// already local. Future runtime broadcasts the change to peers in
/// the same shard within the player's AOI.
fn mmo_replicate(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "mmo.replicate")?;
    let _name = string_arg(&args[0], "mmo.replicate", "name")?;
    // _value = args[1] — passed through unchanged today.
    Ok(Value::NIL)
}

/// Persist a value to the server-side DB. Today saves to a thread-
/// local map. Future runtime flushes through a snapshot ring buffer
/// to SQL / Redis.
fn mmo_persist(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "mmo.persist")?;
    let key = string_arg(&args[0], "mmo.persist", "key")?;
    let json = crate::save::encode(&args[1]).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: format!("mmo.persist: {m}"),
        help: None,
    })?;
    MMO_DB.with(|db| {
        db.borrow_mut().insert(key, json);
    });
    Ok(Value::NIL)
}

/// Read a previously-persisted value, or nil if absent.
fn mmo_load(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mmo.load")?;
    let key = string_arg(&args[0], "mmo.load", "key")?;
    let v = MMO_DB.with(|db| db.borrow().get(&key).cloned());
    Ok(v.map(|j| crate::save::decode(&j)).unwrap_or(Value::NIL))
}

/// Broadcast a one-shot event to peers in the same shard. Today the
/// event lands on the local event queue (so the local script can
/// observe its own broadcasts). Future runtime delivers to other
/// peers in the same AOI.
fn mmo_broadcast(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "mmo.broadcast")?;
    let channel = string_arg(&args[0], "mmo.broadcast", "channel")?;
    let payload = args[1].display();
    // Sender id is "local" today; future runtime fills in the real
    // SteamID / SessionID of the sending peer.
    MMO_EVENTS.with(|q| {
        q.borrow_mut().push_back(("local".to_string(), channel, payload));
    });
    Ok(Value::NIL)
}

/// Drain one event from the queue. Returns `{sender_id, channel,
/// payload}` or nil if empty.
fn mmo_next_event(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "mmo.next_event")?;
    let evt = MMO_EVENTS.with(|q| q.borrow_mut().pop_front());
    match evt {
        Some((sender, channel, payload)) => {
            let mut fields: HashMap<String, Value> = HashMap::new();
            fields.insert("sender_id".to_string(), Value::from_string(sender));
            fields.insert("channel".to_string(), Value::from_string(channel));
            fields.insert("payload".to_string(), Value::from_string(payload));
            Ok(Value::from_object(Rc::new(RefCell::new(Object {
                fields,
                kind: "mmo_event",
            }))))
        }
        None => Ok(Value::NIL),
    }
}

/// Area-of-interest query: which entities are near `(x, y, z)`
/// within `radius`? Composes with Phase 32's
/// `world.spatial_query_radius`. Today returns the same result as
/// the underlying spatial query; future runtime additionally
/// filters by what the player is allowed to see (visibility, friend
/// list, party membership).
fn mmo_entities_near(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "mmo.entities_near")?;
    let x = as_f64(&args[0], "mmo.entities_near")?;
    let y = as_f64(&args[1], "mmo.entities_near")?;
    let z = as_f64(&args[2], "mmo.entities_near")?;
    let r = as_f64(&args[3], "mmo.entities_near")?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let ids = crate::spatial::with_world(|w| {
            w.query_radius(x as f32, y as f32, z as f32, r as f32)
        });
        let items: Vec<Value> = ids
            .into_iter()
            .map(|id| Value::from_int(id as i64))
            .collect();
        Ok(Value::from_list(Rc::new(RefCell::new(items))))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (x, y, z, r);
        Ok(Value::from_list(Rc::new(RefCell::new(Vec::new()))))
    }
}

/// Active shard id. Today always `"default"`; future runtime sets
/// this on shard handoff.
fn mmo_shard_id(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "mmo.shard_id")?;
    let id = MMO_SHARD_ID.with(|s| {
        s.borrow()
            .clone()
            .unwrap_or_else(|| "default".to_string())
    });
    Ok(Value::from_string(id))
}

/// Request a transfer to `shard`. Today sets the local shard id
/// without any network coordination — useful for prototyping multi-
/// zone games as a single-player simulation. Future runtime
/// orchestrates the cross-shard handoff (serialise player state on
/// source, deserialise on destination, loading-screen UX).
fn mmo_transfer_to(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mmo.transfer_to")?;
    let shard = string_arg(&args[0], "mmo.transfer_to", "shard")?;
    MMO_SHARD_ID.with(|s| *s.borrow_mut() = Some(shard));
    Ok(Value::NIL)
}

// ---------------------------------------------------------------
// Phase 41 session 7: workshop.* — user-generated-content traits.
//
// Trait stubs for a Steam Workshop-style publishing pipeline. The
// Steam path could route through `steamworks::UGC` on
// `--features steam-workshop` (a follow-on feature flag); the open-
// source repo ships the contract + no-op fallbacks.
// ---------------------------------------------------------------

pub(super) fn install_workshop(env: &mut Env) {
    let mut w = HashMap::new();
    w.insert(
        "publish".to_string(),
        Value::from_builtin(
            "workshop.publish",
            &["title", "content_path"],
            workshop_publish,
        ),
    );
    w.insert(
        "list_subscribed".to_string(),
        Value::from_builtin(
            "workshop.list_subscribed",
            &[],
            workshop_list_subscribed,
        ),
    );
    w.insert(
        "install".to_string(),
        Value::from_builtin("workshop.install", &["id"], workshop_install),
    );
    env.set(
        "workshop".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: w,
            kind: "module",
        }))),
    );
}

fn workshop_publish(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "workshop.publish")?;
    // No-op on the open-source repo. Steam-feature route + future
    // server runtime fill in the real publish call.
    Ok(Value::NIL)
}

fn workshop_list_subscribed(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "workshop.list_subscribed")?;
    Ok(Value::from_list(Rc::new(RefCell::new(Vec::new()))))
}

fn workshop_install(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "workshop.install")?;
    // Returns false today — no actual workshop integration.
    Ok(Value::from_bool(false))
}


// ---------------------------------------------------------------
// Phase 37: rollback netcode builtins.
// ---------------------------------------------------------------

/// Install the `rollback.*` namespace. Per the Phase 37 RFC,
/// rollback is opt-in via `net.set_mode("rollback")`; this namespace
/// holds the rollback-specific knobs (input prediction, smoothing,
/// max rewind frames) plus the snapshot primitives the rewind
/// engine uses internally.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn install_rollback(env: &mut Env) {
    let mut r = HashMap::new();
    r.insert(
        "snapshot".to_string(),
        Value::from_builtin(
            "rollback.snapshot",
            &["name", "value"],
            rollback_snapshot,
        ),
    );
    r.insert(
        "restore".to_string(),
        Value::from_builtin("rollback.restore", &["name"], rollback_restore),
    );
    r.insert(
        "advance_tick".to_string(),
        Value::from_builtin(
            "rollback.advance_tick",
            &["tick"],
            rollback_advance_tick,
        ),
    );
    r.insert(
        "current_tick".to_string(),
        Value::from_builtin("rollback.current_tick", &[], rollback_current_tick),
    );
    r.insert(
        "discard_after".to_string(),
        Value::from_builtin(
            "rollback.discard_after",
            &["tick"],
            rollback_discard_after,
        ),
    );
    r.insert(
        "set_input_prediction".to_string(),
        Value::from_builtin(
            "rollback.set_input_prediction",
            &["policy"],
            rollback_set_input_prediction,
        ),
    );
    r.insert(
        "input_prediction".to_string(),
        Value::from_builtin(
            "rollback.input_prediction",
            &[],
            rollback_input_prediction,
        ),
    );
    r.insert(
        "set_smoothing".to_string(),
        Value::from_builtin(
            "rollback.set_smoothing",
            &["on"],
            rollback_set_smoothing,
        ),
    );
    r.insert(
        "smoothing".to_string(),
        Value::from_builtin("rollback.smoothing", &[], rollback_smoothing),
    );
    r.insert(
        "max_rewind_frames".to_string(),
        Value::from_builtin(
            "rollback.max_rewind_frames",
            &["n"],
            rollback_set_max_rewind_frames,
        ),
    );
    r.insert(
        "is_replaying".to_string(),
        Value::from_builtin("rollback.is_replaying", &[], rollback_is_replaying),
    );
    r.insert(
        "stats".to_string(),
        Value::from_builtin("rollback.stats", &[], rollback_stats),
    );
    env.set(
        "rollback".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: r,
            kind: "module",
        }))),
    );
}


#[cfg(not(target_arch = "wasm32"))]
fn rollback_snapshot(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "rollback.snapshot")?;
    let name = string_arg(&args[0], "rollback.snapshot", "name")?;
    crate::rollback::snapshot(&name, &args[1]).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: format!("rollback.snapshot: {m}"),
        help: None,
    })?;
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_restore(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.restore")?;
    let name = string_arg(&args[0], "rollback.restore", "name")?;
    Ok(crate::rollback::restore(&name).unwrap_or(Value::NIL))
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_advance_tick(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.advance_tick")?;
    let tick = as_i64(&args[0], "rollback.advance_tick")?;
    if !(0..=i64::from(u32::MAX)).contains(&tick) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("rollback.advance_tick: tick out of range (got {tick})"),
            help: None,
        });
    }
    crate::rollback::advance_tick(tick as u32);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_current_tick(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "rollback.current_tick")?;
    Ok(Value::from_int(crate::rollback::current_tick() as i64))
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_discard_after(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.discard_after")?;
    let tick = as_i64(&args[0], "rollback.discard_after")?;
    if !(0..=i64::from(u32::MAX)).contains(&tick) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("rollback.discard_after: tick out of range (got {tick})"),
            help: None,
        });
    }
    crate::rollback::discard_after(tick as u32);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_set_input_prediction(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.set_input_prediction")?;
    let s = string_arg(&args[0], "rollback.set_input_prediction", "policy")?;
    let p = crate::rollback::InputPrediction::parse(&s).ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!(
            "rollback.set_input_prediction: unknown policy {s:?} — expected \
             \"last-input-repeat\" or \"velocity-extrapolate\""
        ),
        help: None,
    })?;
    crate::rollback::set_input_prediction(p);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_input_prediction(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "rollback.input_prediction")?;
    Ok(Value::from_string(
        crate::rollback::input_prediction().as_str().to_string(),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_set_smoothing(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.set_smoothing")?;
    if !args[0].is_bool() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "rollback.set_smoothing: expected bool for `on`".to_string(),
            help: None,
        });
    }
    crate::rollback::set_smoothing(args[0].as_bool());
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_smoothing(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "rollback.smoothing")?;
    Ok(Value::from_bool(crate::rollback::smoothing()))
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_set_max_rewind_frames(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.max_rewind_frames")?;
    let n = as_i64(&args[0], "rollback.max_rewind_frames")?;
    if !(1..=60).contains(&n) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("rollback.max_rewind_frames: must be 1..=60 (got {n})"),
            help: None,
        });
    }
    crate::rollback::set_max_rewind_frames(n as u32);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_is_replaying(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "rollback.is_replaying")?;
    Ok(Value::from_bool(crate::rollback::is_replaying()))
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_stats(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "rollback.stats")?;
    let s = crate::rollback::stats();
    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("predicted".to_string(), Value::from_int(s.predicted as i64));
    fields.insert("corrected".to_string(), Value::from_int(s.corrected as i64));
    fields.insert(
        "last_correction_frames".to_string(),
        Value::from_int(s.last_correction_frames as i64),
    );
    fields.insert(
        "ring_len".to_string(),
        Value::from_int(s.ring_len as i64),
    );
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "rollback_stats",
    }))))
}


/// Phase 32: `world.*` namespace — spatial partitioning + chunked
/// streaming for open-world 3D. Sessions 2 and 3 ship the spatial
/// query API + the streaming state machine; later sessions plumb
/// these into the 3D renderer for LOD + occlusion + frustum culling.
///
/// The spatial structures live in `crate::spatial::WORLD` (a global
/// Mutex<Option<WorldSpatial>>) so engine-internal worker pool
/// integrations (Phase 32 session 1 lock revision) have somewhere
/// to share state. Scripts always go through these builtins; the
/// raw structures aren't exposed.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn install_world(env: &mut Env) {
    let mut w = HashMap::new();
    w.insert(
        "spatial_clear".to_string(),
        Value::from_builtin("world.spatial_clear", &[], world_spatial_clear),
    );
    w.insert(
        "spatial_insert_dynamic".to_string(),
        Value::from_builtin(
            "world.spatial_insert_dynamic",
            &["id", "x", "y", "z", "radius"],
            world_spatial_insert_dynamic,
        ),
    );
    w.insert(
        "spatial_remove_dynamic".to_string(),
        Value::from_builtin(
            "world.spatial_remove_dynamic",
            &["id"],
            world_spatial_remove_dynamic,
        ),
    );
    w.insert(
        "spatial_add_static".to_string(),
        Value::from_builtin(
            "world.spatial_add_static",
            &["id", "x", "y", "z", "radius"],
            world_spatial_add_static,
        ),
    );
    w.insert(
        "spatial_build_static".to_string(),
        Value::from_builtin("world.spatial_build_static", &[], world_spatial_build_static),
    );
    w.insert(
        "spatial_query_radius".to_string(),
        Value::from_builtin(
            "world.spatial_query_radius",
            &["x", "y", "z", "radius"],
            world_spatial_query_radius,
        ),
    );
    w.insert(
        "spatial_query_box".to_string(),
        Value::from_builtin(
            "world.spatial_query_box",
            &["x0", "y0", "z0", "x1", "y1", "z1"],
            world_spatial_query_box,
        ),
    );
    // ---- Phase 32 session 3: chunked streaming ----
    w.insert(
        "set_chunk_size".to_string(),
        Value::from_builtin(
            "world.set_chunk_size",
            &["meters"],
            world_set_chunk_size,
        ),
    );
    w.insert(
        "set_stream_radius".to_string(),
        Value::from_builtin(
            "world.set_stream_radius",
            &["chunks"],
            world_set_stream_radius,
        ),
    );
    w.insert(
        "set_stream_budget".to_string(),
        Value::from_builtin(
            "world.set_stream_budget",
            &["loads_per_frame", "unloads_per_frame"],
            world_set_stream_budget,
        ),
    );
    w.insert(
        "stream_step".to_string(),
        Value::from_builtin(
            "world.stream_step",
            &["camera_x", "camera_z"],
            world_stream_step,
        ),
    );
    w.insert(
        "mark_chunk_loaded".to_string(),
        Value::from_builtin(
            "world.mark_chunk_loaded",
            &["chunk_id"],
            world_mark_chunk_loaded,
        ),
    );
    w.insert(
        "mark_chunk_unloaded".to_string(),
        Value::from_builtin(
            "world.mark_chunk_unloaded",
            &["chunk_id"],
            world_mark_chunk_unloaded,
        ),
    );
    w.insert(
        "loaded_chunk_count".to_string(),
        Value::from_builtin("world.loaded_chunk_count", &[], world_loaded_chunk_count),
    );
    w.insert(
        "stream_clear".to_string(),
        Value::from_builtin("world.stream_clear", &[], world_stream_clear),
    );
    // ---- Phase 32 session 4: LOD chains ----
    w.insert(
        "set_lod_chain".to_string(),
        Value::from_builtin(
            "world.set_lod_chain",
            &["class", "assets", "switch_distances"],
            world_set_lod_chain,
        ),
    );
    w.insert(
        "lod_for_distance".to_string(),
        Value::from_builtin(
            "world.lod_for_distance",
            &["class", "distance"],
            world_lod_for_distance,
        ),
    );
    w.insert(
        "lod_index_for_distance".to_string(),
        Value::from_builtin(
            "world.lod_index_for_distance",
            &["class", "distance"],
            world_lod_index_for_distance,
        ),
    );
    w.insert(
        "clear_lod".to_string(),
        Value::from_builtin("world.clear_lod", &[], world_clear_lod),
    );
    // ---- Phase 32 session 6: frustum culling ----
    w.insert(
        "spatial_query_frustum".to_string(),
        Value::from_builtin(
            "world.spatial_query_frustum",
            &["matrix"],
            world_spatial_query_frustum,
        ),
    );
    w.insert(
        "frustum_contains_sphere".to_string(),
        Value::from_builtin(
            "world.frustum_contains_sphere",
            &["matrix", "x", "y", "z", "radius"],
            world_frustum_contains_sphere,
        ),
    );
    // ---- Phase 32 session 7: per-asset instance buckets ----
    w.insert(
        "instance_clear".to_string(),
        Value::from_builtin("world.instance_clear", &[], world_instance_clear),
    );
    w.insert(
        "instance_reset".to_string(),
        Value::from_builtin("world.instance_reset", &[], world_instance_reset),
    );
    w.insert(
        "instance_add".to_string(),
        Value::from_builtin(
            "world.instance_add",
            &["asset", "transform"],
            world_instance_add,
        ),
    );
    w.insert(
        "instance_count".to_string(),
        Value::from_builtin(
            "world.instance_count",
            &["asset"],
            world_instance_count,
        ),
    );
    w.insert(
        "instance_total".to_string(),
        Value::from_builtin("world.instance_total", &[], world_instance_total),
    );
    w.insert(
        "instance_bucket_count".to_string(),
        Value::from_builtin(
            "world.instance_bucket_count",
            &[],
            world_instance_bucket_count,
        ),
    );
    w.insert(
        "instance_assets".to_string(),
        Value::from_builtin("world.instance_assets", &[], world_instance_assets),
    );
    // ---- Phase 32 session 8: ergonomic helpers ----
    w.insert(
        "stream_radius_meters".to_string(),
        Value::from_builtin(
            "world.stream_radius_meters",
            &["meters"],
            world_stream_radius_meters,
        ),
    );
    w.insert(
        "entity_lod".to_string(),
        Value::from_builtin(
            "world.entity_lod",
            &["class", "lod_pairs"],
            world_entity_lod,
        ),
    );
    w.insert(
        "world_to_lod".to_string(),
        Value::from_builtin(
            "world.world_to_lod",
            &["class", "ex", "ey", "ez", "cx", "cy", "cz"],
            world_world_to_lod,
        ),
    );
    w.insert(
        "distance_xyz".to_string(),
        Value::from_builtin(
            "world.distance_xyz",
            &["ax", "ay", "az", "bx", "by", "bz"],
            world_distance_xyz,
        ),
    );
    env.set(
        "world".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: w,
            kind: "module",
        }))),
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_clear(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.spatial_clear")?;
    crate::spatial::with_world(|w| w.clear());
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_insert_dynamic(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 5, "world.spatial_insert_dynamic")?;
    let id = as_i64(&args[0], "world.spatial_insert_dynamic")? as u64;
    let x = as_f64(&args[1], "world.spatial_insert_dynamic")? as f32;
    let y = as_f64(&args[2], "world.spatial_insert_dynamic")? as f32;
    let z = as_f64(&args[3], "world.spatial_insert_dynamic")? as f32;
    let r = as_f64(&args[4], "world.spatial_insert_dynamic")? as f32;
    crate::spatial::with_world(|w| {
        w.insert_dynamic(id, crate::spatial::Aabb::from_center_radius(x, y, z, r));
    });
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_remove_dynamic(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.spatial_remove_dynamic")?;
    let id = as_i64(&args[0], "world.spatial_remove_dynamic")? as u64;
    let removed = crate::spatial::with_world(|w| w.remove_dynamic(id));
    Ok(Value::from_bool(removed))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_add_static(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 5, "world.spatial_add_static")?;
    let id = as_i64(&args[0], "world.spatial_add_static")? as u64;
    let x = as_f64(&args[1], "world.spatial_add_static")? as f32;
    let y = as_f64(&args[2], "world.spatial_add_static")? as f32;
    let z = as_f64(&args[3], "world.spatial_add_static")? as f32;
    let r = as_f64(&args[4], "world.spatial_add_static")? as f32;
    crate::spatial::with_world(|w| {
        w.add_static(id, crate::spatial::Aabb::from_center_radius(x, y, z, r));
    });
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_build_static(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.spatial_build_static")?;
    crate::spatial::with_world(|w| w.build_static());
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_query_radius(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "world.spatial_query_radius")?;
    let x = as_f64(&args[0], "world.spatial_query_radius")? as f32;
    let y = as_f64(&args[1], "world.spatial_query_radius")? as f32;
    let z = as_f64(&args[2], "world.spatial_query_radius")? as f32;
    let r = as_f64(&args[3], "world.spatial_query_radius")? as f32;
    let hits: Vec<Value> = crate::spatial::with_world(|w| w.query_radius(x, y, z, r))
        .into_iter()
        .map(|id| Value::from_int(id as i64))
        .collect();
    Ok(Value::from_list(Rc::new(RefCell::new(hits))))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_query_box(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 6, "world.spatial_query_box")?;
    let x0 = as_f64(&args[0], "world.spatial_query_box")? as f32;
    let y0 = as_f64(&args[1], "world.spatial_query_box")? as f32;
    let z0 = as_f64(&args[2], "world.spatial_query_box")? as f32;
    let x1 = as_f64(&args[3], "world.spatial_query_box")? as f32;
    let y1 = as_f64(&args[4], "world.spatial_query_box")? as f32;
    let z1 = as_f64(&args[5], "world.spatial_query_box")? as f32;
    let q = crate::spatial::Aabb {
        min: [x0.min(x1), y0.min(y1), z0.min(z1)],
        max: [x0.max(x1), y0.max(y1), z0.max(z1)],
    };
    let hits: Vec<Value> = crate::spatial::with_world(|w| w.query_box(&q))
        .into_iter()
        .map(|id| Value::from_int(id as i64))
        .collect();
    Ok(Value::from_list(Rc::new(RefCell::new(hits))))
}

// ---- Phase 32 session 3: chunked streaming builtins ----

#[cfg(not(target_arch = "wasm32"))]
fn world_set_chunk_size(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.set_chunk_size")?;
    let meters = as_f64(&args[0], "world.set_chunk_size")? as f32;
    if meters <= 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.set_chunk_size: meters must be positive (got {meters})"),
            help: None,
        });
    }
    crate::streaming::with_streaming(|s| s.chunk_size = meters);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_set_stream_radius(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.set_stream_radius")?;
    let chunks = as_i64(&args[0], "world.set_stream_radius")?;
    if !(1..=64).contains(&chunks) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.set_stream_radius: chunks must be 1..=64 (got {chunks})"),
            help: None,
        });
    }
    crate::streaming::with_streaming(|s| s.stream_radius_chunks = chunks as i32);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_set_stream_budget(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.set_stream_budget")?;
    let loads = as_i64(&args[0], "world.set_stream_budget")?.max(0) as u32;
    let unloads = as_i64(&args[1], "world.set_stream_budget")?.max(0) as u32;
    crate::streaming::with_streaming(|s| {
        s.loads_per_frame = loads;
        s.unloads_per_frame = unloads;
    });
    Ok(Value::NIL)
}

/// Compute one frame's streaming work given the camera position.
/// Returns a tuple `(to_load, to_unload)`, where each side is a list
/// of opaque chunk-id integers. The script forwards loaded chunks
/// to its asset loader (mesh / texture / NPC spawn), and confirms
/// completion via `world.mark_chunk_loaded` / `world.mark_chunk_unloaded`.
/// The actual asset I/O is the script's responsibility — this
/// function is pure bookkeeping.
#[cfg(not(target_arch = "wasm32"))]
fn world_stream_step(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.stream_step")?;
    let cx = as_f64(&args[0], "world.stream_step")? as f32;
    let cz = as_f64(&args[1], "world.stream_step")? as f32;
    let step = crate::streaming::with_streaming(|s| s.step(cx, cz));
    let to_load: Vec<Value> = step
        .to_load
        .iter()
        .map(|c| Value::from_int(c.0 as i64))
        .collect();
    let to_unload: Vec<Value> = step
        .to_unload
        .iter()
        .map(|c| Value::from_int(c.0 as i64))
        .collect();
    let load_list = Value::from_list(Rc::new(RefCell::new(to_load)));
    let unload_list = Value::from_list(Rc::new(RefCell::new(to_unload)));
    Ok(Value::from_tuple(Rc::new(vec![load_list, unload_list])))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_mark_chunk_loaded(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.mark_chunk_loaded")?;
    let id = as_i64(&args[0], "world.mark_chunk_loaded")? as u64;
    crate::streaming::with_streaming(|s| s.mark_loaded(crate::streaming::ChunkId(id)));
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_mark_chunk_unloaded(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.mark_chunk_unloaded")?;
    let id = as_i64(&args[0], "world.mark_chunk_unloaded")? as u64;
    crate::streaming::with_streaming(|s| s.mark_unloaded(crate::streaming::ChunkId(id)));
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_loaded_chunk_count(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.loaded_chunk_count")?;
    let n = crate::streaming::with_streaming(|s| s.loaded_count()) as i64;
    Ok(Value::from_int(n))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_stream_clear(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.stream_clear")?;
    crate::streaming::with_streaming(|s| s.clear());
    Ok(Value::NIL)
}

// ---- Phase 32 session 4: LOD-chain builtins ----

#[cfg(not(target_arch = "wasm32"))]
fn world_set_lod_chain(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "world.set_lod_chain")?;
    let class = string_arg(&args[0], "world.set_lod_chain", "class")?;
    let assets = list_of_strings(&args[1], "world.set_lod_chain", "assets")?;
    let switches = list_of_floats(&args[2], "world.set_lod_chain", "switch_distances")?;
    let chain = crate::lod::LodChain::new(assets, switches.iter().map(|f| *f as f32).collect())
        .map_err(|m| RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.set_lod_chain: {m}"),
            help: None,
        })?;
    crate::lod::with_table(|t| {
        t.insert(class, chain);
    });
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_lod_for_distance(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.lod_for_distance")?;
    let class = string_arg(&args[0], "world.lod_for_distance", "class")?;
    let distance = as_f64(&args[1], "world.lod_for_distance")? as f32;
    let asset = crate::lod::with_table(|t| {
        t.get(&class)
            .map(|chain| chain.asset_for_distance(distance).to_string())
    });
    match asset {
        Some(s) => Ok(Value::from_string(s)),
        None => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.lod_for_distance: no LOD chain registered for class '{class}'"),
            help: Some("call world.set_lod_chain(class, assets, switches) first".to_string()),
        }),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn world_lod_index_for_distance(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.lod_index_for_distance")?;
    let class = string_arg(&args[0], "world.lod_index_for_distance", "class")?;
    let distance = as_f64(&args[1], "world.lod_index_for_distance")? as f32;
    let idx = crate::lod::with_table(|t| t.get(&class).map(|chain| chain.select(distance) as i64));
    match idx {
        Some(i) => Ok(Value::from_int(i)),
        None => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "world.lod_index_for_distance: no LOD chain registered for class '{class}'"
            ),
            help: None,
        }),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn world_clear_lod(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.clear_lod")?;
    crate::lod::with_table(|t| t.clear());
    Ok(Value::NIL)
}

// ---- Phase 32 session 6: frustum-culling builtins ----

/// Read a 4x4 matrix from a Twe value: a list of 4 tuples of 4 floats
/// each (row-major), or a flat list of 16 floats. Either form is
/// accepted because scripts naturally produce both — `[(1.0, 0.0,
/// 0.0, 0.0), (0.0, 1.0, ...)]` mirrors GLSL row-format, while a
/// flat 16-element list is what comes back from a future
/// `camera.view_proj()` builtin.
#[cfg(not(target_arch = "wasm32"))]
fn read_matrix4x4(v: &Value, op: &str) -> Result<[[f32; 4]; 4], RuntimeError> {
    if !v.is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{op}: matrix must be a list (got {})", v.type_name()),
            help: None,
        });
    }
    let rc = v.as_list();
    let outer = rc.borrow();
    if outer.len() == 16 {
        // Flat row-major form.
        let mut m = [[0.0; 4]; 4];
        for (i, val) in outer.iter().enumerate() {
            m[i / 4][i % 4] = as_f64(val, op)? as f32;
        }
        return Ok(m);
    }
    if outer.len() != 4 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{op}: matrix must have 4 rows or 16 flat entries (got {})",
                outer.len()
            ),
            help: None,
        });
    }
    let mut m = [[0.0; 4]; 4];
    for (i, row_v) in outer.iter().enumerate() {
        let elems: Vec<Value> = if row_v.is_tuple() {
            row_v.as_tuple().iter().cloned().collect()
        } else if row_v.is_list() {
            let r = row_v.as_list();
            let cloned = r.borrow().iter().cloned().collect();
            cloned
        } else {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("{op}: matrix row {i} must be a tuple or list"),
                help: None,
            });
        };
        if elems.len() != 4 {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "{op}: matrix row {i} must have 4 elements (got {})",
                    elems.len()
                ),
                help: None,
            });
        }
        for (j, val) in elems.iter().enumerate() {
            m[i][j] = as_f64(val, op)? as f32;
        }
    }
    Ok(m)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_query_frustum(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.spatial_query_frustum")?;
    let m = read_matrix4x4(&args[0], "world.spatial_query_frustum")?;
    let frustum = crate::cull::Frustum::from_view_proj_row_major(m);
    let hits: Vec<Value> = crate::spatial::with_world(|w| w.query_frustum(&frustum))
        .into_iter()
        .map(|id| Value::from_int(id as i64))
        .collect();
    Ok(Value::from_list(Rc::new(RefCell::new(hits))))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_frustum_contains_sphere(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 5, "world.frustum_contains_sphere")?;
    let m = read_matrix4x4(&args[0], "world.frustum_contains_sphere")?;
    let x = as_f64(&args[1], "world.frustum_contains_sphere")? as f32;
    let y = as_f64(&args[2], "world.frustum_contains_sphere")? as f32;
    let z = as_f64(&args[3], "world.frustum_contains_sphere")? as f32;
    let r = as_f64(&args[4], "world.frustum_contains_sphere")? as f32;
    let frustum = crate::cull::Frustum::from_view_proj_row_major(m);
    Ok(Value::from_bool(frustum.may_contain_sphere(x, y, z, r)))
}

// ---- Phase 32 session 7: instance-bucket builtins ----

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_clear(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.instance_clear")?;
    crate::instance::with_buckets(|b| b.clear());
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_reset(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.instance_reset")?;
    crate::instance::with_buckets(|b| b.reset());
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_add(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.instance_add")?;
    let asset = string_arg(&args[0], "world.instance_add", "asset")?;
    let m = read_matrix4x4(&args[1], "world.instance_add")?;
    // Flatten row-major 4x4 to [f32; 16].
    let mut t = [0.0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            t[i * 4 + j] = m[i][j];
        }
    }
    crate::instance::with_buckets(|b| b.add(&asset, t));
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_count(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.instance_count")?;
    let asset = string_arg(&args[0], "world.instance_count", "asset")?;
    let n = crate::instance::with_buckets(|b| b.count(&asset)) as i64;
    Ok(Value::from_int(n))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_total(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.instance_total")?;
    let n = crate::instance::with_buckets(|b| b.total_instances()) as i64;
    Ok(Value::from_int(n))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_bucket_count(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.instance_bucket_count")?;
    let n = crate::instance::with_buckets(|b| b.bucket_count()) as i64;
    Ok(Value::from_int(n))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_assets(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.instance_assets")?;
    let assets: Vec<Value> = crate::instance::with_buckets(|b| b.assets())
        .into_iter()
        .map(Value::from_string)
        .collect();
    Ok(Value::from_list(Rc::new(RefCell::new(assets))))
}

// ---- Phase 32 session 8: ergonomic helpers ----

/// Set the stream radius via meters rather than chunk count. Reads
/// the current `chunk_size` and rounds up so the script doesn't have
/// to remember the chunk grid size. Convenience wrapper over
/// [`world.set_stream_radius`].
#[cfg(not(target_arch = "wasm32"))]
fn world_stream_radius_meters(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.stream_radius_meters")?;
    let m = as_f64(&args[0], "world.stream_radius_meters")? as f32;
    if m <= 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.stream_radius_meters: meters must be positive (got {m})"),
            help: None,
        });
    }
    let chunks = crate::streaming::with_streaming(|s| (m / s.chunk_size).ceil() as i32);
    let chunks = chunks.clamp(1, 64);
    crate::streaming::with_streaming(|s| s.stream_radius_chunks = chunks);
    Ok(Value::from_int(chunks as i64))
}

/// Declare a LOD chain via (asset, max_distance) pairs — a more
/// ergonomic shape than the parallel-arrays form of
/// `world.set_lod_chain`. The last pair's `max_distance` is ignored
/// (its asset covers everything beyond the previous switch); pass
/// any sentinel value (typically a large number).
///
/// Example: `world.entity_lod("Tree", [("near.glb", 25.0),
/// ("med.glb", 100.0), ("far.glb", 1e9)])` registers the same chain
/// as `world.set_lod_chain("Tree", ["near.glb", "med.glb",
/// "far.glb"], [25.0, 100.0])`.
#[cfg(not(target_arch = "wasm32"))]
fn world_entity_lod(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.entity_lod")?;
    let class = string_arg(&args[0], "world.entity_lod", "class")?;
    if !args[1].is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "world.entity_lod: lod_pairs must be a list of (asset, max_distance) tuples"
                .to_string(),
            help: None,
        });
    }
    let rc = args[1].as_list();
    let pairs = rc.borrow();
    if pairs.is_empty() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "world.entity_lod: lod_pairs must have at least one entry".to_string(),
            help: None,
        });
    }
    let mut assets: Vec<String> = Vec::with_capacity(pairs.len());
    let mut switches: Vec<f32> = Vec::with_capacity(pairs.len().saturating_sub(1));
    for (i, p) in pairs.iter().enumerate() {
        if !p.is_tuple() {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("world.entity_lod: pair {i} must be a tuple (asset, distance)"),
                help: None,
            });
        }
        let elems = p.as_tuple();
        if elems.len() != 2 {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "world.entity_lod: pair {i} must be a 2-tuple (asset, distance), got {} elements",
                    elems.len()
                ),
                help: None,
            });
        }
        if !elems[0].is_str() {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("world.entity_lod: pair {i} asset must be a string"),
                help: None,
            });
        }
        assets.push(elems[0].as_string().clone());
        // Skip the last pair's distance — it's implicit +∞.
        if i + 1 < pairs.len() {
            switches.push(as_f64(&elems[1], "world.entity_lod")? as f32);
        }
    }
    let chain = crate::lod::LodChain::new(assets, switches).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: format!("world.entity_lod: {m}"),
        help: None,
    })?;
    crate::lod::with_table(|t| {
        t.insert(class, chain);
    });
    Ok(Value::NIL)
}

/// Compute distance from camera to entity, then return the LOD
/// asset for that class at that distance. Combines the two most
/// common per-frame queries into one builtin so scripts don't pay
/// the lookup overhead twice.
#[cfg(not(target_arch = "wasm32"))]
fn world_world_to_lod(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 7, "world.world_to_lod")?;
    let class = string_arg(&args[0], "world.world_to_lod", "class")?;
    let ex = as_f64(&args[1], "world.world_to_lod")? as f32;
    let ey = as_f64(&args[2], "world.world_to_lod")? as f32;
    let ez = as_f64(&args[3], "world.world_to_lod")? as f32;
    let cx = as_f64(&args[4], "world.world_to_lod")? as f32;
    let cy = as_f64(&args[5], "world.world_to_lod")? as f32;
    let cz = as_f64(&args[6], "world.world_to_lod")? as f32;
    let dx = ex - cx;
    let dy = ey - cy;
    let dz = ez - cz;
    let distance = (dx * dx + dy * dy + dz * dz).sqrt();
    let asset = crate::lod::with_table(|t| {
        t.get(&class)
            .map(|chain| chain.asset_for_distance(distance).to_string())
    });
    match asset {
        Some(s) => Ok(Value::from_string(s)),
        None => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.world_to_lod: no LOD chain registered for class '{class}'"),
            help: Some("call world.entity_lod or world.set_lod_chain first".to_string()),
        }),
    }
}

/// Euclidean distance between two 3D points. Bog-standard but pulled
/// out as a builtin so the per-frame visibility-pass loop doesn't
/// have to allocate a tuple/list to compute it.
#[cfg(not(target_arch = "wasm32"))]
fn world_distance_xyz(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 6, "world.distance_xyz")?;
    let ax = as_f64(&args[0], "world.distance_xyz")?;
    let ay = as_f64(&args[1], "world.distance_xyz")?;
    let az = as_f64(&args[2], "world.distance_xyz")?;
    let bx = as_f64(&args[3], "world.distance_xyz")?;
    let by = as_f64(&args[4], "world.distance_xyz")?;
    let bz = as_f64(&args[5], "world.distance_xyz")?;
    let dx = ax - bx;
    let dy = ay - by;
    let dz = az - bz;
    Ok(Value::from_float((dx * dx + dy * dy + dz * dz).sqrt()))
}

// ---- Phase 32 session 5: terrain.* namespace ----

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn install_terrain(env: &mut Env) {
    let mut t = HashMap::new();
    t.insert(
        "set_chunk_size".to_string(),
        Value::from_builtin(
            "terrain.set_chunk_size",
            &["meters"],
            terrain_set_chunk_size,
        ),
    );
    t.insert(
        "set_chunk_resolution".to_string(),
        Value::from_builtin(
            "terrain.set_chunk_resolution",
            &["samples"],
            terrain_set_chunk_resolution,
        ),
    );
    t.insert(
        "set_chunk".to_string(),
        Value::from_builtin(
            "terrain.set_chunk",
            &["cx", "cz", "heights"],
            terrain_set_chunk,
        ),
    );
    t.insert(
        "has_chunk".to_string(),
        Value::from_builtin("terrain.has_chunk", &["cx", "cz"], terrain_has_chunk),
    );
    t.insert(
        "height_at".to_string(),
        Value::from_builtin("terrain.height_at", &["x", "z"], terrain_height_at),
    );
    t.insert(
        "normal_at".to_string(),
        Value::from_builtin("terrain.normal_at", &["x", "z"], terrain_normal_at),
    );
    t.insert(
        "clear".to_string(),
        Value::from_builtin("terrain.clear", &[], terrain_clear),
    );
    env.set(
        "terrain".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: t,
            kind: "module",
        }))),
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_set_chunk_size(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "terrain.set_chunk_size")?;
    let m = as_f64(&args[0], "terrain.set_chunk_size")? as f32;
    if m <= 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("terrain.set_chunk_size: meters must be positive (got {m})"),
            help: None,
        });
    }
    crate::terrain::with_terrain(|t| t.chunk_size = m);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_set_chunk_resolution(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "terrain.set_chunk_resolution")?;
    let n = as_i64(&args[0], "terrain.set_chunk_resolution")?;
    if !(2..=1024).contains(&n) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("terrain.set_chunk_resolution: must be 2..=1024 (got {n})"),
            help: None,
        });
    }
    crate::terrain::with_terrain(|t| t.chunk_resolution = n as u32);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_set_chunk(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "terrain.set_chunk")?;
    let cx = as_i64(&args[0], "terrain.set_chunk")? as i32;
    let cz = as_i64(&args[1], "terrain.set_chunk")? as i32;
    let heights = list_of_floats(&args[2], "terrain.set_chunk", "heights")?;
    let heights_f32: Vec<f32> = heights.iter().map(|f| *f as f32).collect();
    crate::terrain::with_terrain(|t| t.set_chunk(cx, cz, heights_f32)).map_err(|m| {
        RuntimeError {
            line: 0,
            col: 0,
            message: m,
            help: None,
        }
    })?;
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_has_chunk(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "terrain.has_chunk")?;
    let cx = as_i64(&args[0], "terrain.has_chunk")? as i32;
    let cz = as_i64(&args[1], "terrain.has_chunk")? as i32;
    Ok(Value::from_bool(crate::terrain::with_terrain(|t| {
        t.has_chunk(cx, cz)
    })))
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_height_at(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "terrain.height_at")?;
    let x = as_f64(&args[0], "terrain.height_at")? as f32;
    let z = as_f64(&args[1], "terrain.height_at")? as f32;
    match crate::terrain::with_terrain(|t| t.height_at(x, z)) {
        Some(h) => Ok(Value::from_float(h as f64)),
        None => Ok(Value::NIL),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_normal_at(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "terrain.normal_at")?;
    let x = as_f64(&args[0], "terrain.normal_at")? as f32;
    let z = as_f64(&args[1], "terrain.normal_at")? as f32;
    match crate::terrain::with_terrain(|t| t.normal_at(x, z)) {
        Some(n) => Ok(Value::from_tuple(Rc::new(vec![
            Value::from_float(n[0] as f64),
            Value::from_float(n[1] as f64),
            Value::from_float(n[2] as f64),
        ]))),
        None => Ok(Value::NIL),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_clear(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "terrain.clear")?;
    crate::terrain::with_terrain(|t| t.clear());
    Ok(Value::NIL)
}

// ---- Helpers shared by Phase 32 session 4 / 5 / 6 ----

#[cfg(not(target_arch = "wasm32"))]
fn list_of_strings(v: &Value, op: &str, label: &str) -> Result<Vec<String>, RuntimeError> {
    if !v.is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{op} expects a list of strings for {label}"),
            help: None,
        });
    }
    let rc = v.as_list();
    let elems = rc.borrow();
    let mut out = Vec::with_capacity(elems.len());
    for e in elems.iter() {
        if !e.is_str() {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("{op}: {label} entry must be a string"),
                help: None,
            });
        }
        out.push(e.as_string().clone());
    }
    Ok(out)
}

#[cfg(not(target_arch = "wasm32"))]
fn list_of_floats(v: &Value, op: &str, label: &str) -> Result<Vec<f64>, RuntimeError> {
    if !v.is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{op} expects a list of numbers for {label}"),
            help: None,
        });
    }
    let rc = v.as_list();
    let elems = rc.borrow();
    let mut out = Vec::with_capacity(elems.len());
    for e in elems.iter() {
        out.push(as_f64(e, op)?);
    }
    Ok(out)
}
