//! Type representation for the Twe type system (Phase 4).
//!
//! v0.1 ships **non-strict mode only** per `docs/02-type-system.md`.
//! The non-strict philosophy is "no false positives": when we
//! can't prove a type, we say `Unknown` and stay quiet rather
//! than risk being wrong. Strict-mode soundness lands in v0.2;
//! verified-mode JSON diagnostics land in v0.3.
//!
//! This file is the foundation: the algebraic data type the
//! rest of the type system reasons over. Inference (`src/infer.rs`)
//! produces these values; LSP hover (`src/lsp.rs`) renders them;
//! a future strict pass will compare them at function boundaries.
//!
//! Phase 4a (this commit) is intentionally tight: built-in
//! scalar / container types only. Type variables, generics,
//! tagged unions, structural records, and dimensional units
//! arrive in 4b through 4f.

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

/// Unique identifier for a fresh type variable. Allocated by
/// `TypeVarGen` so collisions are impossible across an inference
/// session. Wraps a u32 to keep the value cheap to clone — a
/// substitution map keyed on this is the hot path during unify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeVarId(pub u32);

/// Allocator for fresh type variables. One per inference session.
/// IDs start at 0 and increment monotonically.
#[derive(Debug, Default)]
pub struct TypeVarGen {
    next: u32,
}

impl TypeVarGen {
    pub fn new() -> Self {
        Self { next: 0 }
    }

    /// Allocate a fresh type variable.
    pub fn fresh(&mut self) -> TypeVarId {
        let id = self.next;
        self.next += 1;
        TypeVarId(id)
    }
}

/// A substitution: a map from type-variable IDs to the types
/// they've been resolved to. The unification algorithm extends
/// this incrementally; `apply_subst` walks a type and replaces
/// any `Var(id)` with its current binding (recursively, until
/// a non-var is hit).
#[derive(Debug, Default, Clone)]
pub struct Substitution {
    map: HashMap<TypeVarId, Type>,
}

impl Substitution {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lookup(&self, id: TypeVarId) -> Option<&Type> {
        self.map.get(&id)
    }

    pub fn insert(&mut self, id: TypeVarId, t: Type) {
        self.map.insert(id, t);
    }

    /// True when no constraints have been recorded — useful for
    /// tests + sanity checks.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// Walk `t` and replace every `Var(id)` for which the
/// substitution has a binding. Resolves transitive chains
/// (`Var(α) -> Var(β) -> Int` becomes `Int`). Returns a new
/// owned type — does not mutate input.
pub fn apply_subst(subst: &Substitution, t: &Type) -> Type {
    match t {
        Type::Var(id) => match subst.lookup(*id) {
            Some(resolved) => apply_subst(subst, resolved),
            None => Type::Var(*id),
        },
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| apply_subst(subst, e)).collect()),
        Type::List(elem) => Type::List(Rc::new(apply_subst(subst, elem))),
        Type::Function { params, ret } => Type::Function {
            params: params.iter().map(|p| apply_subst(subst, p)).collect(),
            ret: Rc::new(apply_subst(subst, ret)),
        },
        Type::Optional(inner) => Type::Optional(Rc::new(apply_subst(subst, inner))),
        Type::Union(parts) => Type::Union(parts.iter().map(|p| apply_subst(subst, p)).collect()),
        // Scalar / nominal types have nothing to substitute.
        _ => t.clone(),
    }
}

/// Unification error — when two types can't be made equal under
/// any substitution. Carried in `Result` so the caller can decide
/// whether to surface (strict mode) or absorb (non-strict mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifyError {
    /// `a` and `b` aren't equal and aren't unifiable. Both sides
    /// already have their substitutions applied when the error is
    /// constructed, so the message reflects what the caller saw.
    Mismatch { a: String, b: String },
    /// Occurs check: trying to unify `α = T(α)` would create an
    /// infinite type. HM rejects this; without the check, `unify`
    /// would loop forever in `apply_subst`.
    OccursCheck { var: TypeVarId, ty: String },
}

impl fmt::Display for UnifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnifyError::Mismatch { a, b } => write!(f, "type mismatch: {a} vs {b}"),
            UnifyError::OccursCheck { var, ty } => {
                write!(f, "infinite type: {var:?} occurs in {ty}")
            }
        }
    }
}

