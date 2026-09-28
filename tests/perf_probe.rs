//! web3d-M1: interpreter perf probe — min-of-N wall time.
//!
//! `cargo bench` (criterion) is the canonical harness, but on a busy
//! developer machine its numbers swung 30–50% between identical builds.
//! Background load can only *add* time, so the minimum of repeated runs
//! is a far steadier estimate. The M1 closeout numbers come from this.
//!
//! Ignored by default (timing assertions don't belong in CI). Run with:
//!
//!     cargo test --release --test perf_probe -- --ignored --nocapture
use std::time::Instant;
fn parse(src: &str) -> twec::ast::Program {
    twec::parser::parse(&twec::lexer::lex(src).unwrap()).unwrap()
}
fn min_ms(n: usize, mut f: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..n {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    best
}
#[test]
#[ignore = "timing probe; run manually with --ignored --nocapture"]
fn probe() {
    let sum = parse("var s = 0\nfor i in 0..100000:\n    s += i\nprint(s)\n");
    let ent_src = std::fs::read_to_string("benches/interp.rs").unwrap();
    let a = ent_src.find("const ENTITY_UPDATE: &str = r#\"").unwrap()
        + "const ENTITY_UPDATE: &str = r#\"".len();
    let b = a + ent_src[a..].find("\"#;").unwrap();
    let ent = parse(&ent_src[a..b]);
    let fib = parse("function fib(n):\n    if n < 2:\n        return n\n    return fib(n - 1) + fib(n - 2)\nprint(fib(20))\n");
    let s = min_ms(40, || {
        twec::eval::run(&sum).unwrap();
    });
    let f = min_ms(40, || {
        twec::eval::run(&fib).unwrap();
    });
    let e = min_ms(15, || {
        twec::eval::run_with_frames(&ent, 30, 1.0 / 60.0).unwrap();
    });
    println!("PERF sum_loop {:.2}ms = {:.0} ns/iter | fib20 {:.2}ms = {:.0} ns/call | entity_update {:.2}ms = {:.0} ns/update",
        s, s * 1e6 / 100001.0, f, f * 1e6 / 21891.0, e, e * 1e6 / 60000.0);
}

/// web3d-M3: the exit benchmark (`examples/swarm_3d.twe`, 5,000 seeking
/// enemies) split into its two script-side costs per frame: the
/// `update` tick and the `render()` calls that fill the 3D draw queue.
/// The renderer itself is not included.
#[test]
#[ignore = "timing probe; run manually with --ignored --nocapture"]
fn swarm_3d() {
    let src = std::fs::read_to_string("examples/swarm_3d.twe").unwrap();
    let program = parse(&src);
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).unwrap();
    let frames = 120;
    let (mut tick, mut render) = (f64::MAX, f64::MAX);
    for _ in 0..frames {
        let t = Instant::now();
        twec::eval::tick_frame(&mut env, 1.0 / 60.0).unwrap();
        tick = tick.min(t.elapsed().as_secs_f64() * 1e3);
        let t = Instant::now();
        twec::eval::render_frame3d(&mut env).unwrap();
        render = render.min(t.elapsed().as_secs_f64() * 1e3);
        assert!(env.render_queue3d.len() > 5000);
        env.render_queue3d.clear();
    }
    println!(
        "PERF swarm_3d (5000 enemies, min of {frames} frames): tick {tick:.2} ms = {:.0} ns/update | render {render:.2} ms = {:.0} ns/entity | script total {:.2} ms/frame",
        tick * 1e6 / 5000.0,
        render * 1e6 / 5000.0,
        tick + render
    );
}
