//! Phase 15 session 3: optional Steam SDK integration.
//!
//! Gated behind `#[cfg(feature = "steam")]`. The default build
//! (no feature flag) compiles the no-op stubs so every call site
//! in stdlib.rs compiles unconditionally; `--features steam`
//! switches in the real steamworks-rs calls.
//!
//! ## Twe surface
//!
//!   achievement.unlock("FIRST_KILL")
//!   stat.set("KILLS_TOTAL", 1000)
//!   stat.get("KILLS_TOTAL")   → int or float
//!   cloud.save("slot1.json", "{...}")
//!   cloud.load("slot1.json")  → string or nil
//!
//! All builtins are registered by `install_steam_builtins` which
//! stdlib calls during play-loop initialisation.

use crate::value::{Env, RuntimeError, Value};

// ---------------------------------------------------------------
// Steam client singleton — initialised once at play-loop start.
// ---------------------------------------------------------------

#[cfg(feature = "steam")]
use std::sync::OnceLock;

#[cfg(feature = "steam")]
static STEAM: OnceLock<Option<steamworks::Client>> = OnceLock::new();

/// Initialise the Steam client. Called once from `play::run_loop`
/// before the first `tick_frame`. Safe to call multiple times —
/// `OnceLock` guarantees exactly one initialisation.
pub fn init() {
    #[cfg(feature = "steam")]
    {
        STEAM.get_or_init(|| match steamworks::Client::init() {
            Ok((client, _)) => {
                eprintln!("[twec] Steam client initialised");
                Some(client)
            }
            Err(e) => {
                eprintln!("[twec] Steam not available: {e}");
                None
            }
        });
    }
}

/// Returns true when Steam is available and the client is live.
pub fn is_available() -> bool {
    #[cfg(feature = "steam")]
    {
        matches!(STEAM.get(), Some(Some(_)))
    }
    #[cfg(not(feature = "steam"))]
    {
        false
    }
}

// ---------------------------------------------------------------
// Builtin implementations
// ---------------------------------------------------------------

pub fn achievement_unlock(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    if args.is_empty() {
        return Ok(Value::NIL);
    }
    let name = args[0].display();
    #[cfg(feature = "steam")]
    {
        if let Some(Some(client)) = STEAM.get() {
            let user_stats = client.user_stats();
            let _ = user_stats.achievement(&name).set();
            let _ = user_stats.store_stats();
        }
    }
    let _ = name; // suppress unused warning in non-steam build
    Ok(Value::NIL)
}

pub fn stat_set(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    if args.len() < 2 {
        return Ok(Value::NIL);
    }
    let name = args[0].display();
    #[cfg(feature = "steam")]
    {
        if let Some(Some(client)) = STEAM.get() {
            let us = client.user_stats();
            if args[1].is_float() {
                let _ = us.set_stat_f32(&name, args[1].as_float() as f32);
            } else if args[1].is_int() {
                let _ = us.set_stat_i32(&name, args[1].as_int() as i32);
            }
            let _ = us.store_stats();
        }
    }
    let _ = name;
    Ok(Value::NIL)
}

pub fn stat_get(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    if args.is_empty() {
        return Ok(Value::NIL);
    }
    let name = args[0].display();
    #[cfg(feature = "steam")]
    {
        if let Some(Some(client)) = STEAM.get() {
            let us = client.user_stats();
            if let Ok(v) = us.get_stat_i32(&name) {
                return Ok(Value::from_int(v as i64));
            }
            if let Ok(v) = us.get_stat_f32(&name) {
                return Ok(Value::from_float(v as f64));
            }
        }
    }
    let _ = name;
    Ok(Value::NIL)
}

pub fn stat_commit(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    #[cfg(feature = "steam")]
    {
        if let Some(Some(client)) = STEAM.get() {
            let _ = client.user_stats().store_stats();
        }
    }
    Ok(Value::NIL)
}

pub fn cloud_save(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    if args.len() < 2 {
        return Ok(Value::NIL);
    }
    let _filename = args[0].display();
    let _payload = args[1].display();
    #[cfg(feature = "steam")]
    {
        if let Some(Some(client)) = STEAM.get() {
            let remote = client.remote_storage();
            let bytes = _payload.as_bytes();
            let _ = remote.file_write(&_filename, bytes);
        }
    }
    Ok(Value::NIL)
}

pub fn cloud_load(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    if args.is_empty() {
        return Ok(Value::NIL);
    }
    let _filename = args[0].display();
    #[cfg(feature = "steam")]
    {
        if let Some(Some(client)) = STEAM.get() {
            let remote = client.remote_storage();
            if let Ok(bytes) = remote.file_read(&_filename) {
                if let Ok(s) = String::from_utf8(bytes) {
                    return Ok(Value::from_string(s));
                }
            }
        }
    }
    Ok(Value::NIL)
}
