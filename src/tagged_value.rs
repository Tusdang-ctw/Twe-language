//! v0.2 Phase 8.5 — NaN-tagged 64-bit value representation.
//!
//! See `docs/08-nan-tagging.md` for the full design — byte
//! layout, smallint policy, GC sequencing.
//!
//! **Session 8a status**: this module is the standalone
//! foundation. It compiles, ships round-trip unit tests, but is
//! NOT yet used by any other code path. The migration to wire
//! `TaggedValue` into the VM (8c), tree-walker (8d), stdlib (8e)
//! happens in subsequent sessions; the legacy `crate::value::Value`
//! enum stays the production representation until 8f.
//!
//! ## Bit layout
//!
//! ```text
//! 63       62..52      51   50..48     47..0
//! [sign]   [exp 11b]  [Q]  [tag 3b]  [payload 48b]
//! ```
//!
//! For a *tagged* value (anything that's not a regular Float):
//! bits 62..52 are all 1 (NaN exponent), bit 51 is 1 (Q-NaN),
//! bits 50..48 carry our 3-bit type tag, and bits 47..0 carry
//! the payload. Floats are stored as their raw IEEE 754 bits;
//! `f64::NAN` canonicalizes to tag `0` so it's distinguishable
//! from our tagged Nil/Bool/Int/etc.
//!
//! ## Why `unsafe`
//!
//! Encoding an `Rc<HeapObject>` into the 48-bit payload requires
//! `Rc::into_raw` / `Rc::from_raw` and pointer-to-int casts.
//! Rust's safety model can't express bit-level pointer aliasing.
//! `Cargo.toml` eased `unsafe_code = "forbid"` → `"deny"` so
//! this one module can scope `#![allow(unsafe_code)]`; every
//! other module in the crate stays safe-Rust.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::rc::Rc;

// ---------- bit layout constants ----------

/// Quiet-NaN signalling prefix: bits 62..51 all set. Any value
/// with these bits set is "tagged" (or `f64::NAN` itself, which
/// uses tag 0).
const QNAN: u64 = 0x7FF8_0000_0000_0000;

/// Bits 50..48 carry the 3-bit type tag.
const TAG_MASK: u64 = 0x0007_0000_0000_0000;

/// Bits 47..0 carry the value payload (small-int, pointer, etc.).
const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

const TAG_SHIFT: u32 = 48;

// Tag values (occupy bits 50..48 — i.e. shifted left by TAG_SHIFT).
// Tag `0` is reserved for `f64::NAN` so a real NaN doesn't get
// misread as Nil.
const TAG_NIL: u64 = 1 << TAG_SHIFT;
const TAG_FALSE: u64 = 2 << TAG_SHIFT;
const TAG_TRUE: u64 = 3 << TAG_SHIFT;
const TAG_INT: u64 = 4 << TAG_SHIFT;
const TAG_STR: u64 = 5 << TAG_SHIFT;
const TAG_OBJ: u64 = 6 << TAG_SHIFT;
// 7 reserved.

// ---------- public types ----------

/// One Twe value, stored in a single 64-bit slot. Cheap to pass
/// in a register; encodes Nil / Bool / Int / Float / Str / Obj
/// without enum-discriminator overhead.
///
/// `TaggedValue` is *not* `Copy` because pointer-tagged variants
/// own an `Rc<HeapObject>`. Cloning bumps the refcount; dropping
/// decrements. Immediates (Nil / Bool / Int / Float) clone + drop
/// for free.
pub struct TaggedValue(u64);

/// Heap-allocated body for any value too big to fit inline.
///
/// Session 8a covered `String` only; session 8b expands to the
/// commonly-used heap variants. **Function / BcFunction / Class /
/// BcClass / Instance / BcInstance / Builtin defer to session
/// 8c+** — those are tightly coupled to eval/vm and migrate
/// alongside their callers (adding them now without migration
/// would mean dead code).
///
/// `Vec`/`RefCell`/`Rc` collections currently hold legacy
/// `crate::value::Value` rather than `TaggedValue`. Sessions
/// 8c–8e change to `TaggedValue` interiors as their consumers
/// migrate; for now this lets `from_legacy` / `to_legacy` shim
/// without re-walking the entire collection on every conversion.
#[derive(Debug)]
pub enum HeapBody {
    String(String),
    /// `i64` outside the i48 fast-path range. v0.2 Phase 8.5
    /// session 8b — replaces session 8a's silent truncation.
    BoxedInt(i64),
    /// Twe-specific percent literal (`50%` → 0.5 stored). Tag
    /// space is full at 6 of 8 (Float NaN / Nil / Bool ×2 / Int
    /// / Str / Obj); promoting Percent to heap is the
    /// least-cost route. Most game code uses Percent rarely.
    Percent(f64),
    /// Twe-specific dimensional quantity (`5kg`, `0.1s`).
    Quantity {
        value: f64,
        unit: Rc<String>,
    },
    /// Numeric range literal (`0..10`, `0..=10`).
    Range {
        start: i64,
        end: i64,
        exclusive: bool,
    },
    /// Immutable tuple. v0.2 Phase 8.5 session 8f: interior is
    /// `Vec<TaggedValue>`. Shared `Rc` with `LegacyValue::Tuple` so
    /// `to_legacy` rewraps without deep-copy.
    Tuple(Rc<Vec<TaggedValue>>),
    /// Mutable list. Same migration as Tuple.
    List(Rc<RefCell<Vec<TaggedValue>>>),
    /// Generic object — Twe stdlib's `key`, `mouse`, sprite/sound
    /// handles, save-loaded data, etc. all use this.
    Object(Rc<RefCell<crate::value::Object>>),
    /// Tree-walker user-defined class.
    Class(Rc<crate::value::ClassDef>),
    /// Tree-walker user-defined function.
    Function(Rc<crate::value::FunctionDef>),
    /// Tree-walker class instance. Mutable per `Rc<RefCell<_>>`.
    Instance(Rc<RefCell<crate::value::Instance>>),
    /// Bytecode-VM compiled function.
    BcFunction(Rc<crate::bytecode::BcFunction>),
    /// Bytecode-VM class definition.
    BcClass(Rc<crate::bytecode::BcClassDef>),
    /// Bytecode-VM instance.
    BcInstance(Rc<RefCell<crate::bytecode::BcInstance>>),
    /// Builtin function. The `func` pointer is `Copy`; we store
    /// the legacy `BuiltinFn` signature unchanged so stdlib
    /// dispatchers don't rebind during the migration.
    Builtin {
        name: &'static str,
        params: &'static [&'static str],
        func: crate::value::BuiltinFn,
    },
}

