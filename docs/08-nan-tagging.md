# Doc 08 — NaN-Tagged Values + Tracing GC

> Design doc for the runtime-representation overhaul scheduled in v0.2 (Phase 8) and currently the last open Phase-8 line item. Authoring this design before the implementation phase so each migration session has a concrete byte layout + sequencing plan to work against.
>
> **Status:** design complete; implementation is its own multi-session push, planned as Phase 8.5 (numbered separately to make the size honest).

---

## Why now

Per `CLAUDE.md` line 52, NaN-tagged 64-bit values have been a "locked decision" since the earliest design phases. They've been deferred since Phase 3's closeout (see `docs/changes/2026-04-29-phase-3-and-4-closeout.md`). The v0.2-v1.0 roadmap (`docs/05-roadmap.md` Phase 8) pulled them forward from the original v0.5 slot because *every* phase that ships first has to be re-validated after — value-representation churn pays back over every later phase.

The Phase 8 plan-agent review (during the v1.0 roadmap session) flagged this explicitly: "NaN tagging is currently the single most-deferred Phase-3 item per `notes/future-phases.md` lines 362–369; it's already overdue."

The honest constraint: it's been deferred this long because it's *expensive* to do correctly. 746 `Value::` pattern-match / construction sites across the codebase, two big files (`src/eval.rs` 3000 lines, `src/vm.rs` 3766 lines), and every code path touches the Value type. The migration is staged here.

---

## What "NaN tagging" means

Per *Crafting Interpreters* Chapter 30 — the canonical reference, already cited in `CLAUDE.md` §"Always-available references":

A 64-bit IEEE 754 double has a sign bit, an 11-bit exponent, and a 52-bit mantissa. NaN is encoded as "all 11 exponent bits set + at least one mantissa bit set." Any specific NaN bit pattern works as a tag — the FPU can't distinguish them, and we don't normally see NaNs in game scripts.

That gives us 51+ free bits inside any "this is a NaN" pattern. Tag the high mantissa bits to encode value type; use the rest for payload (small ints) or pointers (since modern CPUs only use 48 bits of address space).

Twe's locked representation choice is one 64-bit slot per Value. Heap-allocated values (strings, lists, objects, etc.) live behind tagged pointers; immediate values (Nil, Bool, Int, Float) live inline.

---

## Twe's value tag layout

```
high  ........  low
SXXX XXXX XXXX QTTT  PPPP PPPP PPPP PPPP  PPPP PPPP PPPP PPPP  PPPP PPPP PPPP PPPP

S      = sign bit (also used by some tags)
XXXXXXXXXXX = 11-bit exponent (all 1s for NaN)
Q      = QNaN bit (always 1 for our tagged NaNs; quiet-NaN flag)
TTT    = 3-bit type tag (8 type slots)
PPPP... = 48-bit payload
```

Type tag values (3 bits):

| Tag | Type     | Payload                         |
|-----|----------|---------------------------------|
| 000 | Nil      | unused                          |
| 001 | Bool     | low bit of payload = false/true |
| 010 | Int      | i48 (sign-extended on read)     |
| 011 | Percent  | f48 fixed-point (24.24)         |
| 100 | StrPtr   | ptr to `Rc<String>`             |
| 101 | ObjPtr   | ptr to a heap-allocated record (List, Tuple, Object, Instance, BcInstance, Quantity, Range, Function, BcFunction, Class, BcClass, Builtin) |
| 110 | reserved | (future: smallfloat32?)         |
| 111 | reserved | (future: builtin-id immediate?) |

Float values (any non-NaN f64) are stored as-is and recognized by their finite or normal-NaN bit pattern. The fast path: `is_double = (bits & QNAN_MASK) != QNAN_MASK || (tag bits == 0)`.

For ObjPtr, the heap object's first word is a *type header* carrying the actual variant (List vs Tuple vs Object vs ...) plus GC mark bits. This pushes the type-tag work from the value-stack into the heap, but lets the value-slot tag stay 3 bits.

### Smallint cutoff

i48 covers the range `[-2^47, 2^47 − 1]` — about ±140 trillion. Wider than any game-sensible integer. *But* the Twe surface promises `i64` ints. Two options:

1. **Box overflowing ints** behind ObjPtr → `Rc<i64>`. Slow path for big numbers; transparent.
2. **Promote to f64** when an arithmetic op would overflow i48. Loses exactness past 2^53.

