//! Hindley-Milner type inference for the Twe non-strict mode.
//!
//! Phase 4a: literal-driven bottom-up inference.
//! Phase 4b: introduced `Type::Var` + unification (in `types.rs`).
//! Phase 4c (this commit): refactored around an `Inferer` struct
//! that threads a fresh-variable generator + substitution + a
//! scope chain through the walk. Function declarations now
//! allocate fresh vars for each parameter + the return, walk
//! their body collecting constraints (operator argument types,
//! return statements, ident references), and the substitution
//! resolves the signature when usage pins the types down.
//!
//! Per `docs/02-type-system.md`'s non-strict guarantee: when a
//! constraint can't be solved, the offending unification error
//! is **silently absorbed** — the involved type stays `Unknown`
//! rather than becoming a user-facing error. Strict mode (v0.2)
//! will surface those errors at function boundaries.
//!
//! The walk doesn't yet:
//!   - infer types for class methods (the `self.field` shape
//!     needs the structural-record work landing in 4d)
//!   - thread types through `for x in iter:` (would need to know
//!     iter is List<T> -> bind x: T)
//!   - resolve types across mutually-recursive top-level
//!     functions (single forward pass — recursive self-ref
//!     works because the function's signature is registered
//!     before the body is walked, but mutual recursion needs a
//!     two-pass scan of the program)
//!
//! These are 4d / 4e / 4f territory.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use crate::ast::{BinOp, DeclMember, Expr, Program, Stmt, UnOp};
use crate::types::{apply_subst, unify, Substitution, Type, TypeVarGen};

/// Result of inference over a program — top-level names bound to
/// their (best-effort) type, with all type variables fully
/// substituted. Names whose RHS we can't prove anything about
/// land here as `Type::Unknown`.
pub type Bindings = HashMap<String, Type>;

/// Public entry point. Creates a fresh `Inferer`, walks the
/// program, and returns the resolved top-level bindings.
pub fn infer_program(program: &Program) -> Bindings {
    let mut inferer = Inferer::new();
    inferer.walk_program(program);
    inferer.resolved_top_level()
}

/// Public for `lsp.rs` hover lookup of arbitrary expressions —
/// kept for API compatibility with phase 4a callers. Internally
/// uses a fresh `Inferer` with the supplied bindings as its
/// initial top-level scope. No constraint propagation back to
/// the caller's bindings — that's a one-shot best effort.
pub fn infer_expr(expr: &Expr, bindings: &Bindings) -> Type {
    let mut inferer = Inferer::new();
    for (name, ty) in bindings {
        inferer.scopes[0].insert(name.clone(), ty.clone());
    }
    let t = inferer.expr_type(expr);
    inferer.resolve(&t)
}

/// Inference session. One per `infer_program` call. Holds the
/// type-variable generator + substitution + a stack of lexical
/// scopes (innermost on top). The top-level scope is `scopes[0]`;
/// function bodies push a new scope on entry and pop on exit.
struct Inferer {
    var_gen: TypeVarGen,
    subst: Substitution,
    /// Stack of name -> type maps. Pushed on function-body entry,
    /// popped on exit. Lookup walks innermost-first.
    scopes: Vec<HashMap<String, Type>>,
    /// Per-class field shapes. Populated as `walk_stmt` sees
    /// each `Stmt::Decl` (entity / item / scene / particles /
    /// modifier / inventory) — we infer the type of every field
    /// default and stash it here. `expr_type` then resolves
    /// `instance.field` via Instance(class_name) lookup. Class
    /// methods aren't in this table; they're keyed under the
    /// same class but tracked separately by future strict-mode
    /// passes.
    class_shapes: HashMap<String, BTreeMap<String, Type>>,
    /// When walking a method body, this is the enclosing class
    /// name so `self` and bare-name field references resolve
    /// correctly. None outside of methods.
    current_class: Option<String>,
}

impl Inferer {
    fn new() -> Self {
        Self {
            var_gen: TypeVarGen::new(),
            subst: Substitution::new(),
            scopes: vec![HashMap::new()],
            class_shapes: HashMap::new(),
            current_class: None,
        }
    }

    fn fresh_var(&mut self) -> Type {
        Type::Var(self.var_gen.fresh())
    }