/// Discriminator for `HeapBody` variants. Used by
/// `TaggedValue::is_obj_body_kind` so callers can check "is this
/// a List?" / "is this a Quantity?" without paying for a
/// `with_obj_body` closure. v0.2 Phase 8.5 session 8b — also
/// the seed of the `body_kind: u8` field that the GC's
/// `HeapHeader` will carry in session 8g for per-body-type
/// tracing dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapBodyKind {
    String,
    BoxedInt,
    Percent,
    Quantity,
    Range,
    Tuple,
    List,
    Object,
    Class,
    Function,
    Instance,
    BcFunction,
    BcClass,
    BcInstance,
    Builtin,
}

impl HeapBodyKind {
    pub fn of(body: &HeapBody) -> Self {
        match body {
            HeapBody::String(_) => Self::String,
            HeapBody::BoxedInt(_) => Self::BoxedInt,
            HeapBody::Percent(_) => Self::Percent,
            HeapBody::Quantity { .. } => Self::Quantity,
            HeapBody::Range { .. } => Self::Range,
            HeapBody::Tuple(_) => Self::Tuple,
            HeapBody::List(_) => Self::List,
            HeapBody::Object(_) => Self::Object,
            HeapBody::Class(_) => Self::Class,
            HeapBody::Function(_) => Self::Function,
            HeapBody::Instance(_) => Self::Instance,
            HeapBody::BcFunction(_) => Self::BcFunction,
            HeapBody::BcClass(_) => Self::BcClass,
            HeapBody::BcInstance(_) => Self::BcInstance,
            HeapBody::Builtin { .. } => Self::Builtin,
        }
    }
}

/// One heap object. `body` is the actual data; the GC header
/// (mark bit + body-kind cache + linked-list `next` pointer)
/// rides alongside per-object so `Heap::collect` can sweep without
/// a side table. v0.2 Phase 8.5 session 8g.
///
/// `mark` and `next` use `Cell` so the GC's mark/sweep can mutate
/// them through `&HeapObject` without `RefCell::borrow_mut`'s
/// runtime check (mark/sweep is single-threaded by construction —
/// stop-the-world — and never aliases with body access).
#[derive(Debug)]
pub struct HeapObject {
    /// GC mark bit. `false` = white (unreached) at the start of
    /// each cycle. The mark phase sets it `true` (black). The
    /// sweep phase resets `true` → `false` and frees `false`s.
    pub mark: std::cell::Cell<bool>,
    /// Cached body discriminant — used by `is_obj_body_kind`
    /// without a `with_obj_body` borrow, and by `Heap::collect`
    /// when scanning for nested pointers.
    pub body_kind: HeapBodyKind,
    /// Intrusive linked-list pointer threading every heap
    /// allocation onto `Heap::all_objects`. The sweep walk uses
    /// this list. `Cell` so sweep can rewire neighbours through
    /// shared `&HeapObject`.
    pub next: std::cell::Cell<*mut HeapObject>,
    /// The actual value data. `RefCell` keeps mutation interior
    /// (e.g. `List`'s `Vec<TaggedValue>` push / pop).
    pub body: RefCell<HeapBody>,
}

// ---------- constants ----------

impl TaggedValue {
    pub const NIL: Self = Self(QNAN | TAG_NIL);
    pub const FALSE: Self = Self(QNAN | TAG_FALSE);
    pub const TRUE: Self = Self(QNAN | TAG_TRUE);
}

// ---------- constructors ----------

impl TaggedValue {
    /// Encode a `bool` as the matching `TRUE` / `FALSE` constant.
    #[inline]
    pub fn from_bool(b: bool) -> Self {
        if b {
            Self(QNAN | TAG_TRUE)
        } else {
            Self(QNAN | TAG_FALSE)
        }
    }

    /// Encode an `i64`. Values inside the i48 range take the
    /// fast immediate path; values outside box to a
    /// `HeapBody::BoxedInt`. v0.2 Phase 8.5 session 8b
    /// (replaces 8a's silent truncation).
    #[inline]
    pub fn from_int(n: i64) -> Self {
        const I48_MAX: i64 = (1 << 47) - 1;
        const I48_MIN: i64 = -(1 << 47);
        if (I48_MIN..=I48_MAX).contains(&n) {
            let payload = (n as u64) & PAYLOAD_MASK;
            Self(QNAN | TAG_INT | payload)
        } else {
            Self::from_heap(HeapBody::BoxedInt(n))
        }
    }