**Recommendation: box.** Game code rarely uses ints past 2^47; the boxed slow path almost never fires. Documents `int` as i48-fast / i64-correct in `docs/02-type-system.md`.

### Quantity, Range, Tuple, List, Object, Instance, etc.

All ObjPtr-tagged. Each heap object is `Rc<HeapObject>` where:

```rust
struct HeapObject {
    header: HeapHeader,
    body: HeapBody,
}

enum HeapBody {
    String(String),
    List(RefCell<Vec<TaggedValue>>),
    Tuple(Vec<TaggedValue>),
    Object(RefCell<HashMap<String, TaggedValue>>, &'static str),
    Instance(RefCell<Instance>),
    BcInstance(RefCell<BcInstance>),
    Quantity { value: f64, unit: Rc<String> },
    Range { start: i64, end: i64, exclusive: bool },
    Function(Rc<FunctionDef>),
    BcFunction(Rc<BcFunction>),
    Class(Rc<ClassDef>),
    BcClass(Rc<BcClassDef>),
    Builtin { name: &'static str, params: &'static [&'static str], func: BuiltinFn },
}

struct HeapHeader {
    mark: AtomicU8,    // GC mark bit + 7-bit reserved
    body_kind: u8,     // discriminator for HeapBody (debug + cache-warm)
}
```

The `enum HeapBody` is a discriminated tag duplicating `body_kind`, kept aligned so debugging tools can verify either field independently. Production builds compile out the duplicate.

---

## API shape

```rust
pub struct TaggedValue(u64);

impl TaggedValue {
    pub const NIL: Self = Self(/* ... */);
    pub const TRUE: Self = Self(/* ... */);
    pub const FALSE: Self = Self(/* ... */);

    // Constructors
    pub fn from_int(i: i64) -> Self;
    pub fn from_float(f: f64) -> Self;
    pub fn from_str(s: Rc<String>) -> Self;
    pub fn from_obj(o: Rc<HeapObject>) -> Self;
    pub fn from_bool(b: bool) -> Self;

    // Type predicates
    pub fn is_nil(&self) -> bool;
    pub fn is_bool(&self) -> bool;
    pub fn is_int(&self) -> bool;
    pub fn is_float(&self) -> bool;
    pub fn is_number(&self) -> bool;  // int OR float
    pub fn is_str(&self) -> bool;
    pub fn is_obj(&self) -> bool;

    // Extractors (panic if wrong type — callers test first)
    pub fn as_int(&self) -> i64;
    pub fn as_float(&self) -> f64;
    pub fn as_bool(&self) -> bool;
    pub fn as_str(&self) -> Rc<String>;
    pub fn as_obj(&self) -> Rc<HeapObject>;

    // Compatibility shim during migration
    pub fn to_legacy(&self) -> crate::value::Value;
    pub fn from_legacy(v: &crate::value::Value) -> Self;
}
```

The `to_legacy` / `from_legacy` shim is the migration's load-bearing piece: it lets sites convert at the boundary while the interior keeps using the old `Value` enum. As migration progresses, conversions shrink to the actual interpreter boundaries; the shim eventually deletes.

---

## Tracing GC

### Why a real GC at all?

