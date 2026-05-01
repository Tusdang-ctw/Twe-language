//! v0.2 Phase 8.5 session 8g — stop-the-world tracing GC heap allocator.
//!
//! Replaces the `Rc<HeapObject>` ownership story established in 8b–8e
//! with a custom heap-managed allocator. Every `HeapObject` is now
//! owned by the thread-local `Heap` through an intrusive linked list
//! (`HeapObject::next`); `TaggedValue`'s pointer-tagged variants
//! carry a raw `*mut HeapObject` rather than a refcounted `Rc`.
//!
//! ## Why drop refcounting
//!
//! Per `docs/08-nan-tagging.md` § "Why a real GC at all?":
//! 1. Refcount overhead — every `clone()` was an atomic increment.
//!    The bytecode VM did this constantly; profiling Wren and Lua
//!    showed refcounting was a significant chunk of dispatch cost.
//! 2. Cycles are theoretically possible (`instance.field = instance`
//!    builds one); refcounting leaks them.
//! 3. Pause budgets — Roblox's Luau ships with an incremental tracing
//!    GC explicitly because per-frame Rc churn was unacceptable for
//!    game pacing.
//!
//! ## Algorithm
//!
//! Per *Crafting Interpreters* §26 — stop-the-world mark + sweep.
//! Incremental tri-color is a v0.3 optimization (see `docs/08-nan-tagging.md`
//! § "What's deferred from this design").
//!
//! 1. **Mark phase.** Starting from the GC roots (each a `&TaggedValue`),
//!    set `HeapObject::mark = true` on the target object. Then recursively
//!    scan the body for nested `TaggedValue`s and mark them.
//! 2. **Sweep phase.** Walk the `all_objects` linked list. For each
//!    object: if `mark` is true, reset to false (white for next cycle)
//!    and keep. If `mark` is false, unlink and `Box::from_raw` →
//!    `drop`, freeing the heap allocation.
//!
//! ## Triggering
//!
//! Session 8g ships the allocator + `collect()` machinery. Automatic
//! collect at safepoints — between bytecode instructions in the VM,
//! between statements in the tree-walker — lands in 8h alongside the
//! roots wiring. For now, `gc_collect(roots)` is invoked manually
//! (e.g. by tests).
//!
//! ## Why `unsafe`
//!
//! Same justification as `tagged_value.rs`: encoding raw pointers
//! into the 48-bit NaN-tag payload requires `Box::into_raw` /
//! `Box::from_raw` and pointer-to-int casts. Confined to this file
//! and `tagged_value.rs`.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};

use crate::tagged_value::{HeapBody, HeapBodyKind, HeapObject, TaggedValue};

/// Initial GC trigger threshold in bytes-allocated. Crossing this
/// runs `collect()` at the next safepoint. The threshold doubles
/// after each collection that doesn't free much, per *Crafting
/// Interpreters* §26.4. v0.3 tuning lands in 8i.
const INITIAL_GC_THRESHOLD: usize = 1024 * 1024;

/// Heap manager. Owns every `HeapObject` allocated via
/// [`gc_alloc`]. Threaded onto an intrusive linked list rooted at
/// `all_objects` so [`collect`] can walk every allocation during
/// sweep.
pub struct Heap {
    /// Head of the linked list of all heap-allocated objects.
    /// `null` when the heap is empty.
    pub(crate) all_objects: *mut HeapObject,
    /// Bytes allocated since last GC. Crossing `threshold` queues
    /// a collection at the next safepoint.
    pub bytes_allocated: usize,
    /// GC trigger threshold. Grown adaptively in `collect()`.
    pub threshold: usize,
}

impl Heap {
    pub fn new() -> Self {
        Self {
            all_objects: std::ptr::null_mut(),
            bytes_allocated: 0,
            threshold: INITIAL_GC_THRESHOLD,
        }
    }

    /// Allocate a heap object holding `body`. Returns a raw
    /// pointer; the heap owns it through the linked list.
    /// Repeatedly calling this without ever calling
    /// [`collect`] leaks until the thread-local Heap drops.
    pub fn alloc(&mut self, body: HeapBody) -> *mut HeapObject {
        let body_kind = HeapBodyKind::of(&body);
        let obj = Box::new(HeapObject {
            mark: Cell::new(false),
            body_kind,
            next: Cell::new(self.all_objects),
            body: RefCell::new(body),
        });
        let ptr = Box::into_raw(obj);
        self.all_objects = ptr;
        self.bytes_allocated += std::mem::size_of::<HeapObject>();
        ptr
    }

