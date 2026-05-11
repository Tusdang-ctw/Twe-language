//! Phase 36 session 2: Steam P2P transport for the `net.*` lockstep
//! multiplayer surface.
//!
//! Per `docs/changes/2026-05-10-matchmaking-rfc.md` the matchmaking
//! decision is *Steam P2P primary + STUN fallback*. This module owns
//! the Steam side; `src/net_stun.rs` owns the fallback. The Phase 31
//! raw-UDP path in `src/net.rs` is untouched — it still drives LAN +
//! direct-IP play.
//!
//! ## Routing model
//!
//! At session-start, scripts call **one** of:
//!
//! - `net.host(port, peers)` — Phase 31 raw UDP.
//! - `net.connect("ip:port")` — Phase 31 raw UDP.
//! - `net.connect_via_stun(rendezvous, lobby)` — Phase 36 session 3.
//! - `net.create_lobby(name, peers)` — Phase 36 session 4 (Steam path).
//! - `net.join_lobby(id)` — Phase 36 session 4 (Steam path).
//!
//! Whichever path opens first wins the [`active_transport`] slot;
//! every subsequent `net.*` call dispatches through it. Switching
//! transport mid-session is not supported (calls fail with
//! `"transport already open"`).
//!
//! ## Steam SDK gating
//!
//! All real `steamworks::*` calls live behind `#[cfg(feature =
//! "steam-net")]`. The default build compiles a no-op stub that
//! returns `Err("steam-net feature not compiled in; rebuild with
//! --features steam-net")` for every call site, exactly mirroring the
//! shape of `src/steam.rs`. This way `cargo build` on a vanilla
//! checkout still produces a working binary; only operators who own a
//! Steam AppID and link against `steam_api.dll` get the live path.
//!
//! ## What this module owns vs. what it doesn't
//!
//! Owned: the Steam-side transport. Steam-id ⇄ peer-id mapping. Lobby
//! create / find / join calls (their builtins are wired in stdlib.rs
//! during Phase 36 session 4 but the Steam-side primitives live here).
//!
//! Not owned: the lockstep runner (in `src/net.rs`). Per-tick frame
//! ring buffer (in `src/net.rs`). State-hash desync detection (in
//! `src/net.rs`). The dispatch layer that picks UDP vs Steam at each
//! call site (in `src/net.rs`'s `Transport` enum).
//!
//! ## Determinism contract
//!
//! Steam P2P delivers messages reliably or unreliably per channel.
//! Lockstep needs reliable+ordered for inputs and unreliable for
//! state-hash heartbeats. We pin the input channel to channel 0 with
//! `SEND_RELIABLE` and the hash channel to channel 1 with
//! `SEND_UNRELIABLE`. Bye packets share channel 0.

#![cfg(not(target_arch = "wasm32"))]

use std::cell::RefCell;
use std::collections::VecDeque;

/// Steam channel IDs used by the lockstep runner. Steam allows up to
/// 4 channels per peer-pair; we use two.
pub const CHANNEL_INPUT: u32 = 0;
pub const CHANNEL_HASH: u32 = 1;

/// Soft cap on Steam P2P peers. Mirrors `net::MAX_PEERS`.
pub const MAX_STEAM_PEERS: usize = 4;

/// What this module knows about a remote peer on the Steam path.
/// The `internal_id` (0..3) is the lockstep peer id used everywhere
/// else in the runner; the `steam_id_raw` is the SteamID64 used by
/// the SDK.
#[derive(Clone, Copy, Debug)]
pub struct SteamPeerEntry {
    pub internal_id: u8,
    pub steam_id_raw: u64,
}

/// Steam-side session state. Optional — `None` until `host_p2p` or
/// `connect_p2p` succeeds.
struct SteamSession {
    /// 0 for the host, 1..=N-1 for clients.
    local_internal_id: u8,
    /// Total peers including self.
    expected_peers: u8,
    /// Lobby id (0 if no lobby; `host_p2p` without a lobby wraps a
    /// direct-id-to-id connection).
    lobby_id: u64,
    /// Remote peers, sorted by `internal_id`.
    peers: Vec<SteamPeerEntry>,
    /// Outgoing send queue used when the client side is still
    /// finalising the SteamNetworkingMessages handshake.
    pending_outgoing: VecDeque<(u8, Vec<u8>, u32)>,
    /// Incoming queue drained by `poll`. Each entry is
    /// (internal_peer_id, payload, channel).
    pending_incoming: VecDeque<(u8, Vec<u8>, u32)>,
}

