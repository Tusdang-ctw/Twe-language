//! Vectors vs colours, statically (`docs/06` §2.5.7; 2026-11-04).
//!
//! At run time a tuple from `vec3` is a vector and one from `color.*` a
//! colour, and mixing them is an error. This pass finds the mix-ups
//! `twec verify` can prove without running anything, so an author (or a
//! model) hears about them before the program reaches that line.
//!
//! **No false positives.** A kind is only known for an expression whose
//! value is certain to carry it:
//! - `vec3(...)` and `math.cross(...)` are vectors; `math.normalize(x)`
//!   has `x`'s kind;
//! - `color.<constant>` and the `color.*` constructors are colours;
//! - `+` / `-` / `mix` join kinds as the runtime does, and `*` / `/`
//!   keep the tuple side's;
//! - a name has its value's kind only if exactly one `let` / `var`
//!   anywhere in the file binds it, nothing assigns to it, and it is
//!   never a parameter, field, loop variable or import alias (so no
//!   scope can make it mean something else);
//! - a builtin name a program redefines (`vec3`, `color`, `math`, ...)
//!   loses its meaning.
//!
//! Everything else is unknown, and an unknown never produces a
//! diagnostic. `visual` and `particles` blocks are skipped: they compile
//! to WGSL, where `vec3` and colours have GPU types of their own.

use std::collections::{HashMap, HashSet};

use crate::ast::{AssignTarget, BinOp, DeclKind, DeclMember, Expr, Program, StateMember, Stmt};
use crate::tagged_value::TupleKind;

/// One proven mix-up, as `twec verify` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct KindIssue {
    /// `kind-error.mix` (arithmetic / `mix`) or `kind-error.argument`.
    pub kind: &'static str,
    pub line: u32,
    pub col: u32,
    pub message: String,
    pub help: String,
}

/// `<builtin>.<param>` whose argument the runtime decodes as a colour
/// and refuses a vector for (`stdlib::color_of` / `rgba` / `rgba_of`).
/// `tests/verify_v2.rs` checks each names a real builtin parameter.
pub const COLOUR_PARAMS: &[&str] = &[
    "circle.color",
    "circle_outline.color",
    "color.lerp.a",
    "color.lerp.b",
    "color.lerp_linear.a",
    "color.lerp_linear.b",
    "color.to_linear.c",
    "color.to_srgb.c",
    "cube.color",
    "cube_textured.color",
    "fx.blood_splat.color",
    "fx.damage_number.color",
    "fx.dash_trail.color",
    "fx.death_burst.color",
    "fx.hit_flash.color",
    "fx.level_up_ring.color",
    "fx.pickup_pop.color",
    "light.add.color",
    "light.ambient.color",
    "light.background.color",
    "light.fog.color",
    "light.set.color",
    "light2d.add.color",
    "light2d.ambient.color",
    "line.color",
    "mesh.color",
    "mesh_textured.color",
    "postfx.vignette_color.color",
    "rect.color",
    "rect_outline.color",
    "sphere.color",
    "text.color",
    "text_with_font.color",
];

