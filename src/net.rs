//! Phase 31 sessions 2–4: lockstep multiplayer transport + runner.
//!
//! Per `docs/changes/2026-05-10-multiplayer-rfc.md` the model is
//! lockstep over UDP. This module owns the socket, the per-peer
//! input ring buffers, and the tick advance logic.
//!
//! ## Lifecycle
//!
//! 1. One peer calls [`host`] with a port + total peer count. They
//!    become peer 0, bind a UDP socket, and wait for hellos.
//! 2. Other peers call [`connect`] with the host's `host:port`. They
//!    send a hello packet; the host assigns them peer ids 1..N-1 and
//!    replies with their assignment.
//! 3. Every frame, the play loop calls [`send_input`] (snapshots the
//!    local input ambients into a [`Frame`] and sends it to peers),
//!    then [`poll`] (drains incoming packets), then checks
//!    [`tick_ready`] before stepping the simulation.
//! 4. When [`tick_ready`] is true, the runner pulls every peer's
//!    Frame for that tick, merges them into the input ambients via
//!    a peer-id-mapping, and advances the simulation by one tick.
//!
//! ## What's not in this module
//!
//! - Steam P2P routing — follow-on session, gated on `--features
//!   steam-net`.
//! - Rollback prediction — follow-on phase, tick_ready always blocks
//!   in this version.
//! - WebSocket fallback for browsers — Phase 30 ships WASM but the
//!   net module is `#[cfg(not(target_arch = "wasm32"))]` only;
//!   browser multiplayer is a separate path.
//!
//! ## Determinism contract
//!
//! Every peer must produce a bit-exact simulation. Phase 29 closed
//! the determinism primitives (fixed-timestep, replay log, audio
//! scheduling). The lockstep runner relies on:
//! - inputs are the only non-deterministic input (covered by frame
//!   exchange);
//! - `time.physics_dt` is constant across peers;
//! - script-side RNG is seeded identically on every peer (the script
//!   is responsible — the runner can't enforce this, but
//!   [`state_hash`] surfaces divergence within ~60 ticks).

#![cfg(not(target_arch = "wasm32"))]

use std::cell::RefCell;
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::Instant;

const MAGIC: [u8; 2] = [b'T', b'W'];
const VERSION: u8 = 1;

const MSG_HELLO: u8 = 0;
const MSG_INPUT: u8 = 1;
const MSG_HASH: u8 = 2;
const MSG_BYE: u8 = 3;

const HEADER_LEN: usize = 16;

/// Default lockstep input delay (frames). Lower = snappier inputs but
/// less jitter tolerance; higher = more lag absorbed before a peer is
/// considered missing. 4 frames at 60Hz = 66ms of jitter budget which
/// covers ~95th-percentile residential LAN.
pub const DEFAULT_INPUT_DELAY: u32 = 4;

/// Maximum supported peers per session. Lockstep waits on the slowest
/// peer, so latency compounds; 4 is the sweet spot for cooperative
/// games and the upper bound recommended by the RFC.
pub const MAX_PEERS: usize = 4;

/// One simulation-tick's worth of input from one peer.
///
/// Mirrors the replay module's `Frame` struct verbatim — they
/// represent the same thing (one frame of captured input ambients),
/// but live in separate modules because their callers are different.
#[derive(Default, Clone, PartialEq, Debug)]
pub struct Frame {
    pub keys_held: Vec<String>,
    pub keys_pressed: Vec<String>,
    pub mouse_x: f64,
    pub mouse_y: f64,
    pub mb_held: Vec<String>,
    pub mb_press: Vec<String>,
}

impl Frame {
    /// Encode to the line format used by the replay log (without the
    /// trailing newline). Pipe-separated: held|pressed|x|y|mb|mbp.
    pub fn encode_line(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.keys_held.join(","),
            self.keys_pressed.join(","),
            self.mouse_x,
            self.mouse_y,
            self.mb_held.join(","),
            self.mb_press.join(","),
        )
    }

    /// Parse the line format. Returns `None` on any structural error
    /// — callers drop malformed packets silently rather than crash.
    pub fn decode_line(line: &str) -> Option<Self> {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() != 6 {
            return None;
        }
        let mouse_x: f64 = parts[2].parse().ok()?;
        let mouse_y: f64 = parts[3].parse().ok()?;
        Some(Frame {
            keys_held: split_csv(parts[0]),
            keys_pressed: split_csv(parts[1]),
            mouse_x,
            mouse_y,
            mb_held: split_csv(parts[4]),
            mb_press: split_csv(parts[5]),
        })
    }
}

fn split_csv(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(',').map(str::to_string).collect()
}