thread_local! {
    static SESSION: RefCell<Option<SteamSession>> = const { RefCell::new(None) };
    /// Set true by `set_active`; consumed by `is_active`. Used by the
    /// dispatch layer in `net.rs` to know which transport owns the
    /// session.
    static ACTIVE: RefCell<bool> = const { RefCell::new(false) };
}

/// Returns true iff a Steam-side session is currently the active
/// transport. Used by `net.rs` to dispatch send/poll between UDP and
/// Steam paths.
pub fn is_active() -> bool {
    ACTIVE.with(|a| *a.borrow())
}

/// Mark the Steam transport active. Idempotent. Set by `host_p2p` /
/// `connect_p2p` / lobby create+join paths once they've installed a
/// session.
pub fn set_active(flag: bool) {
    ACTIVE.with(|a| *a.borrow_mut() = flag);
}

/// True when `--features steam-net` was passed at compile time AND
/// the Steam client has been initialised + reports available. Mirrors
/// `steam::is_available()` but adds the feature-flag gate so the
/// dispatch layer can treat "feature off" and "Steam offline" the
/// same way.
pub fn is_available() -> bool {
    #[cfg(feature = "steam-net")]
    {
        crate::steam::is_available()
    }
    #[cfg(not(feature = "steam-net"))]
    {
        false
    }
}

/// Query the local SteamID64. Returns 0 if Steam isn't available or
/// the feature isn't compiled in.
pub fn local_steam_id() -> u64 {
    #[cfg(feature = "steam-net")]
    {
        // The actual call would be:
        //   client.user().steam_id().raw()
        // This is wrapped behind a thunk because `steam.rs` owns the
        // OnceLock<Client>; we forward through a public helper there.
        crate::steam::local_steam_id_raw()
    }
    #[cfg(not(feature = "steam-net"))]
    {
        0
    }
}

/// Become host of a Steam P2P session. `expected_peers` includes the
/// host. The host is assigned `internal_id = 0`.
///
/// This call does **not** create a Steam Lobby — that's the lobby
/// primitives' job (Phase 36 session 4). Use this when the script
/// already knows the peer's SteamID (Steam Friends invite path).
pub fn host_p2p(expected_peers: u8) -> Result<(), String> {
    if !is_available() {
        return Err(steam_unavailable_msg());
    }
    if !(2..=MAX_STEAM_PEERS as u8).contains(&expected_peers) {
        return Err(format!(
            "net.host_p2p: expected_peers must be 2..={MAX_STEAM_PEERS} (got {expected_peers})"
        ));
    }
    SESSION.with(|s| {
        *s.borrow_mut() = Some(SteamSession {
            local_internal_id: 0,
            expected_peers,
            lobby_id: 0,
            peers: Vec::new(),
            pending_outgoing: VecDeque::new(),
            pending_incoming: VecDeque::new(),
        });
    });
    set_active(true);
    Ok(())
}

/// Connect to a remote SteamID as a client. The host assigns this
/// peer's `internal_id` once the handshake completes; the script
/// blocks on `net.session_ready()` until that happens.
pub fn connect_p2p(remote_steam_id: u64) -> Result<(), String> {
    if !is_available() {
        return Err(steam_unavailable_msg());
    }
    let host_entry = SteamPeerEntry {
        internal_id: 0,
        steam_id_raw: remote_steam_id,
    };
    SESSION.with(|s| {
        *s.borrow_mut() = Some(SteamSession {
            // Placeholder; the host echoes the real id back during
            // the SteamNetworkingMessages handshake. Until then,
            // session_ready() returns false.
            local_internal_id: u8::MAX,
            expected_peers: 0,
            lobby_id: 0,
            peers: vec![host_entry],
            pending_outgoing: VecDeque::new(),
            pending_incoming: VecDeque::new(),
        });
    });
    set_active(true);
    // No hello packet sent here — peer-id assignment on the Steam
    // path runs through the lobby join-order machinery in
    // Phase 36 session 4. For now, `connect_p2p` opens the routing
    // table; the lobby code overwrites `local_internal_id` /
    // `expected_peers` once both peers are present.
    Ok(())
}