/// Unify two types under `subst`, extending `subst` so the two
/// become equal. Per Robinson's algorithm:
///
/// - Two equal scalars: succeed (no extension).
/// - `Var(α)` vs `T`: bind `α := T` after occurs check.
/// - Two `Var`s: bind one to the other.
/// - Compound types (`Tuple`, `List`, `Function`): recurse into
///   structurally matching positions; arity mismatch is a
///   `Mismatch` error.
/// - `Unknown` unifies with anything (the non-strict escape
///   hatch — see is_compatible_with's comment).
/// - Anything else: `Mismatch`.
///
/// On error, `subst` may have been partially extended; HM
/// implementations vary on whether this matters. For Twe's
/// non-strict mode the caller treats errors as "give up and
/// stay Unknown," so partial extension is harmless.
pub fn unify(a: &Type, b: &Type, subst: &mut Substitution) -> Result<(), UnifyError> {
    // Walk through any existing substitutions first so we're
    // comparing the most-resolved forms.
    let a = apply_subst(subst, a);
    let b = apply_subst(subst, b);
    match (&a, &b) {
        // Unknown is the non-strict bottom — never the source of
        // an error message.
        (Type::Unknown, _) | (_, Type::Unknown) => Ok(()),
        // Variable cases.
        (Type::Var(id1), Type::Var(id2)) if id1 == id2 => Ok(()),
        (Type::Var(id), other) | (other, Type::Var(id)) => {
            if occurs_in(*id, other) {
                Err(UnifyError::OccursCheck {
                    var: *id,
                    ty: other.to_string(),
                })
            } else {
                subst.insert(*id, other.clone());
                Ok(())
            }
        }
        // Equal scalars / nominals.
        (Type::Int, Type::Int)
        | (Type::Float, Type::Float)
        | (Type::Bool, Type::Bool)
        | (Type::Str, Type::Str)
        | (Type::Nil, Type::Nil)
        | (Type::Percent, Type::Percent)
        | (Type::Range, Type::Range) => Ok(()),
        (Type::Quantity(u1), Type::Quantity(u2)) if u1 == u2 => Ok(()),
        (Type::Instance(a), Type::Instance(b)) | (Type::Class(a), Type::Class(b)) if a == b => {
            Ok(())
        }
        // Compound types: recurse.
        (Type::Tuple(a), Type::Tuple(b)) => {
            if a.len() != b.len() {
                return Err(mismatch(&Type::Tuple(a.clone()), &Type::Tuple(b.clone())));
            }
            for (x, y) in a.iter().zip(b.iter()) {
                unify(x, y, subst)?;
            }
            Ok(())
        }
        (Type::List(a), Type::List(b)) => unify(a, b, subst),
        // Optional unifies with its inner (Some case) or with
        // Nil (None case); two Optionals unify on their inners.
        (Type::Optional(a), Type::Optional(b)) => unify(a, b, subst),
        (Type::Optional(inner), Type::Nil) | (Type::Nil, Type::Optional(inner)) => {
            // Nil is one valid inhabitant of T?; succeed without
            // constraining the inner.
            let _ = inner;
            Ok(())
        }
        (Type::Optional(inner), other) | (other, Type::Optional(inner)) => {
            // Unifying T? with a concrete T pins the Some case.
            unify(inner, other, subst)
        }
        // Two Unions unify if they have the same variants (in
        // any order). Anything else with a Union: succeed if the
        // other unifies with at least one variant. We don't
        // permanently extend the substitution from the trial
        // (would need backtracking), so we just check the first
        // working variant — a heuristic that's right when
        // unions are small and disjoint.
        (Type::Union(a), Type::Union(b)) => {
            if a.len() != b.len() {
                return Err(mismatch(&Type::Union(a.clone()), &Type::Union(b.clone())));
            }
            // Simplest correct check: pairwise positional unify.
            // Real union unification requires permutation matching
            // (research-grade); strict mode v0.2 will tighten this.
            for (x, y) in a.iter().zip(b.iter()) {
                unify(x, y, subst)?;
            }
            Ok(())
        }
        (Type::Union(parts), other) | (other, Type::Union(parts)) => {
            // Trial each variant; first one that unifies wins.
            // We snapshot the substitution so a failed trial
            // doesn't pollute the solver state.
            for variant in parts {
                let mut trial = subst.clone();
                if unify(variant, other, &mut trial).is_ok() {
                    *subst = trial;
                    return Ok(());
                }
            }
            Err(mismatch(&Type::Union(parts.clone()), other))
        }
        (
            Type::Function {
                params: ap,
                ret: ar,
            },
            Type::Function {
                params: bp,
                ret: br,
            },
        ) => {
            if ap.len() != bp.len() {
                return Err(mismatch(&a, &b));
            }
            for (x, y) in ap.iter().zip(bp.iter()) {
                unify(x, y, subst)?;
            }
            unify(ar, br, subst)
        }
        _ => Err(mismatch(&a, &b)),
    }
}

