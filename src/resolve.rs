//! web3d-M1: lexical name resolution.
//!
//! Twe's tree-walker historically resolved names *dynamically*: one
//! flat global map, with a function's parameters saved, overwritten and
//! restored around each call. So a `let` inside a function leaked into
//! globals, a callee saw its caller's parameters, and a module function
//! resolved free names in the importer's environment
//! (`docs/changes/2026-09-27-web3d-pivot.md`, M1).
//!
//! This pass walks a `Program` once and resolves every identifier under
//! *lexical* rules — the rules M1 moves the runtime to:
//!
//! - **Frames.** Every function, method, dialogue and event-handler body
//!   (`on update`, `on render`, `every`, `on <predicate>`, `on key_press`,
//!   a state's entry / exit body, `on Class.death`) is its own frame.
//!   A frame sees its own locals and parameters, the enclosing class's
//!   fields and methods (inside `entity` / `scene` / … declarations),
//!   and module globals — never another frame's locals.
//! - **Blocks.** `if` / `elif` / `else`, `while`, `for`, `then`,
//!   `choice` branches and list comprehensions open nested scopes.
//! - **Globals** are the program's top-level `let` / `var`, functions,
//!   declarations, dialogues and imports — visible everywhere,
//!   regardless of order — plus the names the stdlib and runtime
//!   install ([`known_globals`]).
//!
//! Today it runs in *report* mode ([`check`]): it returns every use
//! the lexical rules can't resolve, classified by why, without changing
//! execution. That report is how the M1 migration finds programs that
//! depend on dynamic scoping before the runtime switches.

use std::collections::{HashMap, HashSet};

use crate::ast::{AssignTarget, DeclKind, DeclMember, Expr, Program, StateMember, Stmt};

/// Why a name couldn't be resolved lexically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IssueKind {
    /// Declared in a *different* frame (another function / handler)
    /// and only reachable today through dynamic scoping — e.g. reading
    /// a function's local after it returned, or a callee reading its
    /// caller's parameter.
    FrameLeak,
    /// Declared earlier in the same frame, but inside a block that has
    /// already ended (`if` / `for` / `while` body …).
    BlockEscape,
    /// Assigned (`x = …`) without ever being declared anywhere. Today
    /// that silently creates a global.
    AssignUndeclared,
    /// Read, but declared nowhere and not a known builtin — a typo, or
    /// a name only the runtime injects that [`known_globals`] misses.
    Undeclared,
}

impl IssueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            IssueKind::FrameLeak => "frame-leak",
            IssueKind::BlockEscape => "block-escape",
            IssueKind::AssignUndeclared => "assign-undeclared",
            IssueKind::Undeclared => "undeclared",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeIssue {
    pub kind: IssueKind,
    pub name: String,
    pub line: u32,
    pub col: u32,
}

/// Names visible in every program without a declaration: everything
/// `stdlib::install` binds, plus the ambients the runtime refreshes
/// each tick (`time`, input state, …), found empirically by installing
/// the stdlib into a fresh env and running one headless tick.
pub fn known_globals() -> HashSet<String> {
    let mut env = crate::value::Env::new();
    crate::stdlib::install(&mut env);
    let _ = crate::eval::tick_frame(&mut env, 0.0);
    let mut names: HashSet<String> = env.iter_bindings().map(|(k, _)| k.to_string()).collect();
    // Injected by `net.advance_tick` (lockstep netcode) only once a
    // session is running.
    names.insert("peer".to_string());
    names
}

/// Resolve every identifier in `program` lexically; return the uses
/// that don't resolve, sorted by position. `builtins` is normally
/// [`known_globals`].
pub fn check(program: &Program, builtins: &HashSet<String>) -> Vec<ScopeIssue> {
    let mut r = Resolver::new(program, builtins);
    r.program(program);
    r.finish()
}

#[derive(Default)]
struct Scope {
    names: HashSet<String>,
    /// Frame this scope belongs to (0 = top level).
    frame: u32,
}

struct Use {
    name: String,
    line: u32,
    col: u32,
    frame: u32,
    assign: bool,
}

struct Resolver<'a> {
    builtins: &'a HashSet<String>,
    /// Top-level declarations (visible everywhere, order-independent).
    globals: HashSet<String>,
    /// Class name → (parent, own fields + methods).
    classes: HashMap<String, (Option<String>, HashSet<String>)>,
    scopes: Vec<Scope>,
    /// Members visible through `self` in the current class context.
    class_ctx: Vec<HashSet<String>>,
    next_frame: u32,
    /// Every local declaration: name → frames declaring it.
    declared: HashMap<String, HashSet<u32>>,
    unresolved: Vec<Use>,
}

