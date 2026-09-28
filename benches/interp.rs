//! Criterion benches for the tree-walking interpreter (Twe's only
//! runtime since web3d-M1 removed the bytecode VM).
//!
//! Run with `cargo bench --bench interp`. Reports go to
//! `target/criterion/`; `twec perf-snapshot` / `perf-diff` compare runs
//! against `docs/perf-snapshots/`. Benchmark ids keep the historical
//! `<group>/backend/tree` shape so snapshots stay comparable.
//!
//! On a busy machine criterion's numbers can swing 30–50% between
//! identical builds; `tests/perf_probe.rs` (min-of-N) is the steadier
//! probe and the source of the milestone closeout numbers.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use twec::{eval, lexer, parser};

fn parse(src: &str) -> twec::ast::Program {
    let tokens = lexer::lex(src).expect("lex");
    parser::parse(&tokens).expect("parse")
}

fn run_tree(src: &str) {
    let program = parse(src);
    eval::run(&program).expect("eval");
}

fn run_tree_frames(src: &str, frames: u32) {
    let program = parse(src);
    eval::run_with_frames(&program, frames, 0.016).expect("eval frames");
}

/// Sum 0..=100000 with a `for` loop — pure integer arithmetic on
/// globals; the interpreter's per-statement overhead.
const SUM_LOOP: &str = r#"
var s = 0
for i in 0..100000:
    s += i
print(s)
"#;

/// Naive recursive Fibonacci — function-call overhead. fib(20) is
/// 21 891 calls.
const FIB_RECURSIVE: &str = r#"
function fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

print(fib(20))
"#;

/// Tight floating-point loop — the tagged-float fast path.
const FLOAT_LOOP: &str = r#"
var x = 0.0
for i in 0..100000:
    x += 0.5
print(x)
"#;

/// Game-representative hot path: 2000 entities each running a small
/// arithmetic `update(dt)` for 30 frames (~60k update calls) — the
/// shape of a Vampire-Survivors-class per-frame load. The web3d-M1 and
/// M3 performance targets are stated against this.
const ENTITY_UPDATE: &str = r#"
entity Mob:
    var x = 0.0
    var y = 0.0
    var vx = 1.5
    var vy = 0.5
    update(dt):
        x += vx * dt
        y += vy * dt
        if x > 1000.0:
            x = 0.0

var i = 0
while i < 2000:
    spawn Mob at (0, 0)
    i += 1
"#;

const ENTITY_UPDATE_FRAMES: u32 = 30;

fn bench(c: &mut Criterion, group: &str, f: impl Fn() + Copy) {
    let mut g = c.benchmark_group(group);
    g.bench_function(BenchmarkId::new("backend", "tree"), |b| b.iter(f));
    g.finish();
}

fn bench_sum_loop(c: &mut Criterion) {
    bench(c, "sum_loop", || run_tree(SUM_LOOP));
}

fn bench_fib_recursive(c: &mut Criterion) {
    bench(c, "fib_recursive", || run_tree(FIB_RECURSIVE));
}

fn bench_float_loop(c: &mut Criterion) {
    bench(c, "float_loop", || run_tree(FLOAT_LOOP));
}

fn bench_entity_update(c: &mut Criterion) {
    bench(c, "entity_update", || {
        run_tree_frames(ENTITY_UPDATE, ENTITY_UPDATE_FRAMES)
    });
}

criterion_group!(
    benches,
    bench_sum_loop,
    bench_fib_recursive,
    bench_float_loop,
    bench_entity_update
);
criterion_main!(benches);
