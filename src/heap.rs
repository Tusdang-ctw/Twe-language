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

/// Phase 29 session 2: default per-frame sweep budget. The play-loop
/// safepoint (eval/vm) calls `gc_collect_with` with this much wall
/// time; if a sweep can't complete in the budget, the cursor is
/// preserved across frames and the next safepoint resumes. Default
/// 2ms of a 16.7ms frame ≈ 12% — leaves headroom for game logic.
/// Scripts override via `gc.budget_ms(f)`.
const DEFAULT_GC_BUDGET_NS: u64 = 2_000_000;

/// Phase 29 session 2: incremental-sweep state machine.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SweepPhase {
    /// No sweep in flight. The next `gc_collect_with` will mark roots
    /// and start a fresh sweep.
    Idle,
    /// Mark phase done; sweep cursor is mid-list. The next
    /// `gc_collect_with` skips marking and resumes sweeping from
    /// `sweep_cur`. Allocations during this phase set
    /// `mark = true` on the new object so it survives the in-flight
    /// round (it didn't exist when roots were scanned).
    Sweeping,
}

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
    /// Phase 29 session 2: where the in-flight sweep is up to.
    sweep_phase: SweepPhase,
    /// Sweep cursor — predecessor in the intrusive list, used to
    /// rewire links when freeing the current object.
    sweep_prev: *mut HeapObject,
    /// Sweep cursor — current object under inspection.
    sweep_cur: *mut HeapObject,
    /// Per-call sweep budget in nanoseconds. Lowering this makes
    /// individual safepoints cheaper at the cost of more frames
    /// before sweep finishes. Set to `u64::MAX` to disable budget
    /// (test path uses this for full synchronous collect).
    sweep_budget_ns: u64,
    /// Wall-clock total of the most recently completed sweep cycle,
    /// in nanoseconds. Aggregates across however many incremental
    /// steps the cycle took. Read via `gc.last_collect_ms()` from
    /// scripts; useful for tuning the budget.
    last_collect_ns: u64,
    /// Wall-clock accumulator for the in-flight cycle. Reset to 0
    /// when a new mark+sweep cycle starts; published into
    /// `last_collect_ns` when the cycle completes.
    in_flight_collect_ns: u64,
    /// web3d-M0: objects allocated while an incremental sweep is in
    /// flight. They are kept OFF `all_objects` (unmarked) until the
    /// sweep completes, then spliced onto its head. Two bugs this
    /// replaces: (1) the old scheme pre-marked mid-sweep allocations
    /// and prepended them to `all_objects`, where the cursor never
    /// visited them — their stale `mark = true` survived into the next
    /// cycle, so `mark_value` short-circuited and never traced their
    /// children; (2) freeing the original head while `sweep_prev` was
    /// null overwrote `all_objects`, unlinking every prepended object.
    young: *mut HeapObject,
    /// web3d-M0: GC stress mode. When set, every safepoint collects
    /// with an unlimited sweep budget, and swept objects are poisoned
    /// (`HeapObject::freed`) and parked on `graveyard` instead of being
    /// freed, so any use-after-free panics deterministically. Sticky:
    /// unlike `gc_set_threshold(0)`, it is not reset after a sweep.
    stress: bool,
    /// Poisoned objects kept alive under stress mode; freed on Drop.
    graveyard: *mut HeapObject,
}

impl Heap {
    pub fn new() -> Self {
        Self {
            all_objects: std::ptr::null_mut(),
            bytes_allocated: 0,
            threshold: INITIAL_GC_THRESHOLD,
            sweep_phase: SweepPhase::Idle,
            sweep_prev: std::ptr::null_mut(),
            sweep_cur: std::ptr::null_mut(),
            sweep_budget_ns: DEFAULT_GC_BUDGET_NS,
            last_collect_ns: 0,
            in_flight_collect_ns: 0,
            young: std::ptr::null_mut(),
            stress: stress_from_env(),
            graveyard: std::ptr::null_mut(),
        }
        .with_pending_refreshed()
    }

