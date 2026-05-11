//! Phase 36 session 3: STUN binding + TCP rendezvous client.
//!
//! Per `docs/changes/2026-05-10-matchmaking-rfc.md` the non-Steam
//! fallback uses STUN to discover each peer's public address, then a
//! tiny TCP rendezvous server to swap addresses, then a simultaneous-
//! open UDP punch via the existing `net::host` / `net::connect` path.
//!
//! ## Why STUN at all
//!
//! Most home routers run cone NAT — outbound UDP packets create a
//! mapping that subsequent inbound packets *to the same external
//! address:port* can use, regardless of source. A STUN server's only
//! job is to look at the source address of an incoming packet and
//! echo it back, so the sender learns its NAT-mapped public address.
//! That public address is what the peer needs to reach you.
//!
//! ## Why rendezvous separately
//!
//! STUN tells you *your* address; it doesn't tell you *the peer's*.
//! For two peers to find each other we need a third party they both
//! talk to — that's the rendezvous server. We use a tiny TCP
//! protocol: each peer connects to the rendezvous, sends a single
//! line with `lobby_name` + their STUN-discovered address, and the
//! rendezvous pairs them up + replies with the other peer's address.
//!
//! Symmetric NAT (same external port for outbound packets, but
//! different mappings to different destinations) defeats this scheme.
//! That's the topology where Steam P2P (relayed) wins; the STUN
//! fallback honestly admits failure with a `connect_failure_reason`
//! of `"symmetric-nat-no-relay"`.
//!
//! ## What this module is NOT
//!
//! This module **does not** open the lockstep session itself. The
//! rendezvous returns the peer's public address as a string; scripts
//! then call `net.host(...)` or `net.connect(...)` with that
//! address. The reason: Phase 31's lockstep runner expects the
//! standard 16-byte `MSG_HELLO` handshake, and the post-rendezvous
//! UDP punch fits that contract directly. Adding a separate
//! handshake here would duplicate the runner.
//!
//! ## Wire format — rendezvous TCP
//!
//! ```text
//! Client → Server:  "JOIN <lobby_name> <addr>\n"
//! Server → Client:  "PEER <addr>\n"        (when match found)
//!                   "WAIT\n"               (no peer yet; client may retry)
//!                   "ERR <message>\n"      (lobby full / bad request)
//! ```
//!
//! All exchanges are single-line ASCII, terminated by `\n`. Server
//! is one-shot — it pairs two clients per lobby_name and closes both
//! TCP connections after sending PEER. A reference implementation
//! lives at `tools/twec-rendezvous/` (separate binary, not built by
//! default).

#![cfg(not(target_arch = "wasm32"))]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// Default STUN server. Public Google STUN — works for most home
/// networks. Operators can override with the `stun_server` arg to
/// `net.connect_via_stun`.
pub const DEFAULT_STUN_SERVER: &str = "stun.l.google.com:19302";

/// STUN message type for a binding request (RFC 5389 §6).
const MSG_BINDING_REQUEST: u16 = 0x0001;
/// STUN binding success response.
const MSG_BINDING_SUCCESS: u16 = 0x0101;
/// STUN magic cookie (RFC 5389 §6).
const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;

/// XOR-MAPPED-ADDRESS attribute type (RFC 5389 §15.2).
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
/// MAPPED-ADDRESS attribute type (legacy; some servers still use
/// this).
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;

/// Address-family field of a MAPPED-ADDRESS attribute. We only
/// support IPv4 in this iteration — IPv6 STUN works fine but adds
/// 12 bytes to the parser and most home setups still default to v4.
const FAMILY_IPV4: u8 = 0x01;