    /// Phase 29 session 3: encode an `i64` whose magnitude is known
    /// to fit in i48. Skips the bounds branch in `from_int`. Used by
    /// VM hot paths where the result of two i48 operands stays in
    /// i48 range under typical game workloads. **Wraps silently if
    /// the input is outside i48** — only call when overflow is
    /// genuinely impossible (e.g., a small constant add).
    #[inline]
    pub fn from_imm_int_unchecked(n: i64) -> Self {
        let payload = (n as u64) & PAYLOAD_MASK;
        Self(QNAN | TAG_INT | payload)
    }

    /// Encode an `f64`. Canonicalizes NaN to a single bit pattern
    /// so a payload that happens to look like one of our tags
    /// can't be misread as Nil/Bool/etc.
    #[inline]
    pub fn from_float(f: f64) -> Self {
        if f.is_nan() {
            // f64::NAN canonical bit pattern is `QNAN` exactly,
            // tag 0. Distinguishable from any of our tagged
            // values (which all use tag != 0).
            Self(f64::NAN.to_bits())
        } else {
            Self(f.to_bits())
        }
    }

    /// Encode a string. Allocates a `HeapObject { String(s) }`
    /// on the GC heap and tags the pointer. v0.2 Phase 8.5
    /// session 8g — the heap owns the allocation; sweep frees it.
    pub fn from_string(s: String) -> Self {
        let raw = crate::heap::gc_alloc(HeapBody::String(s)) as usize as u64;
        Self(QNAN | TAG_STR | (raw & PAYLOAD_MASK))
    }

    /// Encode a `&str` — convenience for callers that already
    /// have a borrowed string. Named `from_borrowed_str` rather
    /// than `from_str` to avoid a clippy clash with
    /// `std::str::FromStr::from_str`'s signature
    /// (`fn from_str(s: &str) -> Result<Self, Self::Err>`); we
    /// don't want the trait impl since it forces a `Result`
    /// return that doesn't fit our infallible encoding.
    pub fn from_borrowed_str(s: &str) -> Self {
        Self::from_string(s.to_string())
    }

    /// Internal: heap-allocate any non-string `HeapBody` and
    /// tag the pointer with `TAG_OBJ`. v0.2 Phase 8.5 session 8b
    /// (8g routes through the GC heap allocator).
    fn from_heap(body: HeapBody) -> Self {
        let raw = crate::heap::gc_alloc(body) as usize as u64;
        Self(QNAN | TAG_OBJ | (raw & PAYLOAD_MASK))
    }

    pub fn from_percent(p: f64) -> Self {
        Self::from_heap(HeapBody::Percent(p))
    }

    pub fn from_quantity(value: f64, unit: Rc<String>) -> Self {
        Self::from_heap(HeapBody::Quantity { value, unit })
    }

    pub fn from_range(start: i64, end: i64, exclusive: bool) -> Self {
        Self::from_heap(HeapBody::Range {
            start,
            end,
            exclusive,
        })
    }

    pub fn from_tuple(elems: Rc<Vec<TaggedValue>>) -> Self {
        Self::from_heap(HeapBody::Tuple(elems))
    }

    pub fn from_list(elems: Rc<RefCell<Vec<TaggedValue>>>) -> Self {
        Self::from_heap(HeapBody::List(elems))
    }

    pub fn from_object(obj: Rc<RefCell<crate::value::Object>>) -> Self {
        Self::from_heap(HeapBody::Object(obj))
    }

    pub fn from_class(c: Rc<crate::value::ClassDef>) -> Self {
        Self::from_heap(HeapBody::Class(c))
    }

    pub fn from_function(f: Rc<crate::value::FunctionDef>) -> Self {
        Self::from_heap(HeapBody::Function(f))
    }

    pub fn from_instance(i: Rc<RefCell<crate::value::Instance>>) -> Self {
        Self::from_heap(HeapBody::Instance(i))
    }

    pub fn from_bc_function(f: Rc<crate::bytecode::BcFunction>) -> Self {
        Self::from_heap(HeapBody::BcFunction(f))
    }

    pub fn from_bc_class(c: Rc<crate::bytecode::BcClassDef>) -> Self {
        Self::from_heap(HeapBody::BcClass(c))
    }

    pub fn from_bc_instance(i: Rc<RefCell<crate::bytecode::BcInstance>>) -> Self {
        Self::from_heap(HeapBody::BcInstance(i))
    }

    pub fn from_builtin(
        name: &'static str,
        params: &'static [&'static str],
        func: crate::value::BuiltinFn,
    ) -> Self {
        Self::from_heap(HeapBody::Builtin { name, params, func })
    }
}

// ---------- predicates ----------
//
// These are dispatched on every bytecode instruction (binary_arith,
// compare, JumpIfFalse, ...) so they're forced inline. Without
// `#[inline]`, the optimizer leaves them as separate functions and
// release-build dispatch slows by ~70% on tight integer loops
// (measured against pre-NaN-tag baseline; see Phase 8.5 session 8i).

impl TaggedValue {
    /// True for any value that's NOT a regular non-NaN f64.
    /// (Tag 0 — `f64::NAN` itself — passes `is_float`.)
    #[inline]
    fn is_tagged(&self) -> bool {
        (self.0 & QNAN) == QNAN && (self.0 & TAG_MASK) != 0
    }