/// `<builtin>.<param>` decoded as a 2D point or size (`stdlib::xy_of`),
/// which refuses a colour.
pub const POSITION_PARAMS: &[&str] = &[
    "button.at",
    "button.size",
    "camera2d.cinematic_pan.from",
    "camera2d.cinematic_pan.to",
    "camera2d.follow.deadzone",
    "camera2d.follow.target",
    "checkbox.at",
    "checkbox.size",
    "circle.at",
    "circle_outline.at",
    "dropdown.at",
    "dropdown.size",
    "flex.at",
    "flex.size",
    "fx.blood_splat.at",
    "fx.blood_splat.dir",
    "fx.crit_text.at",
    "fx.damage_number.at",
    "fx.dash_trail.at",
    "fx.death_burst.at",
    "fx.ground_shockwave.at",
    "fx.hit_flash.at",
    "fx.hit_flash.size",
    "fx.level_up_ring.at",
    "fx.muzzle_flash.at",
    "fx.muzzle_flash.dir",
    "fx.pickup_pop.at",
    "grid.at",
    "grid.size",
    "key_input.at",
    "key_input.size",
    "label.at",
    "label.size",
    "light2d.add.at",
    "line.from",
    "line.to",
    "load_atlas.grid",
    "panel.at",
    "panel.size",
    "panel_skinned.at",
    "panel_skinned.size",
    "physics2d.bounce.normal",
    "physics2d.bounce.vel",
    "physics2d.circle_hits.center",
    "physics2d.circle_overlap.c1",
    "physics2d.circle_overlap.c2",
    "physics2d.collide.p1",
    "physics2d.collide.p2",
    "physics2d.collide.v1",
    "physics2d.collide.v2",
    "physics2d.grid_near.center",
    "physics2d.move_and_slide.vel",
    "physics2d.sweep.vel",
    "progress_bar.at",
    "progress_bar.size",
    "rect.at",
    "rect.size",
    "rect_outline.at",
    "rect_outline.size",
    "scroll.at",
    "scroll.size",
    "slider.at",
    "slider.size",
    "sprite_frame.at",
    "sprite_frame_at.at",
    "sprite_frame_at.size",
    "stack.at",
    "stack.size",
    "text.at",
    "text_input.at",
    "text_input.size",
    "text_with_font.at",
];

/// The `color.*` constants (`stdlib::install_color`).
const PALETTE: &[&str] = &[
    "red",
    "green",
    "blue",
    "cyan",
    "yellow",
    "orange",
    "purple",
    "white",
    "black",
    "gray",
    "transparent",
];