fn mismatch(a: &Type, b: &Type) -> UnifyError {
    UnifyError::Mismatch {
        a: a.to_string(),
        b: b.to_string(),
    }
}

/// Occurs check: does `id` appear anywhere inside `t`? Used
/// before binding `α := T` so we never construct an infinite
/// type like `α = list of α`.
fn occurs_in(id: TypeVarId, t: &Type) -> bool {
    match t {
        Type::Var(other) => *other == id,
        Type::Tuple(elems) => elems.iter().any(|e| occurs_in(id, e)),
        Type::List(elem) => occurs_in(id, elem),
        Type::Function { params, ret } => {
            params.iter().any(|p| occurs_in(id, p)) || occurs_in(id, ret)
        }
        Type::Optional(inner) => occurs_in(id, inner),
        Type::Union(parts) => parts.iter().any(|p| occurs_in(id, p)),
        _ => false,
    }
}

/// One Twe type. `Unknown` is the lattice bottom — it means
/// "we couldn't prove anything, defer to runtime." Per non-strict
/// semantics, `Unknown` is treated as compatible with everything;
/// strict mode (v0.2) will reject it where annotations are
/// required.
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// `int` — i64. Distinct from `float` (no implicit narrowing).
    Int,
    /// `float` — f64. Mixed-precision arithmetic auto-promotes
    /// to float per `docs/02 §Built-in types`.
    Float,
    /// `bool` — only `true` / `false`. Per Principle 3, only
    /// `false` is falsy at runtime, but the *type* `bool` covers
    /// just the two literal values.
    Bool,
    /// `string` — UTF-8. Indexing returns a grapheme cluster.
    Str,
    /// `nil` — explicit absence. `nil` is its own type, not a
    /// member of every other type. Use `T?` (Optional) for
    /// "T or nil".
    Nil,
    /// `n%` literals, distinct from float so the printer + future
    /// strict pass can validate percent-only call sites.
    Percent,
    /// Dimensional quantity carrying a unit string. Phase 4f
    /// will promote this to a structured `Quantity { dim, unit }`
    /// with full `length / duration → velocity` checks; the
    /// initial pass just records the unit so we can echo it back.
    Quantity(Rc<String>),
    /// `range` — `start..end` or `start..<end`. Always Int-ranged
    /// in v0.1 per the lexer + tree-walker semantics.
    Range,
    /// Tuple. Element count is part of the type; element types
    /// are positional. `(int, str)` is distinct from `(str, int)`.
    Tuple(Vec<Type>),
    /// Heterogeneous in v0.1 (the runtime allows mixed-type
    /// lists). Carries the element type when the list literal is
    /// homogeneous; otherwise `Unknown`.
    List(Rc<Type>),
    /// Function type with parameter types + return type. Param
    /// names are deliberately not part of the type — keyword
    /// arguments are bound at call time, not in the type.
    Function { params: Vec<Type>, ret: Rc<Type> },
    /// Class instance type — carries the class name. Field types
    /// arrive in Phase 4d when structural records land; for now
    /// we just record which class and let field access return
    /// `Unknown` until the runtime checks.
    Instance(Rc<String>),
    /// Class itself (the value bound by `entity X:` etc.).
    /// Calling it returns an `Instance`.
    Class(Rc<String>),
    /// Bottom of the lattice. Means "we don't know" or "we
    /// declined to compute." Per non-strict mode, `Unknown`
    /// silently absorbs any operation — never the source of an
    /// error message.
    Unknown,
    /// Type variable — a placeholder filled in by unification.
    /// Allocated by `TypeVarGen::fresh`. Resolved through a
    /// `Substitution` via `apply_subst`.
    Var(TypeVarId),
    /// `T?` — shorthand for `T | nil`. Surfaces from inference
    /// when a function body can return both `nil` and a
    /// concrete value, or when the user writes the `?` suffix
    /// in a type annotation (Phase 4f+ parser work).
    Optional(Rc<Type>),
    /// `T | U | ...` — open sum. Surfaces from inference when
    /// a function body returns multiple distinct types. Stored
    /// in source order; equality is order-sensitive (canonical
    /// form is the responsibility of the constructor).
    Union(Vec<Type>),
}

