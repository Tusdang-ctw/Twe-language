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
    /// Immutable tuple. Sessions 8c–8e migrate the inner
    /// `Vec<crate::value::Value>` to `Vec<TaggedValue>`.
    Tuple(Rc<Vec<crate::value::Value>>),
    /// Mutable list. Same migration path as Tuple.
    List(Rc<RefCell<Vec<crate::value::Value>>>),
    /// Generic object — Twe stdlib's `key`, `mouse`, sprite/sound
    /// handles, save-loaded data, etc. all use this.
    Object(Rc<RefCell<crate::value::Object>>),
    /// Class / Instance / BcClass / BcInstance / Function /
    /// BcFunction / Builtin variants land in session 8c+
    /// alongside their callers' migration. Holding them here
    /// today without callers reading them would be dead code.
    Reserved8c,
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
            HeapBody::Reserved8c => panic!("HeapBody::Reserved8c is a placeholder; populate in session 8c"),
        }
    }
}

/// One heap object. `body` is the actual data; session 8g adds a
/// `header: HeapHeader` field with mark bits + body-kind cache.
/// `RefCell` keeps mutation interior so `Rc<HeapObject>` is
/// sufficient for now (real GC allocator lands in 8g).
#[derive(Debug)]
pub struct HeapObject {
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

    /// Encode an `f64`. Canonicalizes NaN to a single bit pattern
    /// so a payload that happens to look like one of our tags
    /// can't be misread as Nil/Bool/etc.
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
    /// and tags the pointer.
    pub fn from_string(s: String) -> Self {
        let rc = Rc::new(HeapObject {
            body: RefCell::new(HeapBody::String(s)),
        });
        let raw = Rc::into_raw(rc) as usize as u64;
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
    /// tag the pointer with `TAG_OBJ`. v0.2 Phase 8.5 session 8b.
    fn from_heap(body: HeapBody) -> Self {
        let rc = Rc::new(HeapObject {
            body: RefCell::new(body),
        });
        let raw = Rc::into_raw(rc) as usize as u64;
        Self(QNAN | TAG_OBJ | (raw & PAYLOAD_MASK))
    }

    pub fn from_percent(p: f64) -> Self {
        Self::from_heap(HeapBody::Percent(p))
    }

    pub fn from_quantity(value: f64, unit: Rc<String>) -> Self {
        Self::from_heap(HeapBody::Quantity { value, unit })
    }

    pub fn from_range(start: i64, end: i64, exclusive: bool) -> Self {
        Self::from_heap(HeapBody::Range { start, end, exclusive })
    }

    pub fn from_tuple(elems: Rc<Vec<crate::value::Value>>) -> Self {
        Self::from_heap(HeapBody::Tuple(elems))
    }

    pub fn from_list(elems: Rc<RefCell<Vec<crate::value::Value>>>) -> Self {
        Self::from_heap(HeapBody::List(elems))
    }

    pub fn from_object(obj: Rc<RefCell<crate::value::Object>>) -> Self {
        Self::from_heap(HeapBody::Object(obj))
    }
}

// ---------- predicates ----------

impl TaggedValue {
    /// True for any value that's NOT a regular non-NaN f64.
    /// (Tag 0 — `f64::NAN` itself — passes `is_float`.)
    fn is_tagged(&self) -> bool {
        (self.0 & QNAN) == QNAN && (self.0 & TAG_MASK) != 0
    }

    pub fn is_nil(&self) -> bool {
        self.0 == (QNAN | TAG_NIL)
    }
    pub fn is_bool(&self) -> bool {
        let tag = self.0 & TAG_MASK;
        self.is_tagged() && (tag == TAG_FALSE || tag == TAG_TRUE)
    }
    pub fn is_int(&self) -> bool {
        self.is_tagged() && (self.0 & TAG_MASK) == TAG_INT
    }
    pub fn is_float(&self) -> bool {
        // Either not-tagged at all, OR tag is 0 (canonical NaN).
        !self.is_tagged()
    }
    pub fn is_number(&self) -> bool {
        self.is_int() || self.is_float()
    }
    pub fn is_str(&self) -> bool {
        self.is_tagged() && (self.0 & TAG_MASK) == TAG_STR
    }
    pub fn is_obj(&self) -> bool {
        self.is_tagged() && (self.0 & TAG_MASK) == TAG_OBJ
    }
    /// True for any heap-allocated variant (Str / Obj). Used by
    /// `Clone` / `Drop` to know whether to bump / decrement the
    /// refcount.
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
    pub fn as_bool(&self) -> bool {
        debug_assert!(self.is_bool(), "as_bool on non-bool");
        (self.0 & TAG_MASK) == TAG_TRUE
    }

