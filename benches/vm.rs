//! Phase 11 session 6: criterion bench harness for the
//! tree-walker / bytecode VM.
//!
//! Run with `cargo bench`. Reports go to `target/criterion/`. The
//! harness focuses on tight loops where the Phase 8.5 NaN-tagging
//! migration left perf on the table — the closeout note (2026-05-01)
//! showed the bytecode VM ~1.1×–1.8× *slower* than the pre-tag VM
//! and ~5× off the 3× speedup-vs-tree-walker exit criterion. Keeping
//! these benches in CI lets future runs detect regressions and
//! validate session 7's dispatch tuning.
//!
//! Each benchmark runs both backends so the *relative* ratio is the
//! observable; absolute numbers shift across machines.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use twec::{compiler, eval, lexer, parser, vm};

fn parse(src: &str) -> twec::ast::Program {
    let tokens = lexer::lex(src).expect("lex");
    parser::parse(&tokens).expect("parse")
}

fn run_tree(src: &str) {
    let program = parse(src);
    eval::run(&program).expect("eval");
}

fn run_bytecode(src: &str) {
    let program = parse(src);
    let chunk = compiler::compile_program(&program).expect("compile");
    let mut machine = vm::VM::new();
    machine.run(&chunk).expect("vm");
}

/// Sum 1..N with a `for` loop. Pure integer arithmetic — the worst
/// case for NaN-tagging because every add allocates nothing yet has
/// to round-trip through the tag predicate.
const SUM_LOOP: &str = r#"
var s = 0
for i in 0..100000:
    s += i
print(s)
"#;

/// Naive recursive Fibonacci — exercises function-call overhead.
/// fib(20) is 21 891 calls, enough to amortize a single-process
/// warmup but cheap enough not to dominate the bench wallclock.
const FIB_RECURSIVE: &str = r#"
function fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

print(fib(20))
"#;

/// Tight floating-point loop — exercises the tagged-float fast path
/// (no boxing, but every op decodes a NaN tag).
const FLOAT_LOOP: &str = r#"
var x = 0.0
for i in 0..100000:
    x += 0.5
print(x)
"#;

fn bench_sum_loop(c: &mut Criterion) {
    let mut g = c.benchmark_group("sum_loop");
    g.bench_function(BenchmarkId::new("backend", "tree"), |b| {
        b.iter(|| run_tree(SUM_LOOP))
    });
    g.bench_function(BenchmarkId::new("backend", "bytecode"), |b| {
        b.iter(|| run_bytecode(SUM_LOOP))
    });
    g.finish();
}

fn bench_fib_recursive(c: &mut Criterion) {
    let mut g = c.benchmark_group("fib_recursive");
    g.bench_function(BenchmarkId::new("backend", "tree"), |b| {
        b.iter(|| run_tree(FIB_RECURSIVE))
    });
    g.bench_function(BenchmarkId::new("backend", "bytecode"), |b| {
        b.iter(|| run_bytecode(FIB_RECURSIVE))
    });
    g.finish();
}

fn bench_float_loop(c: &mut Criterion) {
    let mut g = c.benchmark_group("float_loop");
    g.bench_function(BenchmarkId::new("backend", "tree"), |b| {
        b.iter(|| run_tree(FLOAT_LOOP))
    });
    g.bench_function(BenchmarkId::new("backend", "bytecode"), |b| {
        b.iter(|| run_bytecode(FLOAT_LOOP))
    });
    g.finish();
}

criterion_group!(benches, bench_sum_loop, bench_fib_recursive, bench_float_loop);
criterion_main!(benches);