    #[inline]
    pub fn is_nil(&self) -> bool {
        self.0 == (QNAN | TAG_NIL)
    }
    #[inline]
    pub fn is_bool(&self) -> bool {
        let tag = self.0 & TAG_MASK;
        self.is_tagged() && (tag == TAG_FALSE || tag == TAG_TRUE)
    }
    #[inline]
    pub fn is_int(&self) -> bool {
        self.is_tagged() && (self.0 & TAG_MASK) == TAG_INT
    }
    #[inline]
    pub fn is_float(&self) -> bool {
        // Either not-tagged at all, OR tag is 0 (canonical NaN).
        !self.is_tagged()
    }
    #[inline]
    pub fn is_number(&self) -> bool {
        self.is_int() || self.is_float()
    }
    #[inline]
    pub fn is_str(&self) -> bool {
        self.is_tagged() && (self.0 & TAG_MASK) == TAG_STR
    }
    #[inline]
    pub fn is_obj(&self) -> bool {
        self.is_tagged() && (self.0 & TAG_MASK) == TAG_OBJ
    }
    /// True for any heap-allocated variant (Str / Obj). Used by
    /// `Clone` / `Drop` to know whether to bump / decrement the
    /// refcount.
    #[inline]
    fn is_heap(&self) -> bool {
        self.is_str() || self.is_obj()
    }
}

// ---------- extractors ----------
//
// Each `as_*` panics on type mismatch (caller is expected to
// test first). Strict separation matches the existing
// `match value { Value::Int(n) => ... }` discipline.

impl TaggedValue {
    #[inline]
    pub fn as_bool(&self) -> bool {
        debug_assert!(self.is_bool(), "as_bool on non-bool");
        (self.0 & TAG_MASK) == TAG_TRUE
    }

    /// Read an int-typed value, whether immediate (i48) or
    /// boxed (i64). Callers should pre-test with
    /// `is_int_or_boxed_int()` to know it's safe.
    #[inline]
    pub fn as_int(&self) -> i64 {
        if self.is_int() {
            return self.as_imm_int_unchecked();
        }
        if self.is_obj() {
            return self.with_obj_body(|b| match b {
                HeapBody::BoxedInt(n) => *n,
                other => panic!("as_int on non-int heap body: {other:?}"),
            });
        }
        panic!("as_int on non-int value")
    }

    /// Phase 29 session 3: read the i48 payload of an immediate-int
    /// value without re-running the tag predicate. Caller MUST have
    /// already verified `is_int()` is true. Used by VM hot paths
    /// (`binary_arith`, `compare`) where the predicate has just been
    /// checked and the redundant `as_int` branch is the difference
    /// between a single signed-shift extract and a chain of compares.
    #[inline]
    pub fn as_imm_int_unchecked(&self) -> i64 {
        debug_assert!(self.is_int(), "as_imm_int_unchecked on non-immediate-int");
        // Sign-extend bit 47 across the high 16 bits via arithmetic
        // shift on the signed cast — one instruction on every modern
        // ISA, vs the previous compare-and-or branch.
        ((self.0 << 16) as i64) >> 16
    }

    /// True for either the i48 immediate path or the
    /// `HeapBody::BoxedInt` variant. Callers that want "is this
    /// an integer regardless of representation" should use this
    /// rather than `is_int` (which is only the fast path).
    #[inline]
    pub fn is_int_or_boxed_int(&self) -> bool {
        if self.is_int() {
            return true;
        }
        if self.is_obj() {
            return self.is_obj_body_kind(HeapBodyKind::BoxedInt);
        }
        false
    }

    #[inline]
    pub fn as_float(&self) -> f64 {
        debug_assert!(self.is_float(), "as_float on non-float");
        f64::from_bits(self.0)
    }

    /// Returns a clone of the inner string. The `Rc<HeapObject>`
    /// stays in place; this is one allocation for the returned
    /// `String`. Callers that just need a `&str` should use
    /// `with_str` (TODO session 8c+).
    pub fn as_string(&self) -> String {
        debug_assert!(self.is_str(), "as_string on non-string");
        self.with_heap_object(|obj| match &*obj.body.borrow() {
            HeapBody::String(s) => s.clone(),
            other => panic!("as_string expected HeapBody::String, got {other:?}"),
        })
    }

    /// Inspect the heap body of an obj-tagged value. The closure
    /// receives `&HeapBody`; matching out specific variants lets
    /// callers extract `BoxedInt`, `Tuple`, etc. v0.2 Phase 8.5
    /// session 8b.
    pub fn with_obj_body<R>(&self, f: impl FnOnce(&HeapBody) -> R) -> R {
        debug_assert!(self.is_obj(), "with_obj_body on non-obj");
        self.with_heap_object(|obj| f(&obj.body.borrow()))
    }

    /// True when the obj-tagged value's body matches the given
    /// `HeapBody` discriminant (variant, ignoring payload). v0.2
    /// Phase 8.5 session 8b — convenience predicate so callers
    /// don't have to write `with_obj_body(|b| matches!(b, ...))`.
    pub fn is_obj_body_kind(&self, kind: HeapBodyKind) -> bool {
        if !self.is_obj() {
            return false;
        }
        self.with_obj_body(|b| HeapBodyKind::of(b) == kind)
    }

    /// Twe truthiness: only `false` is falsy. Per Principle 3 +
    /// `docs/03-runtime.md` pitfall #2.
    #[inline]
    pub fn is_truthy(&self) -> bool {
        !self.is_bool() || self.as_bool()
    }

    #[inline]
    pub fn is_falsy(&self) -> bool {
        self.is_bool() && !self.as_bool()
    }

