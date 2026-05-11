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

// ---------------------------------------------------------------
// Phase 36 session 2: Steam P2P transport thunks.
//
// `src/net_steam.rs` calls these from inside `#[cfg(feature =
// "steam-net")]` blocks. Keeping the SDK access here means the
// `OnceLock<Client>` ownership stays in one module; net_steam.rs
// is thin and doesn't need its own Steam client handle.
// ---------------------------------------------------------------

/// Returns the local user's SteamID64 (raw u64). 0 if Steam is not
/// available. Phase 36 only — gated to `steam-net` because the field
/// is otherwise unused.
#[cfg(feature = "steam-net")]
pub fn local_steam_id_raw() -> u64 {
    if let Some(Some(client)) = STEAM.get() {
        client.user().steam_id().raw()
    } else {
        0
    }
}

/// Send a P2P message to a remote SteamID over the
/// `SteamNetworkingMessages` API. `channel` selects the lockstep
/// stream (0 = input/reliable, 1 = state-hash/unreliable per
/// `net_steam::CHANNEL_*`).
///
/// Returns `Err` only when the SDK reports a routable failure;
/// transient drops are silently swallowed by the SDK and surfaced
/// later through `p2p_receive`.
#[cfg(feature = "steam-net")]
pub fn p2p_send(remote_steam_id_raw: u64, channel: u32, payload: &[u8]) -> Result<(), String> {
    use steamworks::networking_types::{NetworkingIdentity, SendFlags};
    let Some(Some(client)) = STEAM.get() else {
        return Err("Steam client not initialised".to_string());
    };
    let messages = client.networking_messages();
    let identity = NetworkingIdentity::new_steam_id(steamworks::SteamId::from_raw(
        remote_steam_id_raw,
    ));
    let flags = match channel {
        // CHANNEL_INPUT — reliable + ordered; lockstep depends on
        // every input frame arriving exactly once in order.
        0 => SendFlags::RELIABLE,
        // CHANNEL_HASH — unreliable; missing a heartbeat is fine,
        // the next one arrives 1Hz later.
        _ => SendFlags::UNRELIABLE,
    };
    messages
        .send_message_to_user(identity, flags, payload, channel)
        .map_err(|e| format!("Steam send: {e:?}"))
}

/// Drain pending P2P messages on the given channels. Returns up to
/// `max_per_channel` messages per channel; the lockstep runner calls
/// this once per frame.
///
/// Each tuple is `(remote_steam_id_raw, channel, payload)`.
#[cfg(feature = "steam-net")]
pub fn p2p_receive(
    channels: &[u32],
    max_per_channel: usize,
) -> Vec<(u64, u32, Vec<u8>)> {
    let Some(Some(client)) = STEAM.get() else {
        return Vec::new();
    };
    let messages = client.networking_messages();
    let mut out: Vec<(u64, u32, Vec<u8>)> = Vec::new();
    for &ch in channels {
        let received = messages.receive_messages_on_channel(ch, max_per_channel);
        for msg in received {
            // `msg.identity_peer().steam_id()` returns Option<SteamId>
            // — peer identification is by SteamID on the messages API.
            let steam_id_raw = msg
                .identity_peer()
                .steam_id()
                .map(|s| s.raw())
                .unwrap_or(0);
            out.push((steam_id_raw, ch, msg.data().to_vec()));
        }
    }
    out
}

// ---------------------------------------------------------------
// Phase 36 session 4: Steam Matchmaking lobby thunks.
//
// These wrap the SDK's async lobby APIs in synchronous-looking
// helpers. The pattern: kick off the API call, register a callback
// that pushes the result to a one-shot channel, block on the channel
// with a 5-second timeout. Matchmaking ops are user-initiated and
// rare (lobby create + lobby join only fire on menu screens, not in
// the play loop), so blocking is acceptable.
// ---------------------------------------------------------------