/// Send `payload` to the peer with the given internal id, on `channel`.
/// On the Steam path this calls `SteamNetworkingMessages.send_message_to_user`.
pub fn send_to_internal(internal_peer_id: u8, payload: &[u8], channel: u32) -> Result<(), String> {
    if !is_available() {
        return Err(steam_unavailable_msg());
    }
    SESSION.with(|s| -> Result<(), String> {
        let mut slot = s.borrow_mut();
        let Some(sess) = slot.as_mut() else {
            return Err("net (steam): no active session".to_string());
        };
        let Some(peer) = sess.peers.iter().find(|p| p.internal_id == internal_peer_id) else {
            // Peer not yet in table — queue the packet so a later
            // poll() that adds the peer can flush it. This handles
            // the connect-side window between hello and hello-ack.
            sess.pending_outgoing
                .push_back((internal_peer_id, payload.to_vec(), channel));
            return Ok(());
        };
        let _ = peer; // suppress unused in non-feature build
        #[cfg(feature = "steam-net")]
        {
            // Real path:
            //
            //   let networking = client.networking_messages();
            //   let send_flags = if channel == CHANNEL_INPUT {
            //       steamworks::SendFlags::RELIABLE
            //   } else {
            //       steamworks::SendFlags::UNRELIABLE
            //   };
            //   networking.send_message_to_user(
            //       SteamId::from_raw(peer.steam_id_raw),
            //       send_flags,
            //       payload,
            //       channel,
            //   )
            //
            // Wrapped behind `crate::steam::p2p_send` so the OnceLock
            // ownership stays in `steam.rs`.
            crate::steam::p2p_send(peer.steam_id_raw, channel, payload)
                .map_err(|e| format!("net (steam): send: {e}"))?;
        }
        Ok(())
    })
}

/// Drain pending Steam P2P messages from the SDK and queue them onto
/// the per-session incoming list. Returns the number of messages
/// processed this tick.
pub fn poll() -> usize {
    if !is_available() {
        return 0;
    }
    #[cfg(not(feature = "steam-net"))]
    {
        0
    }
    #[cfg(feature = "steam-net")]
    {
        let mut n = 0;
        // Real path:
        //
        //   for ch in [CHANNEL_INPUT, CHANNEL_HASH] {
        //       for msg in client.networking_messages().receive_messages_on_channel(ch, 32) {
        //           // identify peer by SteamId, push (internal_id, payload, ch)
        //       }
        //   }
        let drained = crate::steam::p2p_receive(&[CHANNEL_INPUT, CHANNEL_HASH], 32);
        SESSION.with(|s| {
            let mut slot = s.borrow_mut();
            if let Some(sess) = slot.as_mut() {
                for (steam_id_raw, channel, payload) in drained {
                    let pid = sess
                        .peers
                        .iter()
                        .find(|p| p.steam_id_raw == steam_id_raw)
                        .map(|p| p.internal_id)
                        .unwrap_or(u8::MAX);
                    sess.pending_incoming.push_back((pid, payload, channel));
                    n += 1;
                }
            }
        });
        n
    }
}

/// Pop one (internal_peer_id, payload, channel) from the incoming
/// queue. Returns None if empty. The lockstep runner in `net.rs`
/// drains this queue after each `poll()`.
pub fn next_incoming() -> Option<(u8, Vec<u8>, u32)> {
    SESSION.with(|s| s.borrow_mut().as_mut().and_then(|sess| sess.pending_incoming.pop_front()))
}

/// Close the Steam-side session. Idempotent.
pub fn close() {
    SESSION.with(|s| *s.borrow_mut() = None);
    set_active(false);
}

/// Returns `(local_internal_id, expected_peers)` for the current
/// session, or `None` if no session is open.
pub fn session_state() -> Option<(u8, u8)> {
    SESSION.with(|s| {
        s.borrow()
            .as_ref()
            .map(|sess| (sess.local_internal_id, sess.expected_peers))
    })
}

/// Number of peers the host has accepted (excluding self). Used by
/// `session_ready` on both paths.
pub fn accepted_peer_count() -> usize {
    SESSION.with(|s| s.borrow().as_ref().map(|sess| sess.peers.len()).unwrap_or(0))
}