    /// Stop-the-world mark + sweep with a flat root slice. Wraps
    /// the closure-based [`gc_collect_with`] entry point for
    /// tests / callers that already have a Vec of root refs.
    pub fn collect(&mut self, roots: &[&TaggedValue]) {
        for r in roots {
            mark_value(r);
        }
        self.sweep();
    }

    /// Sweep half of mark+sweep. Walk the `all_objects` linked list:
    /// keep marked objects (and reset their mark for the next cycle),
    /// unlink + free unmarked objects. Adapts the GC threshold based
    /// on post-sweep live size, per *Crafting Interpreters* §26.4.
    pub fn sweep(&mut self) {
        let mut prev: *mut HeapObject = std::ptr::null_mut();
        let mut cur = self.all_objects;
        unsafe {
            while !cur.is_null() {
                let next = (*cur).next.get();
                if (*cur).mark.replace(false) {
                    // Marked black → keep, reset to white.
                    prev = cur;
                } else {
                    // White → unlink and free.
                    if prev.is_null() {
                        self.all_objects = next;
                    } else {
                        (*prev).next.set(next);
                    }
                    let _ = Box::from_raw(cur);
                }
                cur = next;
            }
        }

        // Adaptive threshold: if we freed a lot, lower the bar; if
        // not, raise it. Simple heuristic mirroring CI §26.4.
        self.bytes_allocated = self.live_byte_count();
        self.threshold = (self.bytes_allocated * 2).max(INITIAL_GC_THRESHOLD);
    }

    /// Walk `all_objects` and sum the bytes. Used by `collect()`
    /// to update `bytes_allocated` post-sweep.
    fn live_byte_count(&self) -> usize {
        let mut count = 0;
        let mut cur = self.all_objects;
        unsafe {
            while !cur.is_null() {
                count += std::mem::size_of::<HeapObject>();
                cur = (*cur).next.get();
            }
        }
        count
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        // Free every remaining allocation. Runs at thread exit
        // (the thread-local HEAP drops then) and ensures we don't
        // leak across test boundaries.
        let mut cur = self.all_objects;
        unsafe {
            while !cur.is_null() {
                let next = (*cur).next.get();
                let _ = Box::from_raw(cur);
                cur = next;
            }
        }
        self.all_objects = std::ptr::null_mut();
    }
}

thread_local! {
    /// One Heap per thread. Cargo test spreads tests across worker
    /// threads, so this gives each thread its own arena. Each
    /// thread's Heap drops on thread exit (Drop impl frees every
    /// remaining allocation), bounding the leak even when no
    /// collect runs during the program.
    pub(crate) static HEAP: RefCell<Heap> = RefCell::new(Heap::new());
}

/// Allocate a new `HeapObject` on the thread-local heap and return
/// a raw pointer to it. The heap owns the allocation; callers store
/// the raw pointer (typically inside a NaN-tagged `TaggedValue`).
pub fn gc_alloc(body: HeapBody) -> *mut HeapObject {
    HEAP.with(|h| h.borrow_mut().alloc(body))
}

/// Trigger a stop-the-world mark + sweep on the thread-local heap.
/// `roots` is the live GC root set; objects not transitively
/// reachable from a root are freed.
pub fn gc_collect(roots: &[&TaggedValue]) {
    HEAP.with(|h| h.borrow_mut().collect(roots));
}

/// Closure-based GC entry point. The `scan` callback walks the live
/// root set, calling [`mark_value`] on each `&TaggedValue` it visits.
/// After scan returns, the heap sweeps unmarked objects.
///
/// This is the primary API for VM / eval safepoints (8h): the closure
/// captures `&self` and visits the VM stack, globals, scenes, etc.
/// without forcing the caller to flatten roots into a Vec.
///
/// Note: the scan callback runs **before** `HEAP` is borrowed for
/// sweep, so it's safe to allocate (transitively trigger
/// [`gc_alloc`]) inside scan — though scan should be allocation-free
/// in practice.
pub fn gc_collect_with(scan: impl FnOnce()) {
    scan();
    HEAP.with(|h| h.borrow_mut().sweep());
}