    /// Walk the whole program, recording top-level bindings as
    /// we go. Function bodies are walked in a nested scope so
    /// param + local types don't leak into the global env.
    fn walk_program(&mut self, program: &Program) {
        for stmt in &program.stmts {
            self.walk_stmt(stmt);
        }
    }

    fn walk_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, value, .. } => {
                let t = self.expr_type(value);
                self.bind(name.clone(), t);
            }
            Stmt::FunctionDecl { name, params, body, .. } => {
                // Allocate fresh vars for params + return. Register
                // the function's type BEFORE walking the body so
                // recursive self-reference (`function fact(n): ...
                // fact(n - 1) ...`) sees a typed signature.
                let param_vars: Vec<Type> = params.iter().map(|_| self.fresh_var()).collect();
                let ret_var = self.fresh_var();
                let func_t = Type::func(param_vars.clone(), ret_var.clone());
                self.bind(name.clone(), func_t);
                self.walk_function_body(params, body, &param_vars, &ret_var);
            }
            Stmt::Decl { name, members, .. } => {
                self.bind(name.clone(), Type::Class(Rc::new(name.clone())));
                self.walk_class_members(name, members);
            }
            Stmt::Assign { value, .. } => {
                // Reassignment doesn't introduce a new binding;
                // we only walk the value to collect constraints.
                let _ = self.expr_type(value);
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    let _ = self.expr_type(v);
                }
            }
            Stmt::If { cond, then_body, elifs, else_body, .. } => {
                let _ = self.expr_type(cond);
                for s in then_body {
                    self.walk_stmt(s);
                }
                for (c, body) in elifs {
                    let _ = self.expr_type(c);
                    for s in body {
                        self.walk_stmt(s);
                    }
                }
                if let Some(eb) = else_body {
                    for s in eb {
                        self.walk_stmt(s);
                    }
                }
            }
            Stmt::While { cond, body, .. } => {
                let _ = self.expr_type(cond);
                for s in body {
                    self.walk_stmt(s);
                }
            }
            Stmt::For { var, iter, body, .. } => {
                // The loop var's type is the iter's element type
                // when known. List<T> -> T; tuple/range fall through
                // to Unknown for this MVP.
                let iter_t = self.expr_type(iter);
                let elem_t = match self.resolve(&iter_t) {
                    Type::List(elem) => (*elem).clone(),
                    Type::Range => Type::Int,
                    _ => Type::Unknown,
                };
                self.push_scope();
                self.bind(var.clone(), elem_t);
                for s in body {
                    self.walk_stmt(s);
                }
                self.pop_scope();
            }
            Stmt::OnUpdate { body, .. } => {
                self.push_scope();
                self.bind("dt".to_string(), Type::Float);
                for s in body {
                    self.walk_stmt(s);
                }
                self.pop_scope();
            }
            Stmt::Expr(e) => {
                let _ = self.expr_type(e);
            }
            // Statements that don't introduce inference signal:
            Stmt::Break { .. }
            | Stmt::Continue { .. }
            | Stmt::Transition { .. }
            | Stmt::Spawn { .. }
            | Stmt::Despawn { .. } => {}
        }
    }

    /// Walk a class declaration's members, populating the
    /// `class_shapes` entry for `class_name` with each field's
    /// inferred type and each method's signature. Then walk
    /// each method body so its signature gets refined by
    /// constraints from `return` / operators / call sites.
    /// Inside method bodies, `self` resolves to `Instance(class_name)`
    /// and bare-name references to declared fields resolve via
    /// the class shape.
    fn walk_class_members(&mut self, class_name: &str, members: &[DeclMember]) {
        let mut shape: BTreeMap<String, Type> = BTreeMap::new();

        // Pass 1: infer field types from their default-value
        // expressions.
        for m in members {
            if let DeclMember::Field { name, value, .. } = m {
                let t = self.expr_type(value);
                shape.insert(name.clone(), self.resolve(&t));
            }
        }

        // Pass 2: register placeholder method signatures with
        // fresh vars. Methods need to live in the shape too so
        // call sites like `instance.method()` can resolve via
        // the same Field-on-Instance lookup field access uses.
        // Stored as Type::Function — call dispatch flows through.
        let mut method_meta: Vec<(String, Vec<Type>, Type)> = Vec::new();
        for m in members {
            if let DeclMember::Method { name, params, .. } = m {
                let pv: Vec<Type> = params.iter().map(|_| self.fresh_var()).collect();
                let rv = self.fresh_var();
                shape.insert(name.clone(), Type::func(pv.clone(), rv.clone()));
                method_meta.push((name.clone(), pv, rv));
            }
        }

        // Insert the shape BEFORE walking method bodies so
        // self.field / self.other_method() lookups inside a
        // method body see the registered shape.
        self.class_shapes.insert(class_name.to_string(), shape);

        // Pass 3: walk each method body. The shape's method
        // signatures get further refined here through return
        // constraints and operator-driven param pinning.
        for (m_idx, m) in members.iter().enumerate() {
            if let DeclMember::Method { params, body, .. } = m {
                let (_, pv, rv) = &method_meta[method_count_up_to(members, m_idx)];
                self.push_scope();
                let prev_class = self.current_class.take();
                self.current_class = Some(class_name.to_string());
                for (n, t) in params.iter().zip(pv.iter()) {
                    self.bind(n.clone(), t.clone());
                }
                // Bare-name reads of class fields / methods
                // inside the method body resolve via the scope
                // chain. Mirrors the bytecode VM's self-field
                // rewrite (compiler.rs).
                if let Some(shape) = self.class_shapes.get(class_name).cloned() {
                    for (fname, fty) in shape {
                        if !self.lookup_in_top_scope(&fname) {
                            self.bind(fname, fty);
                        }
                    }
                }
                self.walk_function_block(body, rv);
                self.current_class = prev_class;
                self.pop_scope();
            }
        }
    }

    /// True when `name` is bound in the innermost scope — used
    /// during class method setup to avoid shadowing a parameter
    /// with the field of the same name.
    fn lookup_in_top_scope(&self, name: &str) -> bool {
        self.scopes
            .last()
            .map(|s| s.contains_key(name))
            .unwrap_or(false)
    }

    /// Walk a function body in a fresh scope. Params are bound
    /// to their fresh vars; `return` statements unify against
    /// `ret_var`. Local lets inside the body get their own
    /// scope-chain entries.
    fn walk_function_body(
        &mut self,
        params: &[String],
        body: &[Stmt],
        param_vars: &[Type],
        ret_var: &Type,
    ) {
        self.push_scope();
        for (name, ty) in params.iter().zip(param_vars.iter()) {
            self.bind(name.clone(), ty.clone());
        }
        self.walk_function_block(body, ret_var);
        self.pop_scope();
    }

    /// Walk a block-of-statements that's part of a function body,
    /// threading `ret_var` so `return X` can unify X's type
    /// against it. Recurses into nested if/while/for so deeper
    /// returns also reach the outer signature.
    fn walk_function_block(&mut self, stmts: &[Stmt], ret_var: &Type) {
        for stmt in stmts {
            match stmt {
                Stmt::Return { value, .. } => match value {
                    Some(v) => {
                        let t = self.expr_type(v);
                        let _ = unify(&t, ret_var, &mut self.subst);
                    }
                    None => {
                        let _ = unify(&Type::Nil, ret_var, &mut self.subst);
                    }
                },
                Stmt::Let { name, value, .. } => {
                    let t = self.expr_type(value);
                    self.bind(name.clone(), t);
                }
                Stmt::Assign { value, .. } => {
                    let _ = self.expr_type(value);
                }
                Stmt::If { cond, then_body, elifs, else_body, .. } => {
                    let _ = self.expr_type(cond);
                    self.walk_function_block(then_body, ret_var);
                    for (c, body) in elifs {
                        let _ = self.expr_type(c);
                        self.walk_function_block(body, ret_var);
                    }
                    if let Some(eb) = else_body {
                        self.walk_function_block(eb, ret_var);
                    }
                }
                Stmt::While { cond, body, .. } => {
                    let _ = self.expr_type(cond);
                    self.walk_function_block(body, ret_var);
                }
                Stmt::For { var, iter, body, .. } => {
                    let iter_t = self.expr_type(iter);
                    let elem_t = match self.resolve(&iter_t) {
                        Type::List(elem) => (*elem).clone(),
                        Type::Range => Type::Int,
                        _ => Type::Unknown,
                    };
                    self.push_scope();
                    self.bind(var.clone(), elem_t);
                    self.walk_function_block(body, ret_var);
                    self.pop_scope();
                }
                Stmt::Expr(e) => {
                    let _ = self.expr_type(e);
                }
                Stmt::FunctionDecl { name, params, body, .. } => {
                    // Nested function decl — same logic as top
                    // level, isolated from the enclosing return.
                    let pv: Vec<Type> = params.iter().map(|_| self.fresh_var()).collect();
                    let rv = self.fresh_var();
                    self.bind(name.clone(), Type::func(pv.clone(), rv.clone()));
                    self.walk_function_body(params, body, &pv, &rv);
                }
                _ => {}
            }
        }
    }

    /// Compute the type of an expression. May extend the
    /// substitution via unification on operator constraints.
    fn expr_type(&mut self, expr: &Expr) -> Type {
        match expr {
            Expr::Int { .. } => Type::Int,
            Expr::Float { .. } => Type::Float,
            Expr::Bool { .. } => Type::Bool,
            Expr::Str { .. } | Expr::Interp { .. } => Type::Str,
            Expr::Percent { .. } => Type::Percent,
            Expr::Quantity { unit, .. } => Type::Quantity(Rc::new(unit.clone())),
            Expr::Ident { name, .. } => self.lookup(name).unwrap_or(Type::Unknown),
            Expr::SelfRef { .. } => match &self.current_class {
                Some(c) => Type::Instance(Rc::new(c.clone())),
                None => Type::Unknown,
            },
            Expr::Tuple { elems, .. } => {
                Type::Tuple(elems.iter().map(|e| self.expr_type(e)).collect())
            }
            Expr::List { elems, .. } => {
                if elems.is_empty() {
                    return Type::list(self.fresh_var());
                }
                // Walk every element through expr_type sequentially
                // so the inferer's mutable state doesn't get
                // double-borrowed by an iterator chain. Then check
                // homogeneity after the fact.
                let elem_ts: Vec<Type> = elems.iter().map(|e| self.expr_type(e)).collect();
                let head_resolved = self.resolve(&elem_ts[0]);
                let homogeneous = elem_ts[1..]
                    .iter()
                    .all(|t| self.resolve(t) == head_resolved);
                if homogeneous {
                    Type::list(elem_ts.into_iter().next().unwrap())
                } else {
                    Type::list(Type::Unknown)
                }
            }
            Expr::Range { .. } => Type::Range,
            Expr::Index { object, .. } => {
                let obj_t = self.expr_type(object);
                match self.resolve(&obj_t) {
                    Type::List(elem) => (*elem).clone(),
                    _ => Type::Unknown,
                }
            }
            Expr::Field { object, name, .. } => {
                let obj_t = self.expr_type(object);
                let resolved = self.resolve(&obj_t);
                match (&resolved, name.as_str()) {
                    (Type::Tuple(elems), "x") if !elems.is_empty() => elems[0].clone(),
                    (Type::Tuple(elems), "y") if elems.len() >= 2 => elems[1].clone(),
                    (Type::Tuple(elems), "z") if elems.len() >= 3 => elems[2].clone(),
                    (Type::List(_), "length") => Type::Int,
                    (Type::Instance(class_name), field) => self
                        .class_shapes
                        .get(class_name.as_str())
                        .and_then(|shape| shape.get(field).cloned())
                        .unwrap_or(Type::Unknown),
                    _ => Type::Unknown,
                }
            }
            Expr::Call { callee, args, .. } => {
                let callee_raw = self.expr_type(callee);
                let callee_t = self.resolve(&callee_raw);
                let arg_ts: Vec<Type> = args.iter().map(|a| self.expr_type(a)).collect();
                match callee_t {
                    Type::Class(name) => Type::Instance(name),
                    Type::Function { params, ret } => {
                        // Unify each arg against the param type so
                        // call-site usage refines the function's
                        // signature. (For functions whose params
                        // are still fresh vars, this is the
                        // mechanism that pins them down.)
                        for (a, p) in arg_ts.iter().zip(params.iter()) {
                            let _ = unify(a, p, &mut self.subst);
                        }
                        (*ret).clone()
                    }
                    _ => Type::Unknown,
                }
            }
            Expr::Unary { op, operand, .. } => {
                let raw = self.expr_type(operand);
                let inner = self.resolve(&raw);
                match op {
                    UnOp::Neg => match inner {
                        Type::Int => Type::Int,
                        Type::Float => Type::Float,
                        Type::Var(_) => {
                            // Without a constraint, we don't know
                            // if this is int or float. A future
                            // strict pass might add an "is numeric"
                            // bound; for now fall back to a fresh
                            // var so callers can flow-in.
                            self.fresh_var()
                        }
                        _ => Type::Unknown,
                    },
                    UnOp::Not => Type::Bool,
                }
            }
            Expr::Binary { op, left, right, .. } => {
                let l = self.expr_type(left);
                let r = self.expr_type(right);
                self.binop_type(*op, &l, &r)
            }
        }
    }

    fn binop_type(&mut self, op: BinOp, l: &Type, r: &Type) -> Type {
        match op {
            BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt | BinOp::Lte | BinOp::Gte => {
                Type::Bool
            }
            BinOp::In | BinOp::NotIn => Type::Bool,
            BinOp::And | BinOp::Or => {
                // Value-returning short-circuit per the F11 decision.
                // Result type is the common type of both sides;
                // unify them so a known type on either side flows.
                let _ = unify(l, r, &mut self.subst);
                self.resolve(l)
            }
            BinOp::Add => self.infer_add(l, r),
            BinOp::Sub | BinOp::Mul | BinOp::Div => self.infer_arith(op, l, r),
        }
    }

    fn infer_add(&mut self, l: &Type, r: &Type) -> Type {
        let lr = self.resolve(l);
        let rr = self.resolve(r);
        if matches!((&lr, &rr), (Type::Str, Type::Str)) {
            return Type::Str;
        }
        // Either side is Str + the other is a Var → constraint:
        // the var must be Str. (Twe's `+` is overloaded between
        // numeric add and string concat; we pick the branch that
        // matches what's known.)
        match (&lr, &rr) {
            (Type::Str, Type::Var(_)) | (Type::Var(_), Type::Str) => {
                let _ = unify(&lr, &Type::Str, &mut self.subst);
                let _ = unify(&rr, &Type::Str, &mut self.subst);
                return Type::Str;
            }
            _ => {}
        }
        self.infer_arith(BinOp::Add, l, r)
    }

    fn infer_arith(&mut self, op: BinOp, l: &Type, r: &Type) -> Type {
        let lr = self.resolve(l);
        let rr = self.resolve(r);
        if let (Type::Tuple(a), Type::Tuple(b)) = (&lr, &rr) {
            if matches!(op, BinOp::Add | BinOp::Sub) && a.len() == b.len() {
                let elems: Vec<Type> = a
                    .iter()
                    .zip(b.iter())
                    .map(|(x, y)| self.infer_arith(op, x, y))
                    .collect();
                return Type::Tuple(elems);
            }
        }
        if let Type::Tuple(elems) = &lr {
            if matches!(op, BinOp::Mul | BinOp::Div) && is_scalar_type(&rr) {
                let elems: Vec<Type> =
                    elems.iter().map(|x| self.infer_arith(op, x, &rr)).collect();
                return Type::Tuple(elems);
            }
        }
        if let Type::Tuple(elems) = &rr {
            if matches!(op, BinOp::Mul) && is_scalar_type(&lr) {
                let elems: Vec<Type> =
                    elems.iter().map(|y| self.infer_arith(op, &lr, y)).collect();
                return Type::Tuple(elems);
            }
        }
        match (&lr, &rr) {
            (Type::Int, Type::Int) => Type::Int,
            (Type::Float, Type::Float) => Type::Float,
            (Type::Int, Type::Float) | (Type::Float, Type::Int) => Type::Float,
            // If either side is a Var, propagate the constraint:
            // it must be int or float. We can't express "numeric"
            // in our current Type lattice, but we can pin both
            // sides together and produce a fresh var that the call
            // site can resolve. A future "is numeric" trait is the
            // right long-term answer.
            (Type::Var(_), other) | (other, Type::Var(_))
                if matches!(other, Type::Int | Type::Float) =>
            {
                let _ = unify(&lr, other, &mut self.subst);
                let _ = unify(&rr, other, &mut self.subst);
                other.clone()
            }
            (Type::Var(_), Type::Var(_)) => {
                let _ = unify(&lr, &rr, &mut self.subst);
                let v = self.fresh_var();
                let _ = unify(&lr, &v, &mut self.subst);
                v
            }
            _ => Type::Unknown,
        }
    }

    // --- scope chain ---

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    fn bind(&mut self, name: String, t: Type) {
        let last = self.scopes.last_mut().expect("at least one scope");
        last.insert(name, t);
    }

    fn lookup(&self, name: &str) -> Option<Type> {
        for scope in self.scopes.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t.clone());
            }
        }
        None
    }

    /// Apply the current substitution to `t` and return the
    /// most-resolved form.
    fn resolve(&self, t: &Type) -> Type {
        apply_subst(&self.subst, t)
    }

    /// Snapshot the top-level bindings with all type variables
    /// fully substituted. Called after `walk_program`.
    fn resolved_top_level(&self) -> Bindings {
        let mut out = Bindings::new();
        for (name, ty) in &self.scopes[0] {
            out.insert(name.clone(), self.resolve(ty));
        }
        out
    }
}