    /// Twe type name for error messages and `type_of`.
    pub fn type_name(&self) -> &'static str {
        if self.is_nil() {
            return "nil";
        }
        if self.is_bool() {
            return "bool";
        }
        if self.is_int_or_boxed_int() {
            return "int";
        }
        if self.is_float() {
            return "float";
        }
        if self.is_str() {
            return "string";
        }
        if self.is_obj() {
            return self.with_obj_body(|b| match b {
                HeapBody::Percent(_) => "percent",
                HeapBody::Quantity { .. } => "quantity",
                HeapBody::Range { .. } => "range",
                HeapBody::Tuple(_) => "tuple",
                HeapBody::List(_) => "list",
                HeapBody::Object(o) => o.borrow().kind,
                HeapBody::Class(_) | HeapBody::BcClass(_) => "class",
                HeapBody::Instance(_) | HeapBody::BcInstance(_) => "instance",
                HeapBody::Function(_) | HeapBody::BcFunction(_) | HeapBody::Builtin { .. } => {
                    "function"
                }
                HeapBody::String(_) => unreachable!("strings live behind TAG_STR"),
                HeapBody::BoxedInt(_) => "int",
            });
        }
        "unknown"
    }

    /// Source-style display. Matches the legacy `LegacyValue::display`.
    pub fn display(&self) -> String {
        if self.is_nil() {
            return "nil".to_string();
        }
        if self.is_bool() {
            return self.as_bool().to_string();
        }
        if self.is_int_or_boxed_int() {
            return self.as_int().to_string();
        }
        if self.is_float() {
            return format!("{:?}", self.as_float());
        }
        if self.is_str() {
            return self.as_string();
        }
        if self.is_obj() {
            return self.with_obj_body(|b| match b {
                HeapBody::Percent(p) => format!("{p}%"),
                HeapBody::Quantity { value, unit } => format!("{value}{unit}"),
                HeapBody::Range {
                    start,
                    end,
                    exclusive,
                } => {
                    let op = if *exclusive { "..<" } else { ".." };
                    format!("{start}{op}{end}")
                }
                HeapBody::Tuple(elems) => {
                    let parts: Vec<String> = elems.iter().map(|t| t.display()).collect();
                    format!("({})", parts.join(", "))
                }
                HeapBody::List(rc) => {
                    let parts: Vec<String> = rc.borrow().iter().map(|t| t.display()).collect();
                    format!("[{}]", parts.join(", "))
                }
                HeapBody::Object(o) => format!("<{}>", o.borrow().kind),
                HeapBody::Class(c) => format!("<{} {}>", c.kind, c.name),
                HeapBody::Instance(i) => format!("<{}>", i.borrow().class.name),
                HeapBody::Function(func) => format!("<function {}>", func.name),
                HeapBody::BcFunction(func) => format!("<function {}>", func.name),
                HeapBody::BcClass(c) => format!("<{} {}>", c.kind, c.name),
                HeapBody::BcInstance(i) => format!("<{}>", i.borrow().class.name),
                HeapBody::Builtin { name, .. } => format!("<builtin {name}>"),
                HeapBody::String(_) => unreachable!("strings live behind TAG_STR"),
                HeapBody::BoxedInt(n) => n.to_string(),
            });
        }
        "<unknown>".to_string()
    }

    /// Twe value equality. Numeric int↔float cross-compares, strings by
    /// content, tuples / lists element-wise; everything else by `Rc` identity.
    pub fn equals(&self, other: &TaggedValue) -> bool {
        if self.is_nil() && other.is_nil() {
            return true;
        }
        if self.is_bool() && other.is_bool() {
            return self.as_bool() == other.as_bool();
        }
        if self.is_int_or_boxed_int() && other.is_int_or_boxed_int() {
            return self.as_int() == other.as_int();
        }
        if self.is_float() && other.is_float() {
            return self.as_float() == other.as_float();
        }
        if self.is_int_or_boxed_int() && other.is_float() {
            return (self.as_int() as f64) == other.as_float();
        }
        if self.is_float() && other.is_int_or_boxed_int() {
            return self.as_float() == (other.as_int() as f64);
        }
        if self.is_str() && other.is_str() {
            return self.as_string() == other.as_string();
        }
        if self.is_obj() && other.is_obj() {
            return self.with_obj_body(|a| {
                other.with_obj_body(|b| match (a, b) {
                    (HeapBody::Percent(x), HeapBody::Percent(y)) => x == y,
                    (
                        HeapBody::Quantity {
                            value: vx,
                            unit: ux,
                        },
                        HeapBody::Quantity {
                            value: vy,
                            unit: uy,
                        },
                    ) => vx == vy && ux == uy,
                    (
                        HeapBody::Range {
                            start: sa,
                            end: ea,
                            exclusive: xa,
                        },
                        HeapBody::Range {
                            start: sb,
                            end: eb,
                            exclusive: xb,
                        },
                    ) => sa == sb && ea == eb && xa == xb,
                    (HeapBody::Tuple(ra), HeapBody::Tuple(rb)) => {
                        ra.len() == rb.len() && ra.iter().zip(rb.iter()).all(|(x, y)| x.equals(y))
                    }
                    (HeapBody::List(ra), HeapBody::List(rb)) => {
                        let a = ra.borrow();
                        let b = rb.borrow();
                        a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.equals(y))
                    }
                    (HeapBody::Object(ra), HeapBody::Object(rb)) => Rc::ptr_eq(ra, rb),
                    (HeapBody::Class(ra), HeapBody::Class(rb)) => Rc::ptr_eq(ra, rb),
                    (HeapBody::Instance(ra), HeapBody::Instance(rb)) => Rc::ptr_eq(ra, rb),
                    (HeapBody::Function(ra), HeapBody::Function(rb)) => Rc::ptr_eq(ra, rb),
                    (HeapBody::BcFunction(ra), HeapBody::BcFunction(rb)) => Rc::ptr_eq(ra, rb),
                    (HeapBody::BcClass(ra), HeapBody::BcClass(rb)) => Rc::ptr_eq(ra, rb),
                    (HeapBody::BcInstance(ra), HeapBody::BcInstance(rb)) => Rc::ptr_eq(ra, rb),
                    (HeapBody::Builtin { func: fa, .. }, HeapBody::Builtin { func: fb, .. }) => {
                        std::ptr::eq(*fa as *const (), *fb as *const ())
                    }
                    _ => false,
                })
            });
        }
        false
    }

    // ---- per-heap-variant predicates ----
    pub fn is_percent(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::Percent)
    }
    pub fn is_quantity(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::Quantity)
    }
    pub fn is_range(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::Range)
    }
    pub fn is_tuple(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::Tuple)
    }
    pub fn is_list(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::List)
    }
    pub fn is_object(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::Object)
    }
    pub fn is_class(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::Class)
    }
    pub fn is_instance(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::Instance)
    }
    pub fn is_function(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::Function)
    }
    pub fn is_bc_function(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::BcFunction)
    }
    pub fn is_bc_class(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::BcClass)
    }
    pub fn is_bc_instance(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::BcInstance)
    }
    pub fn is_builtin(&self) -> bool {
        self.is_obj_body_kind(HeapBodyKind::Builtin)
    }
    pub fn is_callable(&self) -> bool {
        self.is_function() || self.is_bc_function() || self.is_builtin()
    }

    // ---- per-heap-variant extractors (panic on mismatch) ----
    pub fn as_percent(&self) -> f64 {
        self.with_obj_body(|b| match b {
            HeapBody::Percent(p) => *p,
            other => panic!("as_percent: not a percent: {other:?}"),
        })
    }

    pub fn as_quantity(&self) -> (f64, Rc<String>) {
        self.with_obj_body(|b| match b {
            HeapBody::Quantity { value, unit } => (*value, unit.clone()),
            other => panic!("as_quantity: not a quantity: {other:?}"),
        })
    }

    pub fn as_range(&self) -> (i64, i64, bool) {
        self.with_obj_body(|b| match b {
            HeapBody::Range {
                start,
                end,
                exclusive,
            } => (*start, *end, *exclusive),
            other => panic!("as_range: not a range: {other:?}"),
        })
    }

    pub fn as_tuple(&self) -> Rc<Vec<TaggedValue>> {
        self.with_obj_body(|b| match b {
            HeapBody::Tuple(rc) => rc.clone(),
            other => panic!("as_tuple: not a tuple: {other:?}"),
        })
    }

    pub fn as_list(&self) -> Rc<RefCell<Vec<TaggedValue>>> {
        self.with_obj_body(|b| match b {
            HeapBody::List(rc) => rc.clone(),
            other => panic!("as_list: not a list: {other:?}"),
        })
    }

    pub fn as_object(&self) -> Rc<RefCell<crate::value::Object>> {
        self.with_obj_body(|b| match b {
            HeapBody::Object(rc) => rc.clone(),
            other => panic!("as_object: not an object: {other:?}"),
        })
    }

    pub fn as_class(&self) -> Rc<crate::value::ClassDef> {
        self.with_obj_body(|b| match b {
            HeapBody::Class(rc) => rc.clone(),
            other => panic!("as_class: not a class: {other:?}"),
        })
    }

    pub fn as_instance(&self) -> Rc<RefCell<crate::value::Instance>> {
        self.with_obj_body(|b| match b {
            HeapBody::Instance(rc) => rc.clone(),
            other => panic!("as_instance: not an instance: {other:?}"),
        })
    }

    pub fn as_function(&self) -> Rc<crate::value::FunctionDef> {
        self.with_obj_body(|b| match b {
            HeapBody::Function(rc) => rc.clone(),
            other => panic!("as_function: not a function: {other:?}"),
        })
    }

    pub fn as_bc_function(&self) -> Rc<crate::bytecode::BcFunction> {
        self.with_obj_body(|b| match b {
            HeapBody::BcFunction(rc) => rc.clone(),
            other => panic!("as_bc_function: not a bc_function: {other:?}"),
        })
    }

    pub fn as_bc_class(&self) -> Rc<crate::bytecode::BcClassDef> {
        self.with_obj_body(|b| match b {
            HeapBody::BcClass(rc) => rc.clone(),
            other => panic!("as_bc_class: not a bc_class: {other:?}"),
        })
    }

    pub fn as_bc_instance(&self) -> Rc<RefCell<crate::bytecode::BcInstance>> {
        self.with_obj_body(|b| match b {
            HeapBody::BcInstance(rc) => rc.clone(),
            other => panic!("as_bc_instance: not a bc_instance: {other:?}"),
        })
    }

    pub fn as_builtin(
        &self,
    ) -> (
        &'static str,
        &'static [&'static str],
        crate::value::BuiltinFn,
    ) {
        self.with_obj_body(|b| match b {
            HeapBody::Builtin { name, params, func } => (*name, *params, *func),
            other => panic!("as_builtin: not a builtin: {other:?}"),
        })
    }

    /// Borrow the heap object behind a pointer-tagged value.
    /// The closure must not retain any reference past return —
    /// the borrow lives only for the duration of the call. The
    /// GC heap owns the allocation; this borrow does not affect
    /// liveness.
    fn with_heap_object<R>(&self, f: impl FnOnce(&HeapObject) -> R) -> R {
        debug_assert!(self.is_heap(), "with_heap_object on non-heap value");
        let ptr = self.heap_ptr();
        // SAFETY: a pointer-tagged TaggedValue's payload always
        // points at a live heap object — either still on
        // `Heap::all_objects`, or about to be marked black by an
        // ongoing collect that walks transitively from this root.
        // GC sweep frees only objects that finish a cycle white;
        // by construction, anything we hold a TaggedValue to is
        // either reachable or safely past the relevant safepoint.
        unsafe { f(&*ptr) }
    }

    /// Raw pointer to the heap object. Public for `crate::heap`'s
    /// mark phase. Returns `null` if the value isn't pointer-tagged.
    pub(crate) fn heap_ptr(&self) -> *mut HeapObject {
        debug_assert!(self.is_heap(), "heap_ptr on non-heap value");
        (self.0 & PAYLOAD_MASK) as usize as *mut HeapObject
    }

    /// Cheap predicate: is this a Str-tagged or Obj-tagged value
    /// (i.e. carries a heap pointer)? Used by `crate::heap::mark_value`.
    pub(crate) fn is_heap_pointer(&self) -> bool {
        self.is_heap()
    }
}