/// Install a peer entry. Called by the lobby + handshake handlers
/// when a new SteamID joins the session.
pub fn install_peer(internal_id: u8, steam_id_raw: u64) {
    SESSION.with(|s| {
        let mut slot = s.borrow_mut();
        if let Some(sess) = slot.as_mut() {
            // Replace any existing entry with the same internal_id.
            sess.peers.retain(|p| p.internal_id != internal_id);
            sess.peers.push(SteamPeerEntry {
                internal_id,
                steam_id_raw,
            });
            // Flush any pending packets queued before the peer was
            // installed.
            let mut pending = std::mem::take(&mut sess.pending_outgoing);
            drop(slot);
            while let Some((pid, payload, ch)) = pending.pop_front() {
                let _ = send_to_internal(pid, &payload, ch);
            }
        }
    });
}

/// Set this peer's local internal id. Called when the host echoes the
/// id assignment back through the handshake.
pub fn set_local_internal_id(id: u8, expected_peers: u8) {
    SESSION.with(|s| {
        let mut slot = s.borrow_mut();
        if let Some(sess) = slot.as_mut() {
            sess.local_internal_id = id;
            sess.expected_peers = expected_peers;
        }
    });
}

/// Lobby-side: associate the Steam Lobby ID with this session. Used
/// by Phase 36 session 4's `create_lobby` / `join_lobby` builtins.
pub fn set_lobby_id(lobby_id: u64) {
    SESSION.with(|s| {
        let mut slot = s.borrow_mut();
        if let Some(sess) = slot.as_mut() {
            sess.lobby_id = lobby_id;
        }
    });
}

pub fn lobby_id() -> u64 {
    SESSION.with(|s| s.borrow().as_ref().map(|sess| sess.lobby_id).unwrap_or(0))
}

// ---------------------------------------------------------------
// Phase 36 session 4: lobby primitives.
//
// Steam-feature path: thin wrappers around Steam Matchmaking. The
// real Steam SDK calls live in `crate::steam::lobby_*` thunks (same
// pattern as Phase 36 session 2's p2p_send / p2p_receive). Lobby
// creation + search are async on Steam; we expose synchronous-looking
// builtins that block on the SDK's callback for up to 5 seconds.
//
// No-feature path: every lobby builtin returns an informative error
// telling the operator to use `--features steam-net` or
// `net.connect_via_stun` for non-Steam multiplayer. The rendezvous-
// based lobby broker (a richer protocol than the 2-peer pair-up that
// session 3 ships) is honest deferral to a Phase 36.5 follow-on if
// anyone asks; Steam P2P covers the v1.0 thesis.
// ---------------------------------------------------------------

/// Public lobby record returned by `find_lobbies`. Mirrors the Twe
/// surface (`{id, name, peer_count, max_peers}`).
#[derive(Clone, Debug, PartialEq)]
pub struct LobbyInfo {
    pub id: u64,
    pub name: String,
    pub peer_count: u32,
    pub max_peers: u32,
}

/// Create a public Steam Lobby with the given `name` + `max_peers`
/// cap. Returns the lobby's SteamID. The local user becomes host
/// (peer 0); the session table is populated alongside.
///
/// Steam-feature: calls `Matchmaking::create_lobby(Public, max_peers)`
/// and blocks on the callback for up to 5 seconds. On success, the
/// returned lobby id is set as the session's `lobby_id` and the
/// caller's local internal id is 0.
///
/// No-feature: errors with the standard "rebuild with --features
/// steam-net" message.
pub fn create_lobby(_name: &str, max_peers: u32) -> Result<u64, String> {
    if !is_available() {
        return Err(steam_unavailable_msg());
    }
    if !(2..=MAX_STEAM_PEERS as u32).contains(&max_peers) {
        return Err(format!(
            "net.create_lobby: max_peers must be 2..={MAX_STEAM_PEERS} (got {max_peers})"
        ));
    }
    #[cfg(feature = "steam-net")]
    {
        let lobby_raw = crate::steam::lobby_create(max_peers as u8)
            .map_err(|e| format!("net.create_lobby: {e}"))?;
        // Also set lobby name via lobby metadata.
        let _ = crate::steam::lobby_set_name(lobby_raw, _name);
        // Install host session.
        host_p2p(max_peers as u8)?;
        set_lobby_id(lobby_raw);
        Ok(lobby_raw)
    }
    #[cfg(not(feature = "steam-net"))]
    {
        let _ = max_peers;
        Err(steam_unavailable_msg())
    }
}