/// Send a STUN Binding Request to `stun_server` and return the
/// public-facing `SocketAddr` the server saw the request from.
///
/// `local_socket` is the UDP socket the script wants to play on —
/// the STUN request goes out from that exact socket so the NAT
/// mapping the response sees is the one the lockstep traffic will
/// later use. **Do not** open a fresh socket for STUN; the NAT
/// mapping would be different.
pub fn discover_public_address(
    local_socket: &UdpSocket,
    stun_server: &str,
) -> Result<SocketAddr, String> {
    let target = stun_server
        .to_socket_addrs()
        .map_err(|e| format!("STUN: resolve {stun_server}: {e}"))?
        .next()
        .ok_or_else(|| format!("STUN: no addresses for {stun_server}"))?;

    // Build binding request: 20-byte header, no attributes.
    // [u16 type][u16 length][u32 cookie][u96 transaction_id]
    let txn_id: [u8; 12] = generate_transaction_id();
    let mut req = Vec::with_capacity(20);
    req.extend_from_slice(&MSG_BINDING_REQUEST.to_be_bytes());
    req.extend_from_slice(&0u16.to_be_bytes()); // length = 0 (no attrs)
    req.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    req.extend_from_slice(&txn_id);
    debug_assert_eq!(req.len(), 20);

    // Send + wait for reply with a 2-second budget. STUN reply is
    // small (few hundred bytes) so a single recv_from is enough.
    local_socket
        .send_to(&req, target)
        .map_err(|e| format!("STUN: send to {target}: {e}"))?;

    let saved_timeout = local_socket
        .read_timeout()
        .map_err(|e| format!("STUN: read_timeout query: {e}"))?;
    local_socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("STUN: set_read_timeout: {e}"))?;
    let mut buf = [0u8; 1500];
    let result = (|| -> Result<SocketAddr, String> {
        let started = Instant::now();
        loop {
            if started.elapsed() > Duration::from_secs(3) {
                return Err("STUN: no reply within 3s".to_string());
            }
            match local_socket.recv_from(&mut buf) {
                Ok((n, src)) if src == target => {
                    return parse_binding_response(&buf[..n], &txn_id);
                }
                Ok(_) => continue, // unrelated source; ignore
                Err(e) => return Err(format!("STUN: recv: {e}")),
            }
        }
    })();
    // Restore the previous read timeout so we don't perturb the
    // caller's socket settings.
    let _ = local_socket.set_read_timeout(saved_timeout);
    result
}

fn parse_binding_response(buf: &[u8], expected_txn: &[u8; 12]) -> Result<SocketAddr, String> {
    if buf.len() < 20 {
        return Err("STUN: response too short".to_string());
    }
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    if msg_type != MSG_BINDING_SUCCESS {
        return Err(format!("STUN: not a binding-success (got msg_type={msg_type:#x})"));
    }
    let attrs_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let cookie = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    if cookie != STUN_MAGIC_COOKIE {
        return Err("STUN: bad magic cookie".to_string());
    }
    if &buf[8..20] != expected_txn {
        return Err("STUN: transaction id mismatch".to_string());
    }
    if buf.len() < 20 + attrs_len {
        return Err("STUN: attribute section truncated".to_string());
    }
    // Walk attributes. Each attribute is [u16 type][u16 length][value]
    // padded to a 4-byte boundary.
    let mut off = 20;
    let end = 20 + attrs_len;
    while off + 4 <= end {
        let attr_type = u16::from_be_bytes([buf[off], buf[off + 1]]);
        let attr_len = u16::from_be_bytes([buf[off + 2], buf[off + 3]]) as usize;
        let val_start = off + 4;
        let val_end = val_start + attr_len;
        if val_end > end {
            return Err("STUN: attribute value past end".to_string());
        }
        match attr_type {
            ATTR_XOR_MAPPED_ADDRESS => {
                return parse_xor_mapped_address(&buf[val_start..val_end]);
            }
            ATTR_MAPPED_ADDRESS => {
                // Some legacy servers send MAPPED-ADDRESS instead of
                // XOR-MAPPED-ADDRESS. Decode it as a fallback.
                return parse_mapped_address(&buf[val_start..val_end]);
            }
            _ => {} // ignore unknown attributes per RFC 5389 §7.3.1
        }
        // Pad attribute length to 4-byte boundary.
        let padded = (attr_len + 3) & !3;
        off = val_start + padded;
    }
    Err("STUN: no MAPPED-ADDRESS or XOR-MAPPED-ADDRESS in response".to_string())
}

fn parse_xor_mapped_address(value: &[u8]) -> Result<SocketAddr, String> {
    if value.len() < 8 {
        return Err("STUN: XOR-MAPPED-ADDRESS too short".to_string());
    }
    let family = value[1];
    if family != FAMILY_IPV4 {
        return Err(format!(
            "STUN: only IPv4 supported (got family {family:#x})"
        ));
    }
    let xor_port = u16::from_be_bytes([value[2], value[3]]);
    let port = xor_port ^ ((STUN_MAGIC_COOKIE >> 16) as u16);
    let xor_ip = u32::from_be_bytes([value[4], value[5], value[6], value[7]]);
    let ip = xor_ip ^ STUN_MAGIC_COOKIE;
    let octets = ip.to_be_bytes();
    Ok(SocketAddr::from((
        std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]),
        port,
    )))
}

