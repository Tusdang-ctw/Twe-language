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

use std::fmt;
use std::rc::Rc;

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
    Function {
        params: Vec<Type>,
        ret: Rc<Type>,
    },
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
}

impl Type {
    /// Convenience constructor for a function type.
    pub fn func(params: Vec<Type>, ret: Type) -> Type {
        Type::Function { params, ret: Rc::new(ret) }
    }

    /// Convenience constructor for a list type.
    pub fn list(elem: Type) -> Type {
        Type::List(Rc::new(elem))
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
                a.len() == b.len()
                    && a.iter().zip(b.iter()).all(|(x, y)| x.is_compatible_with(y))
            }
            (Type::List(a), Type::List(b)) => a.is_compatible_with(b),
            (
                Type::Function { params: ap, ret: ar },
                Type::Function { params: bp, ret: br },
            ) => {
                ap.len() == bp.len()
                    && ap.iter().zip(bp.iter()).all(|(x, y)| x.is_compatible_with(y))
                    && ar.is_compatible_with(br)
            }
            (Type::Instance(a), Type::Instance(b)) => a == b,
            (Type::Class(a), Type::Class(b)) => a == b,
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
        assert_eq!(Type::Quantity(Rc::new("ms".into())).to_string(), "quantity<ms>");
        assert_eq!(Type::Quantity(Rc::new("kg".into())).to_string(), "quantity<kg>");
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
        assert_eq!(Type::Class(Rc::new("Hero".into())).to_string(), "<class Hero>");
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
}