    fn with_pending_refreshed(self) -> Self {
        self.refresh_pending();
        self
    }

    /// web3d-M1: recompute `COLLECT_PENDING` from the heap's state.
    /// Called wherever an input to [`gc_should_collect`] changes.
    fn refresh_pending(&self) {
        let pending = self.stress
            || self.bytes_allocated >= self.threshold
            || self.sweep_phase == SweepPhase::Sweeping;
        COLLECT_PENDING.with(|c| c.set(pending));
    }

    /// Allocate a heap object holding `body`. Returns a raw
    /// pointer; the heap owns it through the linked list.
    /// Repeatedly calling this without ever calling
    /// [`collect`] leaks until the thread-local Heap drops.
    ///
    /// web3d-M0 invariant: while an incremental sweep is in progress
    /// (`sweep_phase == Sweeping`), new objects go on the separate
    /// `young` list, unmarked. The sweep cursor only walks
    /// `all_objects`, so they can't be freed this cycle; they join
    /// `all_objects` (still white) when the sweep completes and are
    /// traced normally from the next cycle's roots.
    pub fn alloc(&mut self, body: HeapBody) -> *mut HeapObject {
        let body_kind = HeapBodyKind::of(&body);
        let sweeping = self.sweep_phase == SweepPhase::Sweeping;
        let head = if sweeping {
            self.young
        } else {
            self.all_objects
        };
        let obj = Box::new(HeapObject {
            mark: Cell::new(false),
            freed: Cell::new(false),
            body_kind,
            next: Cell::new(head),
            body: RefCell::new(body),
        });
        let ptr = Box::into_raw(obj);
        if sweeping {
            self.young = ptr;
        } else {
            self.all_objects = ptr;
        }
        self.bytes_allocated += std::mem::size_of::<HeapObject>();
        if self.bytes_allocated >= self.threshold {
            COLLECT_PENDING.with(|c| c.set(true));
        }
        ptr
    }

    /// Move every object on the `young` list onto the head of
    /// `all_objects`. Called when a sweep cycle completes.
    fn splice_young(&mut self) {
        if self.young.is_null() {
            return;
        }
        unsafe {
            let mut tail = self.young;
            while !(*tail).next.get().is_null() {
                tail = (*tail).next.get();
            }
            (*tail).next.set(self.all_objects);
        }
        self.all_objects = self.young;
        self.young = std::ptr::null_mut();
    }

    /// Stop-the-world mark + sweep with a flat root slice. Wraps
    /// the closure-based [`gc_collect_with`] entry point for
    /// tests / callers that already have a Vec of root refs.
    /// Always runs to completion (synchronous) — useful for tests
    /// that expect a deterministic post-collect heap state.
    pub fn collect(&mut self, roots: &[&TaggedValue]) {
        for r in roots {
            mark_value(r);
        }
        self.start_or_continue_sweep_to_completion();
    }