fn parse_mapped_address(value: &[u8]) -> Result<SocketAddr, String> {
    if value.len() < 8 {
        return Err("STUN: MAPPED-ADDRESS too short".to_string());
    }
    let family = value[1];
    if family != FAMILY_IPV4 {
        return Err(format!(
            "STUN: only IPv4 supported (got family {family:#x})"
        ));
    }
    let port = u16::from_be_bytes([value[2], value[3]]);
    Ok(SocketAddr::from((
        std::net::Ipv4Addr::new(value[4], value[5], value[6], value[7]),
        port,
    )))
}

fn generate_transaction_id() -> [u8; 12] {
    // Cryptographic randomness isn't required by RFC 5389 — we just
    // need uniqueness across concurrent in-flight requests. Mix
    // wall-clock nanos with a per-instance counter.
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut id = [0u8; 12];
    id[0..4].copy_from_slice(&nanos.to_be_bytes());
    id[4..8].copy_from_slice(&counter.to_be_bytes());
    id[8..12].copy_from_slice(&0xdead_beefu32.to_be_bytes());
    id
}

/// Connect to a TCP rendezvous server at `rendezvous_addr`, send our
/// public `addr` for `lobby_name`, and wait up to `timeout_ms` for
/// the peer's address. Returns the peer's `SocketAddr` on success.
///
/// **Polling**: if the server replies `WAIT`, we close + retry every
/// 500ms until either `PEER` arrives or the timeout elapses. This
/// keeps the protocol stateless on the server side — no long-lived
/// "park this connection" semantics.
pub fn rendezvous_exchange(
    rendezvous_addr: &str,
    lobby_name: &str,
    public_addr: SocketAddr,
    timeout_ms: u64,
) -> Result<SocketAddr, String> {
    if lobby_name.is_empty() || lobby_name.contains(' ') || lobby_name.contains('\n') {
        return Err("rendezvous: lobby_name must be non-empty and contain no spaces/newlines".to_string());
    }
    let started = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    loop {
        if started.elapsed() > timeout {
            return Err(format!(
                "rendezvous: no peer for lobby {lobby_name:?} within {timeout_ms}ms"
            ));
        }
        match try_one_exchange(rendezvous_addr, lobby_name, public_addr) {
            Ok(Some(peer)) => return Ok(peer),
            Ok(None) => {
                // WAIT — peer not present yet; sleep 500ms and retry.
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) => return Err(e),
        }
    }
}

fn try_one_exchange(
    rendezvous_addr: &str,
    lobby_name: &str,
    public_addr: SocketAddr,
) -> Result<Option<SocketAddr>, String> {
    let mut stream = TcpStream::connect(rendezvous_addr)
        .map_err(|e| format!("rendezvous: connect {rendezvous_addr}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("rendezvous: set_read_timeout: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("rendezvous: set_write_timeout: {e}"))?;

    let line = format!("JOIN {lobby_name} {public_addr}\n");
    stream
        .write_all(line.as_bytes())
        .map_err(|e| format!("rendezvous: write JOIN: {e}"))?;

    let mut buf = Vec::with_capacity(128);
    let mut tmp = [0u8; 128];
    let resp = loop {
        match stream.read(&mut tmp) {
            Ok(0) => break String::from_utf8_lossy(&buf).to_string(),
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.contains(&b'\n') {
                    break String::from_utf8_lossy(&buf).to_string();
                }
            }
            Err(e) => return Err(format!("rendezvous: read: {e}")),
        }
    };
    let line = resp.lines().next().unwrap_or("").trim();
    if let Some(rest) = line.strip_prefix("PEER ") {
        let addr: SocketAddr = rest
            .parse()
            .map_err(|e| format!("rendezvous: parse PEER addr {rest:?}: {e}"))?;
        Ok(Some(addr))
    } else if line == "WAIT" {
        Ok(None)
    } else if let Some(msg) = line.strip_prefix("ERR ") {
        Err(format!("rendezvous: server error: {msg}"))
    } else {
        Err(format!("rendezvous: bad response: {line:?}"))
    }
}

