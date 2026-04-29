//! Bottom-up type inference for the Twe non-strict mode.
//!
//! Phase 4a (this commit): literal-driven inference. Walk
//! expressions; literals carry their type; operator results are
//! computed from operand types where both sides are known; when
//! we can't prove anything, return `Type::Unknown`. Per
//! `docs/02-type-system.md` the non-strict guarantee is "no
//! false positives" — better to say nothing than to be wrong.
//!
//! 4b will introduce **type variables** + **unification**, which
//! lets inference flow through bare-name references and function
//! calls (the meat of Hindley-Milner). Until then, identifier
//! references resolve via a small top-level binding table built
//! from `let X = literal` statements; everything else is Unknown.
//!
//! 4c+ extend with: function-body inference, structural records,
//! tagged unions, dimensional units, generics. The `infer_program`
//! signature stays stable; callers (`twec types`, the LSP hover
//! handler in 4g) read the resulting `Bindings` map.

use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{BinOp, Expr, Program, Stmt, UnOp};
use crate::types::Type;

/// Result of inference over a program — the set of top-level
/// names bound to their (best-effort) type. Names whose RHS we
/// can't prove anything about land here as `Type::Unknown`.
///
/// The map is `name -> type`; when the same name is reassigned
/// later in the script (legal in Twe), the *last* binding wins.
/// This is the simplest behaviour and matches what the LSP hover
/// handler will want anyway: the type at the end of the file.
pub type Bindings = HashMap<String, Type>;

/// Infer types for every top-level binding in `program`. The
/// returned `Bindings` covers `let X = ...`, `var X = ...`, and
/// `function X(...)` declarations at the script's top level.
/// Class declarations bind `Class(name)` so future strict-mode
/// checks can validate constructor calls.
pub fn infer_program(program: &Program) -> Bindings {
    let mut bindings: Bindings = HashMap::new();
    for stmt in &program.stmts {
        match stmt {
            Stmt::Let { name, value, .. } => {
                let t = infer_expr(value, &bindings);
                bindings.insert(name.clone(), t);
            }
            Stmt::FunctionDecl { name, params, .. } => {
                // Parameter and return types are unknown until 4c
                // brings function-body inference. Recording the
                // arity now lets a future strict-mode pass at
                // least flag wrong-arity calls.
                let pts = vec![Type::Unknown; params.len()];
                bindings.insert(name.clone(), Type::func(pts, Type::Unknown));
            }
            Stmt::Decl { name, .. } => {
                // Every kind (entity / item / scene / ...) binds
                // a `Class`; the kind distinction matters for
                // runtime dispatch, not the type.
                bindings.insert(name.clone(), Type::Class(Rc::new(name.clone())));
            }
            // Statements that don't introduce a top-level binding.
            _ => {}
        }
    }
    bindings
}