impl Type {
    /// Convenience constructor for a function type.
    pub fn func(params: Vec<Type>, ret: Type) -> Type {
        Type::Function {
            params,
            ret: Rc::new(ret),
        }
    }

    /// Convenience constructor for a list type.
    pub fn list(elem: Type) -> Type {
        Type::List(Rc::new(elem))
    }

    /// Convenience constructor for an optional type. Idempotent
    /// over `Optional` (`T??` collapses to `T?`) and absorbing
    /// into `Nil` (`Optional<Nil>` becomes `Nil`).
    pub fn optional(inner: Type) -> Type {
        match inner {
            Type::Optional(_) => inner,
            Type::Nil => Type::Nil,
            other => Type::Optional(Rc::new(other)),
        }
    }

    /// Build a union from a list of variants. Deduplicates,
    /// flattens nested unions, normalises `T | nil` to `T?`.
    /// Returns the singleton type when only one variant remains.
    pub fn union(parts: impl IntoIterator<Item = Type>) -> Type {
        // Step 1: flatten nested Unions and collect uniques in
        // first-seen order. Equality is structural via PartialEq.
        let mut flat: Vec<Type> = Vec::new();
        let mut has_nil = false;
        for t in parts {
            match t {
                Type::Union(inner) => {
                    for x in inner {
                        if matches!(x, Type::Nil) {
                            has_nil = true;
                            continue;
                        }
                        if !flat.iter().any(|y| y == &x) {
                            flat.push(x);
                        }
                    }
                }
                Type::Nil => {
                    has_nil = true;
                }
                Type::Optional(inner) => {
                    has_nil = true;
                    let val = (*inner).clone();
                    if !flat.iter().any(|y| y == &val) {
                        flat.push(val);
                    }
                }
                t => {
                    if !flat.iter().any(|y| y == &t) {
                        flat.push(t);
                    }
                }
            }
        }
        // Step 2: collapse to canonical form.
        match (flat.len(), has_nil) {
            (0, true) => Type::Nil,
            (0, false) => Type::Unknown,
            (1, true) => Type::optional(flat.into_iter().next().unwrap()),
            (1, false) => flat.into_iter().next().unwrap(),
            (_, false) => Type::Union(flat),
            (_, true) => Type::optional(Type::Union(flat)),
        }
    }

    /// Whether this type is `Unknown`. Hot path; avoid the
    /// allocator. Used by inference to decide when to fall back
    /// to deferred typing.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Type::Unknown)
    }

    /// Is `other` compatible with `self` under non-strict rules?
    ///
    /// `Unknown` is compatible with everything (in both
    /// directions) — the "no false positives" guarantee. Two
    /// concrete types are compatible only if they're equal.
    /// Tuple compatibility is element-wise. List compatibility
    /// requires the element types to match (`Unknown` either
    /// side passes).
    ///
    /// This is the relation strict mode (v0.2) will use as its
    /// default check; for v0.1 it's only consulted by tests +
    /// the LSP hover printer.
    pub fn is_compatible_with(&self, other: &Type) -> bool {
        if self.is_unknown() || other.is_unknown() {
            return true;
        }
        match (self, other) {
            (Type::Int, Type::Int)
            | (Type::Float, Type::Float)
            | (Type::Bool, Type::Bool)
            | (Type::Str, Type::Str)
            | (Type::Nil, Type::Nil)
            | (Type::Percent, Type::Percent)
            | (Type::Range, Type::Range) => true,
            (Type::Quantity(a), Type::Quantity(b)) => a == b,
            (Type::Tuple(a), Type::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.is_compatible_with(y))
            }
            (Type::List(a), Type::List(b)) => a.is_compatible_with(b),
            (
                Type::Function {
                    params: ap,
                    ret: ar,
                },
                Type::Function {
                    params: bp,
                    ret: br,
                },
            ) => {
                ap.len() == bp.len()
                    && ap
                        .iter()
                        .zip(bp.iter())
                        .all(|(x, y)| x.is_compatible_with(y))
                    && ar.is_compatible_with(br)
            }
            (Type::Instance(a), Type::Instance(b)) => a == b,
            (Type::Class(a), Type::Class(b)) => a == b,
            // Optional: T? compatible with T (Some) and Nil (None);
            // two Optionals compatible if their inners are.
            (Type::Optional(a), Type::Optional(b)) => a.is_compatible_with(b),
            (Type::Optional(inner), other) | (other, Type::Optional(inner)) => {
                matches!(other, Type::Nil) || inner.is_compatible_with(other)
            }
            // Union: a value is compatible with a union if it
            // matches any variant. Two unions compatible if every
            // variant of one is compatible with some variant of
            // the other.
            (Type::Union(a), Type::Union(b)) => {
                a.iter().all(|x| b.iter().any(|y| x.is_compatible_with(y)))
                    && b.iter().all(|y| a.iter().any(|x| y.is_compatible_with(x)))
            }
            (Type::Union(parts), other) | (other, Type::Union(parts)) => {
                parts.iter().any(|p| p.is_compatible_with(other))
            }
            _ => false,
        }
    }
}