fn is_scalar_type(t: &Type) -> bool {
    matches!(t, Type::Int | Type::Float)
}

/// Helper: count how many `DeclMember::Method` entries appear
/// in `members[..upto]`. Used by `walk_class_members` to map a
/// member-position index into the parallel method-meta vector
/// (which only contains entries for methods).
fn method_count_up_to(members: &[DeclMember], upto: usize) -> usize {
    members[..upto]
        .iter()
        .filter(|m| matches!(m, DeclMember::Method { .. }))
        .count()
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

    // --- 4a behaviour, regression tested ---

    #[test]
    fn infers_int_let() {
        assert_eq!(type_of("x", "let x = 42"), Type::Int);
    }

    #[test]
    fn infers_string_let() {
        assert_eq!(type_of("x", "let x = \"hi\""), Type::Str);
    }

    #[test]
    fn infers_homogeneous_list() {
        let t = type_of("xs", "let xs = [1, 2, 3]");
        assert_eq!(t, Type::list(Type::Int));
    }

    #[test]
    fn infers_tuple() {
        let t = type_of("p", "let p = (3, 4)");
        assert_eq!(t, Type::Tuple(vec![Type::Int, Type::Int]));
    }

    #[test]
    fn arithmetic_promotes_int_float_to_float() {
        assert_eq!(type_of("x", "let x = 1 + 2"), Type::Int);
        assert_eq!(type_of("x", "let x = 1 + 2.0"), Type::Float);
    }

    #[test]
    fn class_decl_binds_class_type() {
        let bs = types_of("entity Hero:\n    var hp = 100\n");
        assert_eq!(bs.get("Hero").map(|t| t.to_string()).as_deref(), Some("<class Hero>"));
    }

    #[test]
    fn calling_a_class_yields_an_instance() {
        let bs = types_of("entity Hero:\n    var hp = 100\nlet h = Hero()\n");
        assert_eq!(bs.get("h").map(|t| t.to_string()).as_deref(), Some("Hero"));
    }

    // --- 4c new behaviour: function-body inference ---

    #[test]
    fn function_with_returnable_int_body_infers_int_return() {
        // `function double(x): return x * 2` — body uses x in
        // an int-arithmetic context, so x : int should be
        // pinned and the return type is int.
        let bs = types_of("function double(x):\n    return x * 2\n");
        let t = bs.get("double").expect("double binding");
        assert_eq!(t.to_string(), "function(int) -> int");
    }

    #[test]
    fn function_returning_string_concat_infers_string() {
        let bs = types_of("function shout(s):\n    return s + \"!\"\n");
        let t = bs.get("shout").expect("shout binding");
        assert_eq!(t.to_string(), "function(string) -> string");
    }

    #[test]
    fn function_with_no_return_annotation_stays_open() {
        // `function noop(x): x` — body just evaluates x for side
        // effects. No return statement → return type stays a
        // fresh var. Without further use we resolve it to a var
        // (printed as ?N) — non-strict makes this fine.
        let bs = types_of("function noop(x):\n    x\n");
        let t = bs.get("noop").expect("noop binding");
        // Display should be `function(?N) -> ?M` — both fresh.
        let disp = t.to_string();
        assert!(disp.starts_with("function(?"), "got: {disp}");
        assert!(disp.contains(") -> ?"), "got: {disp}");
    }

    #[test]
    fn explicit_return_value_pins_return_type() {
        let bs = types_of("function pi():\n    return 3.14\n");
        let t = bs.get("pi").expect("pi binding");
        assert_eq!(t.to_string(), "function() -> float");
    }

    #[test]
    fn recursive_function_signature_resolves_through_self_ref() {
        // factorial: `n` is multiplied so it's int; recursive
        // call sees `fact` as `function(int) -> int` (the typed
        // signature was registered before the body was walked),
        // so the unification all lines up.
        let bs = types_of(
            "function fact(n):\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n",
        );
        let t = bs.get("fact").expect("fact binding");
        assert_eq!(t.to_string(), "function(int) -> int");
    }

    #[test]
    fn call_site_resolves_to_function_return_type() {
        let bs = types_of(
            "function double(x):\n    return x * 2\n\nlet result = double(5)\n",
        );
        assert_eq!(bs.get("result"), Some(&Type::Int));
    }

    #[test]
    fn nested_function_decls_are_walked() {
        let bs = types_of(
            "function outer():\n    function inner(x):\n        return x + 1\n    return inner(5)\n",
        );
        // outer's return came from inner(5) which is int.
        assert_eq!(bs.get("outer").map(|t| t.to_string()).as_deref(), Some("function() -> int"));
    }

    #[test]
    fn for_over_list_binds_loop_var_to_element_type() {
        // The loop var only appears inside the body; the binding
        // doesn't escape the for. We test indirectly: a function
        // that iterates a list and returns the loop var.
        let bs = types_of(
            "function head(xs):\n    for x in xs:\n        return x\n    return 0\n",
        );
        // We can't fully infer xs's element type without a usage
        // hint inside the body, so the function may print as
        // `function(?N) -> int` — the return type is pinned
        // because of `return 0`.
        let t = bs.get("head").expect("head binding");
        assert!(t.to_string().contains("-> int"), "got: {t}");
    }

    #[test]
    fn for_over_range_binds_loop_var_to_int() {
        let bs = types_of("function sum_to(n):\n    var total = 0\n    for i in 0..n:\n        total += i\n    return total\n");
        let t = bs.get("sum_to").expect("sum_to binding");
        // `total` is an int; the function returns it, so the
        // return type is int.
        assert!(t.to_string().contains("-> int"), "got: {t}");
    }

    #[test]
    fn unify_failure_on_arg_call_doesnt_panic_or_error() {
        // Call `double` (which expects int) with a string. Per
        // non-strict, we silently absorb the unification failure
        // — `result` ends up Unknown rather than blowing up.
        let bs = types_of(
            "function double(x):\n    return x * 2\n\nlet result = double(\"hi\")\n",
        );
        // The signature is still int -> int; the call result
        // type is whatever `*` produced for the body, which
        // resolves to int.
        let t = bs.get("double").expect("double binding");
        assert_eq!(t.to_string(), "function(int) -> int");
        // result is the function's return type = int (we don't
        // reject the call even though the arg type was wrong).
        assert_eq!(bs.get("result"), Some(&Type::Int));
    }

    // --- 4a behaviour kept ---

    #[test]
    fn ident_referring_to_nothing_is_unknown() {
        assert_eq!(type_of("x", "let x = missing"), Type::Unknown);
    }

    #[test]
    fn unknown_propagates_silently() {
        let bs = types_of("let n = missing\nlet m = n + 1");
        // m is the result of unknown + int. Non-strict: stays
        // Unknown rather than complaining.
        assert_eq!(bs.get("m"), Some(&Type::Unknown));
    }

    #[test]
    fn list_index_returns_element_type() {
        let bs = types_of("let xs = [10, 20, 30]\nlet first = xs[0]");
        assert_eq!(bs.get("first"), Some(&Type::Int));
    }

    #[test]
    fn tuple_field_xyz_resolves() {
        let bs = types_of("let p = (3, 4)\nlet x = p.x");
        assert_eq!(bs.get("x"), Some(&Type::Int));
    }

    #[test]
    fn list_length_field_is_int() {
        let bs = types_of("let xs = [1, 2, 3]\nlet n = xs.length");
        assert_eq!(bs.get("n"), Some(&Type::Int));
    }

    #[test]
    fn comparisons_return_bool() {
        assert_eq!(type_of("x", "let x = 1 < 2"), Type::Bool);
    }

    // --- 4d: instance field access via class shapes ---

    #[test]
    fn instance_field_access_resolves_to_field_type() {
        let bs = types_of(
            "item Counter:\n    value: 0\n\nlet c = Counter()\nlet n = c.value\n",
        );
        assert_eq!(bs.get("c").map(|t| t.to_string()).as_deref(), Some("Counter"));
        assert_eq!(bs.get("n"), Some(&Type::Int));
    }

    #[test]
    fn instance_field_with_string_default_resolves_to_string() {
        let bs = types_of(
            "item NPC:\n    name: \"Bob\"\n\nlet npc = NPC()\nlet who = npc.name\n",
        );
        assert_eq!(bs.get("who"), Some(&Type::Str));
    }

    #[test]
    fn instance_unknown_field_falls_back_to_unknown() {
        let bs = types_of(
            "item Counter:\n    value: 0\n\nlet c = Counter()\nlet x = c.glubjorm\n",
        );
        // Per non-strict ("no false positives"): unknown field
        // is allowed at parse time, returns Unknown.
        assert_eq!(bs.get("x"), Some(&Type::Unknown));
    }

    #[test]
    fn entity_with_var_field_carries_inferred_type() {
        let bs = types_of(
            "entity Mob:\n    var hp = 100\n\nlet m = Mob()\nlet h = m.hp\n",
        );
        assert_eq!(bs.get("h"), Some(&Type::Int));
    }

    #[test]
    fn self_inside_method_resolves_to_instance_type() {
        // The method `hp_value` references self.value; the
        // returned type should be int because self : Counter
        // and Counter.value : int.
        let bs = types_of(
            "item Counter:\n    value: 0\n\n    hp_value():\n        return self.value\n\nlet c = Counter()\nlet h = c.hp_value()\n",
        );
        assert_eq!(bs.get("h"), Some(&Type::Int));
    }

    #[test]
    fn bare_name_in_method_resolves_to_field() {
        // Inside a method body, `value` (with no `self.`) should
        // resolve to the class's field — the same scope rule the
        // bytecode VM uses (compiler.rs's bare-name self-field
        // rewrite). Method should be inferred as returning int.
        let bs = types_of(
            "item Counter:\n    value: 0\n\n    raw():\n        return value\n\nlet c = Counter()\nlet n = c.raw()\n",
        );
        assert_eq!(bs.get("n"), Some(&Type::Int));
    }

    #[test]
    fn multiple_classes_get_separate_shapes() {
        let bs = types_of(
            "item A:\n    x: 1\n\nitem B:\n    y: \"hi\"\n\nlet a = A()\nlet b = B()\nlet ax = a.x\nlet by = b.y\n",
        );
        assert_eq!(bs.get("ax"), Some(&Type::Int));
        assert_eq!(bs.get("by"), Some(&Type::Str));
    }

    #[test]
    fn scene_field_shape_works_too() {
        let bs = types_of(
            "scene S:\n    var n = 0\n    initial: a\n    state a:\n",
        );
        // We can't easily access scene fields from outside a
        // scene declaration without an explicit reference, but
        // we can at least confirm the class binds and the shape
        // is recorded. Test indirectly: a method-style access
        // through Instance(S) — there's no constructor for
        // scenes (they auto-instantiate at runtime), so we
        // can't test access syntactically. This test just
        // confirms the type for S binds without error.
        assert_eq!(bs.get("S").map(|t| t.to_string()).as_deref(), Some("<class S>"));
    }
}