/// Per-peer state on the host side.
#[derive(Clone, Debug)]
struct Peer {
    /// Peer id assigned by the host. The host stores clients with
    /// ids `1..=expected_peers-1`; clients store the host as id 0.
    /// Stored explicitly because the table position is not the id —
    /// e.g. on a client, `peers[0]` is the host, whose id is 0
    /// (not 1).
    id: u8,
    addr: SocketAddr,
    /// Inputs we've received from this peer, keyed by tick.
    /// Sparse map because UDP can deliver out-of-order; the runner
    /// pulls by tick number.
    inputs: HashMap<u32, Frame>,
    /// Last tick we've seen from this peer, for stall detection.
    last_seen_tick: u32,
    /// Wall-clock time of the last packet from this peer. The runner
    /// uses this to time out dead peers (currently informational
    /// only — disconnect handling is a follow-on session).
    last_seen_at: Instant,
}

/// Top-level multiplayer state. The whole module is a singleton —
/// one socket, one peer table, thread-local for parity with replay.rs
/// and to avoid threading a handle through every play-loop call.
struct Session {
    socket: UdpSocket,
    /// 0 for the host, 1..=N-1 for clients. Assigned by the host on
    /// hello-ack; clients learn their id from the reply packet.
    local_peer_id: u8,
    /// Total peers in the session, including this one.
    expected_peers: u8,
    /// Random per-host id; clients echo it back so we can drop
    /// packets from a stale run after a host restart.
    session_id: u32,
    /// Peer table — empty for clients (they only know about the host
    /// at peers[0]); the host fills this with each accepted client.
    peers: Vec<Peer>,
    /// Local outgoing input ring — keyed by tick. Used to retransmit
    /// recent inputs so a single dropped UDP packet doesn't desync.
    local_inputs: HashMap<u32, Frame>,
    /// Highest tick we've sent.
    sent_tick: u32,
    /// Highest tick the runner has consumed (i.e. all peers had input
    /// available for it). Used to garbage-collect old `local_inputs`
    /// + `peers[*].inputs` entries.
    last_consumed_tick: u32,
    /// Number of redundant past inputs to send with each packet.
    /// At INPUT_DELAY=4 + REDUNDANT=4 we tolerate 8 consecutive
    /// dropped packets per peer before stalling.
    redundant_history: u32,
    /// Last computed local state hash, for [`state_hash`] readback.
    last_local_hash: u64,
    /// Hashes received from each peer (most recent only). The runner
    /// compares per tick.
    peer_hashes: HashMap<u8, (u32, u64)>,
    /// Whether we've logged a desync since session start. Once flipped
    /// true, further desync messages are suppressed (one-line policy
    /// — Principle 3, no spam).
    desync_logged: bool,
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

/// Bind a UDP socket on `port` and become the host (peer 0). The
/// host is the source of truth for peer-id assignment; clients
/// connect with [`connect`].
///
/// `expected_peers` is the total peer count including the host — a
/// 2-player game calls `host(7777, 2)` and waits for one client.
pub fn host(port: u16, expected_peers: u8) -> Result<(), String> {
    if !(2..=MAX_PEERS as u8).contains(&expected_peers) {
        return Err(format!(
            "net.host: expected_peers must be 2..={} (got {expected_peers})",
            MAX_PEERS
        ));
    }
    close();
    let bind = format!("0.0.0.0:{port}");
    let socket = UdpSocket::bind(&bind).map_err(|e| format!("net.host: bind {bind}: {e}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|e| format!("net.host: set_nonblocking: {e}"))?;
    let session_id = pseudorandom_u32();
    SESSION.with(|s| {
        *s.borrow_mut() = Some(Session {
            socket,
            local_peer_id: 0,
            expected_peers,
            session_id,
            peers: Vec::new(),
            local_inputs: HashMap::new(),
            sent_tick: 0,
            last_consumed_tick: 0,
            redundant_history: 4,
            last_local_hash: 0,
            peer_hashes: HashMap::new(),
            desync_logged: false,
        });
    });
    Ok(())
}

/// Connect to a host at `host:port`. Sends a hello immediately and
/// returns once the host's hello-ack arrives (or after a 5s
/// handshake timeout). After this call returns Ok, the play loop
/// can start exchanging inputs.
pub fn connect(addr: &str) -> Result<(), String> {
    close();
    let target: SocketAddr = addr
        .parse()
        .map_err(|e| format!("net.connect: parse {addr}: {e}"))?;
    let socket =
        UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("net.connect: bind ephemeral: {e}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|e| format!("net.connect: set_nonblocking: {e}"))?;

    // Send hello, expected_peers field is a placeholder — the host
    // sets the real value and echoes back.
    let mut hello = build_header(MSG_HELLO, 0, 0, 0);
    hello.extend_from_slice(&[0u8; 4]); // expected_peers placeholder
    socket
        .send_to(&hello, target)
        .map_err(|e| format!("net.connect: send hello: {e}"))?;

    // Wait for hello-ack (blocking with timeout). We poll the
    // non-blocking socket until either an ack arrives or the
    // handshake timeout elapses.
    let started = Instant::now();
    let mut buf = [0u8; 1500];
    let mut got_id: Option<(u8, u8, u32)> = None; // (peer_id, expected_peers, session_id)
    while started.elapsed().as_secs() < 5 {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) if src == target => {
                if let Some(h) = parse_header(&buf[..n]) {
                    if h.msg_type == MSG_HELLO {
                        let payload = &buf[HEADER_LEN..n];
                        if payload.len() >= 4 {
                            got_id = Some((h.peer_id, payload[0], h.session_id));
                            break;
                        }
                    }
                }
            }
            Ok(_) => {} // unrelated source; ignore
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => return Err(format!("net.connect: recv: {e}")),
        }
    }
    let Some((peer_id, expected_peers, session_id)) = got_id else {
        return Err(format!("net.connect: no hello-ack from {target} within 5s"));
    };
    let host_peer = Peer {
        id: 0,
        addr: target,
        inputs: HashMap::new(),
        last_seen_tick: 0,
        last_seen_at: Instant::now(),
    };
    SESSION.with(|s| {
        *s.borrow_mut() = Some(Session {
            socket,
            local_peer_id: peer_id,
            expected_peers,
            session_id,
            peers: vec![host_peer],
            local_inputs: HashMap::new(),
            sent_tick: 0,
            last_consumed_tick: 0,
            redundant_history: 4,
            last_local_hash: 0,
            peer_hashes: HashMap::new(),
            desync_logged: false,
        });
    });
    Ok(())
}

/// Close the active session and release the socket. Sends a `bye`
/// packet to every known peer best-effort. Idempotent.
pub fn close() {
    SESSION.with(|s| {
        let mut slot = s.borrow_mut();
        if let Some(sess) = slot.as_mut() {
            let bye = build_header(MSG_BYE, sess.session_id, sess.local_peer_id, sess.sent_tick);
            for peer in &sess.peers {
                let _ = sess.socket.send_to(&bye, peer.addr);
            }
        }
        *slot = None;
    });
}

/// True iff a session has been opened (host or connect succeeded)
/// and not yet closed.
pub fn is_connected() -> bool {
    SESSION.with(|s| s.borrow().is_some())
}

/// Local peer id (0 for host). Returns 0 if no session is active —
/// callers should check `is_connected()` first if they care.
pub fn local_peer_id() -> u8 {
    SESSION.with(|s| s.borrow().as_ref().map(|x| x.local_peer_id).unwrap_or(0))
}

/// Total peers in the session including self.
pub fn peer_count() -> u8 {
    SESSION.with(|s| s.borrow().as_ref().map(|x| x.expected_peers).unwrap_or(0))
}

/// True once every expected peer has joined (host has accepted all
/// hellos / client has received its hello-ack). Use this to delay
/// the start of simulation on the host until all clients connect —
/// otherwise the host would advance past tick 0 alone, locking
/// late joiners out of the lockstep window.
pub fn session_ready() -> bool {
    SESSION.with(|s| {
        let slot = s.borrow();
        let Some(sess) = slot.as_ref() else {
            return false;
        };
        sess.peers.len() + 1 >= sess.expected_peers as usize
    })
}

/// Snapshot the local input as a Frame, store it in the local-input
/// ring at `tick`, and broadcast it (plus `redundant_history`
/// previous frames) to every known peer.
///
/// **Write-once per tick.** A second call with the same `tick` does
/// not overwrite the stored frame — the lockstep determinism
/// contract requires every peer to see the same inputs for tick T,
/// so once T is committed locally and broadcast, it is final. The
/// retransmit-of-history path (the inner `for t in start..=tick`
/// loop below) keeps redelivering whatever the original frame was.
pub fn send_input(tick: u32, frame: Frame) {
    SESSION.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(sess) = slot.as_mut() else { return };
        sess.local_inputs.entry(tick).or_insert(frame);
        sess.sent_tick = sess.sent_tick.max(tick);
        // For each peer, send tick + redundant history.
        for peer in &sess.peers {
            let start = tick.saturating_sub(sess.redundant_history);
            for t in start..=tick {
                if let Some(f) = sess.local_inputs.get(&t) {
                    let mut pkt = build_header(
                        MSG_INPUT,
                        sess.session_id,
                        sess.local_peer_id,
                        t,
                    );
                    pkt.extend_from_slice(f.encode_line().as_bytes());
                    let _ = sess.socket.send_to(&pkt, peer.addr);
                }
            }
        }
    });
}