    /// Phase 29 session 2: begin (or, if already in flight, continue)
    /// an incremental sweep. Walks at most `budget_ns` worth of
    /// objects from the cursor and yields. Returns `true` if the
    /// sweep cycle completed this call.
    pub fn sweep_step(&mut self, budget_ns: u64) -> bool {
        if self.sweep_phase == SweepPhase::Idle {
            self.sweep_prev = std::ptr::null_mut();
            self.sweep_cur = self.all_objects;
            self.sweep_phase = SweepPhase::Sweeping;
            COLLECT_PENDING.with(|c| c.set(true));
        }

        // web3d-M2: `crate::clock`, not `Instant` (which panics on
        // wasm32). With no clock available the sweep simply runs to
        // completion.
        let start = crate::clock::now_secs();
        let elapsed_ns = || match start {
            Some(t0) => ((crate::clock::now_secs().unwrap_or(t0) - t0) * 1e9) as u64,
            None => 0,
        };
        // Objects swept since the last budget check.
        let mut since_check = 0u32;
        unsafe {
            while !self.sweep_cur.is_null() {
                let cur = self.sweep_cur;
                let next = (*cur).next.get();
                if (*cur).mark.replace(false) {
                    // Marked black → keep, reset to white.
                    self.sweep_prev = cur;
                } else {
                    // White → unlink and free.
                    if self.sweep_prev.is_null() {
                        self.all_objects = next;
                    } else {
                        (*self.sweep_prev).next.set(next);
                    }
                    if self.stress {
                        // Poison + park instead of freeing, so a later
                        // deref panics instead of reading reused memory.
                        (*cur).freed.set(true);
                        (*cur).next.set(self.graveyard);
                        self.graveyard = cur;
                    } else {
                        let _ = Box::from_raw(cur);
                    }
                }
                self.sweep_cur = next;

                // Budget check every 256 objects. Per-object was fine
                // natively (rdtsc), but on wasm the clock is a call out
                // to JS: web3d-M3 profiling found the sweep's clock
                // reads at ~18% of a 5,000-entity frame in Chrome.
                // Freeing 256 objects takes microseconds, far under
                // any budget.
                since_check += 1;
                if since_check >= 256 {
                    since_check = 0;
                    if start.is_some() && elapsed_ns() >= budget_ns {
                        self.in_flight_collect_ns += elapsed_ns();
                        return false;
                    }
                }
            }
        }
        // Sweep completed.
        self.in_flight_collect_ns += elapsed_ns();
        self.last_collect_ns = self.in_flight_collect_ns;
        self.in_flight_collect_ns = 0;
        self.sweep_phase = SweepPhase::Idle;
        self.sweep_prev = std::ptr::null_mut();
        self.sweep_cur = std::ptr::null_mut();
        self.splice_young();

        // Adaptive threshold: if we freed a lot, lower the bar; if
        // not, raise it. Simple heuristic mirroring CI §26.4.
        self.bytes_allocated = self.live_byte_count();
        self.threshold = (self.bytes_allocated * 2).max(INITIAL_GC_THRESHOLD);
        self.refresh_pending();
        true
    }

