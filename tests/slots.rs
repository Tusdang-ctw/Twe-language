//! web3d-M3: resolved names must hit their slots.
//!
//! The resolver annotates every local with a slot in its frame
//! (`ast::Res`), and the runtime reads and writes locals by that slot.
//! When the runtime's frame disagrees with the annotation it falls back
//! to a by-name lookup: still correct, but slow, and a sign the two
//! frame models have drifted. This runs every test program and example
//! for a few frames and requires zero fallbacks.

use std::path::{Path, PathBuf};

fn twe_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            // Scaffolding demos need the experimental feature.
            if p.file_name().is_some_and(|n| n == "experimental") {
                continue;
            }
            twe_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "twe") {
            out.push(p);
        }
    }
}

#[test]
fn resolved_names_hit_their_slots() {
    let mut files = Vec::new();
    twe_files(Path::new("tests/programs"), &mut files);
    twe_files(Path::new("examples"), &mut files);
    files.sort();
    let mut ran = 0;
    let mut missed = Vec::new();
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap();
        let Ok(tokens) = twec::lexer::lex(&src) else {
            continue;
        };
        let Ok(program) = twec::parser::parse(&tokens) else {
            continue;
        };
        // Multi-file projects go through the module loader.
        if twec::module::has_imports(&program) {
            continue;
        }
        let before = twec::eval::slot_misses();
        // Some programs stop with a runtime error on purpose (or need a
        // window); the slots they reached still count.
        let _ = twec::eval::run_with_frames(&program, 10, 1.0 / 60.0);
        ran += 1;
        let misses = twec::eval::slot_misses() - before;
        if misses > 0 {
            missed.push(format!("{}: {misses}", path.display()));
        }
    }
    assert!(ran > 50, "only {ran} programs ran");
    assert!(missed.is_empty(), "slot fallbacks:\n{}", missed.join("\n"));
}

/// The zero-fallback test above would pass vacuously if nothing were
/// annotated; check that running a program resolves its names.
#[test]
fn running_a_program_annotates_its_names() {
    use twec::ast::{Expr, Res, Stmt};
    let src = "var g = 1\nfunction f(a):\n    let b = a + g\n    return b\nprint(f(2))\n";
    let program = twec::parser::parse(&twec::lexer::lex(src).unwrap()).unwrap();
    let out = twec::eval::run(&program).unwrap();
    assert_eq!(out.trim(), "3");
    let Stmt::FunctionDecl { body, .. } = &program.stmts[1] else {
        panic!("expected a function");
    };
    let Stmt::Let { value, res, .. } = &body[0] else {
        panic!("expected let");
    };
    assert!(
        matches!(res.get(), Some(Res::Local { slot: 1, .. })),
        "b: {res:?} {:?}",
        res.get()
    );
    let Expr::Binary { left, right, .. } = value else {
        panic!("expected a + g");
    };
    let (Expr::Ident { res: a, .. }, Expr::Ident { res: g, .. }) = (&**left, &**right) else {
        panic!("expected identifiers");
    };
    assert!(
        matches!(a.get(), Some(Res::Local { slot: 0, .. })),
        "a: {:?}",
        a.get()
    );
    assert_eq!(g.get(), Some(&Res::Global));
}