/// Drain pending UDP packets from the OS receive buffer. Should be
/// called every frame, before `tick_ready`. Returns the number of
/// packets that were processed (informational; useful for tests).
pub fn poll() -> usize {
    let mut n_processed = 0;
    SESSION.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(sess) = slot.as_mut() else { return };
        let mut buf = [0u8; 1500];
        loop {
            match sess.socket.recv_from(&mut buf) {
                Ok((n, src)) => {
                    if handle_packet(sess, src, &buf[..n]) {
                        n_processed += 1;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    });
    n_processed
}

fn handle_packet(sess: &mut Session, src: SocketAddr, buf: &[u8]) -> bool {
    let Some(h) = parse_header(buf) else {
        return false;
    };
    // Drop packets from stale runs (host restarted with a new
    // session_id). Hello packets are exempt — that's the path through
    // which a new session_id propagates.
    if h.msg_type != MSG_HELLO && h.session_id != sess.session_id {
        return false;
    }
    match h.msg_type {
        MSG_HELLO if sess.local_peer_id == 0 => {
            // Host: assign peer id, reply with hello-ack.
            let assigned = sess.peers.len() as u8 + 1;
            if assigned >= sess.expected_peers {
                // Already full — silently drop additional hellos.
                return false;
            }
            sess.peers.push(Peer {
                id: assigned,
                addr: src,
                inputs: HashMap::new(),
                last_seen_tick: 0,
                last_seen_at: Instant::now(),
            });
            let mut ack = build_header(MSG_HELLO, sess.session_id, assigned, 0);
            ack.extend_from_slice(&[sess.expected_peers, 0, 0, 0]);
            let _ = sess.socket.send_to(&ack, src);
            true
        }
        MSG_INPUT => {
            if let Some(peer) = sess.peers.iter_mut().find(|p| p.addr == src) {
                let payload = &buf[HEADER_LEN..];
                if let Ok(text) = std::str::from_utf8(payload) {
                    if let Some(frame) = Frame::decode_line(text) {
                        peer.inputs.entry(h.tick).or_insert(frame);
                        peer.last_seen_tick = peer.last_seen_tick.max(h.tick);
                        peer.last_seen_at = Instant::now();
                        return true;
                    }
                }
            } else if sess.local_peer_id != 0 && h.peer_id == 0 {
                // Client + first packet from host before peer table
                // populated — already handled in connect(), but if
                // we get here, install the host as peer 0.
                sess.peers.push(Peer {
                    id: 0,
                    addr: src,
                    inputs: HashMap::new(),
                    last_seen_tick: 0,
                    last_seen_at: Instant::now(),
                });
            }
            false
        }
        MSG_HASH => {
            let payload = &buf[HEADER_LEN..];
            if payload.len() >= 8 {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&payload[..8]);
                let hash = u64::from_le_bytes(bytes);
                sess.peer_hashes.insert(h.peer_id, (h.tick, hash));
                check_desync(sess);
                return true;
            }
            false
        }
        MSG_BYE => {
            sess.peers.retain(|p| p.addr != src);
            true
        }
        _ => false,
    }
}

/// Returns true once every peer has delivered a Frame for `tick`.
/// The lockstep play loop blocks (busy-poll + sleep) on this until
/// it returns true; that's how clocks across peers stay locked.
pub fn tick_ready(tick: u32) -> bool {
    SESSION.with(|s| {
        let slot = s.borrow();
        let Some(sess) = slot.as_ref() else {
            return false;
        };
        // Local input must be in the ring.
        if !sess.local_inputs.contains_key(&tick) {
            return false;
        }
        // Every peer must have sent input for this tick. Note: peer
        // table size is N-1 from local's POV (excluding self); the
        // session is fully populated only when peers.len() ==
        // expected_peers - 1.
        if sess.peers.len() < (sess.expected_peers - 1) as usize {
            return false;
        }
        sess.peers.iter().all(|p| p.inputs.contains_key(&tick))
    })
}

/// Pull the per-peer Frames for `tick`, in peer-id order. Marks the
/// tick consumed and garbage-collects ring entries older than
/// `tick - DEFAULT_INPUT_DELAY*2` so the maps don't grow unbounded.
///
/// Returns `None` if [`tick_ready`] is false for this tick.
pub fn take_inputs(tick: u32) -> Option<Vec<(u8, Frame)>> {
    SESSION.with(|s| {
        let mut slot = s.borrow_mut();
        let sess = slot.as_mut()?;
        if !sess.local_inputs.contains_key(&tick) {
            return None;
        }
        if sess.peers.len() < (sess.expected_peers - 1) as usize {
            return None;
        }
        if !sess.peers.iter().all(|p| p.inputs.contains_key(&tick)) {
            return None;
        }
        let mut out: Vec<(u8, Frame)> = Vec::with_capacity(sess.expected_peers as usize);
        // Local input always present.
        out.push((sess.local_peer_id, sess.local_inputs[&tick].clone()));
        // Remote peers — read each peer's stored `id`, not the
        // table position. From a client's POV, peers[0] is the host
        // (id = 0), not "the first non-self peer."
        for p in &sess.peers {
            out.push((p.id, p.inputs[&tick].clone()));
        }
        // Canonical peer-id ordering so all peers see inputs in the
        // same order (required for the lockstep determinism contract).
        out.sort_by_key(|(id, _)| *id);
        sess.last_consumed_tick = sess.last_consumed_tick.max(tick);
        // Garbage-collect old inputs. Keep enough history for a
        // retransmit window of ~2x INPUT_DELAY.
        let keep_floor = tick.saturating_sub(DEFAULT_INPUT_DELAY * 2);
        sess.local_inputs.retain(|t, _| *t >= keep_floor);
        for peer in &mut sess.peers {
            peer.inputs.retain(|t, _| *t >= keep_floor);
        }
        Some(out)
    })
}

/// Broadcast a state-hash heartbeat. Called by the runner once per
/// `HASH_INTERVAL` ticks for desync detection. Stores the local hash
/// for [`state_hash`] readback.
pub fn send_state_hash(tick: u32, hash: u64) {
    SESSION.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(sess) = slot.as_mut() else { return };
        sess.last_local_hash = hash;
        sess.peer_hashes
            .insert(sess.local_peer_id, (tick, hash));
        let mut pkt = build_header(MSG_HASH, sess.session_id, sess.local_peer_id, tick);
        pkt.extend_from_slice(&hash.to_le_bytes());
        for peer in &sess.peers {
            let _ = sess.socket.send_to(&pkt, peer.addr);
        }
        check_desync(sess);
    });
}