/// Every proven vector/colour mix-up in `program`.
pub fn check_program(program: &Program) -> Vec<KindIssue> {
    let mut names = Names::default();
    names.stmts(&program.stmts);
    let mut checker = Checker {
        lets: names.lets,
        unstable: names.unstable,
        params: None,
        memo: HashMap::new(),
        out: Vec::new(),
    };
    each_expr_in_stmts(&program.stmts, &mut |e| checker.visit(e));
    for stmt in &program.stmts {
        if let Stmt::Decl {
            kind: DeclKind::Entity,
            members,
            ..
        } = stmt
        {
            for m in members {
                if let DeclMember::Look { keys, .. } = m {
                    for k in keys.iter().filter(|k| k.key == "tint" || k.key == "glow") {
                        if checker.kind_of(&k.value, 0) == Some(TupleKind::Vector) {
                            checker.out.push(KindIssue {
                                kind: "kind-error.argument",
                                line: k.line,
                                col: k.col,
                                message: format!(
                                    "look {} expects a colour, got {}",
                                    k.key,
                                    TupleKind::Vector.describe()
                                ),
                                help: COLOUR_HELP.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
    let mut out = checker.out;
    out.sort_by_key(|d| (d.line, d.col));
    out.dedup();
    out
}

const COLOUR_HELP: &str =
    "pass a colour such as `color.red` or `(r, g, b)`; a vec3 is a position or direction";
const POSITION_HELP: &str = "pass a position such as `(x, y)`; `color.*` values are colours";
const MIX_HELP: &str = "a colour and a position don't mix; if you meant it, rebuild one side as a plain tuple, e.g. `(c.x, c.y, c.z)`";

/// The names a program binds, and which of them can't have a fixed kind.
#[derive(Default)]
struct Names<'a> {
    /// `let` / `var` bindings by name: how many, and the last value.
    lets: HashMap<String, (usize, Option<&'a Expr>)>,
    /// Assigned, or bound some other way somewhere (parameter, field,
    /// loop variable, function, class, import alias).
    unstable: HashSet<String>,
}

impl<'a> Names<'a> {
    fn bind_other(&mut self, name: &str) {
        self.unstable.insert(name.to_string());
    }

    fn stmts(&mut self, stmts: &'a [Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &'a Stmt) {
        match s {
            Stmt::Let { name, value, .. } => {
                let e = self.lets.entry(name.clone()).or_insert((0, None));
                e.0 += 1;
                e.1 = Some(value);
                self.expr(value);
            }
            Stmt::Assign { target, value, .. } => {
                if let AssignTarget::Name(n, _) = target {
                    self.bind_other(n);
                }
                self.expr(value);
            }
            Stmt::FunctionDecl {
                name, params, body, ..
            } => {
                self.bind_other(name);
                for p in params {
                    self.bind_other(&p.name);
                }
                self.stmts(body);
            }
            Stmt::For { var, body, .. } => {
                self.bind_other(var);
                self.stmts(body);
            }
            Stmt::OnUpdate { param, body, .. } | Stmt::OnClassEvent { param, body, .. } => {
                self.bind_other(param);
                self.stmts(body);
            }
            Stmt::Import { path, alias, .. } => {
                self.bind_other(alias.as_deref().unwrap_or(path));
            }
            Stmt::Decl { name, members, .. } => {
                self.bind_other(name);
                for m in members {
                    match m {
                        DeclMember::Field { name, .. } => self.bind_other(name),
                        DeclMember::Method {
                            name, params, body, ..
                        } => {
                            self.bind_other(name);
                            for p in params {
                                self.bind_other(&p.name);
                            }
                            self.stmts(body);
                        }
                        DeclMember::State { members, .. } => {
                            for sm in members {
                                match sm {
                                    StateMember::Stmt(s) => self.stmt(s),
                                    StateMember::OnUpdate { param, body, .. } => {
                                        self.bind_other(param);
                                        self.stmts(body);
                                    }
                                    StateMember::Every { body, .. }
                                    | StateMember::OnRender { body, .. }
                                    | StateMember::OnKeyPress { body, .. }
                                    | StateMember::OnPredicate { body, .. }
                                    | StateMember::OnEnter { body, .. }
                                    | StateMember::OnExit { body, .. } => self.stmts(body),
                                }
                            }
                        }
                        DeclMember::InitialState { .. } | DeclMember::Look { .. } => {}
                    }
                }
            }
            Stmt::DialogueDecl { name, body, .. } => {
                self.bind_other(name);
                self.stmts(body);
            }
            _ => for_child_bodies(s, &mut |b| self.stmts(b)),
        }
    }

    /// List comprehensions bind their variable.
    fn expr(&mut self, e: &'a Expr) {
        each_expr(e, &mut |x| {
            if let Expr::ListComp { var, .. } = x {
                self.unstable.insert(var.clone());
            }
        });
    }
}

/// The statement bodies nested in `s` that [`Names::stmt`] doesn't name.
fn for_child_bodies<'a>(s: &'a Stmt, f: &mut dyn FnMut(&'a [Stmt])) {
    match s {
        Stmt::If {
            then_body,
            elifs,
            else_body,
            ..
        } => {
            f(then_body);
            for (_, b) in elifs {
                f(b);
            }
            if let Some(b) = else_body {
                f(b);
            }
        }
        Stmt::While { body, .. } | Stmt::Then { body, .. } | Stmt::OnRender { body, .. } => f(body),
        Stmt::Choice { branches, .. } => {
            for (_, b) in branches {
                f(b);
            }
        }
        _ => {}
    }
}

/// Call `f` on every expression in `stmts`, outermost first, skipping
/// `visual` and `particles` blocks (WGSL semantics, not Twe's).
fn each_expr_in_stmts(stmts: &[Stmt], f: &mut dyn FnMut(&Expr)) {
    for s in stmts {
        each_expr_in_stmt(s, f);
    }
}

fn each_expr_in_stmt(s: &Stmt, f: &mut dyn FnMut(&Expr)) {
    let ex = each_expr;
    match s {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => ex(value, f),
        Stmt::If {
            cond,
            then_body,
            elifs,
            else_body,
            ..
        } => {
            ex(cond, f);
            each_expr_in_stmts(then_body, f);
            for (c, b) in elifs {
                ex(c, f);
                each_expr_in_stmts(b, f);
            }
            if let Some(b) = else_body {
                each_expr_in_stmts(b, f);
            }
        }
        Stmt::While { cond, body, .. } => {
            ex(cond, f);
            each_expr_in_stmts(body, f);
        }
        Stmt::For { iter, body, .. } => {
            ex(iter, f);
            each_expr_in_stmts(body, f);
        }
        Stmt::Then { action, body, .. } => {
            ex(action, f);
            each_expr_in_stmts(body, f);
        }
        Stmt::Return { value: Some(v), .. } => ex(v, f),
        Stmt::Spawn { at: Some(at), .. } => ex(at, f),
        Stmt::Despawn { target, .. } => ex(target, f),
        Stmt::Wait { duration, .. } => ex(duration, f),
        Stmt::Say { actor, text, .. } => {
            if let Some(a) = actor {
                ex(a, f);
            }
            ex(text, f);
        }
        Stmt::Choice { branches, .. } => {
            for (c, b) in branches {
                ex(c, f);
                each_expr_in_stmts(b, f);
            }
        }
        Stmt::Expr(e) => ex(e, f),
        Stmt::FunctionDecl { body, .. }
        | Stmt::OnUpdate { body, .. }
        | Stmt::OnRender { body, .. }
        | Stmt::OnClassEvent { body, .. }
        | Stmt::DialogueDecl { body, .. } => each_expr_in_stmts(body, f),
        Stmt::Decl { kind, members, .. } => {
            if matches!(kind, DeclKind::Visual | DeclKind::Particles) {
                return;
            }
            for m in members {
                match m {
                    DeclMember::Field { value, .. } => ex(value, f),
                    DeclMember::Method { body, .. } => each_expr_in_stmts(body, f),
                    DeclMember::Look { keys, .. } => {
                        for k in keys {
                            ex(&k.value, f);
                        }
                    }
                    DeclMember::State { members, .. } => {
                        for sm in members {
                            match sm {
                                StateMember::Stmt(s) => each_expr_in_stmt(s, f),
                                StateMember::Every { interval, body, .. } => {
                                    ex(interval, f);
                                    each_expr_in_stmts(body, f);
                                }
                                StateMember::OnPredicate {
                                    predicate, body, ..
                                } => {
                                    ex(predicate, f);
                                    each_expr_in_stmts(body, f);
                                }
                                StateMember::OnRender { body, .. }
                                | StateMember::OnKeyPress { body, .. }
                                | StateMember::OnUpdate { body, .. }
                                | StateMember::OnEnter { body, .. }
                                | StateMember::OnExit { body, .. } => each_expr_in_stmts(body, f),
                            }
                        }
                    }
                    DeclMember::InitialState { .. } => {}
                }
            }
        }
        _ => {}
    }
}

/// Call `f` on `e` and every expression inside it, outermost first.
fn each_expr(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match e {
        Expr::Interp { exprs, .. } => {
            for x in exprs {
                each_expr(x, f);
            }
        }
        Expr::Tuple { elems, .. } | Expr::List { elems, .. } => {
            for x in elems {
                each_expr(x, f);
            }
        }
        Expr::ListComp {
            element,
            iterable,
            condition,
            ..
        } => {
            each_expr(element, f);
            each_expr(iterable, f);
            if let Some(c) = condition {
                each_expr(c, f);
            }
        }
        Expr::Range { start, end, .. } => {
            each_expr(start, f);
            each_expr(end, f);
        }
        Expr::Index { object, index, .. } => {
            each_expr(object, f);
            each_expr(index, f);
        }
        Expr::Field { object, .. } => each_expr(object, f),
        Expr::Call {
            callee,
            args,
            kwargs,
            ..
        } => {
            each_expr(callee, f);
            for x in args {
                each_expr(x, f);
            }
            for (_, x) in kwargs {
                each_expr(x, f);
            }
        }
        Expr::Spawn { at: Some(at), .. } => each_expr(at, f),
        Expr::Unary { operand, .. } => each_expr(operand, f),
        Expr::Binary { left, right, .. } => {
            each_expr(left, f);
            each_expr(right, f);
        }
        Expr::IfExpr {
            cond,
            then_expr,
            elifs,
            else_expr,
            ..
        } => {
            each_expr(cond, f);
            each_expr(then_expr, f);
            for (c, x) in elifs {
                each_expr(c, f);
                each_expr(x, f);
            }
            each_expr(else_expr, f);
        }
        _ => {}
    }
}

/// How deep one name's value may refer to another's before giving up.
const MAX_NAME_DEPTH: usize = 32;

struct Checker<'a> {
    lets: HashMap<String, (usize, Option<&'a Expr>)>,
    unstable: HashSet<String>,
    /// Builtin parameter names, looked up the first time a call needs them.
    params: Option<HashMap<String, Vec<String>>>,
    /// Kinds of stable names already worked out.
    memo: HashMap<String, Option<TupleKind>>,
    out: Vec<KindIssue>,
}

impl<'a> Checker<'a> {
    /// Whether the program binds `name` itself, hiding the builtin.
    fn user_defined(&self, name: &str) -> bool {
        self.lets.contains_key(name) || self.unstable.contains(name)
    }

    /// `name` (or `ns.name`) when the callee is a builtin the program
    /// didn't redefine.
    fn builtin_name(&self, callee: &Expr) -> Option<String> {
        match callee {
            Expr::Ident { name, .. } if !self.user_defined(name) => Some(name.clone()),
            Expr::Field { object, name, .. } => match object.as_ref() {
                Expr::Ident { name: ns, .. } if !self.user_defined(ns) => {
                    Some(format!("{ns}.{name}"))
                }
                Expr::Field { .. } => {
                    // `postfx.vignette_color` style two-level names.
                    let inner = self.builtin_name(object)?;
                    Some(format!("{inner}.{name}"))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// The kind `e`'s value is certain to have, if any (see the module docs).
    fn kind_of(&mut self, e: &Expr, depth: usize) -> Option<TupleKind> {
        if depth > MAX_NAME_DEPTH {
            return None;
        }
        match e {
            Expr::Ident { name, .. } => self.name_kind(name, depth),
            Expr::Field { object, name, .. } => match object.as_ref() {
                Expr::Ident { name: ns, .. }
                    if ns == "color"
                        && !self.user_defined(ns)
                        && PALETTE.contains(&name.as_str()) =>
                {
                    Some(TupleKind::Color)
                }
                _ => None,
            },
            Expr::Call { callee, args, .. } => {
                let name = self.builtin_name(callee)?;
                match name.as_str() {
                    "vec3" | "math.cross" => Some(TupleKind::Vector),
                    "math.normalize" => self.kind_of(args.first()?, depth + 1),
                    "color.from_hex" | "color.hsv" | "color.lerp" | "color.lerp_linear"
                    | "color.to_linear" | "color.to_srgb" => Some(TupleKind::Color),
                    "mix" => {
                        let a = self.kind_of(args.first()?, depth + 1);
                        let b = self.kind_of(args.get(1)?, depth + 1);
                        join(a, b).ok()?
                    }
                    _ => None,
                }
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let l = self.kind_of(left, depth + 1);
                let r = self.kind_of(right, depth + 1);
                match op {
                    BinOp::Add | BinOp::Sub => join(l, r).ok()?,
                    BinOp::Mul | BinOp::Div => l.or(r),
                    _ => None,
                }
            }
            Expr::IfExpr {
                then_expr,
                elifs,
                else_expr,
                ..
            } => {
                let first = self.kind_of(then_expr, depth + 1)?;
                for (_, x) in elifs {
                    if self.kind_of(x, depth + 1) != Some(first) {
                        return None;
                    }
                }
                (self.kind_of(else_expr, depth + 1) == Some(first)).then_some(first)
            }
            _ => None,
        }
    }

    fn name_kind(&mut self, name: &str, depth: usize) -> Option<TupleKind> {
        if let Some(k) = self.memo.get(name) {
            return *k;
        }
        let value = match self.lets.get(name) {
            Some((1, Some(v))) if !self.unstable.contains(name) => *v,
            _ => return None,
        };
        // Guards a binding that (wrongly) refers to itself.
        self.memo.insert(name.to_string(), None);
        let k = self.kind_of(value, depth + 1);
        self.memo.insert(name.to_string(), k);
        k
    }

    fn params_of(&mut self, builtin: &str) -> Option<Vec<String>> {
        let table = self.params.get_or_insert_with(|| {
            crate::stdlib::manifest()
                .into_iter()
                .map(|s| (s.name, s.params))
                .collect()
        });
        table.get(builtin).cloned()
    }

    /// Report what `e` itself (not its children) provably gets wrong.
    fn visit(&mut self, e: &Expr) {
        match e {
            Expr::Binary {
                op: op @ (BinOp::Add | BinOp::Sub),
                left,
                right,
                line,
                col,
            } => {
                let l = self.kind_of(left, 0);
                let r = self.kind_of(right, 0);
                if let (Some(a), Some(b)) = (l, r) {
                    if a != b {
                        let verb = if matches!(op, BinOp::Add) {
                            "add"
                        } else {
                            "subtract"
                        };
                        self.out.push(KindIssue {
                            kind: "kind-error.mix",
                            line: *line,
                            col: *col,
                            message: format!("can't {verb} {} and {}", a.describe(), b.describe()),
                            help: MIX_HELP.to_string(),
                        });
                    }
                }
            }
            Expr::Call {
                callee,
                args,
                kwargs,
                line,
                col,
            } => {
                let Some(name) = self.builtin_name(callee) else {
                    return;
                };
                if name == "mix" && args.len() >= 2 {
                    let a = self.kind_of(&args[0], 0);
                    let b = self.kind_of(&args[1], 0);
                    if let (Some(a), Some(b)) = (a, b) {
                        if a != b {
                            self.out.push(KindIssue {
                                kind: "kind-error.mix",
                                line: *line,
                                col: *col,
                                message: format!(
                                    "mix: can't blend {} with {}",
                                    a.describe(),
                                    b.describe()
                                ),
                                help: "mix two colours or two vectors, not one of each".to_string(),
                            });
                        }
                    }
                }
                let checked = |p: &str| {
                    let label = format!("{name}.{p}");
                    if COLOUR_PARAMS.contains(&label.as_str()) {
                        Some((label, TupleKind::Vector))
                    } else if POSITION_PARAMS.contains(&label.as_str()) {
                        Some((label, TupleKind::Color))
                    } else {
                        None
                    }
                };
                let prefix = format!("{name}.");
                let any = COLOUR_PARAMS
                    .iter()
                    .chain(POSITION_PARAMS)
                    .any(|l| l.strip_prefix(&prefix).is_some_and(|p| !p.contains('.')));
                if !any {
                    return;
                }
                let Some(params) = self.params_of(&name) else {
                    return;
                };
                for (i, p) in params.iter().enumerate() {
                    let Some((label, wrong)) = checked(p) else {
                        continue;
                    };
                    let arg = args
                        .get(i)
                        .or_else(|| kwargs.iter().find(|(k, _)| k == p).map(|(_, v)| v));
                    let Some(arg) = arg else {
                        continue;
                    };
                    if self.kind_of(arg, 0) == Some(wrong) {
                        let (expected, help) = if wrong == TupleKind::Vector {
                            ("a colour", COLOUR_HELP)
                        } else {
                            ("a position", POSITION_HELP)
                        };
                        self.out.push(KindIssue {
                            kind: "kind-error.argument",
                            line: arg.line(),
                            col: arg.col(),
                            message: format!(
                                "{label} expects {expected}, got {}",
                                wrong.describe()
                            ),
                            help: help.to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

/// `+` / `-` / `mix` on two known-or-unknown kinds: a known kind wins
/// over an unknown one (if the unknown side were the other kind, the
/// program would have stopped there); two different known kinds are
/// the mix-up.
fn join(a: Option<TupleKind>, b: Option<TupleKind>) -> Result<Option<TupleKind>, ()> {
    match (a, b) {
        (Some(x), Some(y)) => TupleKind::join(x, y).map(Some).ok_or(()),
        (x, None) | (None, x) => Ok(x),
    }
}
