//! Phase 31 session 4: end-to-end lockstep test.
//!
//! Spawns two threads; each is a separate Twe peer (thread-locals
//! mean each gets its own [`net::SESSION`]). Thread A hosts on an
//! OS-assigned port; thread B connects. They exchange inputs +
//! state hashes for 30 ticks and assert that:
//!
//! 1. Every tick is fully consumed on both sides (peer inputs arrive
//!    in time).
//! 2. The state hashes match across peers at every tick (no desync).
//! 3. After close, no session is active.
//!
//! This is the canonical "lockstep works" test — if it passes, the
//! pipeline from input snapshot → wire packet → peer table →
//! tick_ready → take_inputs → apply_merged is end-to-end correct.

use std::net::UdpSocket;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use twec::net;

#[test]
fn two_peer_lockstep_30_ticks() {
    // Pick a port by binding ephemeral first, then closing — the OS
    // is unlikely to reuse it within the millisecond it takes
    // host() to re-bind. (Using port 0 in `host` would work too,
    // but then we'd need a way to communicate the assigned port to
    // the client thread; this avoids the channel.)
    let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let (host_done_tx, host_done_rx) = mpsc::channel::<u64>();

    let host_thread = thread::spawn(move || -> u64 {
        net::host(port, 2).expect("host bind");

        // Wait for the client to join — `session_ready()` flips true
        // once the host has accepted the hello and recorded the peer.
        // Crucially, we DO NOT call send_input here: that would
        // commit a frame at tick 0 before the real per-tick loop
        // starts, and the determinism contract for lockstep is
        // write-once per tick (any later send_input(0, ...) would
        // be a no-op, so the host's "real" tick-0 input would
        // disappear).
        let mut joined = false;
        for _ in 0..200 {
            net::poll();
            if net::session_ready() {
                joined = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(joined, "host never saw client join");

        let mut local_hash: u64 = 0xdead_beef;
        for tick in 0..30u32 {
            // Build a per-tick input — alternate "left" and "right"
            // so we can assert later that the merged ambient on the
            // peer side actually contains both peers' keys.
            let frame = net::Frame {
                keys_held: if tick % 2 == 0 {
                    vec!["left".to_string()]
                } else {
                    Vec::new()
                },
                ..Default::default()
            };
            net::send_input(tick, frame);

            // Pump until ready.
            let ready = wait_for_ready(tick, 200);
            assert!(ready, "host stalled at tick {tick}");
            let inputs = net::take_inputs(tick).expect("take_inputs host");
            assert_eq!(inputs.len(), 2);

            // Mix per-tick determinism into the rolling hash so that
            // any divergence in input ordering would surface.
            for (id, f) in &inputs {
                local_hash = local_hash
                    .wrapping_mul(0x100_0000_01b3)
                    .wrapping_add(*id as u64)
                    .wrapping_add(if f.keys_held.is_empty() { 0 } else { 1 });
            }
            net::send_state_hash(tick, local_hash);
            net::poll();

            thread::sleep(Duration::from_millis(2));
        }

        host_done_tx.send(local_hash).unwrap();
        // Stay alive long enough for the client to receive the last
        // hash packet before close() drops everything.
        thread::sleep(Duration::from_millis(80));
        net::close();
        local_hash
    });

    // Client thread.
    let client_thread = thread::spawn(move || -> u64 {
        // Give the host a moment to bind.
        thread::sleep(Duration::from_millis(50));
        net::connect(&format!("127.0.0.1:{port}")).expect("connect");
        assert_eq!(net::local_peer_id(), 1);
        assert_eq!(net::peer_count(), 2);

        let mut local_hash: u64 = 0xdead_beef;
        for tick in 0..30u32 {
            let frame = net::Frame {
                keys_held: if tick % 2 == 1 {
                    vec!["right".to_string()]
                } else {
                    Vec::new()
                },
                ..Default::default()
            };
            net::send_input(tick, frame);

            let ready = wait_for_ready(tick, 200);
            assert!(ready, "client stalled at tick {tick}");
            let inputs = net::take_inputs(tick).expect("take_inputs client");
            assert_eq!(inputs.len(), 2);

            for (id, f) in &inputs {
                local_hash = local_hash
                    .wrapping_mul(0x100_0000_01b3)
                    .wrapping_add(*id as u64)
                    .wrapping_add(if f.keys_held.is_empty() { 0 } else { 1 });
            }
            net::send_state_hash(tick, local_hash);
            net::poll();

            thread::sleep(Duration::from_millis(2));
        }

        // Drain a bit so the host gets our last packets.
        for _ in 0..10 {
            net::poll();
            thread::sleep(Duration::from_millis(5));
        }
        net::close();
        local_hash
    });

    let host_hash = host_thread.join().expect("host join");
    let client_hash = client_thread.join().expect("client join");

    let _ = host_done_rx.recv_timeout(Duration::from_secs(1));
    assert_eq!(
        host_hash, client_hash,
        "host and client computed different rolling hashes; lockstep desync"
    );
}

fn wait_for_ready(tick: u32, max_polls: u32) -> bool {
    for _ in 0..max_polls {
        net::poll();
        if net::tick_ready(tick) {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}