    /// Read an int-typed value, whether immediate (i48) or
    /// boxed (i64). Callers should pre-test with
    /// `is_int_or_boxed_int()` to know it's safe.
    pub fn as_int(&self) -> i64 {
        if self.is_int() {
            // Sign-extend the 48-bit payload to i64.
            let payload = self.0 & PAYLOAD_MASK;
            return if payload & (1 << 47) != 0 {
                (payload | !PAYLOAD_MASK) as i64
            } else {
                payload as i64
            };
        }
        if self.is_obj() {
            return self.with_obj_body(|b| match b {
                HeapBody::BoxedInt(n) => *n,
                other => panic!("as_int on non-int heap body: {other:?}"),
            });
        }
        panic!("as_int on non-int value")
    }

    /// True for either the i48 immediate path or the
    /// `HeapBody::BoxedInt` variant. Callers that want "is this
    /// an integer regardless of representation" should use this
    /// rather than `is_int` (which is only the fast path).
    pub fn is_int_or_boxed_int(&self) -> bool {
        if self.is_int() {
            return true;
        }
        if self.is_obj() {
            return self.is_obj_body_kind(HeapBodyKind::BoxedInt);
        }
        false
    }

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

    /// Borrow the heap object behind a pointer-tagged value.
    /// The closure runs while the `Rc` is alive; the refcount
    /// stays balanced. **The caller's closure must not hold
    /// references past return** — once `with_heap_object`
    /// returns, the `Rc` may drop.
    fn with_heap_object<R>(&self, f: impl FnOnce(&HeapObject) -> R) -> R {
        debug_assert!(self.is_heap(), "with_heap_object on non-heap value");
        let raw = (self.0 & PAYLOAD_MASK) as usize as *const HeapObject;
        // SAFETY: pointer was produced by `Rc::into_raw` and the
        // refcount-balance invariants of `TaggedValue` keep it
        // valid. We `increment_strong_count` to take a temporary
        // share, then `Rc::from_raw` consumes that share when the
        // function returns.
        unsafe {
            Rc::increment_strong_count(raw);
            let rc: Rc<HeapObject> = Rc::from_raw(raw);
            f(&rc)
        }
    }
}

// ---------- Clone + Drop (refcount management) ----------

impl Clone for TaggedValue {
    fn clone(&self) -> Self {
        if self.is_heap() {
            let raw = (self.0 & PAYLOAD_MASK) as usize as *const HeapObject;
            // SAFETY: pointer was produced by Rc::into_raw and is
            // valid for the lifetime of the original TaggedValue.
            // increment_strong_count takes one extra share; the
            // matching decrement runs in `Drop`.
            unsafe {
                Rc::increment_strong_count(raw);
            }
        }
        Self(self.0)
    }
}

impl Drop for TaggedValue {
    fn drop(&mut self) {
        if self.is_heap() {
            let raw = (self.0 & PAYLOAD_MASK) as usize as *const HeapObject;
            // SAFETY: every TaggedValue holds exactly one share
            // of the inner Rc; reconstructing + dropping
            // `Rc::from_raw` here decrements the refcount.
            unsafe {
                let _drop_one_share: Rc<HeapObject> = Rc::from_raw(raw);
            }
        }
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

// ---------- legacy-Value shim ----------
//
// Migration glue per `docs/08-nan-tagging.md` "API shape". Lets
// callers convert at the boundary while interiors stay on the
// legacy `crate::value::Value` enum. As migration progresses
// (8c–8e), conversions migrate inward; in 8f the shim deletes.

impl TaggedValue {
    /// Convert from the legacy `crate::value::Value` enum. Only
    /// the variants this module covers (Nil / Bool / Int / Float
    /// / Str) round-trip cleanly in session 8a; everything else
    /// is mapped to a placeholder tagged-Object pointing at a
    /// `HeapBody::String` carrying the type name. Session 8b
    /// expands `HeapBody` to cover the full Value surface.
    pub fn from_legacy(v: &crate::value::Value) -> Self {
        use crate::value::Value;
        match v {
            Value::Nil => Self::NIL,
            Value::Bool(b) => Self::from_bool(*b),
            Value::Int(n) => Self::from_int(*n),
            Value::Float(f) => Self::from_float(*f),
            Value::Str(rc) => Self::from_string((**rc).clone()),
            // Placeholder for not-yet-supported variants.
            // Session 8b expands HeapBody and routes these
            // through their actual heap representations.
            other => Self::from_string(format!("<unsupported-in-8a: {}>", other.type_name())),
        }
    }

    /// Convert back to the legacy `crate::value::Value` enum.
    /// Lossy in the same direction — placeholder strings round-
    /// trip back as plain `Value::Str`. Session 8b adds the
    /// real conversions.
    pub fn to_legacy(&self) -> crate::value::Value {
        use crate::value::Value;
        if self.is_nil() {
            Value::Nil
        } else if self.is_bool() {
            Value::Bool(self.as_bool())
        } else if self.is_int() {
            Value::Int(self.as_int())
        } else if self.is_float() {
            Value::Float(self.as_float())
        } else if self.is_str() {
            Value::Str(Rc::new(self.as_string()))
        } else {
            // Placeholder: real Object/Tuple/etc. conversions
            // land in session 8b. For now, fall back to Nil so
            // tests that exercise the shim get a deterministic
            // output rather than a panic.
            Value::Nil
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
        assert_eq!(t.as_bool(), true);
        assert_eq!(f.as_bool(), false);
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
        for f in [0.0_f64, 1.0, -1.0, 3.14, 1e100, -1e-100, f64::INFINITY, f64::NEG_INFINITY] {
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
    fn clone_bumps_refcount_and_drop_balances() {
        let v1 = TaggedValue::from_borrowed_str("shared");
        let v2 = v1.clone();
        let v3 = v1.clone();
        // All three see the same string.
        assert_eq!(v1.as_string(), "shared");
        assert_eq!(v2.as_string(), "shared");
        assert_eq!(v3.as_string(), "shared");
        // Drop two of the three. The third must still read.
        drop(v2);
        drop(v3);
        assert_eq!(v1.as_string(), "shared");
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
        assert!(TaggedValue::from_float(3.14).is_number());
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
            HeapBody::Range { start, end, exclusive } => {
                assert_eq!(*start, 0);
                assert_eq!(*end, 10);
                assert!(*exclusive);
            }
            other => panic!("expected Range, got {other:?}"),
        });
    }

    #[test]
    fn tuple_round_trip_holds_legacy_values() {
        use crate::value::Value;
        let elems = Rc::new(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let v = TaggedValue::from_tuple(elems);
        assert!(v.is_obj_body_kind(HeapBodyKind::Tuple));
        v.with_obj_body(|b| match b {
            HeapBody::Tuple(rc) => {
                assert_eq!(rc.len(), 3);
                assert!(matches!(rc[0], Value::Int(1)));
            }
            other => panic!("expected Tuple, got {other:?}"),
        });
    }

    #[test]
    fn list_round_trip_holds_mutable_legacy_values() {
        use crate::value::Value;
        let inner = Rc::new(RefCell::new(vec![Value::Int(7), Value::Int(8)]));
        let v = TaggedValue::from_list(inner.clone());
        assert!(v.is_obj_body_kind(HeapBodyKind::List));
        // Mutate through the original Rc; the TaggedValue should
        // see the update because List shares the inner Rc.
        inner.borrow_mut().push(Value::Int(9));
        v.with_obj_body(|b| match b {
            HeapBody::List(rc) => assert_eq!(rc.borrow().len(), 3),
            other => panic!("expected List, got {other:?}"),
        });
    }

    #[test]
    fn object_round_trip_carries_kind_string() {
        use std::collections::HashMap;
        let mut fields = HashMap::new();
        fields.insert("hp".to_string(), crate::value::Value::Int(100));
        let obj = Rc::new(RefCell::new(crate::value::Object {
            fields,
            kind: "test",
        }));
        let v = TaggedValue::from_object(obj);
        assert!(v.is_obj_body_kind(HeapBodyKind::Object));
        v.with_obj_body(|b| match b {
            HeapBody::Object(rc) => {
                let o = rc.borrow();
                assert_eq!(o.kind, "test");
                assert!(matches!(o.fields.get("hp"), Some(crate::value::Value::Int(100))));
            }
            other => panic!("expected Object, got {other:?}"),
        });
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

    #[test]
    fn legacy_shim_round_trips_primitives() {
        use crate::value::Value;
        let cases: Vec<Value> = vec![
            Value::Nil,
            Value::Bool(true),
            Value::Bool(false),
            Value::Int(123),
            Value::Float(0.5),
            Value::Str(Rc::new("hi".to_string())),
        ];
        for legacy in cases {
            let tagged = TaggedValue::from_legacy(&legacy);
            let back = tagged.to_legacy();
            // Compare via Debug-string since Value isn't Eq.
            assert_eq!(format!("{legacy:?}"), format!("{back:?}"));
        }
    }
}