impl fmt::Display for Type {
    /// Pretty-print a type the way it would appear in a hover
    /// tooltip. Matches Twe's source syntax where possible
    /// (`(int, str)` for tuples, `int[]` for lists, `T?` for
    /// optionals once Optional lands).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Int => write!(f, "int"),
            Type::Float => write!(f, "float"),
            Type::Bool => write!(f, "bool"),
            Type::Str => write!(f, "string"),
            Type::Nil => write!(f, "nil"),
            Type::Percent => write!(f, "percent"),
            Type::Quantity(unit) => write!(f, "quantity<{unit}>"),
            Type::Range => write!(f, "range"),
            Type::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{e}")?;
                }
                if elems.len() == 1 {
                    write!(f, ",")?; // distinguish from parens
                }
                write!(f, ")")
            }
            Type::List(elem) => write!(f, "{elem}[]"),
            Type::Function { params, ret } => {
                write!(f, "function(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {ret}")
            }
            Type::Instance(name) => write!(f, "{name}"),
            Type::Class(name) => write!(f, "<class {name}>"),
            Type::Unknown => write!(f, "?"),
            Type::Var(id) => write!(f, "?{}", id.0),
            Type::Optional(inner) => match inner.as_ref() {
                // Don't double-print `??` for `Optional<Optional<T>>` —
                // emit `T??` so the source intent is clear.
                Type::Optional(_) => write!(f, "{inner}?"),
                // `(int | str)?` needs the parens to bind the `?`
                // tightly; bare `int | str?` would parse as
                // `int | (str?)` in a future strict-mode parser.
                Type::Union(_) => write!(f, "({inner})?"),
                _ => write!(f, "{inner}?"),
            },
            Type::Union(parts) => {
                if parts.is_empty() {
                    return write!(f, "?");
                }
                for (i, p) in parts.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    // Wrap nested Unions in parens so the
                    // grouping reads correctly.
                    match p {
                        Type::Union(_) => write!(f, "({p})")?,
                        _ => write!(f, "{p}")?,
                    }
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_scalars() {
        assert_eq!(Type::Int.to_string(), "int");
        assert_eq!(Type::Float.to_string(), "float");
        assert_eq!(Type::Bool.to_string(), "bool");
        assert_eq!(Type::Str.to_string(), "string");
        assert_eq!(Type::Nil.to_string(), "nil");
        assert_eq!(Type::Percent.to_string(), "percent");
        assert_eq!(Type::Range.to_string(), "range");
        assert_eq!(Type::Unknown.to_string(), "?");
    }

    #[test]
    fn display_quantity_carries_unit() {
        assert_eq!(
            Type::Quantity(Rc::new("ms".into())).to_string(),
            "quantity<ms>"
        );
        assert_eq!(
            Type::Quantity(Rc::new("kg".into())).to_string(),
            "quantity<kg>"
        );
    }

    #[test]
    fn display_tuple() {
        assert_eq!(
            Type::Tuple(vec![Type::Int, Type::Str]).to_string(),
            "(int, string)",
        );
        // Single-element tuple gets a trailing comma to match
        // source syntax `(x,)`.
        assert_eq!(Type::Tuple(vec![Type::Int]).to_string(), "(int,)");
    }

    #[test]
    fn display_list() {
        assert_eq!(Type::list(Type::Int).to_string(), "int[]");
        assert_eq!(Type::list(Type::Unknown).to_string(), "?[]");
    }

    #[test]
    fn display_function() {
        let t = Type::func(vec![Type::Int, Type::Int], Type::Int);
        assert_eq!(t.to_string(), "function(int, int) -> int");
        let t = Type::func(vec![], Type::Nil);
        assert_eq!(t.to_string(), "function() -> nil");
    }

    #[test]
    fn display_instance_and_class() {
        assert_eq!(Type::Instance(Rc::new("Hero".into())).to_string(), "Hero");
        assert_eq!(
            Type::Class(Rc::new("Hero".into())).to_string(),
            "<class Hero>"
        );
    }

    #[test]
    fn unknown_is_compatible_with_everything() {
        assert!(Type::Unknown.is_compatible_with(&Type::Int));
        assert!(Type::Int.is_compatible_with(&Type::Unknown));
        assert!(Type::Unknown.is_compatible_with(&Type::Unknown));
        assert!(Type::Unknown.is_compatible_with(&Type::list(Type::Str)));
    }

    #[test]
    fn concrete_types_only_match_themselves() {
        assert!(Type::Int.is_compatible_with(&Type::Int));
        assert!(!Type::Int.is_compatible_with(&Type::Float));
        assert!(!Type::Str.is_compatible_with(&Type::Bool));
    }

    #[test]
    fn tuple_compat_is_elementwise_with_unknown_holes() {
        let a = Type::Tuple(vec![Type::Int, Type::Str]);
        let b = Type::Tuple(vec![Type::Int, Type::Str]);
        let c = Type::Tuple(vec![Type::Int, Type::Unknown]);
        let d = Type::Tuple(vec![Type::Int, Type::Bool]);
        let e = Type::Tuple(vec![Type::Int]);
        assert!(a.is_compatible_with(&b));
        assert!(a.is_compatible_with(&c));
        assert!(c.is_compatible_with(&a));
        assert!(!a.is_compatible_with(&d));
        assert!(!a.is_compatible_with(&e));
    }

    #[test]
    fn list_compat_walks_into_element() {
        assert!(Type::list(Type::Int).is_compatible_with(&Type::list(Type::Int)));
        assert!(!Type::list(Type::Int).is_compatible_with(&Type::list(Type::Str)));
        assert!(Type::list(Type::Int).is_compatible_with(&Type::list(Type::Unknown)));
    }

    #[test]
    fn function_compat_checks_arity_and_components() {
        let f1 = Type::func(vec![Type::Int, Type::Int], Type::Int);
        let f2 = Type::func(vec![Type::Int, Type::Int], Type::Int);
        let f3 = Type::func(vec![Type::Int, Type::Int], Type::Bool); // diff ret
        let f4 = Type::func(vec![Type::Int], Type::Int); // diff arity
        assert!(f1.is_compatible_with(&f2));
        assert!(!f1.is_compatible_with(&f3));
        assert!(!f1.is_compatible_with(&f4));
    }

    #[test]
    fn quantity_compat_requires_same_unit() {
        let ms = Type::Quantity(Rc::new("ms".into()));
        let s = Type::Quantity(Rc::new("s".into()));
        assert!(ms.is_compatible_with(&Type::Quantity(Rc::new("ms".into()))));
        assert!(!ms.is_compatible_with(&s));
    }

    // --- 4b: type variables + unification ---

    #[test]
    fn fresh_vars_have_unique_ids() {
        let mut g = TypeVarGen::new();
        let a = g.fresh();
        let b = g.fresh();
        let c = g.fresh();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn var_displays_with_question_prefix() {
        let id = TypeVarId(7);
        assert_eq!(Type::Var(id).to_string(), "?7");
    }

    #[test]
    fn apply_subst_resolves_a_var() {
        let mut s = Substitution::new();
        let a = TypeVarId(0);
        s.insert(a, Type::Int);
        assert_eq!(apply_subst(&s, &Type::Var(a)), Type::Int);
    }

    #[test]
    fn apply_subst_resolves_chains() {
        let mut s = Substitution::new();
        let a = TypeVarId(0);
        let b = TypeVarId(1);
        s.insert(a, Type::Var(b));
        s.insert(b, Type::Str);
        assert_eq!(apply_subst(&s, &Type::Var(a)), Type::Str);
    }

    #[test]
    fn apply_subst_walks_into_compounds() {
        let mut s = Substitution::new();
        let a = TypeVarId(0);
        s.insert(a, Type::Int);
        let t = Type::list(Type::Var(a));
        assert_eq!(apply_subst(&s, &t), Type::list(Type::Int));
        let t = Type::Tuple(vec![Type::Var(a), Type::Bool]);
        assert_eq!(
            apply_subst(&s, &t),
            Type::Tuple(vec![Type::Int, Type::Bool])
        );
    }

    #[test]
    fn unify_equal_scalars_does_nothing() {
        let mut s = Substitution::new();
        unify(&Type::Int, &Type::Int, &mut s).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn unify_var_with_concrete_binds_it() {
        let mut s = Substitution::new();
        let a = TypeVarId(0);
        unify(&Type::Var(a), &Type::Int, &mut s).unwrap();
        assert_eq!(apply_subst(&s, &Type::Var(a)), Type::Int);
        // And the reverse direction works the same way.
        let mut s = Substitution::new();
        unify(&Type::Float, &Type::Var(a), &mut s).unwrap();
        assert_eq!(apply_subst(&s, &Type::Var(a)), Type::Float);
    }

    #[test]
    fn unify_two_vars_links_them() {
        let mut s = Substitution::new();
        let a = TypeVarId(0);
        let b = TypeVarId(1);
        unify(&Type::Var(a), &Type::Var(b), &mut s).unwrap();
        // After unification, both should resolve to whatever
        // either later binds to.
        unify(&Type::Var(b), &Type::Bool, &mut s).unwrap();
        assert_eq!(apply_subst(&s, &Type::Var(a)), Type::Bool);
        assert_eq!(apply_subst(&s, &Type::Var(b)), Type::Bool);
    }

    #[test]
    fn unify_fails_on_concrete_mismatch() {
        let mut s = Substitution::new();
        let err = unify(&Type::Int, &Type::Str, &mut s).expect_err("should fail");
        match err {
            UnifyError::Mismatch { a, b } => {
                assert_eq!(a, "int");
                assert_eq!(b, "string");
            }
            _ => panic!("wrong error variant: {err:?}"),
        }
    }

    #[test]
    fn unify_recurses_into_tuples() {
        let mut s = Substitution::new();
        let a = TypeVarId(0);
        let b = TypeVarId(1);
        let t1 = Type::Tuple(vec![Type::Var(a), Type::Var(b)]);
        let t2 = Type::Tuple(vec![Type::Int, Type::Str]);
        unify(&t1, &t2, &mut s).unwrap();
        assert_eq!(apply_subst(&s, &Type::Var(a)), Type::Int);
        assert_eq!(apply_subst(&s, &Type::Var(b)), Type::Str);
    }

    #[test]
    fn unify_tuple_arity_mismatch_errors() {
        let mut s = Substitution::new();
        let t1 = Type::Tuple(vec![Type::Int, Type::Int]);
        let t2 = Type::Tuple(vec![Type::Int]);
        assert!(unify(&t1, &t2, &mut s).is_err());
    }

    #[test]
    fn unify_lists_recurse() {
        let mut s = Substitution::new();
        let a = TypeVarId(0);
        unify(&Type::list(Type::Var(a)), &Type::list(Type::Float), &mut s).unwrap();
        assert_eq!(apply_subst(&s, &Type::Var(a)), Type::Float);
    }

    #[test]
    fn unify_function_signatures() {
        let mut s = Substitution::new();
        let p = TypeVarId(0);
        let r = TypeVarId(1);
        let f1 = Type::func(vec![Type::Var(p)], Type::Var(r));
        let f2 = Type::func(vec![Type::Int], Type::Bool);
        unify(&f1, &f2, &mut s).unwrap();
        assert_eq!(apply_subst(&s, &Type::Var(p)), Type::Int);
        assert_eq!(apply_subst(&s, &Type::Var(r)), Type::Bool);
    }

    #[test]
    fn unify_function_arity_mismatch_errors() {
        let mut s = Substitution::new();
        let f1 = Type::func(vec![Type::Int, Type::Int], Type::Int);
        let f2 = Type::func(vec![Type::Int], Type::Int);
        assert!(unify(&f1, &f2, &mut s).is_err());
    }

    #[test]
    fn unify_unknown_with_anything_succeeds() {
        // Non-strict's escape hatch: Unknown unifies with
        // anything (in either direction), no constraint added.
        let mut s = Substitution::new();
        unify(&Type::Unknown, &Type::Int, &mut s).unwrap();
        assert!(s.is_empty());
        unify(&Type::list(Type::Str), &Type::Unknown, &mut s).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn occurs_check_rejects_infinite_types() {
        // α = list of α — would create an infinite type.
        let mut s = Substitution::new();
        let a = TypeVarId(0);
        let err = unify(&Type::Var(a), &Type::list(Type::Var(a)), &mut s).expect_err("should fail");
        match err {
            UnifyError::OccursCheck { var, .. } => assert_eq!(var, a),
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn unify_resolves_through_existing_bindings() {
        // α := Int already; unify α with Int again → ok (idempotent).
        // unify α with Str → mismatch (Int vs Str).
        let mut s = Substitution::new();
        let a = TypeVarId(0);
        s.insert(a, Type::Int);
        unify(&Type::Var(a), &Type::Int, &mut s).unwrap();
        assert!(unify(&Type::Var(a), &Type::Str, &mut s).is_err());
    }

    // --- 4e: Optional + Union ---

    #[test]
    fn optional_displays_with_question_suffix() {
        assert_eq!(Type::optional(Type::Int).to_string(), "int?");
        assert_eq!(Type::optional(Type::Str).to_string(), "string?");
    }

    #[test]
    fn optional_collapses_double_wrapping() {
        // T?? collapses to T?.
        let inner = Type::optional(Type::Int);
        let outer = Type::optional(inner);
        assert_eq!(outer.to_string(), "int?");
    }

    #[test]
    fn optional_of_nil_is_just_nil() {
        // T? where T = nil reduces to nil — there's only one
        // inhabitant.
        assert_eq!(Type::optional(Type::Nil), Type::Nil);
    }

    #[test]
    fn union_displays_with_pipe() {
        let u = Type::union(vec![Type::Int, Type::Str]);
        assert_eq!(u.to_string(), "int | string");
    }

    #[test]
    fn union_dedupes() {
        let u = Type::union(vec![Type::Int, Type::Int, Type::Str]);
        assert_eq!(u.to_string(), "int | string");
    }

    #[test]
    fn union_with_nil_becomes_optional() {
        // T | nil canonicalises to T?.
        let u = Type::union(vec![Type::Int, Type::Nil]);
        assert_eq!(u.to_string(), "int?");
    }

    #[test]
    fn singleton_union_is_just_the_singleton() {
        let u = Type::union(vec![Type::Int]);
        assert_eq!(u, Type::Int);
    }

    #[test]
    fn empty_union_is_unknown() {
        let u = Type::union(Vec::<Type>::new());
        assert_eq!(u, Type::Unknown);
    }

    #[test]
    fn union_flattens_nested_unions() {
        let inner = Type::Union(vec![Type::Int, Type::Bool]);
        let u = Type::union(vec![inner, Type::Str]);
        assert_eq!(u.to_string(), "int | bool | string");
    }

    #[test]
    fn optional_compatible_with_nil_and_inner() {
        let opt = Type::optional(Type::Int);
        assert!(opt.is_compatible_with(&Type::Nil));
        assert!(opt.is_compatible_with(&Type::Int));
        assert!(!opt.is_compatible_with(&Type::Str));
        // Reflexive both directions.
        assert!(Type::Nil.is_compatible_with(&opt));
        assert!(Type::Int.is_compatible_with(&opt));
    }

    #[test]
    fn union_compatible_with_any_variant() {
        let u = Type::union(vec![Type::Int, Type::Str]);
        assert!(u.is_compatible_with(&Type::Int));
        assert!(u.is_compatible_with(&Type::Str));
        assert!(!u.is_compatible_with(&Type::Bool));
    }

    #[test]
    fn unify_optional_with_nil_succeeds() {
        let mut s = Substitution::new();
        let opt = Type::optional(Type::Int);
        unify(&opt, &Type::Nil, &mut s).unwrap();
    }

    #[test]
    fn unify_optional_with_inner_succeeds() {
        let mut s = Substitution::new();
        let opt = Type::optional(Type::Int);
        unify(&opt, &Type::Int, &mut s).unwrap();
    }

    #[test]
    fn unify_union_picks_first_matching_variant() {
        let mut s = Substitution::new();
        let u = Type::union(vec![Type::Int, Type::Str]);
        unify(&u, &Type::Int, &mut s).unwrap();
        unify(&u, &Type::Str, &mut s).unwrap();
        assert!(unify(&u, &Type::Bool, &mut s).is_err());
    }
}
