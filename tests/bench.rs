//! Bytecode VM micro-benchmarks vs the tree-walker.
//!
//! Each `#[test]` here is `#[ignore]`d so it's opt-in via:
//! `cargo test --release -- --ignored --nocapture`
//!
//! Numbers are wall-clock from `Instant::now()` averaged over a
//! handful of iterations. They're intentionally NOT assertions —
//! perf varies by host and we don't want CI flakes. Read the
//! printed lines to compare backends and to track wins across
//! optimization sessions. The Phase-3 exit criterion in
//! `docs/05-roadmap.md` is "Phase-2 game runs at 60fps with 500+
//! entities" through the bytecode VM, which is the headline goal.
//!
//! Workloads:
//!   1. fib(25)        — recursion + arithmetic + comparisons
//!   2. sum_loop(N)    — tight `while` over Int math
//!   3. list_iter(N)   — for-in over a List + indexing
//!   4. method_call(N) — N invocations of a small method on an instance
//!   5. scene_tick(N)  — N ticks of a scene with a state every-clock
//!   6. entity_tick(N) — single tick of a scene with N spawned entities

use std::time::Instant;

use twec::{compiler, eval, lexer, parser, value::Env, vm::VM};

/// Compile + run a program through the bytecode VM. Returns
/// the wall time and the captured `print` output (so the
/// caller can sanity-check that the workload actually ran).
fn time_bytecode(src: &str, frames: u32, dt: f64) -> (std::time::Duration, String) {
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    let chunk = compiler::compile_program(&program).expect("compile");
    let mut vm = VM::new();
    let start = Instant::now();
    vm.run(&chunk).expect("run");
    for _ in 0..frames {
        vm.tick(dt).expect("tick");
    }
    let elapsed = start.elapsed();
    (elapsed, vm.take_out())
}

/// Same workload through the tree-walker.
fn time_tree(src: &str, frames: u32, dt: f64) -> (std::time::Duration, String) {
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    let mut env = Env::new();
    twec::stdlib::install(&mut env);
    let start = Instant::now();
    eval::run_top_level(&mut env, &program).expect("run_top_level");
    for _ in 0..frames {
        eval::tick_frame(&mut env, dt).expect("tick");
    }
    let elapsed = start.elapsed();
    (elapsed, std::mem::take(&mut env.out))
}

/// Print one row of the comparison table. Flagging the speedup
/// with one decimal — small enough to read quickly, big enough
/// to spot regressions.
fn report(name: &str, src: &str, frames: u32, dt: f64) {
    let (bc_time, bc_out) = time_bytecode(src, frames, dt);
    let (tw_time, tw_out) = time_tree(src, frames, dt);
    assert_eq!(
        bc_out, tw_out,
        "{name}: bytecode and tree-walker output diverged — fix correctness before measuring perf"
    );
    let bc_us = bc_time.as_secs_f64() * 1e6;
    let tw_us = tw_time.as_secs_f64() * 1e6;
    let ratio = if bc_us > 0.0 { tw_us / bc_us } else { 0.0 };
    println!("  {name:<22} bc {bc_us:>10.1} us   tree {tw_us:>10.1} us   bc/tree x{ratio:.2}");
}

#[test]
#[ignore]
fn bench_fib_25() {
    let src =
        "function fib(n):\n    if n < 2:\n        return n\n    return fib(n - 1) + fib(n - 2)\n\nprint(fib(25))\n";
    println!("\n[bench] fib(25)");
    report("fib(25)", src, 0, 0.0);
}

#[test]
#[ignore]
fn bench_sum_loop_100k() {
    let src =
        "var i = 0\nvar total = 0\nwhile i < 100000:\n    total += i\n    i += 1\nprint(total)\n";
    println!("\n[bench] sum_loop(100_000)");
    report("sum_loop(100k)", src, 0, 0.0);
}

#[test]
#[ignore]
fn bench_list_iter_10k() {
    // Build a list of 10k ints, then iterate it summing each.
    let src = "let xs = []\nvar i = 0\nwhile i < 10000:\n    xs.append(i)\n    i += 1\nvar total = 0\nfor x in xs:\n    total += x\nprint(total)\n";
    println!("\n[bench] list_iter(10_000)");
    report("list_iter(10k)", src, 0, 0.0);
}

#[test]
#[ignore]
fn bench_method_call_10k() {
    // Call a method 10k times — exercises OP_INVOKE, push_call_frame,
    // OP_RETURN frame teardown, and the receiver/self handling.
    let src = "item Counter:\n    n: 0\n    bump(amount):\n        self.n = self.n + amount\n\nlet c = Counter()\nvar i = 0\nwhile i < 10000:\n    c.bump(1)\n    i += 1\nprint(c.n)\n";
    println!("\n[bench] method_call(10_000)");
    report("method_call(10k)", src, 0, 0.0);
}

#[test]
#[ignore]
fn bench_scene_tick_1k() {
    // 1000 ticks of a scene with a state every-clock that fires
    // each tick (interval matches dt). Exercises tick_scene, the
    // every-clock loop, the bare-name self-field rewrite, and the
    // VM-side method invocation path.
    let src = "scene S:\n    var n: int = 0\n\n    initial: a\n\n    state a:\n        every 16ms:\n            n += 1\n";
    println!("\n[bench] scene_tick(1_000)");
    report("scene_tick(1k)", src, 1000, 0.016);
}

#[test]
#[ignore]
fn bench_entity_tick_500() {
    // 500 spawned entities, single tick. This is the headline
    // workload for the Phase-3 exit criterion: 60fps with 500+
    // entities means the per-tick budget per entity is
    // 16.67ms / 500 = ~33us.
    let src = "entity Mob:\n    var n = 0\n    update(dt):\n        n += 1\n\nvar i = 0\nwhile i < 500:\n    spawn Mob at (0, 0)\n    i += 1\n";
    println!("\n[bench] entity_tick(500) — single tick of 500 entities");
    report("entity_tick(500)", src, 1, 0.016);
}

#[test]
#[ignore]
fn bench_entity_tick_500_x_60() {
    // The actual 60-frame slice — what one second of game runtime
    // looks like with 500 entities. Compare against 1000 ms budget.
    let src = "entity Mob:\n    var n = 0\n    update(dt):\n        n += 1\n\nvar i = 0\nwhile i < 500:\n    spawn Mob at (0, 0)\n    i += 1\n";
    println!("\n[bench] entity_tick(500) x 60 frames — one second of runtime");
    report("entity_tick(500)x60", src, 60, 0.016);
}