/// Best-effort type of one expression in the given binding env.
/// Returns `Type::Unknown` when we can't prove anything; per
/// non-strict semantics that's the safe default.
pub fn infer_expr(expr: &Expr, bindings: &Bindings) -> Type {
    match expr {
        Expr::Int { .. } => Type::Int,
        Expr::Float { .. } => Type::Float,
        Expr::Bool { .. } => Type::Bool,
        Expr::Str { .. } | Expr::Interp { .. } => Type::Str,
        Expr::Percent { .. } => Type::Percent,
        Expr::Quantity { unit, .. } => Type::Quantity(Rc::new(unit.clone())),
        Expr::Ident { name, .. } => bindings.get(name).cloned().unwrap_or(Type::Unknown),
        Expr::SelfRef { .. } => Type::Unknown,
        Expr::Tuple { elems, .. } => {
            Type::Tuple(elems.iter().map(|e| infer_expr(e, bindings)).collect())
        }
        Expr::List { elems, .. } => {
            // Homogeneous list -> list of element type. Mixed -> list of Unknown.
            let mut iter = elems.iter().map(|e| infer_expr(e, bindings));
            let head = match iter.next() {
                Some(t) => t,
                None => return Type::list(Type::Unknown),
            };
            let homogeneous = iter.all(|t| t == head);
            if homogeneous {
                Type::list(head)
            } else {
                Type::list(Type::Unknown)
            }
        }
        Expr::Range { .. } => Type::Range,
        Expr::Index { object, .. } => {
            // Indexing a list returns the element type when known;
            // a tuple of known length + literal int index would
            // also work, but that requires constant-folding the
            // index expr — defer to a future session.
            match infer_expr(object, bindings) {
                Type::List(elem) => (*elem).clone(),
                _ => Type::Unknown,
            }
        }
        Expr::Field { object, name, .. } => {
            // Built-in field access we know the type of:
            //   tuple.x .y .z -> element type at that index
            //   list.length   -> int
            match (infer_expr(object, bindings), name.as_str()) {
                (Type::Tuple(elems), "x") if !elems.is_empty() => elems[0].clone(),
                (Type::Tuple(elems), "y") if elems.len() >= 2 => elems[1].clone(),
                (Type::Tuple(elems), "z") if elems.len() >= 3 => elems[2].clone(),
                (Type::List(_), "length") => Type::Int,
                _ => Type::Unknown,
            }
        }
        Expr::Call { callee, .. } => {
            // Calling a Class instantiates it; calling a Function
            // returns its declared return type. Everything else is
            // Unknown until 4c lands proper function inference.
            match infer_expr(callee, bindings) {
                Type::Class(name) => Type::Instance(name),
                Type::Function { ret, .. } => (*ret).clone(),
                _ => Type::Unknown,
            }
        }
        Expr::Unary { op, operand, .. } => {
            let inner = infer_expr(operand, bindings);
            match op {
                UnOp::Neg => match inner {
                    Type::Int => Type::Int,
                    Type::Float => Type::Float,
                    _ => Type::Unknown,
                },
                UnOp::Not => Type::Bool,
            }
        }
        Expr::Binary { op, left, right, .. } => {
            let l = infer_expr(left, bindings);
            let r = infer_expr(right, bindings);
            infer_binop(*op, &l, &r)
        }
    }
}

/// Result type of a binary operator given operand types. Mirrors
/// the runtime semantics in `eval::apply_arith`:
///
/// - Numeric ops promote int + float -> float.
/// - String + string -> string (concat).
/// - Comparisons, in / not in, and / or, equality return bool.
/// - Tuple element-wise arithmetic preserves tuple shape.
///
/// Anything we can't prove returns Unknown.
fn infer_binop(op: BinOp, l: &Type, r: &Type) -> Type {
    match op {
        BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt | BinOp::Lte | BinOp::Gte => Type::Bool,
        BinOp::In | BinOp::NotIn => Type::Bool,
        // `and`/`or` are value-returning per the F11 decision: the
        // result is the type of whichever operand is selected,
        // which we approximate as the union — for a non-strict v1,
        // when both sides have the same type return that, else
        // Unknown.
        BinOp::And | BinOp::Or => {
            if l == r {
                l.clone()
            } else {
                Type::Unknown
            }
        }
        BinOp::Add => infer_add(l, r),
        BinOp::Sub | BinOp::Mul | BinOp::Div => infer_arith(op, l, r),
    }
}

/// `+` is the only operator that splits string concat from
/// numeric add. Tuple element-wise add also lives here.
fn infer_add(l: &Type, r: &Type) -> Type {
    if matches!((l, r), (Type::Str, Type::Str)) {
        return Type::Str;
    }
    infer_arith(BinOp::Add, l, r)
}

