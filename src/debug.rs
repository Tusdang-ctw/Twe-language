//! web3d-M10 session 1: a breakpoint / step debugger for the tree-
//! walker. A thread-local control block holds the active breakpoints,
//! the step mode, and a pause callback. `eval_stmt` calls
//! [`on_statement`] at every statement; when a breakpoint matches (or a
//! step is pending) it builds a [`Paused`] snapshot of the current
//! scope and calls the callback, which returns a [`Resume`] action.
//!
//! The callback model lets the same engine serve both a headless test
//! (the callback asserts and returns an action) and the Studio
//! debugger (the callback writes a "paused" event and blocks reading the
//! next command — the game loop is genuinely paused mid-tick, which is
//! what a debugger does).
//!
//! Scope: statements dispatched through `eval_stmt` — function / method
//! bodies, `on update` / `on render` handlers, loops. State on-entry
//! bodies that `wait` run through the resumable-fiber path and are not
//! yet instrumented (a documented follow-on).

use crate::value::Env;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

thread_local! {
    /// Fast path: `on_statement` runs for every statement, so when no
    /// debugger is attached on this thread it must cost ~nothing — a
    /// thread-local `Cell<bool>` read, checked before the `RefCell`
    /// borrow. Thread-local (not a global atomic) so a debug session on
    /// one thread never gates another's (debugging is per-thread; the
    /// play loop is single-threaded).
    static DEBUG_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// Why execution paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    Breakpoint,
    Step,
}

/// What to do after a pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Run until the next breakpoint.
    Continue,
    /// Pause at the next statement, descending into calls.
    StepInto,
    /// Pause at the next statement at this call depth or shallower.
    StepOver,
    /// Stop debugging; run freely to the end.
    Stop,
}

/// A snapshot handed to the pause callback.
#[derive(Debug, Clone)]
pub struct Paused {
    pub line: u32,
    pub reason: PauseReason,
    pub call_depth: u32,
    pub globals: Vec<(String, String)>,
    pub locals: Vec<(String, String)>,
    pub entity_count: usize,
}

enum StepMode {
    Run,
    /// Pause at the next statement whose call depth is `<= limit`
    /// (`u32::MAX` for step-into: any depth).
    Step { limit: u32 },
}

struct Ctl {
    breakpoints: HashSet<u32>,
    mode: StepMode,
    stopped: bool,
    on_pause: Box<dyn FnMut(&Paused) -> Resume>,
}

thread_local! {
    static DEBUG: RefCell<Option<Ctl>> = const { RefCell::new(None) };
}

/// Install a debug session: the set of breakpoint lines and a callback
/// invoked (on this thread) whenever execution pauses. Replaces any
/// existing session on this thread.
pub fn install(breakpoints: HashSet<u32>, on_pause: Box<dyn FnMut(&Paused) -> Resume>) {
    DEBUG.with(|cell| {
        *cell.borrow_mut() = Some(Ctl {
            breakpoints,
            mode: StepMode::Run,
            stopped: false,
            on_pause,
        });
    });
    DEBUG_ACTIVE.with(|a| a.set(true));
}

/// Remove the debug session on this thread.
pub fn clear() {
    DEBUG.with(|cell| *cell.borrow_mut() = None);
    DEBUG_ACTIVE.with(|a| a.set(false));
}

/// True when a debug session is installed on this thread.
pub fn is_active() -> bool {
    DEBUG.with(|cell| cell.borrow().is_some())
}

/// Replace the breakpoint set of the active session (no-op if none).
pub fn set_breakpoints(breakpoints: HashSet<u32>) {
    DEBUG.with(|cell| {
        if let Some(ctl) = cell.borrow_mut().as_mut() {
            ctl.breakpoints = breakpoints;
        }
    });
}

/// The statement hook. Called from `eval_stmt` with the statement's
/// source line and the live `Env`. Cheap (an immediate return) when no
/// session is installed. When a breakpoint matches or a step is pending,
/// it snapshots the scope, calls the pause callback, and applies the
/// returned action.
pub fn on_statement(line: u32, env: &Env) {
    // Fast path: a single relaxed atomic load when no debugger is on.
    if !DEBUG_ACTIVE.with(|a| a.get()) {
        return;
    }
    DEBUG.with(|cell| {
        let mut opt = cell.borrow_mut();
        let Some(ctl) = opt.as_mut() else {
            return;
        };
        if ctl.stopped {
            return;
        }
        let depth = env.call_depth;
        let hit_bp = ctl.breakpoints.contains(&line);
        let hit_step = matches!(ctl.mode, StepMode::Step { limit } if depth <= limit);
        if !hit_bp && !hit_step {
            return;
        }
        let paused = Paused {
            line,
            reason: if hit_bp {
                PauseReason::Breakpoint
            } else {
                PauseReason::Step
            },
            call_depth: depth,
            globals: env.debug_globals(),
            locals: env.debug_locals(),
            entity_count: env.debug_entity_count(),
        };
        let action = (ctl.on_pause)(&paused);
        ctl.mode = match action {
            Resume::Continue => StepMode::Run,
            Resume::StepInto => StepMode::Step { limit: u32::MAX },
            Resume::StepOver => StepMode::Step { limit: depth },
            Resume::Stop => {
                ctl.stopped = true;
                StepMode::Run
            }
        };
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn run_src(src: &str) {
        let tokens = crate::lexer::lex(src).expect("lex");
        let program = crate::parser::parse(&tokens).expect("parse");
        let _ = crate::eval::run(&program);
    }

    #[test]
    fn breakpoint_pauses_at_line_with_scope() {
        // Globals are visible at the paused statement.
        let hits = Rc::new(AtomicU32::new(0));
        let seen_line = Rc::new(AtomicU32::new(0));
        let h = hits.clone();
        let sl = seen_line.clone();
        let captured_globals = Rc::new(RefCell::new(Vec::new()));
        let cg = captured_globals.clone();
        install(
            HashSet::from([3]),
            Box::new(move |p| {
                h.fetch_add(1, Ordering::SeqCst);
                sl.store(p.line, Ordering::SeqCst);
                *cg.borrow_mut() = p.globals.clone();
                Resume::Continue
            }),
        );
        // Line 1: let a. Line 2: let b. Line 3 (breakpoint): let c.
        run_src("let a = 1\nlet b = 2\nlet c = a + b\n");
        clear();
        assert_eq!(hits.load(Ordering::SeqCst), 1, "breakpoint hit once");
        assert_eq!(seen_line.load(Ordering::SeqCst), 3);
        // At line 3, a and b are already bound.
        let g = captured_globals.borrow();
        assert!(g.iter().any(|(n, v)| n == "a" && v == "1"));
        assert!(g.iter().any(|(n, v)| n == "b" && v == "2"));
    }

    #[test]
    fn step_into_pauses_every_statement() {
        let count = Rc::new(AtomicU32::new(0));
        let c = count.clone();
        // Break at line 1, then step through the rest.
        install(
            HashSet::from([1]),
            Box::new(move |_p| {
                let n = c.fetch_add(1, Ordering::SeqCst);
                // Breakpoint hit (n=0) then step through two more statements.
                if n < 2 {
                    Resume::StepInto
                } else {
                    Resume::Stop
                }
            }),
        );
        run_src("let a = 1\nlet b = 2\nlet d = 3\n");
        clear();
        // One breakpoint hit + at least one step.
        assert!(count.load(Ordering::SeqCst) >= 2, "stepped past the breakpoint");
    }

    #[test]
    fn no_session_is_a_no_op() {
        assert!(!is_active());
        // Should not pause or panic.
        run_src("let a = 1\n");
    }
}