impl<'a> Resolver<'a> {
    fn new(program: &Program, builtins: &'a HashSet<String>) -> Self {
        let mut globals = HashSet::new();
        let mut classes = HashMap::new();
        for stmt in &program.stmts {
            match stmt {
                Stmt::Let { name, .. }
                | Stmt::FunctionDecl { name, .. }
                | Stmt::DialogueDecl { name, .. } => {
                    globals.insert(name.clone());
                }
                Stmt::Decl {
                    name,
                    parent,
                    members,
                    ..
                } => {
                    globals.insert(name.clone());
                    let mut own = HashSet::new();
                    for m in members {
                        match m {
                            DeclMember::Field { name, .. } | DeclMember::Method { name, .. } => {
                                own.insert(name.clone());
                            }
                            _ => {}
                        }
                    }
                    classes.insert(name.clone(), (parent.clone(), own));
                }
                Stmt::Import { path, alias, .. } => {
                    let bound = alias
                        .clone()
                        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_string());
                    globals.insert(bound);
                }
                _ => {}
            }
        }
        Self {
            builtins,
            globals,
            classes,
            scopes: vec![Scope::default()],
            class_ctx: Vec::new(),
            next_frame: 1,
            declared: HashMap::new(),
            unresolved: Vec::new(),
        }
    }

    // ---- scope management ----

    fn frame(&self) -> u32 {
        self.scopes.last().map(|s| s.frame).unwrap_or(0)
    }

    fn push_block(&mut self) {
        let frame = self.frame();
        self.scopes.push(Scope {
            names: HashSet::new(),
            frame,
        });
    }

    /// Enter a new frame whose parameters are `params`.
    fn push_frame<'p>(&mut self, params: impl IntoIterator<Item = &'p str>) {
        let frame = self.next_frame;
        self.next_frame += 1;
        self.scopes.push(Scope {
            names: HashSet::new(),
            frame,
        });
        for p in params {
            self.declare(p);
        }
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str) {
        let frame = self.frame();
        if let Some(s) = self.scopes.last_mut() {
            s.names.insert(name.to_string());
        }
        self.declared
            .entry(name.to_string())
            .or_default()
            .insert(frame);
    }

    /// All members (fields + methods) of `class` and its parents.
    fn class_members(&self, class: &str) -> HashSet<String> {
        let mut out = HashSet::new();
        let mut cur = Some(class.to_string());
        let mut guard = 0;
        while let Some(c) = cur {
            guard += 1;
            if guard > 64 {
                break; // defensive: inheritance cycle
            }
            match self.classes.get(&c) {
                Some((parent, own)) => {
                    out.extend(own.iter().cloned());
                    cur = parent.clone();
                }
                None => break,
            }
        }
        out
    }

    fn resolves(&self, name: &str) -> bool {
        // Locals: walk the current frame's scopes, innermost first; at
        // top level (frame 0) that is the top-level block chain.
        let frame = self.frame();
        for s in self.scopes.iter().rev() {
            if s.frame != frame {
                break;
            }
            if s.names.contains(name) {
                return true;
            }
        }
        if self.class_ctx.last().is_some_and(|m| m.contains(name)) {
            return true;
        }
        self.globals.contains(name) || self.builtins.contains(name)
    }

    fn use_name(&mut self, name: &str, line: u32, col: u32, assign: bool) {
        if !self.resolves(name) {
            self.unresolved.push(Use {
                name: name.to_string(),
                line,
                col,
                frame: self.frame(),
                assign,
            });
        }
    }

    fn finish(self) -> Vec<ScopeIssue> {
        let mut out: Vec<ScopeIssue> = self
            .unresolved
            .into_iter()
            .map(|u| {
                let kind = match self.declared.get(&u.name) {
                    Some(frames) if frames.iter().any(|f| *f != u.frame) => IssueKind::FrameLeak,
                    Some(_) => IssueKind::BlockEscape,
                    None if u.assign => IssueKind::AssignUndeclared,
                    None => IssueKind::Undeclared,
                };
                ScopeIssue {
                    kind,
                    name: u.name,
                    line: u.line,
                    col: u.col,
                }
            })
            .collect();
        out.sort_by_key(|i| (i.line, i.col));
        out
    }

    // ---- traversal ----

    fn program(&mut self, program: &Program) {
        for stmt in &program.stmts {
            // Top-level `let`s are globals (collected in `new`), not
            // block-locals, so don't re-declare them into scope 0.
            if let Stmt::Let { value, .. } = stmt {
                self.expr(value);
            } else {
                self.stmt(stmt);
            }
        }
    }

    fn block(&mut self, stmts: &[Stmt]) {
        self.push_block();
        for s in stmts {
            self.stmt(s);
        }
        self.pop();
    }

    fn frame_body<'p>(&mut self, params: impl IntoIterator<Item = &'p str>, body: &[Stmt]) {
        self.push_frame(params);
        for s in body {
            self.stmt(s);
        }
        self.pop();
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, value, .. } => {
                self.expr(value);
                self.declare(name);
            }
            Stmt::Assign {
                target,
                value,
                line,
                col,
                ..
            } => {
                self.expr(value);
                match target {
                    AssignTarget::Name(n) => self.use_name(n, *line, *col, true),
                    AssignTarget::Field { object, .. } => self.expr(object),
                }
            }
            Stmt::If {
                cond,
                then_body,
                elifs,
                else_body,
                ..
            } => {
                self.expr(cond);
                self.block(then_body);
                for (c, b) in elifs {
                    self.expr(c);
                    self.block(b);
                }
                if let Some(b) = else_body {
                    self.block(b);
                }
            }
            Stmt::OnUpdate { param, body, .. } => self.frame_body([param.as_str()], body),
            Stmt::OnRender { body, .. } => self.frame_body([], body),
            Stmt::OnClassEvent { param, body, .. } => self.frame_body([param.as_str()], body),
            Stmt::Decl {
                kind,
                name,
                members,
                ..
            } => self.decl(*kind, name, members),
            Stmt::FunctionDecl {
                name, params, body, ..
            } => {
                if self.frame() != 0 || self.scopes.len() > 1 {
                    self.declare(name);
                }
                self.frame_body(params.iter().map(|p| p.name.as_str()), body);
            }
            Stmt::DialogueDecl { name, body, .. } => {
                if self.frame() != 0 || self.scopes.len() > 1 {
                    self.declare(name);
                }
                self.frame_body([], body);
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    self.expr(v);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond);
                self.block(body);
            }
            Stmt::For {
                var, iter, body, ..
            } => {
                self.expr(iter);
                self.push_block();
                self.declare(var);
                for s in body {
                    self.stmt(s);
                }
                self.pop();
            }
            Stmt::Spawn { at, .. } => {
                if let Some(e) = at {
                    self.expr(e);
                }
            }
            Stmt::Despawn { target, .. } => self.expr(target),
            Stmt::Wait { duration, .. } => self.expr(duration),
            Stmt::Then { action, body, .. } => {
                self.expr(action);
                self.block(body);
            }
            Stmt::Say { actor, text, .. } => {
                if let Some(a) = actor {
                    self.expr(a);
                }
                self.expr(text);
            }
            Stmt::Choice { branches, .. } => {
                for (label, body) in branches {
                    self.expr(label);
                    self.block(body);
                }
            }
            Stmt::Import { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. }
            | Stmt::Transition { .. } => {}
            Stmt::Expr(e) => self.expr(e),
        }
    }

    fn decl(&mut self, kind: DeclKind, name: &str, members: &[DeclMember]) {
        // `visual` bodies are a restricted shader subset checked by
        // `visual_check`, never executed by the tree-walker.
        if kind == DeclKind::Visual {
            return;
        }
        let members_visible = self.class_members(name);
        self.class_ctx.push(members_visible);
        for m in members {
            match m {
                DeclMember::Field { value, .. } => self.expr(value),
                DeclMember::Method { params, body, .. } => {
                    self.frame_body(params.iter().map(|p| p.name.as_str()), body);
                }
                DeclMember::InitialState { .. } => {}
                DeclMember::State { members, .. } => self.state(members),
            }
        }
        self.class_ctx.pop();
    }

    fn state(&mut self, members: &[StateMember]) {
        // The bare statements and `on enter:` bodies form one entry
        // sequence — one frame.
        self.push_frame([]);
        for m in members {
            match m {
                StateMember::Stmt(s) => self.stmt(s),
                StateMember::OnEnter { body, .. } => {
                    for s in body {
                        self.stmt(s);
                    }
                }
                _ => {}
            }
        }
        self.pop();
        for m in members {
            match m {
                StateMember::Every { interval, body, .. } => {
                    self.expr(interval);
                    self.frame_body([], body);
                }
                StateMember::OnRender { body, .. }
                | StateMember::OnKeyPress { body, .. }
                | StateMember::OnExit { body, .. } => self.frame_body([], body),
                StateMember::OnUpdate { param, body, .. } => {
                    self.frame_body([param.as_str()], body)
                }
                StateMember::OnPredicate {
                    predicate, body, ..
                } => {
                    self.expr(predicate);
                    self.frame_body([], body);
                }
                StateMember::Stmt(_) | StateMember::OnEnter { .. } => {}
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Ident { name, line, col } => self.use_name(name, *line, *col, false),
            Expr::Str { .. }
            | Expr::Int { .. }
            | Expr::Float { .. }
            | Expr::Bool { .. }
            | Expr::Percent { .. }
            | Expr::Quantity { .. }
            | Expr::SelfRef { .. }
            | Expr::Hole { .. } => {}
            Expr::Interp { exprs, .. } => {
                for x in exprs {
                    self.expr(x);
                }
            }
            Expr::Tuple { elems, .. } | Expr::List { elems, .. } => {
                for x in elems {
                    self.expr(x);
                }
            }
            Expr::ListComp {
                element,
                var,
                iterable,
                condition,
                ..
            } => {
                self.expr(iterable);
                self.push_block();
                self.declare(var);
                if let Some(c) = condition {
                    self.expr(c);
                }
                self.expr(element);
                self.pop();
            }
            Expr::Range { start, end, .. } => {
                self.expr(start);
                self.expr(end);
            }
            Expr::Index { object, index, .. } => {
                self.expr(object);
                self.expr(index);
            }
            Expr::Field { object, .. } => self.expr(object),
            Expr::Call {
                callee,
                args,
                kwargs,
                ..
            } => {
                self.expr(callee);
                for a in args {
                    self.expr(a);
                }
                for (_, v) in kwargs {
                    self.expr(v);
                }
            }
            Expr::Unary { operand, .. } => self.expr(operand),
            Expr::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::IfExpr {
                cond,
                then_expr,
                elifs,
                else_expr,
                ..
            } => {
                self.expr(cond);
                self.expr(then_expr);
                for (c, v) in elifs {
                    self.expr(c);
                    self.expr(v);
                }
                self.expr(else_expr);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issues(src: &str) -> Vec<(IssueKind, String)> {
        let tokens = crate::lexer::lex(src).expect("lex");
        let program = crate::parser::parse(&tokens).expect("parse");
        check(&program, &known_globals())
            .into_iter()
            .map(|i| (i.kind, i.name))
            .collect()
    }

    #[test]
    fn function_local_read_after_return_is_a_frame_leak() {
        let got =
            issues("function f():\n    let tmp = 5\n    return tmp\nprint(f())\nprint(tmp)\n");
        assert_eq!(got, vec![(IssueKind::FrameLeak, "tmp".to_string())]);
    }

    #[test]
    fn callee_reading_callers_param_is_a_frame_leak() {
        let got = issues(
            "let x = 1\nfunction g():\n    return y\nfunction h(y):\n    return g()\nprint(h(99))\n",
        );
        assert_eq!(got, vec![(IssueKind::FrameLeak, "y".to_string())]);
    }

    #[test]
    fn globals_are_visible_in_functions_regardless_of_order() {
        assert!(
            issues("function f():\n    return later + 1\nlet later = 2\nprint(f())\n").is_empty()
        );
    }

    #[test]
    fn block_local_read_after_block_is_a_block_escape() {
        let got = issues("function f(c):\n    if c:\n        let a = 1\n    return a\n");
        assert_eq!(got, vec![(IssueKind::BlockEscape, "a".to_string())]);
    }

    #[test]
    fn class_fields_and_methods_resolve_inside_methods() {
        let src = "entity Mob:\n    var hp = 3\n    function hurt(n):\n        hp -= n\n        check()\n    function check():\n        return hp\n";
        assert!(issues(src).is_empty(), "{:?}", issues(src));
    }

    #[test]
    fn parent_fields_resolve_in_subclass_methods() {
        let src = "entity Base:\n    var hp = 3\nentity Kid extends Base:\n    function hit():\n        hp -= 1\n";
        assert!(issues(src).is_empty(), "{:?}", issues(src));
    }

    #[test]
    fn loop_and_comprehension_vars_are_scoped() {
        assert!(issues("for i in 0..3:\n    print(i)\nprint([k * 2 for k in 0..3])\n").is_empty());
        let got = issues("for i in 0..3:\n    print(i)\nprint(i)\n");
        assert_eq!(got, vec![(IssueKind::BlockEscape, "i".to_string())]);
    }

    #[test]
    fn assignment_to_undeclared_name_is_reported() {
        let got = issues("function f():\n    total = 3\n");
        assert_eq!(
            got,
            vec![(IssueKind::AssignUndeclared, "total".to_string())]
        );
    }

    #[test]
    fn stdlib_and_input_ambients_are_known() {
        assert!(issues("print(math.sqrt(4))\nprint(key_press)\nprint(time)\n").is_empty());
    }
}