/// Returns true if the heap's `bytes_allocated` has crossed
/// `threshold`, meaning the next safepoint should trigger collection.
/// Cheap to call (single thread-local borrow + two field reads).
pub fn gc_should_collect() -> bool {
    HEAP.with(|h| {
        let h = h.borrow();
        h.bytes_allocated >= h.threshold
    })
}

/// Test/bench helper: override the GC threshold so safepoints fire
/// sooner. Production code should not use this.
pub fn gc_set_threshold(threshold: usize) {
    HEAP.with(|h| h.borrow_mut().threshold = threshold);
}

/// Number of objects currently alive on the thread-local heap.
/// Test-only helper for asserting collect freed what was expected.
#[cfg(test)]
pub fn gc_live_count() -> usize {
    HEAP.with(|h| {
        let heap = h.borrow();
        let mut count = 0;
        let mut cur = heap.all_objects;
        unsafe {
            while !cur.is_null() {
                count += 1;
                cur = (*cur).next.get();
            }
        }
        count
    })
}

// ---------- mark phase ----------
//
// Walks a value's transitive references and sets `mark = true` on
// every reachable HeapObject. Used by `collect()` and exposed via
// `mark_value` for tests / 8h's roots wiring.

/// Mark `v` and everything reachable from it. Idempotent — already-marked
/// objects short-circuit (cycle-safe).
pub fn mark_value(v: &TaggedValue) {
    if !v.is_heap_pointer() {
        return;
    }
    let ptr = v.heap_ptr();
    if ptr.is_null() {
        return;
    }
    unsafe {
        if (*ptr).mark.get() {
            return; // already marked — break cycles
        }
        (*ptr).mark.set(true);
        // Recursively mark nested TaggedValues in the body.
        mark_body(&(*ptr).body.borrow());
    }
}

fn mark_body(body: &HeapBody) {
    match body {
        HeapBody::Tuple(rc) => {
            for v in rc.iter() {
                mark_value(v);
            }
        }
        HeapBody::List(rc) => {
            for v in rc.borrow().iter() {
                mark_value(v);
            }
        }
        HeapBody::Object(rc) => {
            for v in rc.borrow().fields.values() {
                mark_value(v);
            }
        }
        HeapBody::Class(c) => {
            for v in c.field_defaults.values() {
                mark_value(v);
            }
        }
        HeapBody::Instance(rc) => {
            let inst = rc.borrow();
            for v in inst.fields.values() {
                mark_value(v);
            }
            // saved_returning / saved_params on suspended fiber frames
            // also carry TaggedValues — mark them so a fiber's resume
            // values don't get swept while the fiber is paused.
            for frame in &inst.fiber_frames {
                if let crate::value::FrameKind::Function {
                    saved_returning,
                    saved_params,
                    ..
                } = &frame.kind
                {
                    if let Some(v) = saved_returning {
                        mark_value(v);
                    }
                    for (_name, slot) in saved_params {
                        if let Some(v) = slot {
                            mark_value(v);
                        }
                    }
                }
            }
        }
        HeapBody::BcClass(c) => {
            for v in c.field_defaults.values() {
                mark_value(v);
            }
            // 8h: methods + states own `Rc<BcFunction>`s whose chunks
            // hold the only path to nested string / function / class
            // constants. Without walking them every constant gets swept.
            for f in c.methods.values() {
                mark_bc_function_constants(f);
            }
            for s in c.states.values() {
                mark_bc_state(s);
            }
        }
        HeapBody::BcInstance(rc) => {
            let inst = rc.borrow();
            for v in inst.fields.values() {
                mark_value(v);
            }
            for v in &inst.fiber_stack {
                mark_value(v);
            }
            // 8h: suspended-fiber frames hold their resumption function
            // — that function's constants must survive across the
            // suspension boundary.
            for frame in &inst.fiber_frames {
                mark_bc_function_constants(&frame.function);
            }
        }
        // 8h: bytecode function chunks carry a constants pool whose
        // string / function / class entries are TaggedValues. The
        // constants are reachable only through this BcFunction's
        // chunk, so mark them now.
        HeapBody::BcFunction(rc) => {
            mark_bc_function_constants(rc);
        }
        // Tree-walker FunctionDef bodies are AST statements (literals
        // stored as `ast::Lit`, not `TaggedValue`); nothing to scan.
        // Builtin captures are pure function pointers + names.
        HeapBody::Function(_) | HeapBody::Builtin { .. } => {}
        // Leaf bodies — no nested TaggedValues to scan.
        HeapBody::String(_)
        | HeapBody::BoxedInt(_)
        | HeapBody::Percent(_)
        | HeapBody::Quantity { .. }
        | HeapBody::Range { .. } => {}
    }
}

