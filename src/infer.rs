//! Hindley-Milner type inference for Twe.
//!
//! Phase 4a: literal-driven bottom-up inference.
//! Phase 4b: introduced `Type::Var` + unification (in `types.rs`).
//! Phase 4c: refactored around an `Inferer` struct that threads a
//! fresh-variable generator + substitution + a scope chain through
//! the walk.
//! Phase 6 session 1 (this commit): strict mode. The same
//! inference engine, with reporting policy as a flag — non-strict
//! silently absorbs unification failures (Luau's no-false-positives
//! contract); strict surfaces them as `TypeError` diagnostics with
//! line/col context. Opt-in via a `# strict` directive in the first
//! few lines of the file.
//!
//! Non-strict guarantee: when a constraint can't be solved, the
//! offending unification error is **silently absorbed** — the
//! involved type stays `Unknown` rather than becoming a user-facing
//! error. This is the v0.1 default; programs see strict reporting
//! only after they explicitly opt in.
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

/// One strict-mode type-checking diagnostic. Carries source
/// position + a human-readable summary; `kind` distinguishes the
/// constraint that failed (`"comparison"`, `"return"`, `"call"`)
/// so error messages can name the form that triggered the failure.
/// Phase 6 session 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeError {
    pub line: u32,
    pub col: u32,
    pub message: String,
    pub help: Option<String>,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

/// Detect a strict-mode opt-in directive in the source. Twe v0.1
/// uses a magic-comment style — `# strict` (or `#! strict`,
/// shebang-friendly) on one of the first ten non-blank lines of
/// the file.
///
/// Magic comment over a top-level keyword because: (1) a keyword
/// would steal the identifier from existing programs (one of the
/// 32 on-disk programs could legally have `let strict = true`),
/// and (2) the comment form is the established convention (Perl
/// `use strict`, Luau `--!strict`, Python `# coding: utf-8`).
///
/// Restricted to the first ten lines so a `# strict` deep in the
/// file (in dead code or a string-formatted help text) doesn't
/// flip the mode by accident.
pub fn detect_strict(source: &str) -> bool {
    let needle_a = "# strict";
    let needle_b = "#! strict";
    let needle_c = "#strict";
    let needle_d = "#!strict";
    for line in source.lines().take(10) {
        let trimmed = line.trim();
        if trimmed == needle_a || trimmed == needle_b || trimmed == needle_c || trimmed == needle_d
        {
            return true;
        }
    }
    false
}

/// Public entry point. Creates a fresh `Inferer`, walks the
/// program, and returns the resolved top-level bindings. Always
/// runs in non-strict mode — strict callers should use
/// `infer_program_strict` so they can surface diagnostics.
pub fn infer_program(program: &Program) -> Bindings {
    let mut inferer = Inferer::new(false);
    inferer.walk_program(program);
    inferer.resolved_top_level()
}

/// Strict-aware entry point. Walks the program, returns the
/// resolved top-level bindings AND any `TypeError` diagnostics
/// the strict reporter accumulated. With `strict = false`, the
/// returned `Vec<TypeError>` is always empty (non-strict drops
/// failures), so callers that want strict reporting just pass
/// `true`.
pub fn infer_program_strict(program: &Program, strict: bool) -> (Bindings, Vec<TypeError>) {
    let mut inferer = Inferer::new(strict);
    inferer.walk_program(program);
    let bindings = inferer.resolved_top_level();
    (bindings, inferer.errors)
}

/// Public for `lsp.rs` hover lookup of arbitrary expressions —
/// kept for API compatibility with phase 4a callers. Internally
/// uses a fresh `Inferer` with the supplied bindings as its
/// initial top-level scope. No constraint propagation back to
/// the caller's bindings — that's a one-shot best effort.
pub fn infer_expr(expr: &Expr, bindings: &Bindings) -> Type {
    let mut inferer = Inferer::new(false);
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
    /// Reporting policy. False (the default) drops unification
    /// failures silently — Luau's no-false-positives stance.
    /// True accumulates them in `errors` for the caller to
    /// surface. Phase 6 session 1.
    strict: bool,
    /// Diagnostics collected when `strict = true`. Kept in source
    /// order (each unify call site pushes immediately on failure).
    errors: Vec<TypeError>,
}

impl Inferer {
    fn new(strict: bool) -> Self {
        let mut inferer = Self {
            var_gen: TypeVarGen::new(),
            subst: Substitution::new(),
            scopes: vec![HashMap::new()],
            class_shapes: HashMap::new(),
            current_class: None,
            strict,
            errors: Vec::new(),
        };
        // Phase 6 session 5: seed the outermost scope with stdlib
        // names so strict-mode "unknown name" doesn't fire for
        // every `print` / `vec3` / `math.*` call. The seeded names
        // get `Type::Unknown` because the inferer doesn't know
        // their signatures (a future session that pulls signatures
        // from stdlib metadata can replace this).
        for n in stdlib_names() {
            inferer.scopes[0].insert((*n).to_string(), Type::Unknown);
        }
        inferer
    }