    /// Drain the in-flight sweep to completion, ignoring any
    /// configured budget. Used by the synchronous `collect()` path
    /// (tests + tooling) which expect a fully-collected heap on
    /// return.
    fn start_or_continue_sweep_to_completion(&mut self) {
        // u64::MAX budget → loop never yields early.
        let _ = self.sweep_step(u64::MAX);
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
        // leak across test boundaries. `young` and `graveyard` are
        // separate lists (web3d-M0) and must be freed too.
        for head in [self.all_objects, self.young, self.graveyard] {
            let mut cur = head;
            unsafe {
                while !cur.is_null() {
                    let next = (*cur).next.get();
                    let _ = Box::from_raw(cur);
                    cur = next;
                }
            }
        }
        self.all_objects = std::ptr::null_mut();
        self.young = std::ptr::null_mut();
        self.graveyard = std::ptr::null_mut();
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
///
/// Phase 29 session 2: this is now incremental. Marking happens only
/// when the previous sweep cycle has completed (`sweep_phase` was
/// `Idle`); otherwise the in-flight cycle resumes directly. The
/// sweep step yields after `sweep_budget_ns` — typically 2 ms — and
/// the cursor persists across calls, so a large heap is collected
/// over multiple safepoints rather than stalling one.
pub fn gc_collect_with(scan: impl FnOnce()) {
    let in_flight = HEAP.with(|h| h.borrow().sweep_phase == SweepPhase::Sweeping);
    if !in_flight {
        // Fresh cycle — scan roots before starting sweep. The
        // explicit root stack (web3d-M0) covers values the caller
        // holds on the Rust stack across the safepoint.
        scan();
        ROOT_STACK.with(|r| {
            for v in r.borrow().iter() {
                mark_value(v);
            }
        });
    }
    let budget = HEAP.with(|h| {
        let h = h.borrow();
        if h.stress {
            u64::MAX
        } else {
            h.sweep_budget_ns
        }
    });
    HEAP.with(|h| {
        let _ = h.borrow_mut().sweep_step(budget);
    });
}

/// Returns true if the heap's `bytes_allocated` has crossed
/// `threshold`, meaning the next safepoint should trigger collection.
/// Hot path: called from every VM-dispatch / eval-statement safepoint;
/// must be cheap when over threshold is false.
///
/// Phase 29 session 2: also returns true while a sweep is in flight,
/// so safepoints continue draining the cursor across frames until
/// the cycle completes.
#[inline]
pub fn gc_should_collect() -> bool {
    // web3d-M1: a single thread-local flag, kept in sync by the heap
    // (`Heap::refresh_pending`), instead of borrowing the whole heap on
    // every statement boundary.
    COLLECT_PENDING.with(|c| c.get())
}

thread_local! {
    /// True when the next safepoint should collect: stress mode, the
    /// allocation threshold crossed, or a sweep in flight.
    static COLLECT_PENDING: Cell<bool> = const { Cell::new(false) };
}

// ---------- web3d-M0: stress mode + explicit root stack ----------

fn stress_from_env() -> bool {
    std::env::var("TWE_GC_STRESS").is_ok_and(|v| v != "0" && !v.is_empty())
}

/// Turn GC stress mode on or off for this thread's heap. Stress mode
/// collects at every safepoint with an unlimited sweep budget and
/// poisons swept objects instead of freeing them (see
/// `HeapObject::freed`). Sticky until turned off. Also enabled at heap
/// creation by `TWE_GC_STRESS=1`. Test / fuzz tooling only — memory
/// is not reclaimed while it's on.
pub fn gc_set_stress(on: bool) {
    HEAP.with(|h| {
        let mut h = h.borrow_mut();
        h.stress = on;
        h.refresh_pending();
    });
}

/// Is stress mode on for this thread's heap?
pub fn gc_stress() -> bool {
    HEAP.with(|h| h.borrow().stress)
}

thread_local! {
    /// Extra GC roots: values the interpreter holds only on the Rust
    /// stack across a safepoint (a `for` loop's snapshot, a saved
    /// `self`, …). Pushed through [`RootScope`], which truncates back
    /// on drop so early returns / `?` can't leak entries.
    static ROOT_STACK: RefCell<Vec<TaggedValue>> = const { RefCell::new(Vec::new()) };
}

/// RAII scope over the explicit root stack. Values pushed through
/// [`RootScope::push`] stay rooted until the scope drops.
pub struct RootScope {
    base: usize,
}

impl RootScope {
    pub fn new() -> Self {
        Self {
            base: ROOT_STACK.with(|r| r.borrow().len()),
        }
    }

    /// Root `v` for the lifetime of this scope. Non-heap values are
    /// ignored (nothing to keep alive).
    pub fn push(&self, v: TaggedValue) {
        if v.is_heap_pointer() {
            ROOT_STACK.with(|r| r.borrow_mut().push(v));
        }
    }

    /// Root every value in `vs` for the lifetime of this scope.
    pub fn push_all(&self, vs: &[TaggedValue]) {
        ROOT_STACK.with(|r| {
            let mut r = r.borrow_mut();
            r.extend(vs.iter().copied().filter(|v| v.is_heap_pointer()));
        });
    }
}

impl Default for RootScope {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RootScope {
    fn drop(&mut self) {
        ROOT_STACK.with(|r| r.borrow_mut().truncate(self.base));
    }
}

/// Test/bench helper: override the GC threshold so safepoints fire
/// sooner. Production code should not use this.
pub fn gc_set_threshold(threshold: usize) {
    HEAP.with(|h| {
        let mut h = h.borrow_mut();
        h.threshold = threshold;
        h.refresh_pending();
    });
}

/// Phase 29 session 2: configure the per-safepoint sweep budget in
/// nanoseconds. Lower = smoother frame times but slower memory
/// reclamation; `u64::MAX` disables the budget (sweep always
/// finishes synchronously). Exposed to scripts as `gc.budget_ms(f)`.
pub fn gc_set_budget_ns(budget_ns: u64) {
    HEAP.with(|h| h.borrow_mut().sweep_budget_ns = budget_ns);
}

/// Phase 29 session 2: read the current sweep budget in nanoseconds.
pub fn gc_budget_ns() -> u64 {
    HEAP.with(|h| h.borrow().sweep_budget_ns)
}

/// Phase 29 session 2: wall-clock cost of the most recently completed
/// sweep cycle, in nanoseconds. Aggregates across however many
/// incremental steps the cycle took. Returns 0 before any cycle has
/// completed. Exposed to scripts as `gc.last_collect_ms()`.
pub fn gc_last_collect_ns() -> u64 {
    HEAP.with(|h| h.borrow().last_collect_ns)
}

/// Phase 29 session 2: number of bytes currently held by live
/// objects on the thread-local heap. Recomputed at the end of each
/// sweep cycle; reflects the bytes_allocated counter between cycles.
pub fn gc_bytes_alive() -> usize {
    HEAP.with(|h| h.borrow().bytes_allocated)
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
        assert!(
            !(*ptr).freed.get(),
            "GC use-after-free: a root references a {:?} object that was already swept",
            (*ptr).body_kind
        );
        if (*ptr).mark.get() {
            return; // already marked — break cycles
        }
        (*ptr).mark.set(true);
        // Recursively mark nested TaggedValues in the body.
        mark_body(&(*ptr).body.borrow());
    }
}

/// Mark a class's field defaults and walk its parent chain
/// (web3d-M0: the parent chain was previously unmarked).
fn mark_class(class: &crate::value::ClassDef) {
    let mut cur = Some(class);
    while let Some(c) = cur {
        for v in c.field_defaults.values() {
            mark_value(v);
        }
        // web3d-M1: methods keep their defining module alive.
        for m in c.methods.values() {
            if let Some(h) = &m.home {
                mark_value(h);
            }
        }
        // web3d-M3: the instance field layout (the same values as the
        // chain's defaults today, marked in case that ever changes),
        // and the modules a `look:`'s keys resolve in.
        for (_, v) in &c.field_layout {
            mark_value(v);
        }
        if let Some(look) = &c.look {
            for slot in [
                &look.mesh,
                &look.tint,
                &look.scale,
                &look.facing,
                &look.material,
            ]
            .into_iter()
            .flatten()
            {
                if let Some(h) = &slot.home {
                    mark_value(h);
                }
            }
        }
        cur = c.parent.as_deref();
    }
}

fn mark_body(body: &HeapBody) {
    match body {
        HeapBody::Tuple(_) | HeapBody::SmallTuple { .. } => {
            for v in body.tuple_elems().expect("tuple") {
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
        HeapBody::Class(c) => mark_class(c),
        HeapBody::Instance(rc) => {
            let inst = rc.borrow();
            for v in inst.fields.values() {
                mark_value(v);
            }
            // web3d-M1: the instance's cached wrapper (see
            // `Instance::cached_value`).
            if let Some(v) = &inst.cached_value {
                mark_value(v);
            }
            // web3d-M0: the instance's class (and its parent chain)
            // holds field defaults that `instantiate` copies from; an
            // instance can outlive every Class-tagged value.
            mark_class(&inst.class);
            // saved_returning / saved_params on suspended fiber frames
            // also carry TaggedValues — mark them so a fiber's resume
            // values don't get swept while the fiber is paused.
            for frame in &inst.fiber_frames {
                crate::value::mark_fiber_frame(frame);
            }
        }
        // Tree-walker FunctionDef bodies are AST statements (literals
        // stored as `ast::Lit`, not `TaggedValue`); nothing to scan.
        // Builtin captures are pure function pointers + names.
        // web3d-M1: a function keeps its home module alive.
        HeapBody::Function(def) => {
            if let Some(h) = &def.home {
                mark_value(h);
            }
        }
        HeapBody::Builtin { .. } => {}
        // Leaf bodies — no nested TaggedValues to scan.
        HeapBody::String(_)
        | HeapBody::BoxedInt(_)
        | HeapBody::Percent(_)
        | HeapBody::Quantity { .. }
        | HeapBody::Range { .. } => {}
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
        gc_collect(&[]);
        let inner = TaggedValue::from_borrowed_str("nested");
        let tup = TaggedValue::from_tuple(vec![inner]);
        // Only `tup` is rooted. Marking it should reach `inner` through
        // the tuple's elements, keeping the string alive.
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

    #[test]
    fn incremental_sweep_yields_then_resumes() {
        // Phase 29 session 2: a tight per-step budget should leave
        // sweep mid-list, and the next call should resume rather
        // than restart. Even with a 1ns budget — basically "yield
        // immediately" — repeated calls must eventually reclaim
        // every unrooted allocation.
        gc_collect(&[]); // start from a known clean state
        let baseline = live();

        // Allocate a bunch of unrooted strings.
        for _ in 0..50 {
            let _ = TaggedValue::from_borrowed_str("unrooted");
        }
        assert!(live() >= baseline + 50);

        // Mark phase only — drives sweep_phase to Sweeping with no
        // roots, so all 50 strings are scheduled to die.
        gc_set_budget_ns(1);
        for _ in 0..200 {
            // Drive sweep through gc_collect_with so the no-mark path
            // for in-flight cycles is exercised. After many tiny
            // budgeted steps the cycle must complete.
            gc_collect_with(|| {
                // No roots — everything is collectable.
            });
            if live() <= baseline {
                break;
            }
        }
        // Restore generous budget so other tests behave normally.
        gc_set_budget_ns(u64::MAX);

        let after = live();
        assert!(
            after <= baseline,
            "incremental sweep should drain to baseline; after={after} baseline={baseline}"
        );
    }

    #[test]
    fn allocations_during_sweep_survive_the_round() {
        // Phase 29 session 2: an object allocated mid-sweep must be
        // pre-marked so the cursor doesn't free it before the script
        // ever stored a root reference. We simulate this by starting
        // a sweep with a 1ns budget (yields immediately), allocating
        // a fresh string while sweep_phase == Sweeping, and asserting
        // the string survives the sweep cycle's eventual completion.
        gc_collect(&[]); // start clean
        gc_set_budget_ns(1);
        // Force the heap into Sweeping with no live roots — the prev
        // cycle's leftover allocations are queued for sweep.
        let _ = TaggedValue::from_borrowed_str("about to die");
        gc_collect_with(|| {});
        // Now allocate during in-flight sweep. The new object should
        // be pre-marked.
        let survivor = TaggedValue::from_borrowed_str("born during sweep");
        // Drain sweep to completion.
        for _ in 0..1000 {
            gc_collect_with(|| {});
            // Eventually the in-flight cycle finishes and a new one
            // can't start without scan picking up `survivor`. Since
            // we pass an empty closure, `survivor` is unrooted from
            // the GC's perspective; it survives only because of the
            // mid-sweep pre-mark.
            if !heap_in_sweep() {
                break;
            }
        }
        gc_set_budget_ns(u64::MAX);
        // The string's heap pointer is still readable.
        assert!(survivor.is_str());
        assert_eq!(survivor.as_string(), "born during sweep");
    }

    fn heap_in_sweep() -> bool {
        HEAP.with(|h| h.borrow().sweep_phase == SweepPhase::Sweeping)
    }
}