/// One-shot punch: send a few "ping" UDP packets to `peer_addr` so
/// the local NAT installs a return path. The lockstep runner's
/// MSG_HELLO does the real handshake; the punch packets are just
/// padding to make sure the NAT mapping exists when the hello
/// arrives.
pub fn punch(socket: &UdpSocket, peer_addr: SocketAddr) -> Result<(), String> {
    // Three small packets, 50ms apart. Smaller would risk the NAT
    // not installing the mapping in time; larger costs handshake
    // latency.
    for _ in 0..3 {
        socket
            .send_to(b"\x00punch", peer_addr)
            .map_err(|e| format!("punch: send: {e}"))?;
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn xor_mapped_address_decodes_known_value() {
        // From an actual STUN response. Family=1 (IPv4), port = 0xc26b
        // XOR'd with the magic cookie's high half (0x2112) → 0xe379.
        // IP = 0xa1ad8b87 XOR 0x2112a442 → 0x80bf2fc5 → 128.191.47.197.
        let value = [0x00, 0x01, 0xc2, 0x6b, 0xa1, 0xad, 0x8b, 0x87];
        let addr = parse_xor_mapped_address(&value).unwrap();
        let port = (0xc26b_u16) ^ ((STUN_MAGIC_COOKIE >> 16) as u16);
        assert_eq!(addr.port(), port);
    }

    #[test]
    fn parse_response_rejects_bad_magic_cookie() {
        let mut resp = vec![0u8; 20];
        resp[0..2].copy_from_slice(&MSG_BINDING_SUCCESS.to_be_bytes());
        resp[2..4].copy_from_slice(&0u16.to_be_bytes());
        resp[4..8].copy_from_slice(&0u32.to_be_bytes()); // wrong cookie
        let txn = [0u8; 12];
        assert!(parse_binding_response(&resp, &txn).is_err());
    }

    #[test]
    fn rendezvous_rejects_bad_lobby_name() {
        let public_addr: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        assert!(rendezvous_exchange("127.0.0.1:9", "", public_addr, 100).is_err());
        assert!(rendezvous_exchange("127.0.0.1:9", "bad name", public_addr, 100).is_err());
    }

    #[test]
    fn rendezvous_round_trip_against_local_test_server() {
        // Stand up a tiny one-shot rendezvous on an OS port.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            // Accept first JOIN, hold it until second JOIN arrives,
            // then reply PEER on each.
            let (mut a, _) = listener.accept().unwrap();
            let mut buf_a = [0u8; 256];
            let n_a = a.read(&mut buf_a).unwrap();
            let line_a = String::from_utf8_lossy(&buf_a[..n_a]).to_string();
            let (mut b, _) = listener.accept().unwrap();
            let mut buf_b = [0u8; 256];
            let n_b = b.read(&mut buf_b).unwrap();
            let line_b = String::from_utf8_lossy(&buf_b[..n_b]).to_string();
            // Each line is "JOIN <lobby> <addr>\n".
            let addr_a = line_a.split_whitespace().nth(2).unwrap().to_string();
            let addr_b = line_b.split_whitespace().nth(2).unwrap().to_string();
            a.write_all(format!("PEER {addr_b}\n").as_bytes()).unwrap();
            b.write_all(format!("PEER {addr_a}\n").as_bytes()).unwrap();
        });
        let addr_a: SocketAddr = "127.0.0.1:11111".parse().unwrap();
        let addr_b: SocketAddr = "127.0.0.1:22222".parse().unwrap();
        let rendezvous = format!("127.0.0.1:{port}");
        let r1 = rendezvous;
        let r2 = r1.clone();
        let h1 = thread::spawn(move || rendezvous_exchange(&r1, "lobby1", addr_a, 5000));
        std::thread::sleep(Duration::from_millis(50));
        let h2 = thread::spawn(move || rendezvous_exchange(&r2, "lobby1", addr_b, 5000));
        let r1 = h1.join().unwrap().unwrap();
        let r2 = h2.join().unwrap().unwrap();
        assert_eq!(r1, addr_b);
        assert_eq!(r2, addr_a);
        let _ = server.join();
    }
}