    /// Wrapper around `unify` that drops the error in non-strict
    /// mode (the existing v0.1 behaviour) and pushes a `TypeError`
    /// in strict mode. `kind` names the constraint that failed
    /// (`"comparison"`, `"return"`, `"call argument"`) so the
    /// diagnostic reads naturally; `line`/`col` come from the
    /// triggering Expr.
    fn try_unify(&mut self, a: &Type, b: &Type, line: u32, col: u32, kind: &str) {
        let result = unify(a, b, &mut self.subst);
        if !self.strict {
            return;
        }
        if let Err(e) = result {
            // Phase 13 session 5: structural width-subtyping
            // rescue. If one side is a Record and the other is a
            // class Instance whose shape supplies all the record's
            // fields with compatible types, suppress the
            // mismatch — the value is structurally assignable.
            // Sides aren't symmetric: a Record(R) is the *expected*
            // contract; the Instance is the *provided* value. We
            // try both orientations because `try_unify` is called
            // with arbitrary argument order across the inferer.
            let resolved_a = crate::types::apply_subst(&self.subst, a);
            let resolved_b = crate::types::apply_subst(&self.subst, b);
            let shapes_lookup = |name: &str| self.class_shapes.get(name).cloned();
            if crate::types::is_record_subtype_of(&resolved_a, &resolved_b, &shapes_lookup)
                || crate::types::is_record_subtype_of(&resolved_b, &resolved_a, &shapes_lookup)
            {
                return;
            }
            // Phase 13 session 6: Luau-style lax-strict narrowing.
            // Suppress the mismatch when one side is a Union (or an
            // Optional, which is `T | Nil`) whose variants include
            // the other side. The user's annotation acts as an
            // implicit narrowing assertion: they're claiming the
            // value will be that variant at runtime, and we trust
            // the assertion the same way we'd trust an explicit
            // `as` cast in a nominal type system. This is exactly
            // the rule Luau's "lax mode" applies — strict reports
            // things it can prove wrong, and a narrowing isn't
            // proof of error.
            if union_contains_variant(&resolved_a, &resolved_b)
                || union_contains_variant(&resolved_b, &resolved_a)
            {
                return;
            }
            let (a_str, b_str) = match &e {
                crate::types::UnifyError::Mismatch { a, b } => (a.clone(), b.clone()),
                crate::types::UnifyError::OccursCheck { var, ty } => {
                    (format!("type variable {var:?}"), ty.clone())
                }
            };
            self.errors.push(TypeError {
                line,
                col,
                message: format!("{kind}: type mismatch — {a_str} vs {b_str}"),
                help: Some(
                    "the inferred operand types disagree under strict mode; either change one side or annotate the expected type"
                        .to_string(),
                ),
            });
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
            Stmt::Let {
                name,
                value,
                ty,
                line,
                col,
            } => {
                let inferred = self.expr_type(value);
                // Phase 6 session 2: if the user annotated the
                // binding (`let x: int = ...`), unify the value's
                // inferred type against the annotation. Strict
                // surfaces a mismatch; non-strict drops it.
                if let Some(annotated) = ty {
                    self.try_unify(&inferred, annotated, *line, *col, "let annotation");
                    self.bind(name.clone(), annotated.clone());
                } else {
                    self.bind(name.clone(), inferred);
                }
            }
            Stmt::FunctionDecl {
                name,
                params,
                ret,
                body,
                line,
                col,
                ..
            } => {
                // Allocate fresh vars for params + return. Register
                // the function's type BEFORE walking the body so
                // recursive self-reference (`function fact(n): ...
                // fact(n - 1) ...`) sees a typed signature.
                //
                // Phase 6 session 2: when a param or return type is
                // annotated, unify its fresh var against the
                // annotation right here. Subsequent constraints
                // (call-site arg types, body return values) then
                // either fit cleanly or trigger a strict-mode
                // diagnostic via `try_unify`.
                let param_vars: Vec<Type> = params.iter().map(|_| self.fresh_var()).collect();
                let ret_var = self.fresh_var();
                for (i, p) in params.iter().enumerate() {
                    if let Some(ann) = &p.ty {
                        self.try_unify(&param_vars[i], ann, *line, *col, "param annotation");
                    }
                }
                if let Some(ann) = ret {
                    self.try_unify(&ret_var, ann, *line, *col, "return annotation");
                }
                let func_t = Type::func(param_vars.clone(), ret_var.clone());
                self.bind(name.clone(), func_t);
                self.walk_function_body(params, body, &param_vars, &ret_var, *line, *col);
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
            Stmt::If {
                cond,
                then_body,
                elifs,
                else_body,
                ..
            } => {
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
            Stmt::For {
                var, iter, body, ..
            } => {
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
            Stmt::OnRender { body, .. } => {
                self.push_scope();
                for s in body {
                    self.walk_stmt(s);
                }
                self.pop_scope();
            }
            Stmt::OnClassEvent { param, body, .. } => {
                // Bind the dying-entity param as Unknown so the body
                // type-checks loosely; field reads on `e` defer to
                // runtime (matches non-strict's existing handling of
                // unknown receivers). Strict-mode validation that the
                // class is declared can ride a follow-on session.
                // Phase 9 session 7b.
                self.push_scope();
                self.bind(param.clone(), Type::Unknown);
                for s in body {
                    self.walk_stmt(s);
                }
                self.pop_scope();
            }
            Stmt::Expr(e) => {
                let _ = self.expr_type(e);
            }
            Stmt::Wait { duration, .. } => {
                // The duration expression should be a quantity with a
                // time unit. We don't enforce that in non-strict;
                // just walk the expression so any sub-expressions get
                // their types resolved.
                let _ = self.expr_type(duration);
            }
            Stmt::Then { action, body, .. } => {
                // `<action> then <body>`: walk the action (its type is
                // the wait duration) and the body for diagnostics.
                let _ = self.expr_type(action);
                self.push_scope();
                for s in body {
                    self.walk_stmt(s);
                }
                self.pop_scope();
            }
            Stmt::DialogueDecl { name, body, .. } => {
                // Treat a dialogue decl like a parameterless
                // function: bind the name as a callable in scope,
                // walk the body so member expressions resolve. The
                // body's `say`/`choice`/`wait` arms type-check via
                // their own arms.
                let dialogue_ty = crate::types::Type::func(Vec::new(), crate::types::Type::Unknown);
                self.scopes
                    .last_mut()
                    .unwrap()
                    .insert(name.clone(), dialogue_ty);
                for s in body {
                    self.walk_stmt(s);
                }
            }
            Stmt::Say { actor, text, .. } => {
                if let Some(a) = actor {
                    let _ = self.expr_type(a);
                }
                let _ = self.expr_type(text);
            }
            Stmt::Choice { branches, .. } => {
                for (label, body) in branches {
                    let _ = self.expr_type(label);
                    for s in body {
                        self.walk_stmt(s);
                    }
                }
            }
            // Statements that don't introduce inference signal:
            Stmt::Break { .. }
            | Stmt::Continue { .. }
            | Stmt::Transition { .. }
            | Stmt::Spawn { .. }
            | Stmt::Despawn { .. }
            | Stmt::Import { .. } => {}
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
        // expressions. Phase 6 session 4: when a field carries
        // an explicit annotation, unify the value's type against
        // it (strict surfaces a mismatch) and record the
        // annotation as the field's canonical type.
        for m in members {
            if let DeclMember::Field {
                name,
                value,
                ty,
                line,
                col,
            } = m
            {
                let inferred = self.expr_type(value);
                if let Some(annotated) = ty {
                    self.try_unify(&inferred, annotated, *line, *col, "field annotation");
                    shape.insert(name.clone(), annotated.clone());
                } else {
                    shape.insert(name.clone(), self.resolve(&inferred));
                }
            }
        }

        // Pass 2: register placeholder method signatures with
        // fresh vars. Methods need to live in the shape too so
        // call sites like `instance.method()` can resolve via
        // the same Field-on-Instance lookup field access uses.
        // Stored as Type::Function — call dispatch flows through.
        // Phase 6 session 4: param + return annotations on the
        // method are unified against the fresh vars at this
        // point so usage refines (or violates, in strict) them.
        let mut method_meta: Vec<(String, Vec<Type>, Type)> = Vec::new();
        for m in members {
            if let DeclMember::Method {
                name,
                params,
                ret,
                line,
                col,
                ..
            } = m
            {
                let pv: Vec<Type> = params.iter().map(|_| self.fresh_var()).collect();
                let rv = self.fresh_var();
                for (i, p) in params.iter().enumerate() {
                    if let Some(ann) = &p.ty {
                        self.try_unify(&pv[i], ann, *line, *col, "method param annotation");
                    }
                }
                if let Some(ann) = ret {
                    self.try_unify(&rv, ann, *line, *col, "method return annotation");
                }
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
            if let DeclMember::Method {
                params,
                body,
                line,
                col,
                ..
            } = m
            {
                let (_, pv, rv) = &method_meta[method_count_up_to(members, m_idx)];
                self.push_scope();
                let prev_class = self.current_class.take();
                self.current_class = Some(class_name.to_string());
                for (p, t) in params.iter().zip(pv.iter()) {
                    self.bind(p.name.clone(), t.clone());
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
                let mut returns: Vec<Type> = Vec::new();
                self.walk_function_block(body, &mut returns);
                self.finalise_return_type(rv, returns, *line, *col);
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
    /// to their fresh vars; `return` statements collect into
    /// `returns` so the post-walk pass can union them and unify
    /// the result against `ret_var`. Local lets inside the body
    /// get their own scope-chain entries.
    fn walk_function_body(
        &mut self,
        params: &[crate::ast::Param],
        body: &[Stmt],
        param_vars: &[Type],
        ret_var: &Type,
        line: u32,
        col: u32,
    ) {
        self.push_scope();
        for (p, ty) in params.iter().zip(param_vars.iter()) {
            self.bind(p.name.clone(), ty.clone());
        }
        let mut returns: Vec<Type> = Vec::new();
        self.walk_function_block(body, &mut returns);
        self.finalise_return_type(ret_var, returns, line, col);
        self.pop_scope();
    }

    /// Union the collected return types and unify the result
    /// against `ret_var`. A function with multiple distinct
    /// return types yields a Union; one or more `return nil`
    /// statements alongside concrete returns yields Optional.
    /// Zero return statements leaves `ret_var` unconstrained
    /// (caller-side may pin it via a call site, otherwise it
    /// prints as `?N` per non-strict).
    fn finalise_return_type(&mut self, ret_var: &Type, returns: Vec<Type>, line: u32, col: u32) {
        if returns.is_empty() {
            return;
        }
        let resolved: Vec<Type> = returns.iter().map(|t| self.resolve(t)).collect();
        let combined = Type::union(resolved);
        self.try_unify(ret_var, &combined, line, col, "return");
    }

    /// Walk a block-of-statements that's part of a function body,
    /// collecting every `return X` value's type into `returns`
    /// (later unioned by `finalise_return_type` for multi-return
    /// functions). Recurses into nested if/while/for so deeper
    /// returns also bubble up to the outer signature.
    fn walk_function_block(&mut self, stmts: &[Stmt], returns: &mut Vec<Type>) {
        for stmt in stmts {
            match stmt {
                Stmt::Return { value, .. } => match value {
                    Some(v) => {
                        let t = self.expr_type(v);
                        returns.push(t);
                    }
                    None => {
                        returns.push(Type::Nil);
                    }
                },
                Stmt::Let { name, value, .. } => {
                    let t = self.expr_type(value);
                    self.bind(name.clone(), t);
                }
                Stmt::Assign { value, .. } => {
                    let _ = self.expr_type(value);
                }
                Stmt::If {
                    cond,
                    then_body,
                    elifs,
                    else_body,
                    ..
                } => {
                    let _ = self.expr_type(cond);
                    self.walk_function_block(then_body, returns);
                    for (c, body) in elifs {
                        let _ = self.expr_type(c);
                        self.walk_function_block(body, returns);
                    }
                    if let Some(eb) = else_body {
                        self.walk_function_block(eb, returns);
                    }
                }
                Stmt::While { cond, body, .. } => {
                    let _ = self.expr_type(cond);
                    self.walk_function_block(body, returns);
                }
                Stmt::For {
                    var, iter, body, ..
                } => {
                    let iter_t = self.expr_type(iter);
                    let elem_t = match self.resolve(&iter_t) {
                        Type::List(elem) => (*elem).clone(),
                        Type::Range => Type::Int,
                        _ => Type::Unknown,
                    };
                    self.push_scope();
                    self.bind(var.clone(), elem_t);
                    self.walk_function_block(body, returns);
                    self.pop_scope();
                }
                Stmt::Expr(e) => {
                    let _ = self.expr_type(e);
                }
                Stmt::FunctionDecl {
                    name,
                    params,
                    ret,
                    body,
                    line,
                    col,
                    ..
                } => {
                    // Nested function decl — same logic as top
                    // level, isolated from the enclosing return.
                    let pv: Vec<Type> = params.iter().map(|_| self.fresh_var()).collect();
                    let rv = self.fresh_var();
                    for (i, p) in params.iter().enumerate() {
                        if let Some(ann) = &p.ty {
                            self.try_unify(&pv[i], ann, *line, *col, "param annotation");
                        }
                    }
                    if let Some(ann) = ret {
                        self.try_unify(&rv, ann, *line, *col, "return annotation");
                    }
                    self.bind(name.clone(), Type::func(pv.clone(), rv.clone()));
                    self.walk_function_body(params, body, &pv, &rv, *line, *col);
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
            // Phase 33 session 9: typed hole. The inferer treats it
            // as a fresh type variable so it unifies with whatever
            // the surrounding context requires — that's how verify
            // can later report "expected type at this hole." Strict
            // mode does NOT push an error here; the hole is reported
            // separately by `verify::collect_holes` as a Warning.
            Expr::Hole { .. } => self.fresh_var(),
            Expr::Ident { name, line, col } => {
                match self.lookup(name) {
                    Some(t) => t,
                    None => {
                        // Phase 6 session 5: strict mode surfaces
                        // unknown identifiers as a "did you mean"
                        // diagnostic. Non-strict drops to Unknown
                        // silently — no false-positive contract.
                        if self.strict {
                            let names: Vec<&String> =
                                self.scopes.iter().flat_map(|s| s.keys()).collect();
                            let suggestion =
                                crate::value::did_you_mean(name, &names).map(str::to_string);
                            self.errors.push(TypeError {
                                line: *line,
                                col: *col,
                                message: format!("unknown name `{name}`"),
                                help: match suggestion {
                                    Some(s) => Some(format!("did you mean `{s}`?")),
                                    None => Some(
                                        "names must be declared with `let` / `var` / `function` / a class definition before use"
                                            .to_string(),
                                    ),
                                },
                            });
                        }
                        Type::Unknown
                    }
                }
            }
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
            Expr::ListComp {
                element,
                var,
                iterable,
                condition,
                ..
            } => {
                // Bind the loop var to the iterable's element type, then
                // infer the element expression — same scoping as a
                // `for` loop. Returns List<element-type>.
                let iter_t = self.expr_type(iterable);
                let elem_t = match self.resolve(&iter_t) {
                    Type::List(elem) => (*elem).clone(),
                    Type::Range => Type::Int,
                    _ => Type::Unknown,
                };
                self.push_scope();
                self.bind(var.clone(), elem_t);
                if let Some(cond) = condition {
                    let _ = self.expr_type(cond);
                }
                let element_t = self.expr_type(element);
                self.pop_scope();
                Type::list(element_t)
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
            Expr::Call {
                callee,
                args,
                line,
                col,
                ..
            } => {
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
                            self.try_unify(a, p, *line, *col, "call argument");
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
            Expr::Binary {
                op,
                left,
                right,
                line,
                col,
            } => {
                let l = self.expr_type(left);
                let r = self.expr_type(right);
                self.binop_type(*op, &l, &r, *line, *col)
            }
            Expr::IfExpr {
                cond,
                then_expr,
                elifs,
                else_expr,
                line,
                col,
            } => {
                // Cond must be Bool. Each branch is an expression; we
                // unify them so a known concrete on any arm pins fresh
                // vars on the others (mirrors `and`/`or` value-returning
                // strategy from F11). Result type is the unified arm type.
                let c = self.expr_type(cond);
                self.try_unify(&c, &Type::Bool, *line, *col, "if-expression condition");
                let t = self.expr_type(then_expr);
                let mut acc = t;
                for (elif_cond, elif_expr) in elifs {
                    let ec = self.expr_type(elif_cond);
                    self.try_unify(&ec, &Type::Bool, *line, *col, "elif-expression condition");
                    let ev = self.expr_type(elif_expr);
                    self.try_unify(&acc, &ev, *line, *col, "if-expression branches");
                }
                let e = self.expr_type(else_expr);
                self.try_unify(&acc, &e, *line, *col, "if-expression branches");
                acc = self.resolve(&acc);
                acc
            }
        }
    }

    fn binop_type(&mut self, op: BinOp, l: &Type, r: &Type, line: u32, col: u32) -> Type {
        match op {
            BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt | BinOp::Lte | BinOp::Gte => {
                // Unify operand types so a known concrete on one
                // side pins a fresh var on the other (e.g. `n < 0`
                // pins n : int). Failure is silently absorbed in
                // non-strict; strict mode reports a "comparison"
                // mismatch.
                self.try_unify(l, r, line, col, "comparison");
                Type::Bool
            }
            BinOp::In | BinOp::NotIn => Type::Bool,
            BinOp::And | BinOp::Or => {
                // Value-returning short-circuit per the F11 decision.
                // Result type is the common type of both sides;
                // unify them so a known type on either side flows.
                self.try_unify(l, r, line, col, "and/or operands");
                self.resolve(l)
            }
            BinOp::Add => self.infer_add(l, r, line, col),
            BinOp::Sub | BinOp::Mul | BinOp::Div => self.infer_arith(op, l, r, line, col),
        }
    }

    fn infer_add(&mut self, l: &Type, r: &Type, line: u32, col: u32) -> Type {
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
                self.try_unify(&lr, &Type::Str, line, col, "string `+`");
                self.try_unify(&rr, &Type::Str, line, col, "string `+`");
                return Type::Str;
            }
            _ => {}
        }
        self.infer_arith(BinOp::Add, l, r, line, col)
    }

    fn infer_arith(&mut self, op: BinOp, l: &Type, r: &Type, line: u32, col: u32) -> Type {
        let lr = self.resolve(l);
        let rr = self.resolve(r);
        // Dimensional unit checking. Per docs/02-type-system.md
        // "5m + 3s" is a type error — the units don't match, so
        // the operation has no defined result. Per non-strict
        // semantics we emit Unknown silently rather than raising;
        // strict mode (v0.2) will surface this as an error.
        // Same-unit + / - preserves the unit. * / / by a number
        // is scaling and keeps the unit. Quantity / Quantity of
        // the same unit produces a unitless float (the units
        // cancel). Mixed units in * or / produce a combined unit
        // string (e.g. `m/s`).
        if let (Type::Quantity(u1), Type::Quantity(u2)) = (&lr, &rr) {
            return self.infer_quantity_arith(op, u1, u2);
        }
        // Quantity scaled by a number on either side.
        if let Type::Quantity(unit) = &lr {
            if matches!(op, BinOp::Mul | BinOp::Div) && is_scalar_type(&rr) {
                return Type::Quantity(unit.clone());
            }
        }
        if let Type::Quantity(unit) = &rr {
            if matches!(op, BinOp::Mul) && is_scalar_type(&lr) {
                return Type::Quantity(unit.clone());
            }
        }
        if let (Type::Tuple(a), Type::Tuple(b)) = (&lr, &rr) {
            if matches!(op, BinOp::Add | BinOp::Sub) && a.len() == b.len() {
                let elems: Vec<Type> = a
                    .iter()
                    .zip(b.iter())
                    .map(|(x, y)| self.infer_arith(op, x, y, line, col))
                    .collect();
                return Type::Tuple(elems);
            }
        }
        if let Type::Tuple(elems) = &lr {
            if matches!(op, BinOp::Mul | BinOp::Div) && is_scalar_type(&rr) {
                let elems: Vec<Type> = elems
                    .iter()
                    .map(|x| self.infer_arith(op, x, &rr, line, col))
                    .collect();
                return Type::Tuple(elems);
            }
        }
        if let Type::Tuple(elems) = &rr {
            if matches!(op, BinOp::Mul) && is_scalar_type(&lr) {
                let elems: Vec<Type> = elems
                    .iter()
                    .map(|y| self.infer_arith(op, &lr, y, line, col))
                    .collect();
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
                self.try_unify(&lr, other, line, col, "arithmetic");
                self.try_unify(&rr, other, line, col, "arithmetic");
                other.clone()
            }
            (Type::Var(_), Type::Var(_)) => {
                self.try_unify(&lr, &rr, line, col, "arithmetic");
                let v = self.fresh_var();
                self.try_unify(&lr, &v, line, col, "arithmetic");
                v
            }
            _ => {
                // Two concrete types that don't pattern-match —
                // this is the heart of "5m + 3s is a type error"
                // (per docs/02 §"Dimensional units"). Strict mode
                // surfaces it; non-strict drops to Unknown.
                if self.strict {
                    self.errors.push(TypeError {
                        line,
                        col,
                        message: format!(
                            "arithmetic: type mismatch — {} vs {}",
                            self.resolve(&lr),
                            self.resolve(&rr)
                        ),
                        help: Some(
                            "operands of `+` / `-` / `*` / `/` must be numeric (or matching units / tuples)".to_string(),
                        ),
                    });
                }
                Type::Unknown
            }
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
    /// fully substituted. Called after `walk_program`. Excludes
    /// the stdlib seed names (Phase 6 session 5) — those are
    /// scope-machinery for strict-mode resolution, not user
    /// bindings, so `twec types <file>` shouldn't print them.
    fn resolved_top_level(&self) -> Bindings {
        let mut out = Bindings::new();
        let seeds: std::collections::HashSet<&str> = stdlib_names().iter().copied().collect();
        for (name, ty) in &self.scopes[0] {
            if seeds.contains(name.as_str()) {
                continue;
            }
            out.insert(name.clone(), self.resolve(ty));
        }
        out
    }
}

fn is_scalar_type(t: &Type) -> bool {
    matches!(t, Type::Int | Type::Float)
}

/// Phase 13 session 6: does `union` (a Union or Optional, which is
/// `T | Nil`) contain `variant` as one of its members under
/// `is_compatible_with`? Used by strict-lax narrowing to suppress
/// a mismatch when the *provided* type is a wider union and the
/// *expected* type is one of its variants — the user's annotation
/// acts as an implicit narrowing assertion.
///
/// Returns false for non-Union/Optional types so callers can use
/// it as a one-shot rescue without first pattern-matching.
fn union_contains_variant(union: &Type, variant: &Type) -> bool {
    match union {
        Type::Union(parts) => parts.iter().any(|p| p.is_compatible_with(variant)),
        Type::Optional(inner) => matches!(variant, Type::Nil) || inner.is_compatible_with(variant),
        _ => false,
    }
}

/// Names the stdlib registers as globals — the seed list for
/// strict-mode identifier resolution. Mirrors the `install_*`
/// functions in `src/stdlib.rs`. Plus `true` / `false` (parser-
/// recognised literal forms) and `self` / `nil` (also handled at
/// the parser level, listed here so the strict-mode unknown-name
/// check has uniform coverage). Phase 6 session 5.
///
/// When the stdlib grows (or shrinks) a top-level binding, this
/// list needs an update — it's a parallel registry. A future
/// session that pulls signatures from a single shared
/// `stdlib::globals()` table can replace this. Until then, drift
/// here means strict mode complains about a real builtin or
/// silently accepts a typo.
fn stdlib_names() -> &'static [&'static str] {
    &[
        // top-level builtins
        "print",
        "load",
        "vec3",
        "cube",
        "rect",
        "circle",
        "line",
        "text",
        "sprite",
        "sound",
        "screen",
        "time",
        "math",
        "random",
        "key",
        "key_press",
        "color",
        "entities",
        "camera",
        // rarity tier symbols (installed in stdlib::install)
        "common",
        "uncommon",
        "rare",
        "epic",
        "legendary",
        // boolean literal forms accepted by the parser
        "true",
        "false",
        // self / nil — both have explicit Expr handling but listing
        // them keeps the strict check's coverage uniform
        "self",
        "nil",
    ]
}

impl Inferer {
    /// Dimensional arithmetic between two `Type::Quantity`.
    ///
    /// Rules:
    /// - Add / Sub: units must match (same unit string). On
    ///   mismatch, return Unknown (non-strict's no-false-
    ///   positive escape; strict mode will surface as error).
    /// - Mul: combine units as `a*b` (kg*m, etc.). Same unit
    ///   squared writes as `unit^2`.
    /// - Div: same unit cancels to a unitless `Float`.
    ///   Different units write as `a/b` (m/s).
    ///
    /// The combined unit string is purely textual — Phase 4f
    /// doesn't yet parse compound units back into structured
    /// dimensions. Strict mode + a future dimension solver would
    /// canonicalise (`kg*m*s` vs `m*kg*s`).
    fn infer_quantity_arith(&self, op: BinOp, u1: &Rc<String>, u2: &Rc<String>) -> Type {
        match op {
            BinOp::Add | BinOp::Sub => {
                if u1 == u2 {
                    Type::Quantity(u1.clone())
                } else {
                    Type::Unknown
                }
            }
            BinOp::Mul => {
                let combined = if u1 == u2 {
                    format!("{u1}^2")
                } else {
                    format!("{u1}*{u2}")
                };
                Type::Quantity(Rc::new(combined))
            }
            BinOp::Div => {
                if u1 == u2 {
                    // Same unit cancels — `5m / 3m` is unitless.
                    Type::Float
                } else {
                    Type::Quantity(Rc::new(format!("{u1}/{u2}")))
                }
            }
            _ => Type::Unknown,
        }
    }
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

    fn strict_errors(src: &str) -> Vec<TypeError> {
        let with_newline = format!("{src}\n");
        let tokens = lexer::lex(&with_newline).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let strict = detect_strict(&with_newline);
        let (_bindings, errors) = infer_program_strict(&program, strict);
        errors
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
        assert_eq!(
            bs.get("Hero").map(|t| t.to_string()).as_deref(),
            Some("<class Hero>")
        );
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
        let bs = types_of("function double(x):\n    return x * 2\n\nlet result = double(5)\n");
        assert_eq!(bs.get("result"), Some(&Type::Int));
    }

    #[test]
    fn nested_function_decls_are_walked() {
        let bs = types_of(
            "function outer():\n    function inner(x):\n        return x + 1\n    return inner(5)\n",
        );
        // outer's return came from inner(5) which is int.
        assert_eq!(
            bs.get("outer").map(|t| t.to_string()).as_deref(),
            Some("function() -> int")
        );
    }

    #[test]
    fn for_over_list_binds_loop_var_to_element_type() {
        // The loop var only appears inside the body; the binding
        // doesn't escape the for. We test indirectly: a function
        // that iterates a list and returns the loop var, then a
        // sentinel int. With 4e's multi-return unions, the
        // function returns `int | ?M` (where ?M is xs's element
        // type that we couldn't pin from inside the loop). The
        // important thing is the return type *includes* int; we
        // don't assert the precise union shape because the var
        // numbering depends on allocation order.
        let bs = types_of("function head(xs):\n    for x in xs:\n        return x\n    return 0\n");
        let t = bs.get("head").expect("head binding");
        let s = t.to_string();
        assert!(s.contains("int"), "got: {s}");
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
        let bs = types_of("function double(x):\n    return x * 2\n\nlet result = double(\"hi\")\n");
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
        let bs = types_of("item Counter:\n    value: 0\n\nlet c = Counter()\nlet n = c.value\n");
        assert_eq!(
            bs.get("c").map(|t| t.to_string()).as_deref(),
            Some("Counter")
        );
        assert_eq!(bs.get("n"), Some(&Type::Int));
    }

    #[test]
    fn instance_field_with_string_default_resolves_to_string() {
        let bs = types_of("item NPC:\n    name: \"Bob\"\n\nlet npc = NPC()\nlet who = npc.name\n");
        assert_eq!(bs.get("who"), Some(&Type::Str));
    }

    #[test]
    fn instance_unknown_field_falls_back_to_unknown() {
        let bs = types_of("item Counter:\n    value: 0\n\nlet c = Counter()\nlet x = c.glubjorm\n");
        // Per non-strict ("no false positives"): unknown field
        // is allowed at parse time, returns Unknown.
        assert_eq!(bs.get("x"), Some(&Type::Unknown));
    }

    #[test]
    fn entity_with_var_field_carries_inferred_type() {
        let bs = types_of("entity Mob:\n    var hp = 100\n\nlet m = Mob()\nlet h = m.hp\n");
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

    // --- 4e: Optional + Union from multi-return functions ---

    #[test]
    fn function_with_return_value_or_nil_yields_optional() {
        // Bare `return` (no value) is how Twe writes the nil
        // case. Combined with `return n` (int), the return
        // type is int?.
        let bs = types_of("function maybe(n):\n    if n > 0:\n        return n\n    return\n");
        let t = bs.get("maybe").expect("maybe binding");
        let s = t.to_string();
        assert!(s.contains("-> int?"), "got: {s}");
    }

    #[test]
    fn function_returning_distinct_types_yields_union() {
        // Two return statements with different concrete types
        // (int vs str) → return type is `int | string`.
        let bs =
            types_of("function pick(flag):\n    if flag:\n        return 1\n    return \"hi\"\n");
        let t = bs.get("pick").expect("pick binding");
        let s = t.to_string();
        assert!(
            s.contains("int | string") || s.contains("string | int"),
            "got: {s}"
        );
    }

    #[test]
    fn function_with_all_same_return_type_stays_singleton() {
        // Two `return X` of the same type → just X, not X | X.
        let bs = types_of("function abs_(n):\n    if n < 0:\n        return -n\n    return n\n");
        let t = bs.get("abs_").expect("abs binding");
        assert_eq!(t.to_string(), "function(int) -> int");
    }

    #[test]
    fn bare_return_with_no_value_yields_nil_branch() {
        // `return` with no value contributes a Nil to the union.
        // Combined with `return X`, the result is X?.
        let bs = types_of("function maybe2(n):\n    if n < 0:\n        return\n    return n + 1\n");
        let t = bs.get("maybe2").expect("maybe2 binding");
        let s = t.to_string();
        assert!(s.contains("-> int?"), "got: {s}");
    }

    // --- 4f: dimensional unit arithmetic ---

    #[test]
    fn same_unit_addition_preserves_unit() {
        // 100ms + 50ms : quantity<ms>
        let bs = types_of("let a = 100ms\nlet b = 50ms\nlet c = a + b\n");
        assert_eq!(
            bs.get("c").map(|t| t.to_string()).as_deref(),
            Some("quantity<ms>")
        );
    }

    #[test]
    fn mismatched_units_falls_back_to_unknown() {
        // 5m + 3s — units don't match; per non-strict, the
        // operation produces Unknown rather than raising. Strict
        // mode (v0.2) will surface this as an error.
        let bs = types_of("let dist = 5m\nlet time = 3s\nlet broken = dist + time\n");
        assert_eq!(bs.get("broken"), Some(&Type::Unknown));
    }

    #[test]
    fn quantity_times_scalar_keeps_unit() {
        // 100ms * 2 -> quantity<ms>; 3 * 100ms -> quantity<ms>
        let bs = types_of("let dt = 100ms\nlet doubled = dt * 2\nlet tripled = 3 * dt\n");
        assert_eq!(
            bs.get("doubled").map(|t| t.to_string()).as_deref(),
            Some("quantity<ms>")
        );
        assert_eq!(
            bs.get("tripled").map(|t| t.to_string()).as_deref(),
            Some("quantity<ms>")
        );
    }

    #[test]
    fn quantity_div_quantity_same_unit_cancels_to_float() {
        // 100ms / 50ms is unitless.
        let bs = types_of("let a = 100ms\nlet b = 50ms\nlet ratio = a / b\n");
        assert_eq!(bs.get("ratio"), Some(&Type::Float));
    }

    #[test]
    fn quantity_div_quantity_different_units_combines() {
        // 5m / 3s -> quantity<m/s> (velocity, by convention)
        let bs = types_of("let dist = 5m\nlet time = 3s\nlet speed = dist / time\n");
        assert_eq!(
            bs.get("speed").map(|t| t.to_string()).as_deref(),
            Some("quantity<m/s>")
        );
    }

    #[test]
    fn quantity_mul_quantity_combines_units() {
        // 5kg * 9m -> quantity<kg*m>
        let bs = types_of("let m = 5kg\nlet d = 9m\nlet work = m * d\n");
        assert_eq!(
            bs.get("work").map(|t| t.to_string()).as_deref(),
            Some("quantity<kg*m>")
        );
    }

    #[test]
    fn quantity_squared_uses_caret_notation() {
        // Same unit on both sides of mul -> unit^2.
        let bs = types_of("let s = 10m\nlet area = s * s\n");
        assert_eq!(
            bs.get("area").map(|t| t.to_string()).as_deref(),
            Some("quantity<m^2>")
        );
    }

    #[test]
    fn scene_field_shape_works_too() {
        let bs = types_of("scene S:\n    var n = 0\n    initial: a\n    state a:\n");
        // We can't easily access scene fields from outside a
        // scene declaration without an explicit reference, but
        // we can at least confirm the class binds and the shape
        // is recorded. Test indirectly: a method-style access
        // through Instance(S) — there's no constructor for
        // scenes (they auto-instantiate at runtime), so we
        // can't test access syntactically. This test just
        // confirms the type for S binds without error.
        assert_eq!(
            bs.get("S").map(|t| t.to_string()).as_deref(),
            Some("<class S>")
        );
    }

    // --- Phase 6 session 1: strict mode ---

    #[test]
    fn detect_strict_recognises_canonical_directive() {
        assert!(detect_strict("# strict\nlet x = 1"));
        assert!(detect_strict("#! strict\nlet x = 1"));
        assert!(detect_strict("#strict\nlet x = 1"));
        assert!(detect_strict("#!strict\nlet x = 1"));
    }

    #[test]
    fn detect_strict_finds_directive_in_first_ten_lines() {
        let src = "# the program\n\n# strict\nlet x = 1";
        assert!(detect_strict(src));
    }

    #[test]
    fn detect_strict_ignores_directive_past_first_ten_lines() {
        // Eleven blank/non-directive lines first, then the magic
        // comment — should NOT trigger strict mode.
        let mut src = String::new();
        for _ in 0..11 {
            src.push_str("# noise\n");
        }
        src.push_str("# strict\n");
        src.push_str("let x = 1\n");
        assert!(!detect_strict(&src));
    }

    #[test]
    fn detect_strict_ignores_partial_match() {
        // `# strict mode` (with trailing words) is not the
        // canonical directive — keep the surface tight.
        assert!(!detect_strict("# strict mode\nlet x = 1"));
        assert!(!detect_strict("# strict-ish\nlet x = 1"));
    }

    #[test]
    fn non_strict_program_collects_no_errors() {
        // Same shape as the strict test below, but no directive.
        // The unification failures still happen internally; they
        // just stay silent.
        let errors = strict_errors("let bad = \"hi\" < 5\n");
        assert!(
            errors.is_empty(),
            "non-strict should drop errors, got {errors:?}"
        );
    }

    #[test]
    fn strict_mode_surfaces_comparison_mismatch() {
        // `<` between Str and Int — unify fails. Strict mode
        // surfaces it as a "comparison: type mismatch" error.
        let errors = strict_errors("# strict\nlet bad = \"hi\" < 5\n");
        assert_eq!(errors.len(), 1, "expected one error, got {errors:?}");
        let e = &errors[0];
        assert!(
            e.message.contains("comparison") && e.message.contains("type mismatch"),
            "got: {}",
            e.message
        );
    }

    #[test]
    fn strict_mode_surfaces_return_type_conflict() {
        // Multi-return functions where the branches disagree on
        // type should surface the union/unify failure under strict.
        // Note: with the current Optional+Union widening, two
        // distinct concrete returns produce a Union, so the
        // ret_var unifies cleanly. To trigger a conflict we need
        // a function whose body's USE pins ret_var to a concrete
        // type (e.g. via call-site arg unify) before the union
        // is built. The simplest fire is calling a function with
        // mismatched arg types so the call-site arg-unify fails.
        let errors =
            strict_errors("# strict\nfunction f(n):\n    return n + 1\n\nlet x = f(\"hi\") + 1\n");
        // The call site `f("hi")` passes Str where param is Int
        // (pinned by `n + 1` inside the body). Strict should
        // surface the call-arg mismatch.
        assert!(
            errors.iter().any(|e| e.message.contains("call argument")),
            "expected call-argument error, got {errors:?}"
        );
    }

    #[test]
    fn strict_mode_carries_line_and_col_from_source() {
        let errors = strict_errors("# strict\nlet bad = \"hi\" < 5\n");
        assert_eq!(errors.len(), 1);
        // Line 2 of the program (line 1 is the directive).
        assert_eq!(errors[0].line, 2);
        // Column points at the `<` operator (col 16: 1-indexed
        // position right after `"hi" `).
        assert!(
            errors[0].col >= 11 && errors[0].col <= 20,
            "col was {}",
            errors[0].col
        );
    }

    #[test]
    fn strict_mode_includes_help_text() {
        let errors = strict_errors("# strict\nlet bad = \"hi\" < 5\n");
        assert!(errors[0].help.is_some(), "errors should carry help text");
    }

    // --- Phase 6 session 2: annotation-driven enforcement ---

    #[test]
    fn strict_let_annotation_violation_surfaces() {
        // `let x: int = "hi"` — annotated int, value is string.
        // Strict surfaces a "let annotation: type mismatch."
        let errors = strict_errors("# strict\nlet x: int = \"hi\"\n");
        assert!(
            errors.iter().any(|e| e.message.contains("let annotation")),
            "expected let-annotation error, got {errors:?}"
        );
    }

    #[test]
    fn strict_let_annotation_clean_passes() {
        let errors = strict_errors("# strict\nlet x: int = 42\n");
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
    }

    #[test]
    fn strict_param_annotation_pins_arg_check() {
        // Without annotations, `add(n)` would accept any n. With
        // `n: int`, the param's fresh var is unified to Int at
        // function-decl time; the call site `add("hi")` then fails
        // the "call argument" unify in strict mode.
        let src = "# strict\nfunction add(n: int):\n    return n + 1\n\nadd(\"hi\")\n";
        let errors = strict_errors(src);
        assert!(
            errors.iter().any(|e| e.message.contains("call argument")),
            "expected call-argument error from annotated param, got {errors:?}"
        );
    }

    #[test]
    fn strict_return_annotation_violation_surfaces() {
        // `function f() -> int: return "hi"` — annotated int return,
        // body returns string. The return-type union becomes Str;
        // unifying against the int-pinned ret_var fails.
        let src = "# strict\nfunction f() -> int:\n    return \"hi\"\n";
        let errors = strict_errors(src);
        assert!(
            !errors.is_empty(),
            "expected an error from return-annotation violation, got none"
        );
    }

    #[test]
    fn non_strict_drops_annotation_violations() {
        // Same shape as the strict cases but no `# strict`. Errors
        // stay silent — the v0.1 default contract.
        let errors_let = strict_errors("let x: int = \"hi\"\n");
        let errors_param =
            strict_errors("function add(n: int):\n    return n + 1\n\nadd(\"hi\")\n");
        let errors_ret = strict_errors("function f() -> int:\n    return \"hi\"\n");
        assert!(errors_let.is_empty());
        assert!(errors_param.is_empty());
        assert!(errors_ret.is_empty());
    }

    #[test]
    fn unrecognised_type_name_silently_skips_enforcement() {
        // `User` is not a primitive. Strict mode shouldn't error
        // just because the annotation refers to a class we don't
        // model — `parse_type` returns `None` and the inferer
        // never gets an annotation to unify against.
        let errors = strict_errors("# strict\nlet u: User = 42\n");
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
    }

    // --- Phase 6 session 5: strict-mode identifier resolution ---

    #[test]
    fn strict_unknown_identifier_surfaces() {
        // `gibberish_name` isn't bound anywhere. Strict reports
        // "unknown name" with a help line; non-strict drops to
        // Type::Unknown silently.
        let errors = strict_errors("# strict\nlet x = gibberish_name + 1\n");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("unknown name `gibberish_name`")),
            "expected unknown-name error, got {errors:?}"
        );
    }

    #[test]
    fn strict_unknown_identifier_suggests_close_match() {
        // `goblion` (mid-word `i` typo) is one edit-distance from
        // user-bound `goblin` — strict surfaces "did you mean".
        let src = "# strict\nlet goblin = 42\nlet x = goblion + 1\n";
        let errors = strict_errors(src);
        let unknown = errors
            .iter()
            .find(|e| e.message.contains("unknown name `goblion`"))
            .expect("expected unknown-name error");
        assert!(
            unknown
                .help
                .as_ref()
                .map(|h| h.contains("did you mean `goblin`?"))
                .unwrap_or(false),
            "expected did-you-mean suggestion, got {:?}",
            unknown.help
        );
    }

    #[test]
    fn strict_doesnt_complain_about_stdlib_names() {
        // `print`, `vec3`, `math`, etc. are seeded into the
        // outermost scope so a strict program doesn't get one
        // diagnostic per stdlib call.
        let src = "# strict\nprint(\"hi\")\nlet v = vec3(1, 2, 3)\n";
        let errors = strict_errors(src);
        // The `print` and `vec3` calls return Type::Unknown
        // (we don't have signatures), so no `let` annotation
        // unifies; should be no errors at all.
        assert!(
            errors.is_empty(),
            "stdlib names shouldn't trip strict mode, got {errors:?}"
        );
    }

    #[test]
    fn non_strict_doesnt_report_unknown_identifier() {
        // The existing `unknown_propagates_silently` test pins
        // this for non-strict via `infer_program`; this one pins
        // it for `infer_program_strict(_, false)` too.
        let errors = strict_errors("let x = totally_unknown_name + 1\n");
        assert!(errors.is_empty(), "non-strict must drop, got {errors:?}");
    }

    // --- Phase 6 session 4: class member annotations ---

    #[test]
    fn strict_field_annotation_violation_surfaces() {
        let src = "# strict\nentity Hero:\n    hp: int = \"hi\"\n";
        let errors = strict_errors(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("field annotation")),
            "expected field-annotation error, got {errors:?}"
        );
    }

    #[test]
    fn strict_field_annotation_clean_passes() {
        let src = "# strict\nentity Hero:\n    hp: int = 100\n";
        let errors = strict_errors(src);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
    }

    #[test]
    fn strict_method_param_annotation_pins_call_site() {
        // `take_damage(amount: int)` annotates the param. A
        // body that adds amount to a string would pin amount to
        // string, conflicting with the int annotation; strict
        // reports the param-annotation conflict.
        let src = concat!(
            "# strict\n",
            "entity Hero:\n",
            "    hp: int = 100\n",
            "    take_damage(amount: int):\n",
            "        return amount + \"oops\"\n",
        );
        let errors = strict_errors(src);
        assert!(
            !errors.is_empty(),
            "expected at least one strict-mode error, got none"
        );
    }

    #[test]
    fn strict_method_return_annotation_violation_surfaces() {
        // Method declares return int, body returns string.
        let src = concat!(
            "# strict\n",
            "entity Hero:\n",
            "    hp: int = 100\n",
            "    name() -> int:\n",
            "        return \"hi\"\n",
        );
        let errors = strict_errors(src);
        assert!(
            !errors.is_empty(),
            "expected return-annotation conflict to surface"
        );
    }

    #[test]
    fn non_strict_drops_class_annotation_violations() {
        let src = concat!(
            "entity Hero:\n",
            "    hp: int = \"hi\"\n",
            "    name() -> int:\n",
            "        return \"oops\"\n",
        );
        let errors = strict_errors(src);
        assert!(errors.is_empty(), "non-strict should drop, got {errors:?}");
    }

    // --- Phase 13 session 6: Luau-style lax-strict narrowing ---

    #[test]
    fn union_contains_variant_recognises_union_member() {
        let u = Type::union(vec![Type::Int, Type::Str]);
        assert!(union_contains_variant(&u, &Type::Int));
        assert!(union_contains_variant(&u, &Type::Str));
        assert!(!union_contains_variant(&u, &Type::Bool));
    }

    #[test]
    fn union_contains_variant_recognises_optional_inner_and_nil() {
        let opt = Type::optional(Type::Int);
        assert!(union_contains_variant(&opt, &Type::Int));
        assert!(union_contains_variant(&opt, &Type::Nil));
        assert!(!union_contains_variant(&opt, &Type::Str));
    }

    #[test]
    fn union_contains_variant_returns_false_for_non_unions() {
        // Not a Union or Optional — caller can dispatch through
        // this without first checking the shape.
        assert!(!union_contains_variant(&Type::Int, &Type::Int));
        assert!(!union_contains_variant(&Type::Str, &Type::Str));
    }

    #[test]
    fn strict_lax_accepts_narrowing_from_optional_to_inner() {
        // Function body has paths returning int and (implicitly via
        // an unguarded `if`) nil. The inferred return widens to a
        // union containing both. An explicit `-> int` annotation
        // would, under strict-strict, surface a return mismatch.
        // Lax accepts: the user's annotation says int, and that's
        // implicitly a runtime narrowing assertion.
        //
        // The simplest concrete fire: build a function whose body
        // returns `int | nil` and annotate it `-> int`. Today the
        // language doesn't track implicit-nil-fall-through, so we
        // simulate the situation by returning literal nil from one
        // branch.
        let src = concat!(
            "# strict\n",
            "function f(c) -> int:\n",
            "    if c:\n",
            "        return 1\n",
            "    else:\n",
            "        return nil\n",
        );
        let errors = strict_errors(src);
        assert!(
            errors.is_empty(),
            "lax narrowing should accept int|nil where int annotated; got {errors:?}",
        );
    }

    #[test]
    fn strict_lax_accepts_narrowing_from_union_to_member() {
        // Same idea via an explicit Int|Str return; an annotation
        // pinning Int should be accepted under lax.
        let src = concat!(
            "# strict\n",
            "function f(c) -> int:\n",
            "    if c:\n",
            "        return 1\n",
            "    else:\n",
            "        return \"hi\"\n",
        );
        let errors = strict_errors(src);
        assert!(
            errors.is_empty(),
            "lax narrowing should accept int|str where int annotated; got {errors:?}",
        );
    }

    #[test]
    fn strict_lax_still_rejects_unrelated_types() {
        // If the annotation doesn't match *any* variant of the
        // produced union, the strict error still fires. Lax only
        // suppresses when the contract names a real variant.
        let src = concat!(
            "# strict\n",
            "function f(c) -> bool:\n",
            "    if c:\n",
            "        return 1\n",
            "    else:\n",
            "        return \"hi\"\n",
        );
        let errors = strict_errors(src);
        assert!(
            !errors.is_empty(),
            "annotation that names no variant should still error",
        );
    }
}