fn check_desync(sess: &mut Session) {
    if sess.desync_logged {
        return;
    }
    // Find the most recent tick that has a hash from every peer.
    let mut by_tick: HashMap<u32, Vec<u64>> = HashMap::new();
    for (t, h) in sess.peer_hashes.values() {
        by_tick.entry(*t).or_default().push(*h);
    }
    for (tick, hashes) in by_tick {
        if hashes.len() == sess.expected_peers as usize {
            let first = hashes[0];
            if hashes.iter().any(|h| *h != first) {
                eprintln!(
                    "[twec net] DESYNC at tick {tick}: hashes diverged ({hashes:?}). \
                     Game state is now divergent across peers; bug-report this run."
                );
                sess.desync_logged = true;
                return;
            }
        }
    }
}

/// Return the local peer's most recent state hash. Useful for tests
/// and the `net.state_hash()` builtin.
pub fn local_state_hash() -> u64 {
    SESSION.with(|s| s.borrow().as_ref().map(|x| x.last_local_hash).unwrap_or(0))
}

// ---------- header / framing ----------

#[derive(Debug)]
struct Header {
    msg_type: u8,
    session_id: u32,
    peer_id: u8,
    tick: u32,
}

fn build_header(msg_type: u8, session_id: u32, peer_id: u8, tick: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(HEADER_LEN + 64);
    v.push(MAGIC[0]);
    v.push(MAGIC[1]);
    v.push(VERSION);
    v.push(msg_type);
    v.extend_from_slice(&session_id.to_le_bytes());
    v.push(peer_id);
    v.push(0); // reserved
    v.push(0); // reserved (pad to align tick at offset 12)
    v.push(0); // reserved
    v.extend_from_slice(&tick.to_le_bytes());
    debug_assert_eq!(v.len(), HEADER_LEN);
    v
}

