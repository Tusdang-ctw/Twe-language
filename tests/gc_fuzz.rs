//! web3d-M0: seeded differential GC fuzzer.
//!
//! Generates *valid*, allocation-heavy Twe programs from a small
//! grammar — nested list comprehensions, temporaries passed through
//! user functions, `for` loops over temporaries whose bodies allocate —
//! the shapes that exposed the use-after-free fixed in web3d-M0
//! (`docs/changes/2026-09-27-web3d-m0-gc-soundness.md`). Each program
//! runs twice in-process: normally, and under GC stress mode (collect
//! at every safepoint, poison swept objects). The outputs must match,
//! and neither run may error or panic.
//!
//! `twec mutate` isn't used here: it emits deliberately *broken*
//! programs from a few deterministic rules, which is the wrong input
//! for a GC fuzzer.
//!
//! Seeds: `TWE_GC_FUZZ_SEEDS` (default 200, fast enough for every
//! `cargo test`); CI's nightly job runs 1000. A failure prints the seed
//! and the program so it can be turned into a regression file.

use std::panic::{catch_unwind, AssertUnwindSafe};

use twec::{eval, lexer, parser};

/// Tiny deterministic PRNG (xorshift64*), so seeds reproduce exactly
/// without a `rand` dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const VARS: usize = 4;

/// Library of helper functions every generated program starts with.
/// All values are flat lists of ints, so every expression is valid.
const PRELUDE: &str = "\
function f0(a, b):
    var out = []
    for x in a:
        out.append(x)
    for y in b:
        out.append(y * 2)
    return out

function f1(n):
    return [i for i in 0..<n]

function sumf(xs):
    var s = 0
    for x in xs:
        s += x
    return s

var acc = 0
";

/// A list-valued expression (never empty, so `[0]` indexing is safe).
fn list_expr(r: &mut Rng, depth: u32) -> String {
    let choice = if depth == 0 { r.below(3) } else { r.below(6) };
    match choice {
        0 => format!("[{}, {}]", r.below(50), r.below(50)),
        1 => format!("[k * {} for k in 0..<{}]", r.below(5) + 1, r.below(6) + 1),
        2 => format!("v{}", r.below(VARS as u64)),
        3 => format!(
            "f0({}, {})",
            list_expr(r, depth - 1),
            list_expr(r, depth - 1)
        ),
        4 => format!("[x + {} for x in {}]", r.below(9), list_expr(r, depth - 1)),
        _ => format!("f1({})", r.below(8) + 1),
    }
}

fn int_expr(r: &mut Rng, depth: u32) -> String {
    match r.below(3) {
        0 => r.below(100).to_string(),
        1 => format!("sumf({})", list_expr(r, depth)),
        _ => format!("{}[0]", list_expr(r, depth)),
    }
}

fn statement(r: &mut Rng) -> String {
    match r.below(6) {
        0 | 1 => format!("v{} = {}\n", r.below(VARS as u64), list_expr(r, 3)),
        2 => format!(
            "for it in {}:\n    let junk = {}\n    acc += it\n",
            list_expr(r, 3),
            list_expr(r, 2)
        ),
        // Heap-valued elements dereferenced after the body allocates —
        // the shape of the original use-after-free. With flat int lists
        // a swept iterable frees nothing the loop reads again, so a
        // missing root would go unnoticed (found by mutation-testing
        // this fuzzer against the fix).
        3 => format!(
            "for it in [[x, [x, x + 1]] for x in {}]:\n    let junk = {}\n    let junk2 = [junk, [1]]\n    acc += it[0] + it[1][1]\n",
            list_expr(r, 3),
            list_expr(r, 2)
        ),
        4 => format!("acc += {}\n", int_expr(r, 3)),
        _ => format!("print(sumf(v{}))\n", r.below(VARS as u64)),
    }
}

fn program(seed: u64) -> String {
    let mut r = Rng::new(seed);
    let mut src = String::from(PRELUDE);
    for i in 0..VARS {
        src.push_str(&format!("var v{i} = [{}, {}]\n", r.below(20), i));
    }
    for _ in 0..(8 + r.below(12)) {
        src.push_str(&statement(&mut r));
    }
    for i in 0..VARS {
        src.push_str(&format!("print(sumf(v{i}))\n"));
    }
    src.push_str("print(acc)\n");
    src
}

/// Run `src` on this thread with stress mode on or off. `Err` carries
/// a runtime error or a panic message (e.g. "GC use-after-free").
fn run(src: &str, stress: bool) -> Result<String, String> {
    let tokens = lexer::lex(src).map_err(|e| format!("lex: {e}"))?;
    let program = parser::parse(&tokens).map_err(|e| format!("parse: {e}"))?;
    twec::heap::gc_set_stress(stress);
    let result = catch_unwind(AssertUnwindSafe(|| eval::run(&program)));
    twec::heap::gc_set_stress(false);
    match result {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(format!("runtime error: {e}")),
        Err(panic) => Err(format!(
            "panic: {}",
            panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default()
        )),
    }
}

#[test]
fn generated_programs_agree_under_gc_stress() {
    let seeds: u64 = std::env::var("TWE_GC_FUZZ_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    for seed in 0..seeds {
        let src = program(seed);
        let normal = run(&src, false);
        let stressed = run(&src, true);
        let ok = matches!((&normal, &stressed), (Ok(a), Ok(b)) if a == b);
        assert!(
            ok,
            "seed {seed} diverged or failed\n  normal: {normal:?}\n  stress: {stressed:?}\n\
             --- program ---\n{src}"
        );
    }
}

#[test]
fn generator_is_deterministic() {
    assert_eq!(program(42), program(42));
    assert_ne!(program(1), program(2));
}