// ---------- Copy / Clone ----------
//
// Pre-8g, `TaggedValue` ran a refcount dance on Clone / Drop because
// pointer-tagged variants owned an `Rc<HeapObject>`. With the GC
// heap owning every allocation (8g), `TaggedValue` is just a u64 —
// freely copyable, no destructor needed.

impl Copy for TaggedValue {}

impl Clone for TaggedValue {
    fn clone(&self) -> Self {
        *self
    }
}

// ---------- Debug ----------

impl std::fmt::Debug for TaggedValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_nil() {
            write!(f, "Nil")
        } else if self.is_bool() {
            write!(f, "Bool({})", self.as_bool())
        } else if self.is_int() {
            write!(f, "Int({})", self.as_int())
        } else if self.is_float() {
            write!(f, "Float({})", self.as_float())
        } else if self.is_str() {
            write!(f, "Str({:?})", self.as_string())
        } else if self.is_obj() {
            write!(f, "Obj(<heap>)")
        } else {
            write!(f, "TaggedValue(0x{:016x})", self.0)
        }
    }
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nil_round_trip() {
        let v = TaggedValue::NIL;
        assert!(v.is_nil());
        assert!(!v.is_bool());
        assert!(!v.is_int());
        assert!(!v.is_float());
    }

    #[test]
    fn bool_round_trip() {
        let t = TaggedValue::from_bool(true);
        let f = TaggedValue::from_bool(false);
        assert!(t.is_bool());
        assert!(f.is_bool());
        assert!(t.as_bool());
        assert!(!f.as_bool());
        // Predicates exclude each other.
        assert!(!t.is_int());
        assert!(!t.is_float());
        assert!(!f.is_nil());
    }

    #[test]
    fn int_round_trip_small() {
        for n in [0_i64, 1, -1, 42, -42, 1_000_000, -1_000_000] {
            let v = TaggedValue::from_int(n);
            assert!(v.is_int(), "is_int failed for {n}");
            assert_eq!(v.as_int(), n, "round-trip failed for {n}");
        }
    }

    #[test]
    fn int_round_trip_i48_extremes() {
        let max_i48: i64 = (1 << 47) - 1;
        let min_i48: i64 = -(1 << 47);
        let v_max = TaggedValue::from_int(max_i48);
        let v_min = TaggedValue::from_int(min_i48);
        assert_eq!(v_max.as_int(), max_i48);
        assert_eq!(v_min.as_int(), min_i48);
    }

    #[test]
    fn float_round_trip() {
        for f in [
            0.0_f64,
            1.0,
            -1.0,
            2.5,
            1e100,
            -1e-100,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            let v = TaggedValue::from_float(f);
            assert!(v.is_float(), "is_float failed for {f}");
            assert_eq!(v.as_float(), f, "round-trip failed for {f}");
        }
    }

    #[test]
    fn float_nan_canonicalizes_and_remains_a_float() {
        let v = TaggedValue::from_float(f64::NAN);
        assert!(v.is_float(), "NaN should still test as float");
        assert!(!v.is_nil(), "canonical NaN must not collide with Nil tag");
        assert!(v.as_float().is_nan());
    }

    #[test]
    fn string_round_trip() {
        let v = TaggedValue::from_borrowed_str("hello, world");
        assert!(v.is_str());
        assert!(!v.is_obj());
        assert_eq!(v.as_string(), "hello, world");
    }

    #[test]
    fn empty_string_round_trips() {
        let v = TaggedValue::from_borrowed_str("");
        assert!(v.is_str());
        assert_eq!(v.as_string(), "");
    }

    #[test]
    fn unicode_string_round_trips() {
        let v = TaggedValue::from_borrowed_str("こんにちは 🌸");
        assert_eq!(v.as_string(), "こんにちは 🌸");
    }

    #[test]
    fn predicates_partition_correctly() {
        // Every value reports exactly one is_* category.
        let cases: Vec<(TaggedValue, &str)> = vec![
            (TaggedValue::NIL, "nil"),
            (TaggedValue::from_bool(true), "bool"),
            (TaggedValue::from_bool(false), "bool"),
            (TaggedValue::from_int(7), "int"),
            (TaggedValue::from_float(2.5), "float"),
            (TaggedValue::from_borrowed_str("x"), "str"),
        ];
        for (v, expected) in cases {
            let actual = if v.is_nil() {
                "nil"
            } else if v.is_bool() {
                "bool"
            } else if v.is_int() {
                "int"
            } else if v.is_float() {
                "float"
            } else if v.is_str() {
                "str"
            } else if v.is_obj() {
                "obj"
            } else {
                "???"
            };
            assert_eq!(actual, expected, "{v:?}");
        }
    }

    #[test]
    fn is_number_covers_int_and_float() {
        assert!(TaggedValue::from_int(3).is_number());
        assert!(TaggedValue::from_float(2.5).is_number());
        assert!(!TaggedValue::NIL.is_number());
        assert!(!TaggedValue::from_bool(true).is_number());
        assert!(!TaggedValue::from_borrowed_str("x").is_number());
    }

    // --- v0.2 Phase 8.5 session 8b: heap-variant round-trips ---

    #[test]
    fn boxed_int_round_trip_above_i48() {
        let big = (1_i64 << 47) + 5; // outside i48 fast path
        let v = TaggedValue::from_int(big);
        assert!(!v.is_int(), "above-i48 must take the boxed path");
        assert!(v.is_int_or_boxed_int());
        assert_eq!(v.as_int(), big);
    }

    #[test]
    fn boxed_int_round_trip_below_i48() {
        let small = -(1_i64 << 47) - 5;
        let v = TaggedValue::from_int(small);
        assert!(!v.is_int());
        assert!(v.is_int_or_boxed_int());
        assert_eq!(v.as_int(), small);
    }

    #[test]
    fn percent_round_trip() {
        let v = TaggedValue::from_percent(0.25);
        assert!(v.is_obj());
        assert!(v.is_obj_body_kind(HeapBodyKind::Percent));
        v.with_obj_body(|b| match b {
            HeapBody::Percent(p) => assert_eq!(*p, 0.25),
            other => panic!("expected Percent, got {other:?}"),
        });
    }

    #[test]
    fn quantity_round_trip() {
        let v = TaggedValue::from_quantity(5.0, Rc::new("kg".to_string()));
        assert!(v.is_obj_body_kind(HeapBodyKind::Quantity));
        v.with_obj_body(|b| match b {
            HeapBody::Quantity { value, unit } => {
                assert_eq!(*value, 5.0);
                assert_eq!(&**unit, "kg");
            }
            other => panic!("expected Quantity, got {other:?}"),
        });
    }

    #[test]
    fn range_round_trip() {
        let v = TaggedValue::from_range(0, 10, true);
        assert!(v.is_obj_body_kind(HeapBodyKind::Range));
        v.with_obj_body(|b| match b {
            HeapBody::Range {
                start,
                end,
                exclusive,
            } => {
                assert_eq!(*start, 0);
                assert_eq!(*end, 10);
                assert!(*exclusive);
            }
            other => panic!("expected Range, got {other:?}"),
        });
    }

    #[test]
    fn tuple_round_trip_holds_tagged_values() {
        let elems = Rc::new(vec![
            TaggedValue::from_int(1),
            TaggedValue::from_int(2),
            TaggedValue::from_int(3),
        ]);
        let v = TaggedValue::from_tuple(elems);
        assert!(v.is_tuple());
        let rc = v.as_tuple();
        assert_eq!(rc.len(), 3);
        assert!(rc[0].is_int());
        assert_eq!(rc[0].as_int(), 1);
    }

    #[test]
    fn list_round_trip_holds_mutable_tagged_values() {
        let inner = Rc::new(RefCell::new(vec![
            TaggedValue::from_int(7),
            TaggedValue::from_int(8),
        ]));
        let v = TaggedValue::from_list(inner.clone());
        assert!(v.is_list());
        inner.borrow_mut().push(TaggedValue::from_int(9));
        let rc = v.as_list();
        assert_eq!(rc.borrow().len(), 3);
    }

    #[test]
    fn object_round_trip_carries_kind_string() {
        use std::collections::HashMap;
        let mut fields = HashMap::new();
        fields.insert("hp".to_string(), TaggedValue::from_int(100));
        let obj = Rc::new(RefCell::new(crate::value::Object {
            fields,
            kind: "test",
        }));
        let v = TaggedValue::from_object(obj);
        assert!(v.is_object());
        let rc = v.as_object();
        let o = rc.borrow();
        assert_eq!(o.kind, "test");
        let hp = o.get_field("hp").expect("hp");
        assert!(hp.is_int());
        assert_eq!(hp.as_int(), 100);
    }

    #[test]
    fn heap_body_kind_classifies_each_variant() {
        // Sanity: HeapBodyKind::of returns the matching kind
        // for every populated HeapBody variant.
        assert_eq!(
            HeapBodyKind::of(&HeapBody::String(String::new())),
            HeapBodyKind::String
        );
        assert_eq!(
            HeapBodyKind::of(&HeapBody::BoxedInt(0)),
            HeapBodyKind::BoxedInt
        );
        assert_eq!(
            HeapBodyKind::of(&HeapBody::Percent(0.0)),
            HeapBodyKind::Percent
        );
        assert_eq!(
            HeapBodyKind::of(&HeapBody::Range {
                start: 0,
                end: 0,
                exclusive: false
            }),
            HeapBodyKind::Range
        );
    }

    // The legacy shim (`from_legacy` / `to_legacy` + the `LegacyValue`
    // enum) was deleted at the end of 8f. The shim's round-trip tests
    // are gone with it; the predicate/extractor tests above pin the
    // production representation.
}