fn parse_header(buf: &[u8]) -> Option<Header> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    if buf[0] != MAGIC[0] || buf[1] != MAGIC[1] || buf[2] != VERSION {
        return None;
    }
    let msg_type = buf[3];
    let mut sid = [0u8; 4];
    sid.copy_from_slice(&buf[4..8]);
    let session_id = u32::from_le_bytes(sid);
    let peer_id = buf[8];
    let mut tick = [0u8; 4];
    tick.copy_from_slice(&buf[12..16]);
    let tick = u32::from_le_bytes(tick);
    Some(Header {
        msg_type,
        session_id,
        peer_id,
        tick,
    })
}

fn pseudorandom_u32() -> u32 {
    // We don't need cryptographic randomness — just a per-host
    // signature so packets from a previous run get filtered. Use
    // wall-clock nanoseconds xored with a process-stable salt.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    nanos ^ 0xa5a5_5a5a
}

// ---------- snapshot serialization (Phase 31 session 5) ----------

/// Encode any serializable Twe Value as canonical JSON. Uses
/// `crate::save::encode` for the value→json mapping (so Tweisms like
/// `Percent`, `Range`, `Quantity` round-trip via tagged objects) and
/// `crate::json::to_string` for the emit (object fields are emitted
/// in sorted BTreeMap order — required for cross-peer determinism).
///
/// Used by `net.snapshot_json(state)` (debug pretty-print of game
/// state) and as the input to [`hash_value`] (deterministic state
/// hash for desync detection).
pub fn snapshot_json(value: &crate::value::Value) -> Result<String, String> {
    let json = crate::save::encode(value)?;
    Ok(crate::json::to_string(&json))
}