#[cfg(feature = "steam-net")]
pub fn lobby_create(max_peers: u8) -> Result<u64, String> {
    use std::sync::mpsc;
    use std::time::Duration;
    let Some(Some(client)) = STEAM.get() else {
        return Err("Steam client not initialised".to_string());
    };
    let (tx, rx) = mpsc::channel::<Result<u64, String>>();
    client.matchmaking().create_lobby(
        steamworks::LobbyType::Public,
        max_peers as u32,
        move |result| {
            let _ = match result {
                Ok(lobby_id) => tx.send(Ok(lobby_id.raw())),
                Err(e) => tx.send(Err(format!("create_lobby callback: {e:?}"))),
            };
        },
    );
    // Pump callbacks until the channel fills or the timeout elapses.
    let started = std::time::Instant::now();
    loop {
        client.run_callbacks();
        if let Ok(r) = rx.try_recv() {
            return r;
        }
        if started.elapsed() > Duration::from_secs(5) {
            return Err("create_lobby: no callback within 5s".to_string());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(feature = "steam-net")]
pub fn lobby_set_name(lobby_raw: u64, name: &str) -> Result<(), String> {
    let Some(Some(client)) = STEAM.get() else {
        return Err("Steam client not initialised".to_string());
    };
    let lobby = steamworks::LobbyId::from_raw(lobby_raw);
    client
        .matchmaking()
        .set_lobby_data(lobby, "name", name);
    Ok(())
}

#[cfg(feature = "steam-net")]
pub fn lobby_request_list(query: &str, max: usize) -> Result<Vec<crate::net_steam::LobbyInfo>, String> {
    use std::sync::mpsc;
    use std::time::Duration;
    let Some(Some(client)) = STEAM.get() else {
        return Err("Steam client not initialised".to_string());
    };
    let (tx, rx) = mpsc::channel::<Result<Vec<u64>, String>>();
    // Note: a name-substring filter against Steam's matchmaking
    // index requires `add_request_lobby_list_string_filter`. The
    // server-side filter accepts an exact key+value match; we filter
    // client-side by substring after retrieval since lobby names
    // aren't indexed for substring search.
    client.matchmaking().request_lobby_list(move |result| {
        let _ = match result {
            Ok(ids) => tx.send(Ok(ids.into_iter().map(|i| i.raw()).collect())),
            Err(e) => tx.send(Err(format!("request_lobby_list callback: {e:?}"))),
        };
    });
    let started = std::time::Instant::now();
    let lobby_ids = loop {
        client.run_callbacks();
        if let Ok(r) = rx.try_recv() {
            break r?;
        }
        if started.elapsed() > Duration::from_secs(5) {
            return Err("request_lobby_list: no callback within 5s".to_string());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut out: Vec<crate::net_steam::LobbyInfo> = Vec::new();
    for raw in lobby_ids.into_iter().take(max) {
        let lobby = steamworks::LobbyId::from_raw(raw);
        let name = client
            .matchmaking()
            .lobby_data(lobby, "name")
            .unwrap_or_default()
            .to_string();
        if !query.is_empty() && !name.contains(query) {
            continue;
        }
        let peer_count = client.matchmaking().lobby_member_count(lobby) as u32;
        let max_peers = client.matchmaking().lobby_member_limit(lobby).unwrap_or(0) as u32;
        out.push(crate::net_steam::LobbyInfo {
            id: raw,
            name,
            peer_count,
            max_peers,
        });
    }
    Ok(out)
}

#[cfg(feature = "steam-net")]
pub fn lobby_join(lobby_raw: u64) -> Result<bool, String> {
    use std::sync::mpsc;
    use std::time::Duration;
    let Some(Some(client)) = STEAM.get() else {
        return Err("Steam client not initialised".to_string());
    };
    let (tx, rx) = mpsc::channel::<Result<bool, String>>();
    let lobby = steamworks::LobbyId::from_raw(lobby_raw);
    client.matchmaking().join_lobby(lobby, move |result| {
        let _ = match result {
            Ok(_) => tx.send(Ok(true)),
            Err(()) => tx.send(Ok(false)),
        };
    });
    let started = std::time::Instant::now();
    loop {
        client.run_callbacks();
        if let Ok(r) = rx.try_recv() {
            return r;
        }
        if started.elapsed() > Duration::from_secs(5) {
            return Err("join_lobby: no callback within 5s".to_string());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Returns `(expected_peers, my_internal_id, owner_steam_id_raw)` for
/// a joined lobby. Called immediately after `lobby_join` to assign
/// the lockstep peer-id mapping.
#[cfg(feature = "steam-net")]
pub fn lobby_member_info(lobby_raw: u64) -> Result<(u8, u8, u64), String> {
    let Some(Some(client)) = STEAM.get() else {
        return Err("Steam client not initialised".to_string());
    };
    let lobby = steamworks::LobbyId::from_raw(lobby_raw);
    let mm = client.matchmaking();
    let expected_peers = mm.lobby_member_count(lobby) as u8;
    let owner = mm.lobby_owner(lobby);
    let owner_raw = owner.raw();
    let local = client.user().steam_id().raw();
    let my_internal_id = if local == owner_raw {
        0
    } else {
        // Members are returned in deterministic order; find ourselves
        // and assign internal id = position-among-non-owners + 1.
        let members = mm.lobby_members(lobby);
        let non_owner_positions: Vec<u64> = members
            .into_iter()
            .filter(|m| m.raw() != owner_raw)
            .map(|m| m.raw())
            .collect();
        let pos = non_owner_positions
            .iter()
            .position(|id| *id == local)
            .ok_or_else(|| "lobby_member_info: local not in lobby members".to_string())?;
        (pos + 1) as u8
    };
    Ok((expected_peers, my_internal_id, owner_raw))
}

#[cfg(feature = "steam-net")]
pub fn lobby_leave(lobby_raw: u64) {
    if let Some(Some(client)) = STEAM.get() {
        let lobby = steamworks::LobbyId::from_raw(lobby_raw);
        client.matchmaking().leave_lobby(lobby);
    }
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