fn infer_arith(op: BinOp, l: &Type, r: &Type) -> Type {
    // Tuple element-wise add/sub between same-length tuples, and
    // tuple <-> scalar mul/div. Mirrors `eval::apply_arith`.
    if let (Type::Tuple(a), Type::Tuple(b)) = (l, r) {
        if matches!(op, BinOp::Add | BinOp::Sub) && a.len() == b.len() {
            let elems: Vec<Type> = a
                .iter()
                .zip(b.iter())
                .map(|(x, y)| infer_arith(op, x, y))
                .collect();
            return Type::Tuple(elems);
        }
    }
    if let Type::Tuple(elems) = l {
        if matches!(op, BinOp::Mul | BinOp::Div) && is_scalar_type(r) {
            let elems: Vec<Type> =
                elems.iter().map(|x| infer_arith(op, x, r)).collect();
            return Type::Tuple(elems);
        }
    }
    if let Type::Tuple(elems) = r {
        if matches!(op, BinOp::Mul) && is_scalar_type(l) {
            let elems: Vec<Type> =
                elems.iter().map(|y| infer_arith(op, l, y)).collect();
            return Type::Tuple(elems);
        }
    }
    match (l, r) {
        (Type::Int, Type::Int) => Type::Int,
        (Type::Float, Type::Float) => Type::Float,
        (Type::Int, Type::Float) | (Type::Float, Type::Int) => Type::Float,
        _ => Type::Unknown,
    }
}