/// Hash a Twe value to a u64 by serializing it to canonical JSON and
/// running FNV-1a over the bytes. Stable across machines + Rust
/// versions (no allocator addresses, no HashMap iteration order).
///
/// Used by `net.hash(state)` so scripts can compute a state hash to
/// pass to [`send_state_hash`] without having to fold every
/// individual field by hand.
pub fn hash_value(value: &crate::value::Value) -> Result<u64, String> {
    let s = snapshot_json(value)?;
    Ok(fnv1a(s.as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ---------- input ambient bridge ----------
//
// The lockstep runner needs to (1) read the local input ambients
// into a Frame at send time, and (2) write the merged peer inputs
// back into the ambients before tick_frame runs. The replay module
// already has these helpers but they're private; we duplicate the
// minimal version here to avoid a pub-API churn on replay.rs.

use crate::value::{Env, Object, Value};
use std::rc::Rc;

/// Snapshot the input ambients into a Frame. Mirrors
/// `replay::snapshot_inputs`.
pub fn snapshot_local(env: &Env) -> Frame {
    Frame {
        keys_held: collect_true_field_names(env, "key"),
        keys_pressed: collect_true_field_names(env, "key_press"),
        mouse_x: read_mouse_axis(env, 0),
        mouse_y: read_mouse_axis(env, 1),
        mb_held: collect_true_field_names(env, "mouse_held"),
        mb_press: collect_true_field_names(env, "mouse_press"),
    }
}

fn collect_true_field_names(env: &Env, ambient: &str) -> Vec<String> {
    let opt = env.get(ambient);
    let Some(v) = opt.as_ref() else {
        return Vec::new();
    };
    if !v.is_object() {
        return Vec::new();
    }
    let rc = v.as_object();
    let o = rc.borrow();
    let mut names: Vec<String> = o
        .fields
        .iter()
        .filter_map(|(k, v)| {
            if v.is_bool() && v.as_bool() {
                Some(k.clone())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

fn read_mouse_axis(env: &Env, axis: usize) -> f64 {
    let opt = env.get("mouse");
    let Some(v) = opt.as_ref() else {
        return 0.0;
    };
    if !v.is_object() {
        return 0.0;
    }
    let rc = v.as_object();
    let o = rc.borrow();
    let key = if axis == 0 { "x" } else { "y" };
    if let Some(f) = o.fields.get(key) {
        if f.is_float() {
            return f.as_float();
        }
        if f.is_int_or_boxed_int() {
            return f.as_int() as f64;
        }
    }
    0.0
}

/// Apply a per-peer set of Frames to the input ambients as a
/// merged view: a key is "held" if ANY peer has it held; a peer-id
/// view is exposed via the `peer` ambient (a list-of-objects).
///
/// The simple "OR all peers" merge fits cooperative games (everyone
/// shares one input track). Adversarial games disambiguate via the
/// `peer` ambient — `peer[0].key.left`, `peer[1].key.right` — which
/// the runner installs alongside the merged view.
///
/// Per-peer `key` / `key_press` objects are populated with the same
/// field set as the existing global `key` ambient (every supported
/// key name) so `peer[0].key.w` is readable whether or not "w" is
/// currently held — values are just `false` when unheld. Without
/// this, scripts reading missing fields would hit Principle-3
/// "field not found" errors on the first frame.
pub fn apply_merged(env: &mut Env, frames: &[(u8, Frame)]) {
    // Read the current global `key` / `key_press` field templates so
    // per-peer objects match shape. If they're missing (e.g. very
    // first frame before stdlib install), fall back to whatever
    // keys are held in any peer's frame.
    let key_template = field_name_template(env, "key");
    let key_press_template = field_name_template(env, "key_press");
    let mb_template = field_name_template(env, "mouse_held");
    let mb_press_template = field_name_template(env, "mouse_press");

    // Build per-peer objects.
    let mut peers_list: Vec<Value> = Vec::with_capacity(frames.len());
    for (id, f) in frames {
        let mut fields: HashMap<String, Value> = HashMap::new();
        fields.insert("id".to_string(), Value::from_int(*id as i64));
        fields.insert(
            "key".to_string(),
            bool_field_object(&key_template, &f.keys_held),
        );
        fields.insert(
            "key_press".to_string(),
            bool_field_object(&key_press_template, &f.keys_pressed),
        );
        fields.insert("mouse_x".to_string(), Value::from_float(f.mouse_x));
        fields.insert("mouse_y".to_string(), Value::from_float(f.mouse_y));
        fields.insert(
            "mouse_held".to_string(),
            bool_field_object(&mb_template, &f.mb_held),
        );
        fields.insert(
            "mouse_press".to_string(),
            bool_field_object(&mb_press_template, &f.mb_press),
        );
        peers_list.push(Value::from_object(Rc::new(RefCell::new(Object {
            fields,
            kind: "peer",
        }))));
    }
    env.set(
        "peer".to_string(),
        Value::from_list(Rc::new(RefCell::new(peers_list))),
    );

    // Merge: union of held keys across all peers.
    let union_held = union_all(frames, |f| &f.keys_held);
    let union_pressed = union_all(frames, |f| &f.keys_pressed);
    let union_mb_held = union_all(frames, |f| &f.mb_held);
    let union_mb_press = union_all(frames, |f| &f.mb_press);
    set_bool_ambient(env, "key", &union_held);
    set_bool_ambient(env, "key_press", &union_pressed);
    set_bool_ambient(env, "mouse_held", &union_mb_held);
    set_bool_ambient(env, "mouse_press", &union_mb_press);
    // Mouse position is taken from peer 0 (host) for the merged view;
    // adversarial games use peer[i].mouse_x / mouse_y.
    if let Some((_, f0)) = frames.iter().min_by_key(|(id, _)| *id) {
        write_mouse_pos(env, f0.mouse_x, f0.mouse_y);
    }
}

fn union_all<F>(frames: &[(u8, Frame)], f: F) -> Vec<String>
where
    F: Fn(&Frame) -> &Vec<String>,
{
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (_, fr) in frames {
        for k in f(fr) {
            set.insert(k.clone());
        }
    }
    set.into_iter().collect()
}

fn bool_field_object(template: &[String], true_keys: &[String]) -> Value {
    let mut fields: HashMap<String, Value> = HashMap::new();
    for k in template {
        fields.insert(k.clone(), Value::from_bool(false));
    }
    for k in true_keys {
        fields.insert(k.clone(), Value::from_bool(true));
    }
    Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "input",
    })))
}

/// Read the field-name list from the named ambient so per-peer
/// objects match shape. If the ambient doesn't exist yet (very
/// early in startup), returns an empty template — the per-peer
/// object will only contain held keys, which is the same shape
/// `apply_merged` had in Phase 31 sessions 2–4.
fn field_name_template(env: &Env, ambient: &str) -> Vec<String> {
    let opt = env.get(ambient);
    let Some(v) = opt.as_ref() else {
        return Vec::new();
    };
    if !v.is_object() {
        return Vec::new();
    }
    let rc = v.as_object();
    let o = rc.borrow();
    o.fields.keys().cloned().collect()
}

fn set_bool_ambient(env: &mut Env, name: &str, true_keys: &[String]) {
    let opt = env.get(name);
    if let Some(v) = opt.as_ref() {
        if v.is_object() {
            let rc = v.as_object();
            let mut o = rc.borrow_mut();
            for (_, slot) in o.fields.iter_mut() {
                if slot.is_bool() {
                    *slot = Value::from_bool(false);
                }
            }
            for k in true_keys {
                o.insert_field(k, Value::from_bool(true));
            }
            return;
        }
    }
    let mut fields: HashMap<String, Value> = HashMap::new();
    for k in true_keys {
        fields.insert(k.clone(), Value::from_bool(true));
    }
    env.set(
        name.to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields,
            kind: "input",
        }))),
    );
}