/// Find public lobbies matching a name substring. Empty query → all
/// public lobbies. Returns at most 50 entries.
///
/// Steam-feature: calls `Matchmaking::request_lobby_list` with a
/// `lobby_distance_filter` of "worldwide" + a name-string filter.
/// Blocks for up to 5 seconds.
///
/// No-feature: errors.
pub fn find_lobbies(_query: &str) -> Result<Vec<LobbyInfo>, String> {
    if !is_available() {
        return Err(steam_unavailable_msg());
    }
    #[cfg(feature = "steam-net")]
    {
        crate::steam::lobby_request_list(_query, 50).map_err(|e| format!("net.find_lobbies: {e}"))
    }
    #[cfg(not(feature = "steam-net"))]
    {
        Err(steam_unavailable_msg())
    }
}

/// Join a public lobby by SteamID. Returns true on success, false
/// when the lobby is full or no longer exists.
///
/// Steam-feature: calls `Matchmaking::join_lobby` and blocks on the
/// callback. On success, the local peer becomes one of `1..N-1`
/// (the Steam Matchmaking API assigns join-order to slot index).
pub fn join_lobby(lobby_raw: u64) -> Result<bool, String> {
    if !is_available() {
        return Err(steam_unavailable_msg());
    }
    if lobby_raw == 0 {
        return Err("net.join_lobby: lobby id must be non-zero".to_string());
    }
    #[cfg(feature = "steam-net")]
    {
        let join_ok = crate::steam::lobby_join(lobby_raw)
            .map_err(|e| format!("net.join_lobby: {e}"))?;
        if !join_ok {
            return Ok(false);
        }
        // Query lobby for member list + assign internal id by
        // member index. The host (lobby owner) is internal id 0;
        // everyone else is in member-list-position order.
        let (expected_peers, my_internal_id, owner_steam_id) =
            crate::steam::lobby_member_info(lobby_raw)
                .map_err(|e| format!("net.join_lobby: lobby_member_info: {e}"))?;
        // Install a session marking us as the assigned internal id.
        SESSION.with(|s| {
            *s.borrow_mut() = Some(SteamSession {
                local_internal_id: my_internal_id,
                expected_peers,
                lobby_id: lobby_raw,
                peers: vec![SteamPeerEntry {
                    internal_id: 0,
                    steam_id_raw: owner_steam_id,
                }],
                pending_outgoing: VecDeque::new(),
                pending_incoming: VecDeque::new(),
            });
        });
        set_active(true);
        Ok(true)
    }
    #[cfg(not(feature = "steam-net"))]
    {
        let _ = lobby_raw;
        Err(steam_unavailable_msg())
    }
}

/// Leave the current lobby. Idempotent. Closes the Steam-side
/// session.
pub fn leave_lobby() {
    #[cfg(feature = "steam-net")]
    {
        let id = lobby_id();
        if id != 0 {
            crate::steam::lobby_leave(id);
        }
    }
    close();
}

fn steam_unavailable_msg() -> String {
    if cfg!(feature = "steam-net") {
        "Steam P2P transport: feature compiled in but Steam client not running. \
         Start Steam, sign in, and ensure steam_appid.txt is present next to the binary."
            .to_string()
    } else {
        "Steam P2P transport: this build was not compiled with --features steam-net. \
         Rebuild with `cargo build --features steam-net` to enable."
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_build_reports_steam_unavailable() {
        // The crate test runner runs without --features steam-net, so
        // is_available() must be false and host_p2p must error
        // accordingly. The error message is the user-facing line that
        // tells operators what to do — assert it stays informative.
        assert!(!is_available(), "no-feature build must not advertise Steam P2P");
        let err = host_p2p(2).expect_err("host_p2p must reject when Steam is unavailable");
        assert!(
            err.contains("--features steam-net") || err.contains("Steam client not running"),
            "error must guide the operator: got {err:?}"
        );
    }

    #[test]
    fn close_is_idempotent() {
        close();
        close();
        assert!(!is_active());
    }

    #[test]
    fn session_state_is_none_when_no_session() {
        close();
        assert_eq!(session_state(), None);
        assert_eq!(accepted_peer_count(), 0);
        assert_eq!(local_steam_id(), 0);
        assert_eq!(lobby_id(), 0);
    }
}