Twe currently uses `Rc<RefCell<...>>` everywhere. This is correct for the values we have (no cycles in well-formed Twe programs — instances reference classes but classes don't reference instances). But:

1. **Refcount overhead** — every `clone()` is an atomic increment. The bytecode VM does this constantly; profiling Wren and Lua showed refcounting was a big part of dispatch cost.
2. **Cycles are theoretically possible** — `instance.field = instance` builds one. Today this leaks; a tracing GC collects.
3. **Pause budgets** — Roblox's Luau ships with an incremental tracing GC explicitly because per-frame Rc churn was unacceptable for game pacing.

### Algorithm: incremental tri-color mark + sweep

Per *Crafting Interpreters* §26 (the bytecode-VM GC chapter):

- **White**: not-yet-marked. Sweep collects.
- **Grey**: marked but not yet scanned for further references.
- **Black**: marked + scanned.

Steps per cycle:
1. **Roots** — grey out: VM stack, globals, active scene, fiber frames + fiber stack on every BcInstance, env bindings on the tree-walker.
2. **Process grey** — pop a grey object, mark black, mark each child grey.
3. **Sweep** — walk every heap object, free whites, reset blacks → white for next cycle.

**Incrementality**: split (1)+(2) across multiple frames using a fixed work-budget per call to `gc_step`. Steps (1) and (2) interleave with mutation, requiring a write barrier:

```rust
fn write_field(slot: &mut TaggedValue, new_value: TaggedValue) {
    if is_black(slot) && is_white(new_value) {
        gc_grey(new_value);  // protect against premature sweep
    }
    *slot = new_value;
}
```

For v0.2 first cut: stop-the-world mark-sweep. Incremental version is a v0.3 optimization. The stop-the-world version is simple enough to ship alongside NaN tagging without doubling the migration cost.

### Heap allocator

Replace `Rc::new(...)` with a custom heap-managed `gc::alloc(...)` that:
1. Bumps a free-list pointer.
2. Threads the new allocation onto a "all heap objects" linked list (for sweep).
3. Triggers `gc_full()` when allocation crosses a threshold.

Existing `Rc<HeapObject>` semantics stay during migration (the shim still uses Rc); the swap to GC-allocated happens once every site is on TaggedValue.

---

## Migration sequencing

This is the load-bearing part of the doc. NaN tagging + GC isn't one session; it's a phase. Each session ships a runnable artifact (per `CLAUDE.md` working contract).

### Phase 8.5 — NaN tagging + tracing GC

**Status:** planned. Each numbered session below maps to one commit + closeout note.

#### Session 8a — TaggedValue module

- New `src/tagged_value.rs` with `TaggedValue(u64)`, encode/decode, predicates, extractors.
- Round-trip unit tests for every type.
- `to_legacy` / `from_legacy` shim against the existing `Value` enum.
- Module is unused by any existing code path. Standalone correctness.
- *Ships a runnable artifact* via `cargo test`.

#### Session 8b — Heap object header

- `src/heap.rs` with `HeapObject { header, body }` + `HeapBody` enum.
- `HeapObject::new(body)` constructor returns `Rc<HeapObject>` (Rc-managed during migration; will become GC-managed in 8e).
- TaggedValue::from_obj / .as_obj wired against this.
- Tests for round-trip Tuple, List, Object, Quantity through the heap path.

#### Session 8c — VM migration ✅ shipped 2026-04-30

- `src/vm.rs`: value stack changed from `Vec<Value>` to `Vec<TaggedValue>`; globals migrated to `HashMap<String, TaggedValue>`. The dispatch loop's existing pattern-match handlers stay on `Value` for now and shim through `to_legacy()` at the read points (`slot_get`, `peek_top`) / `from_legacy()` at the write points (`push`). The deeper rewrite of every match arm to `TaggedValue` predicates folds into session 8f when the legacy `Value` enum deletes — keeping 8c mechanical kept the regression surface minimal.
- `BcInstance` and `BcInstance::fiber_stack` stay on legacy `Value` internally; conversion at the field-access boundary (the OP_WAIT save and `resume_state_entry` restore both translate per-element).
- The `from_legacy` / `to_legacy` shim now covers every `Value` variant — the 8a/8b shim only covered primitives + the heap variants 8b added; 8c expanded `HeapBody` with `Class` / `Function` / `Instance` / `BcFunction` / `BcClass` / `BcInstance` / `Builtin` so non-primitive Values round-trip through the heap path.
- 497 → 499 tests pass: shim round-trip tests added for heap variants and BcFunction (per the design's "every session adds at least one new test exercising the migrated path" rule).

#### Session 8d — Tree-walker migration ✅ shipped 2026-04-30

- `Env::bindings` migrated from `HashMap<String, Value>` to `HashMap<String, TaggedValue>` in `src/value.rs`. The `Env::get` signature changed from `Option<&Value>` to `Option<Value>` (cloned + converted at the boundary) because the underlying storage now stores tagged slots — there's no `Value` to borrow into. `Env::iter_bindings` now yields owned `(String, Value)` tuples for the same reason.
- `Instance::fields` migrated to `HashMap<String, TaggedValue>`. Direct accessors throughout `eval.rs` shim with `t.clone().to_legacy()` on read and `TaggedValue::from_legacy(&v)` on insert. The interior pattern matches still operate on legacy `Value` until 8f.
- Object::fields stays on `HashMap<String, Value>` for now — that migrates with stdlib in 8e.
- All 499 tests still pass; clippy clean.

#### Session 8e — Stdlib + save migration ✅ shipped 2026-04-30

- `Object::fields` migrated from `HashMap<String, Value>` to `HashMap<String, TaggedValue>` in `src/value.rs`. `BcInstance::fields` migrated in lockstep in `src/bytecode.rs`. Stdlib's module objects (`math`, `key`, `mouse`, `screen`, `time`, `color`, `random`, `entities`, `tilemap`, `camera`, `sound`, `music`, `key_press`, `mouse_held`, `mouse_press`) all now store `TaggedValue` slots.
- Added helper methods on `Object` / `Instance` / `BcInstance`: `get_field(&str) -> Option<Value>` and `insert_field(impl Into<String>, Value)`. Most stdlib + eval call sites swept from direct `.fields.get(...)` / `.fields.insert(...)` to the helpers — single mechanical replace per file.
- Added `legacy_fields_to_tagged(HashMap<String, Value>) -> HashMap<String, TaggedValue>` for the stdlib bootstrap pattern (build a `HashMap` imperatively with `Value` literals, hand off as `TaggedValue` at the `Object { ... }` construction).
- `src/save.rs` `encode` shims via `to_legacy()` when iterating `o.fields`; `decode` builds a `HashMap<String, Value>` and converts at the `Object { ... }` boundary.
- Tests in `tests/eval.rs`, `src/save.rs`, `src/tagged_value.rs` swept to use the new helpers.
- Stdlib's interior pattern matches still operate on legacy `Value` (the rule "every `Value::` in builtins becomes `TaggedValue::*`" lands at 8f when the legacy enum deletes — keeping 8e mechanical kept the regression surface in line with 8c–8d).
- All 499 tests still pass; clippy clean.

#### Session 8f — Delete legacy Value (✅ structural half shipped 2026-04-30, predicate cleanup deferred)

What 8f shipped this session:

- `src/value.rs`: `pub enum Value { … }` renamed to `pub enum LegacyValue { … }`; `pub type Value = TaggedValue;` added so every existing `Value`-typed signature, struct field, and constructor site automatically aligns with the NaN-tagged representation.
- All struct-field storage flipped to `TaggedValue` (= `Value` alias): `ClassDef::field_defaults`, `BcClassDef::field_defaults`, `Env::self_value`, `Env::returning`, `Env::bindings`, `Object::fields`, `Instance::fields`, `BcInstance::fields`, `BcInstance::fiber_stack`, `FrameKind::Function::saved_returning` / `saved_params`. Same for the `BuiltinFn` signature: `fn(&mut Env, &[TaggedValue]) -> Result<TaggedValue, _>`.
- `HeapBody::Tuple` / `HeapBody::List` interiors migrated to `Vec<TaggedValue>`. `LegacyValue::Tuple` / `List` now wrap the same Rc, so `from_legacy` / `to_legacy` share the heap rather than deep-copying — preserves mutation semantics through the shim.
- All 992 `Value::Foo(...)` constructor / pattern sites in the 12 source files + `tests/eval.rs` were rewritten by a mechanical migration pass: constructors became `Value::from_*(...)`, `Value::NIL` / `TRUE` / `FALSE` constants, etc. New helper API on `TaggedValue` covers this (`from_tuple` / `as_tuple`, `is_object` / `as_object`, `is_truthy`, `display`, `type_name`, `equals`, etc., plus per-heap-variant predicate + extractor pairs for every `HeapBodyKind`).
- 499 tests still pass. `cargo build --release` clean. `cargo clippy --lib --tests` shows only the four pre-existing `approx_constant` warnings on `3.14`-as-test-float in `save.rs` / `tagged_value.rs`.

What 8f deferred (call it 8f-followup or 8f.5):

- ~373 match-arm dispatch sites still spell out `match X.to_legacy() { LegacyValue::Foo(x) => ... }` rather than predicate-dispatching directly on `TaggedValue` (`if X.is_foo() { let x = X.as_foo(); ... }`). The shim in `src/value.rs` (`pub trait ToLegacyShim` for `Option<TaggedValue>` / tuples + `LegacyValue::display` / `type_name`) keeps these compiling.
- Therefore `enum LegacyValue`, `TaggedValue::to_legacy` / `from_legacy`, and the `ToLegacyShim` trait still ship. The "100% verified by zero `Value::` patterns outside `tagged_value.rs`" exit criterion is **not** met yet — the alias makes the production representation NaN-tagged, but the legacy enum is still load-bearing for match dispatch.
- Strict-mode inferer diagnostics still mention legacy variant names when they appear; updating to TaggedValue terminology rides the same predicate-conversion pass.

**Why split:** the structural-half is the unblocking work for 8g (GC allocator) and 8h (roots wiring) — both need every storage location to be `TaggedValue` and the heap interior to be `Vec<TaggedValue>`, both of which now hold. The match-arm cleanup is independent volume that can land on its own track without gating GC.

**Scope reality (preserved from original plan):** 917 `Value::` pattern-match sites across the codebase as of post-8e. Constructor + storage migration ate ~550 of those mechanically; the remaining ~370 live behind the `to_legacy()` shim and are the predicate-dispatch follow-up. Each site is still mechanical (`match v.to_legacy() { LegacyValue::Int(n) => ... }` → `if v.is_int_or_boxed_int() { let n = v.as_int(); ... }`); the volume is what makes it its own session.

#### Session 8g — GC heap allocator

- `src/heap.rs` gains `Heap { all_objects: *mut HeapObject, threshold: usize }`.
- `Heap::alloc(body) -> *mut HeapObject` replaces `Rc::new`.
- Linked-list of all heap objects for sweep walk.
- Stop-the-world mark + sweep `Heap::collect(roots: &[TaggedValue])`.
- Triggered from VM/eval at safepoints (between bytecode instructions in VM; between statements in eval).

#### Session 8h — Roots wiring

- VM: stack, globals, active_scene, every active_entity, every BcInstance's fiber_frames + fiber_stack.
- Eval: env bindings, active_scene, active_entities, env.self_value, env.returning, every Instance's fiber_frames.
- Each is a "scan this" callback registered with the GC.
- Tests: cycle-detection (`obj.field = obj`) collects after one full GC cycle.

#### Session 8i — Bench + tune

- `cargo bench` survival-clone benchmark against pre-migration baseline.
- Hard exit criterion from `docs/05-roadmap.md` Phase 8: 3× speedup vs. pre-tag VM. Tune until met.

### Total estimated size

- Sessions 8a / 8b: M each (couple hours of new-module work + tests).
- Sessions 8c / 8d / 8e: L each (mechanical migration; tedious; high regression risk).
- Sessions 8f / 8g / 8h / 8i: M / L / M / M.

Realistic calendar time: **4–8 weeks of focused part-time work**, not "one session." `docs/05-roadmap.md` reflects this with the Phase 8 size marker XL.

---

## What's deferred from this design

- **Generational GC** — young/old-generation split. Most game allocation is short-lived (per-frame temporaries); a generational collector would reduce GC pause time. But it doubles implementation complexity. Defer to v1.x.
- **Concurrent GC** — collector running on a background thread. Twe's runtime is single-threaded by design (`CLAUDE.md` "What is locked"); concurrent GC would force concurrency into the VM. Off the v1.0 critical path.
- **Compacting GC** — moves objects to defragment the heap. Pointer-tagging makes compacting hard (every pointer needs forwarding). Stop-the-world mark+sweep doesn't compact. Acceptable for v1.0.
- **Smallfloat32 immediate** — could put a 32-bit float in a NaN-tagged value's payload. Saves heap allocation for f32-precision floats. Not worth the predicate overhead.
- **String interning** — repeated string literals share one allocation. Tied to GC; ride a v1.x optimization phase.

---

## Risks

1. **Regression from any of 8c–8e** is silent. A botched conversion that compiles+runs but produces subtly wrong results is the worst-case bug class. Mitigation: tight per-session test discipline; every session must keep 467+ tests passing and add at least one new test exercising the migrated path.

2. **Performance regression before GC lands** (sessions 8c–8f). Removing Rc clone-counting but not yet having a real GC means temporarily leaking heap objects. Acceptable during a 4-8 week migration; flag if `cargo test` peak-memory grows beyond ~3× pre-migration.

3. **Incremental marking complexity** if we change v0.2's stop-the-world plan to incremental mid-flight. Stay stop-the-world for v0.2; revisit v1.x.

4. **Closure / fiber root scanning** is non-trivial — every `Vec<Frame>` (eval) and `Vec<BcFiberFrame>` (VM) on every Instance is a root path the collector must walk. Pin in session 8h with explicit tests.

---

## References

- *Crafting Interpreters* Chapter 30 — NaN-tagged value layout (canonical).
- *Crafting Interpreters* Chapter 26 — bytecode-VM GC.
- Wren VM source `wren_value.h` — production NaN-tagging in a similar-shaped scripting language.
- Luau's GC paper (`docs/04-reading-list.md`) — incremental tri-color mark + sweep at scale.
- `docs/05-roadmap.md` Phase 8 — exit criteria reference (3× speedup vs pre-tag VM).
- `notes/future-phases.md` "Triage backlog" — historical NaN-tagging deferrals.