fn write_mouse_pos(env: &mut Env, x: f64, y: f64) {
    let opt = env.get("mouse");
    if let Some(v) = opt.as_ref() {
        if v.is_object() {
            let rc = v.as_object();
            let mut o = rc.borrow_mut();
            o.insert_field("x", Value::from_float(x));
            o.insert_field("y", Value::from_float(y));
            return;
        }
    }
    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("x".to_string(), Value::from_float(x));
    fields.insert("y".to_string(), Value::from_float(y));
    env.set(
        "mouse".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields,
            kind: "input",
        }))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_value_is_deterministic_for_equivalent_objects() {
        // Two objects with the same fields inserted in different
        // orders must hash identically — that's the guarantee that
        // makes net.hash safe to use across peers without worrying
        // about HashMap iteration order.
        use crate::value::{Object, Value};
        use std::cell::RefCell;
        use std::collections::HashMap;
        use std::rc::Rc;

        let mut a = HashMap::new();
        a.insert("score".to_string(), Value::from_int(7));
        a.insert("ball_x".to_string(), Value::from_float(123.5));
        a.insert("ball_y".to_string(), Value::from_float(45.0));

        let mut b = HashMap::new();
        b.insert("ball_y".to_string(), Value::from_float(45.0));
        b.insert("score".to_string(), Value::from_int(7));
        b.insert("ball_x".to_string(), Value::from_float(123.5));

        let va = Value::from_object(Rc::new(RefCell::new(Object {
            fields: a,
            kind: "snap",
        })));
        let vb = Value::from_object(Rc::new(RefCell::new(Object {
            fields: b,
            kind: "snap",
        })));
        assert_eq!(hash_value(&va).unwrap(), hash_value(&vb).unwrap());
    }

    #[test]
    fn hash_value_differs_for_different_state() {
        use crate::value::Value;
        let a = Value::from_int(5);
        let b = Value::from_int(6);
        assert_ne!(hash_value(&a).unwrap(), hash_value(&b).unwrap());
    }

    #[test]
    fn frame_line_round_trips() {
        let f = Frame {
            keys_held: vec!["left".to_string(), "space".to_string()],
            keys_pressed: vec!["space".to_string()],
            mouse_x: 123.5,
            mouse_y: 45.0,
            mb_held: vec!["left".to_string()],
            mb_press: Vec::new(),
        };
        let line = f.encode_line();
        let back = Frame::decode_line(&line).expect("decode");
        assert_eq!(f, back);
    }

    #[test]
    fn header_round_trips() {
        let h = build_header(MSG_INPUT, 0xdead_beef, 2, 1234);
        let p = parse_header(&h).expect("parse");
        assert_eq!(p.msg_type, MSG_INPUT);
        assert_eq!(p.session_id, 0xdead_beef);
        assert_eq!(p.peer_id, 2);
        assert_eq!(p.tick, 1234);
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut h = build_header(MSG_INPUT, 1, 0, 0);
        h[0] = b'X';
        assert!(parse_header(&h).is_none());
    }

    #[test]
    fn host_rejects_bad_peer_count() {
        assert!(host(0, 1).is_err());
        assert!(host(0, (MAX_PEERS as u8) + 1).is_err());
    }

    #[test]
    fn host_then_close_is_idempotent() {
        host(0, 2).unwrap(); // port 0 = OS picks one
        assert!(is_connected());
        close();
        assert!(!is_connected());
        close(); // calling twice is fine
    }

    #[test]
    fn local_peer_id_is_zero_for_host() {
        host(0, 2).unwrap();
        assert_eq!(local_peer_id(), 0);
        assert_eq!(peer_count(), 2);
        close();
    }

    #[test]
    fn two_peer_loopback_handshake_and_input_exchange() {
        // Bring up a host on an OS-assigned port, read it back from
        // the socket, then connect from a second "thread" (we can't
        // use SESSION concurrently, so this test threads the whole
        // session through manually using a second UdpSocket).
        // Instead of going through `connect`, we drive the host
        // half via the public API and the client half by hand.
        host(0, 2).unwrap();
        let host_port = SESSION.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .socket
                .local_addr()
                .unwrap()
                .port()
        });
        let host_addr: SocketAddr = format!("127.0.0.1:{host_port}").parse().unwrap();

        // Manual client.
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.set_nonblocking(true).unwrap();

        // Send hello from client.
        let mut hello = build_header(MSG_HELLO, 0, 0, 0);
        hello.extend_from_slice(&[0u8; 4]);
        client.send_to(&hello, host_addr).unwrap();

        // Drive the host to drain + ack.
        std::thread::sleep(std::time::Duration::from_millis(20));
        poll();

        // Client reads hello-ack.
        let mut buf = [0u8; 1500];
        let mut got = false;
        for _ in 0..50 {
            match client.recv_from(&mut buf) {
                Ok((n, _)) => {
                    let h = parse_header(&buf[..n]).expect("ack header");
                    assert_eq!(h.msg_type, MSG_HELLO);
                    assert_eq!(h.peer_id, 1);
                    assert_eq!(buf[HEADER_LEN], 2); // expected_peers
                    got = true;
                    break;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        assert!(got, "client did not receive hello-ack");

        // Host should now have one peer.
        let n_peers = SESSION.with(|s| s.borrow().as_ref().unwrap().peers.len());
        assert_eq!(n_peers, 1);

        // Send an input frame from the manual client at tick 0.
        let frame = Frame {
            keys_held: vec!["up".to_string()],
            keys_pressed: Vec::new(),
            mouse_x: 0.0,
            mouse_y: 0.0,
            mb_held: Vec::new(),
            mb_press: Vec::new(),
        };
        let session_id = SESSION.with(|s| s.borrow().as_ref().unwrap().session_id);
        let mut pkt = build_header(MSG_INPUT, session_id, 1, 0);
        pkt.extend_from_slice(frame.encode_line().as_bytes());
        client.send_to(&pkt, host_addr).unwrap();

        // Host snapshot's its own input for tick 0.
        send_input(0, Frame::default());

        // Drain.
        std::thread::sleep(std::time::Duration::from_millis(20));
        poll();

        assert!(tick_ready(0), "host should be tick-ready after exchanging inputs");
        let inputs = take_inputs(0).expect("take_inputs");
        assert_eq!(inputs.len(), 2);
        // Find the peer-1 frame and confirm its 'up' came through.
        let p1 = inputs.iter().find(|(id, _)| *id == 1).expect("peer 1");
        assert_eq!(p1.1.keys_held, vec!["up".to_string()]);

        close();
    }
}