/// Walk a `BcFunction`'s chunk constants and mark each `TaggedValue`.
/// Used by `mark_body` (when reaching a BcFunction through a
/// TaggedValue) and by `VM::scan_roots` (when walking active
/// CallFrames whose function is held as a naked `Rc<BcFunction>`).
/// v0.2 Phase 8.5 session 8h.
pub fn mark_bc_function_constants(f: &crate::bytecode::BcFunction) {
    for v in &f.chunk.constants {
        mark_value(v);
    }
}

/// Walk every `Rc<BcFunction>` reachable through a `BcStateDef` and
/// mark its chunk constants. Called from `mark_body` for BcClass.
fn mark_bc_state(s: &crate::bytecode::BcStateDef) {
    mark_bc_function_constants(&s.on_entry);
    for (_, f) in &s.every_clocks {
        mark_bc_function_constants(f);
    }
    if let Some(f) = &s.on_update {
        mark_bc_function_constants(f);
    }
    if let Some(f) = &s.on_render {
        mark_bc_function_constants(f);
    }
    for f in s.on_key_press.values() {
        mark_bc_function_constants(f);
    }
    for (pred, body) in &s.on_predicates {
        mark_bc_function_constants(pred);
        mark_bc_function_constants(body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: scrape the heap's live count between operations to
    /// observe collect's effect.
    fn live() -> usize {
        gc_live_count()
    }

    #[test]
    fn alloc_threads_through_linked_list() {
        let baseline = live();
        let _a = TaggedValue::from_borrowed_str("a");
        let _b = TaggedValue::from_borrowed_str("b");
        let _c = TaggedValue::from_borrowed_str("c");
        assert_eq!(live(), baseline + 3);
    }

    #[test]
    fn collect_with_no_roots_frees_everything_allocated_this_test() {
        let baseline = live();
        let _temp = TaggedValue::from_borrowed_str("ephemeral");
        assert!(live() > baseline);
        gc_collect(&[]);
        // Everything allocated by this test is unreachable now (no roots).
        // Pre-existing allocations from other tests in the same
        // thread also drop — that's expected for a stop-the-world GC
        // with no roots passed in. We only assert "no leak from this
        // test's own allocations".
        let after = live();
        assert!(after <= baseline, "after={after} baseline={baseline}");
    }

    #[test]
    fn collect_keeps_reachable_string() {
        gc_collect(&[]); // start clean
        let alive = TaggedValue::from_borrowed_str("survivor");
        let dead = TaggedValue::from_borrowed_str("doomed");
        // Pretend `alive` is a root; `dead` is not.
        let _ = dead;
        gc_collect(&[&alive]);
        // `alive` should still read back its content (not freed).
        assert!(alive.is_str());
        assert_eq!(alive.as_string(), "survivor");
    }

    #[test]
    fn collect_through_tuple_marks_children() {
        use std::rc::Rc;
        gc_collect(&[]);
        let inner = TaggedValue::from_borrowed_str("nested");
        let tup = TaggedValue::from_tuple(Rc::new(vec![inner]));
        // Only `tup` is rooted. Marking it should reach `inner` through
        // the tuple's Rc<Vec<TaggedValue>>, keeping the string alive.
        gc_collect(&[&tup]);
        // Reading the tuple's element back works — child wasn't swept.
        let elems = tup.as_tuple();
        assert!(elems[0].is_str());
        assert_eq!(elems[0].as_string(), "nested");
    }

    #[test]
    fn cycle_detection_does_not_loop_forever() {
        // Build an Object whose own field stores a TaggedValue
        // pointing to the same Object. Without the `if mark { return; }`
        // guard in `mark_value`, this would infinite-recurse.
        use std::cell::RefCell;
        use std::rc::Rc;
        gc_collect(&[]);
        let obj = TaggedValue::from_object(Rc::new(RefCell::new(crate::value::Object {
            fields: std::collections::HashMap::new(),
            kind: "test",
        })));
        // Insert a self-reference.
        let rc = obj.as_object();
        rc.borrow_mut().insert_field("self_ref", obj);
        // Mark with `obj` as a root — must terminate.
        gc_collect(&[&obj]);
        // Object survived; cycle didn't crash.
        assert!(obj.is_object());
    }
}