fn is_scalar_type(t: &Type) -> bool {
    matches!(t, Type::Int | Type::Float)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer;
    use crate::parser;

    fn types_of(src: &str) -> Bindings {
        let tokens = lexer::lex(&format!("{src}\n")).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        infer_program(&program)
    }

    fn type_of(name: &str, src: &str) -> Type {
        types_of(src)
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("no binding for `{name}`"))
    }

    #[test]
    fn infers_int_let() {
        assert_eq!(type_of("x", "let x = 42"), Type::Int);
    }

    #[test]
    fn infers_float_let() {
        assert_eq!(type_of("x", "let x = 1.5"), Type::Float);
    }

    #[test]
    fn infers_bool_let() {
        assert_eq!(type_of("x", "let x = true"), Type::Bool);
        assert_eq!(type_of("x", "let x = false"), Type::Bool);
    }

    #[test]
    fn infers_string_let() {
        assert_eq!(type_of("x", "let x = \"hi\""), Type::Str);
    }

    #[test]
    fn interpolation_is_a_string() {
        assert_eq!(type_of("x", "let n = 5\nlet x = \"hi {n}\""), Type::Str);
    }

    #[test]
    fn infers_percent_and_quantity() {
        assert_eq!(type_of("x", "let x = 25%"), Type::Percent);
        let t = type_of("x", "let x = 100ms");
        assert_eq!(t.to_string(), "quantity<ms>");
    }

    #[test]
    fn infers_tuple_with_mixed_elements() {
        let t = type_of("p", "let p = (3, 4)");
        assert_eq!(t, Type::Tuple(vec![Type::Int, Type::Int]));
        let t = type_of("p", "let p = (\"hi\", 5)");
        assert_eq!(t, Type::Tuple(vec![Type::Str, Type::Int]));
    }

    #[test]
    fn infers_homogeneous_list() {
        let t = type_of("xs", "let xs = [1, 2, 3]");
        assert_eq!(t, Type::list(Type::Int));
    }

    #[test]
    fn infers_mixed_list_as_unknown_element() {
        let t = type_of("xs", "let xs = [1, \"two\", 3]");
        assert_eq!(t, Type::list(Type::Unknown));
    }

    #[test]
    fn infers_empty_list_as_unknown_element() {
        let t = type_of("xs", "let xs = []");
        assert_eq!(t, Type::list(Type::Unknown));
    }

    #[test]
    fn infers_range() {
        assert_eq!(type_of("r", "let r = 0..10"), Type::Range);
        assert_eq!(type_of("r", "let r = 0..<10"), Type::Range);
    }

    #[test]
    fn arithmetic_promotes_int_float_to_float() {
        assert_eq!(type_of("x", "let x = 1 + 2"), Type::Int);
        assert_eq!(type_of("x", "let x = 1 + 2.0"), Type::Float);
        assert_eq!(type_of("x", "let x = 2.0 / 3"), Type::Float);
    }

    #[test]
    fn string_plus_string_is_string() {
        assert_eq!(type_of("x", "let x = \"a\" + \"b\""), Type::Str);
    }

    #[test]
    fn comparisons_return_bool() {
        assert_eq!(type_of("x", "let x = 1 < 2"), Type::Bool);
        assert_eq!(type_of("x", "let x = \"a\" == \"b\""), Type::Bool);
        assert_eq!(type_of("x", "let x = 5 in [1, 2, 3]"), Type::Bool);
    }

    #[test]
    fn unary_neg_preserves_int_float() {
        assert_eq!(type_of("x", "let x = -5"), Type::Int);
        assert_eq!(type_of("x", "let x = -1.5"), Type::Float);
    }

    #[test]
    fn unary_not_returns_bool() {
        assert_eq!(type_of("x", "let x = not true"), Type::Bool);
        assert_eq!(type_of("x", "let x = not 5"), Type::Bool);
    }

    #[test]
    fn ident_refers_to_earlier_binding() {
        let bs = types_of("let n = 42\nlet m = n");
        assert_eq!(bs.get("n"), Some(&Type::Int));
        assert_eq!(bs.get("m"), Some(&Type::Int));
    }

    #[test]
    fn ident_referring_to_nothing_is_unknown() {
        assert_eq!(type_of("x", "let x = missing"), Type::Unknown);
    }

    #[test]
    fn function_decl_records_arity_with_unknown_types() {
        let bs = types_of("function add(a, b):\n    return a + b\n");
        assert_eq!(
            bs.get("add"),
            Some(&Type::func(vec![Type::Unknown, Type::Unknown], Type::Unknown)),
        );
    }

    #[test]
    fn class_decl_binds_class_type() {
        let bs = types_of("entity Hero:\n    var hp = 100\n");
        let t = bs.get("Hero").expect("Hero binding");
        assert_eq!(t.to_string(), "<class Hero>");
    }

    #[test]
    fn calling_a_class_yields_an_instance() {
        // First declare the class so the binding exists, then
        // infer the type of `Hero()`.
        let bs = types_of("entity Hero:\n    var hp = 100\nlet h = Hero()\n");
        assert_eq!(bs.get("h").map(|t| t.to_string()).as_deref(), Some("Hero"));
    }

    #[test]
    fn tuple_field_xyz_resolves_through_inference() {
        let bs = types_of("let p = (3, 4)\nlet x = p.x");
        assert_eq!(bs.get("x"), Some(&Type::Int));
    }

    #[test]
    fn list_length_field_is_int() {
        let bs = types_of("let xs = [1, 2, 3]\nlet n = xs.length");
        assert_eq!(bs.get("n"), Some(&Type::Int));
    }

    #[test]
    fn list_index_returns_element_type() {
        let bs = types_of("let xs = [10, 20, 30]\nlet first = xs[0]");
        assert_eq!(bs.get("first"), Some(&Type::Int));
    }

    #[test]
    fn tuple_element_wise_add_preserves_shape() {
        let bs = types_of("let p = (3, 4)\nlet q = (1, 0)\nlet sum = p + q");
        assert_eq!(
            bs.get("sum"),
            Some(&Type::Tuple(vec![Type::Int, Type::Int])),
        );
    }

    #[test]
    fn tuple_times_scalar_preserves_shape_and_promotes() {
        let bs = types_of("let p = (3, 4)\nlet scaled = p * 2.5");
        // scalar 2.5 is float; element-wise mul promotes ints.
        assert_eq!(
            bs.get("scaled"),
            Some(&Type::Tuple(vec![Type::Float, Type::Float])),
        );
    }

    #[test]
    fn unknown_propagates_silently() {
        // `x` is unknown; arithmetic with it stays unknown without
        // raising any signal — non-strict's no-false-positives rule.
        let bs = types_of("let n = missing\nlet m = n + 1");
        assert_eq!(bs.get("m"), Some(&Type::Unknown));
    }
}
