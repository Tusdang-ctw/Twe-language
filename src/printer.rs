//! Canonical-form printer: AST -> Twe source.
//!
//! Drives `twec fmt`. The contract:
//!
//!   1. **Round-trip safe**: `parse(print(p))` is structurally
//!      equal to `p`, modulo line / col positions and modulo
//!      lost comments + blank lines (which the AST doesn't carry
//!      in v0.1).
//!   2. **Idempotent**: `print(parse(print(p))) == print(p)`. The
//!      `tests/fmt.rs` suite asserts this on every test program.
//!   3. **Single canonical style**: 4-space indent, `<op> ` spacing
//!      around binary operators, no trailing whitespace, one blank
//!      line between top-level declarations, no blank lines inside
//!      function/method/state bodies.
//!
//! Comments and blank lines are dropped — the AST doesn't preserve
//! them. A future "trivia-preserving formatter" would need lex-time
//! attachment of comment tokens to surrounding nodes.

use crate::ast::{
    AssignOp, AssignTarget, BinOp, DeclKind, DeclMember, Expr, Program, StateMember, Stmt, UnOp,
};

/// Format a whole program to a `String` ending in `\n`. Empty
/// programs round-trip to the empty string (no leading newline).
/// A blank line is inserted between any pair where either side is
/// a top-level declaration (`entity` / `scene` / `function` /
/// top-level `on update`) — keeps related-stmts groupings (like a
/// run of plain `let`s + `print`s in a script) compact, while
/// breathing space sits around the heavier declarative blocks.
pub fn print_program(p: &Program) -> String {
    let mut out = String::new();
    let mut prev: Option<&Stmt> = None;
    for stmt in &p.stmts {
        if let Some(p) = prev {
            if takes_blank_neighbor(stmt) || takes_blank_neighbor(p) {
                out.push('\n');
            }
        }
        print_stmt(&mut out, stmt, 0);
        prev = Some(stmt);
    }
    out
}

fn takes_blank_neighbor(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Decl { .. }
            | Stmt::FunctionDecl { .. }
            | Stmt::OnUpdate { .. }
            | Stmt::OnRender { .. }
            | Stmt::OnClassEvent { .. }
    )
}

const INDENT: &str = "    "; // four spaces; matches every existing program

/// Phase 13 session 9: emit `@deprecated("since vX.Y")` on the
/// line before its annotated declaration. Empty `since` falls
/// back to the bare `@deprecated` form so input → AST → output is
/// stable for both shapes.
fn print_deprecation(
    out: &mut String,
    depth: usize,
    dep: &Option<crate::ast::Deprecation>,
) {
    let Some(dep) = dep else { return };
    push_indent(out, depth);
    out.push_str("@deprecated");
    if let Some(since) = &dep.since {
        out.push('(');
        out.push('"');
        for ch in since.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                _ => out.push(ch),
            }
        }
        out.push('"');
        out.push(')');
    }
    out.push('\n');
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str(INDENT);
    }
}

// --- statements ---

fn print_stmt(out: &mut String, stmt: &Stmt, depth: usize) {
    match stmt {
        Stmt::Let { name, value, .. } => {
            push_indent(out, depth);
            out.push_str("let ");
            out.push_str(name);
            out.push_str(" = ");
            print_expr(out, value, Prec::Lowest);
            out.push('\n');
        }
        Stmt::Assign {
            target, op, value, ..
        } => {
            push_indent(out, depth);
            print_assign_target(out, target);
            out.push(' ');
            out.push_str(assign_op_str(*op));
            out.push(' ');
            print_expr(out, value, Prec::Lowest);
            out.push('\n');
        }
        Stmt::If {
            cond,
            then_body,
            elifs,
            else_body,
            ..
        } => {
            push_indent(out, depth);
            out.push_str("if ");
            print_expr(out, cond, Prec::Lowest);
            out.push_str(":\n");
            print_block(out, then_body, depth + 1);
            for (cond, body) in elifs {
                push_indent(out, depth);
                out.push_str("elif ");
                print_expr(out, cond, Prec::Lowest);
                out.push_str(":\n");
                print_block(out, body, depth + 1);
            }
            if let Some(else_body) = else_body {
                push_indent(out, depth);
                out.push_str("else:\n");
                print_block(out, else_body, depth + 1);
            }
        }
        Stmt::OnUpdate { param, body, .. } => {
            push_indent(out, depth);
            out.push_str("on update(");
            out.push_str(param);
            out.push_str("):\n");
            print_block(out, body, depth + 1);
        }
        Stmt::OnRender { body, .. } => {
            push_indent(out, depth);
            out.push_str("on render():\n");
            print_block(out, body, depth + 1);
        }
        Stmt::OnClassEvent {
            class,
            event,
            param,
            body,
            ..
        } => {
            push_indent(out, depth);
            out.push_str("on ");
            out.push_str(class);
            out.push('.');
            out.push_str(event);
            out.push('(');
            out.push_str(param);
            out.push_str("):\n");
            print_block(out, body, depth + 1);
        }
        Stmt::Decl {
            kind,
            name,
            parent,
            members,
            deprecation,
            ..
        } => {
            print_deprecation(out, depth, deprecation);
            push_indent(out, depth);
            out.push_str(kind.as_str());
            out.push(' ');
            out.push_str(name);
            if let Some(parent) = parent {
                out.push_str(" extends ");
                out.push_str(parent);
            }
            out.push_str(":\n");
            print_decl_members(out, members, depth + 1, *kind);
        }
        Stmt::FunctionDecl {
            name,
            params,
            ret,
            body,
            deprecation,
            ..
        } => {
            print_deprecation(out, depth, deprecation);
            push_indent(out, depth);
            out.push_str("function ");
            out.push_str(name);
            out.push('(');
            push_params(out, params);
            out.push(')');
            if let Some(ret_ty) = ret {
                out.push_str(" -> ");
                out.push_str(&ret_ty.to_string());
            }
            out.push_str(":\n");
            print_block(out, body, depth + 1);
        }
        Stmt::Return { value, .. } => {
            push_indent(out, depth);
            out.push_str("return");
            if let Some(v) = value {
                out.push(' ');
                print_expr(out, v, Prec::Lowest);
            }
            out.push('\n');
        }
        Stmt::While { cond, body, .. } => {
            push_indent(out, depth);
            out.push_str("while ");
            print_expr(out, cond, Prec::Lowest);
            out.push_str(":\n");
            print_block(out, body, depth + 1);
        }
        Stmt::For {
            var, iter, body, ..
        } => {
            push_indent(out, depth);
            out.push_str("for ");
            out.push_str(var);
            out.push_str(" in ");
            print_expr(out, iter, Prec::Lowest);
            out.push_str(":\n");
            print_block(out, body, depth + 1);
        }
        Stmt::Break { .. } => {
            push_indent(out, depth);
            out.push_str("break\n");
        }
        Stmt::Continue { .. } => {
            push_indent(out, depth);
            out.push_str("continue\n");
        }
        Stmt::Transition { target, .. } => {
            push_indent(out, depth);
            out.push_str("-> ");
            out.push_str(target);
            out.push('\n');
        }
        Stmt::Spawn { class, at, .. } => {
            push_indent(out, depth);
            out.push_str("spawn ");
            out.push_str(class);
            if let Some(at) = at {
                out.push_str(" at ");
                print_expr(out, at, Prec::Lowest);
            }
            out.push('\n');
        }
        Stmt::Despawn { target, .. } => {
            push_indent(out, depth);
            out.push_str("despawn ");
            print_expr(out, target, Prec::Lowest);
            out.push('\n');
        }
        Stmt::Wait { duration, .. } => {
            push_indent(out, depth);
            out.push_str("wait ");
            print_expr(out, duration, Prec::Lowest);
            out.push('\n');
        }
        Stmt::DialogueDecl { name, body, .. } => {
            push_indent(out, depth);
            out.push_str("dialogue ");
            out.push_str(name);
            out.push_str(":\n");
            print_block(out, body, depth + 1);
        }
        Stmt::Say { actor, text, .. } => {
            push_indent(out, depth);
            out.push_str("say ");
            if let Some(a) = actor {
                print_expr(out, a, Prec::Lowest);
                out.push_str(": ");
            }
            print_expr(out, text, Prec::Lowest);
            out.push('\n');
        }
        Stmt::Choice { branches, .. } => {
            push_indent(out, depth);
            out.push_str("choice:\n");
            for (label, body) in branches {
                push_indent(out, depth + 1);
                print_expr(out, label, Prec::Lowest);
                out.push_str(":\n");
                print_block(out, body, depth + 2);
            }
        }
        Stmt::Import { path, alias, .. } => {
            push_indent(out, depth);
            out.push_str("import \"");
            // Path strings are forward-slash logical paths; they
            // never contain quote chars in practice but escape
            // defensively so `twec fmt` round-trips arbitrary input.
            for ch in path.chars() {
                match ch {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    _ => out.push(ch),
                }
            }
            out.push('"');
            if let Some(name) = alias {
                out.push_str(" as ");
                out.push_str(name);
            }
            out.push('\n');
        }
        Stmt::Expr(e) => {
            push_indent(out, depth);
            print_expr(out, e, Prec::Lowest);
            out.push('\n');
        }
    }
}

fn print_block(out: &mut String, stmts: &[Stmt], depth: usize) {
    if stmts.is_empty() {
        // Empty body: indented `pass`-equivalent isn't a Twe thing,
        // so emit a stub no-op. The parser accepts `nil` as a
        // statement (Stmt::Expr(Expr::Ident "nil") would fail —
        // there's no nil literal). The cleanest no-op the parser
        // accepts is `0` (a constant int statement). Any program
        // that round-trips through here had a body with at least
        // one statement, so this branch is mostly defensive.
        push_indent(out, depth);
        out.push_str("0\n");
        return;
    }
    for s in stmts {
        print_stmt(out, s, depth);
    }
}

fn print_decl_members(out: &mut String, members: &[DeclMember], depth: usize, _kind: DeclKind) {
    // Print fields, then initial: state, then methods, then states
    // — this matches the convention all the test programs use and
    // keeps the formatted output readable. We preserve the source
    // order within each group, however.
    if members.is_empty() {
        push_indent(out, depth);
        out.push_str("0\n"); // defensive empty-body stub
        return;
    }
    let mut first = true;
    for m in members {
        if !first && member_takes_blank_line(m) {
            out.push('\n');
        }
        first = false;
        print_decl_member(out, m, depth);
    }
}

fn member_takes_blank_line(m: &DeclMember) -> bool {
    matches!(
        m,
        DeclMember::Method { .. } | DeclMember::State { .. } | DeclMember::InitialState { .. }
    )
}

fn print_decl_member(out: &mut String, m: &DeclMember, depth: usize) {
    match m {
        DeclMember::Field {
            name, value, ty, ..
        } => {
            push_indent(out, depth);
            // `var X = expr` reads better than `X: expr` for
            // variable fields — but the AST doesn't distinguish
            // `var` from `:` form; both parse to DeclMember::Field.
            // We use the `:` form (equivalent in semantics, and
            // shorter) because it round-trips through the parser.
            // Phase 6 session 4: when the field has an explicit
            // annotation, render it as `name: type = value` so
            // strict-mode-annotated programs round-trip.
            out.push_str(name);
            match ty {
                Some(t) => {
                    out.push_str(": ");
                    out.push_str(&t.to_string());
                    out.push_str(" = ");
                    print_expr(out, value, Prec::Lowest);
                }
                None => {
                    out.push_str(": ");
                    print_expr(out, value, Prec::Lowest);
                }
            }
            out.push('\n');
        }
        DeclMember::Method {
            name,
            params,
            ret,
            body,
            ..
        } => {
            push_indent(out, depth);
            // Method form: `name(params)[ -> ret]:`. Phase 6
            // session 4 lifted method param + return annotations
            // onto the AST; print them so the formatter
            // round-trips annotated methods.
            out.push_str(name);
            out.push('(');
            push_params(out, params);
            out.push(')');
            if let Some(rt) = ret {
                out.push_str(" -> ");
                out.push_str(&rt.to_string());
            }
            out.push_str(":\n");
            print_block(out, body, depth + 1);
        }
        DeclMember::InitialState { name, .. } => {
            push_indent(out, depth);
            out.push_str("initial: ");
            out.push_str(name);
            out.push('\n');
        }
        DeclMember::State { name, members, .. } => {
            push_indent(out, depth);
            out.push_str("state ");
            out.push_str(name);
            out.push_str(":\n");
            print_state_members(out, members, depth + 1);
        }
    }
}

fn print_state_members(out: &mut String, members: &[StateMember], depth: usize) {
    // The parser accepts an empty state body (`state done:` with
    // nothing after — common for terminal idle states). Emit
    // nothing inside; the next sibling's indentation closes the
    // block naturally.
    let mut first = true;
    for m in members {
        if !first && state_member_takes_blank(m) {
            out.push('\n');
        }
        first = false;
        print_state_member(out, m, depth);
    }
    let _ = depth;
}

fn state_member_takes_blank(m: &StateMember) -> bool {
    matches!(
        m,
        StateMember::Every { .. }
            | StateMember::OnRender { .. }
            | StateMember::OnKeyPress { .. }
            | StateMember::OnUpdate { .. }
    )
}

fn print_state_member(out: &mut String, m: &StateMember, depth: usize) {
    match m {
        StateMember::Stmt(s) => print_stmt(out, s, depth),
        StateMember::Every { interval, body, .. } => {
            push_indent(out, depth);
            out.push_str("every ");
            print_expr(out, interval, Prec::Lowest);
            out.push_str(":\n");
            print_block(out, body, depth + 1);
        }
        StateMember::OnRender { body, .. } => {
            push_indent(out, depth);
            out.push_str("on render():\n");
            print_block(out, body, depth + 1);
        }
        StateMember::OnKeyPress { key, body, .. } => {
            push_indent(out, depth);
            out.push_str("on key_press.");
            out.push_str(key);
            out.push_str(":\n");
            print_block(out, body, depth + 1);
        }
        StateMember::OnUpdate { param, body, .. } => {
            push_indent(out, depth);
            out.push_str("on update(");
            out.push_str(param);
            out.push_str("):\n");
            print_block(out, body, depth + 1);
        }
        StateMember::OnPredicate {
            predicate, body, ..
        } => {
            push_indent(out, depth);
            out.push_str("on ");
            print_expr(out, predicate, Prec::Lowest);
            out.push_str(":\n");
            print_block(out, body, depth + 1);
        }
    }
}

fn push_params(out: &mut String, params: &[crate::ast::Param]) {
    let mut first = true;
    for p in params {
        if !first {
            out.push_str(", ");
        }
        first = false;
        out.push_str(&p.name);
        if let Some(ty) = &p.ty {
            out.push_str(": ");
            out.push_str(&ty.to_string());
        }
    }
}

fn print_assign_target(out: &mut String, target: &AssignTarget) {
    match target {
        AssignTarget::Name(n) => out.push_str(n),
        AssignTarget::Field { object, name } => {
            print_expr(out, object, Prec::Postfix);
            out.push('.');
            out.push_str(name);
        }
    }
}

fn assign_op_str(op: AssignOp) -> &'static str {
    match op {
        AssignOp::Set => "=",
        AssignOp::AddAssign => "+=",
        AssignOp::SubAssign => "-=",
        AssignOp::MulAssign => "*=",
        AssignOp::DivAssign => "/=",
    }
}

// --- expressions ---

/// Operator precedence levels. Higher number = binds tighter. The
/// printer adds parens around a child whose precedence is strictly
/// less than the parent's level (left-associative ops handle the
/// equal case correctly without parens because we consume from the
/// left).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    Lowest,
    Or,
    And,
    Not,
    Compare,
    AddSub,
    MulDiv,
    Range,
    Unary,
    Postfix,
    Atom,
}

fn binop_prec(op: BinOp) -> Prec {
    match op {
        BinOp::Or => Prec::Or,
        BinOp::And => Prec::And,
        BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Lte | BinOp::Gt | BinOp::Gte => Prec::Compare,
        BinOp::In | BinOp::NotIn => Prec::Compare,
        BinOp::Add | BinOp::Sub => Prec::AddSub,
        BinOp::Mul | BinOp::Div => Prec::MulDiv,
    }
}

fn binop_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Eq => "==",
        BinOp::Neq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Lte => "<=",
        BinOp::Gte => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::In => "in",
        BinOp::NotIn => "not in",
    }
}

fn print_expr(out: &mut String, expr: &Expr, parent_prec: Prec) {
    match expr {
        Expr::Str { value, .. } => print_string_literal(out, value),
        Expr::Interp { parts, exprs, .. } => print_interp(out, parts, exprs),
        Expr::Int { value, .. } => out.push_str(&value.to_string()),
        Expr::Float { value, .. } => print_float(out, *value),
        Expr::Bool { value, .. } => out.push_str(if *value { "true" } else { "false" }),
        Expr::Percent { value, .. } => {
            // Lexer stores the human-facing value verbatim — `5%`
            // becomes Percent(5.0), not Percent(0.05). Round-trip
            // by printing the same number with `%` appended.
            if value.fract() == 0.0 && value.is_finite() {
                out.push_str(&format!("{}%", *value as i64));
            } else {
                out.push_str(&format!("{value}%"));
            }
        }
        Expr::Quantity { value, unit, .. } => {
            // Whole-number quantity emits without `.0`.
            if value.fract() == 0.0 && value.is_finite() {
                out.push_str(&format!("{}{unit}", *value as i64));
            } else {
                out.push_str(&format!("{value}{unit}"));
            }
        }
        Expr::Ident { name, .. } => out.push_str(name),
        Expr::SelfRef { .. } => out.push_str("self"),
        Expr::Tuple { elems, .. } => {
            out.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                print_expr(out, e, Prec::Lowest);
            }
            // Single-element tuple needs a trailing comma to
            // distinguish it from a parenthesized expression,
            // matching the parser's `(x,)` form.
            if elems.len() == 1 {
                out.push(',');
            }
            out.push(')');
        }
        Expr::List { elems, .. } => {
            out.push('[');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                print_expr(out, e, Prec::Lowest);
            }
            out.push(']');
        }
        Expr::Range {
            start,
            end,
            exclusive,
            ..
        } => {
            let need_paren = parent_prec > Prec::Range;
            if need_paren {
                out.push('(');
            }
            print_expr(out, start, Prec::Range);
            out.push_str(if *exclusive { "..<" } else { ".." });
            print_expr(out, end, Prec::Range);
            if need_paren {
                out.push(')');
            }
        }
        Expr::Index { object, index, .. } => {
            print_expr(out, object, Prec::Postfix);
            out.push('[');
            print_expr(out, index, Prec::Lowest);
            out.push(']');
        }
        Expr::Field { object, name, .. } => {
            print_expr(out, object, Prec::Postfix);
            out.push('.');
            out.push_str(name);
        }
        Expr::Call {
            callee,
            args,
            kwargs,
            ..
        } => {
            print_expr(out, callee, Prec::Postfix);
            out.push('(');
            let mut first = true;
            for a in args {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                print_expr(out, a, Prec::Lowest);
            }
            for (name, value) in kwargs {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                out.push_str(name);
                out.push_str(": ");
                print_expr(out, value, Prec::Lowest);
            }
            out.push(')');
        }
        Expr::Unary { op, operand, .. } => {
            let (sym, child_prec) = match op {
                UnOp::Neg => ("-", Prec::Unary),
                UnOp::Not => ("not ", Prec::Not),
            };
            let need_paren = parent_prec > child_prec;
            if need_paren {
                out.push('(');
            }
            out.push_str(sym);
            print_expr(out, operand, child_prec);
            if need_paren {
                out.push(')');
            }
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            let prec = binop_prec(*op);
            let need_paren = parent_prec > prec;
            if need_paren {
                out.push('(');
            }
            // Left-assoc: child at same prec on the left is fine
            // without parens; child on the right needs parens if
            // its prec is strictly less, but to keep the printer
            // simple and round-trip-safe we treat right as needing
            // strictly greater prec (which matches Twe's
            // left-associative parsing).
            print_expr(out, left, prec);
            out.push(' ');
            out.push_str(binop_str(*op));
            out.push(' ');
            print_expr(out, right, prec.next());
            if need_paren {
                out.push(')');
            }
        }
        Expr::IfExpr {
            cond,
            then_expr,
            elifs,
            else_expr,
            ..
        } => {
            // Single-line ternary form is always parenthesised when
            // emitted from a non-lowest precedence context, since the
            // unbounded right-extent of `else: ...` could otherwise
            // bind to surrounding operators.
            let need_paren = parent_prec > Prec::Lowest;
            if need_paren {
                out.push('(');
            }
            out.push_str("if ");
            print_expr(out, cond, Prec::Lowest);
            out.push_str(": ");
            print_expr(out, then_expr, Prec::Lowest);
            for (ec, ee) in elifs {
                out.push_str(" elif ");
                print_expr(out, ec, Prec::Lowest);
                out.push_str(": ");
                print_expr(out, ee, Prec::Lowest);
            }
            out.push_str(" else: ");
            print_expr(out, else_expr, Prec::Lowest);
            if need_paren {
                out.push(')');
            }
        }
    }
}

impl Prec {
    fn next(self) -> Prec {
        // The "next" precedence level (one rung tighter). Used to
        // force a right-side child to a strictly higher level —
        // preserves left-associativity on round-trip.
        match self {
            Prec::Lowest => Prec::Or,
            Prec::Or => Prec::And,
            Prec::And => Prec::Not,
            Prec::Not => Prec::Compare,
            Prec::Compare => Prec::AddSub,
            Prec::AddSub => Prec::MulDiv,
            Prec::MulDiv => Prec::Range,
            Prec::Range => Prec::Unary,
            Prec::Unary => Prec::Postfix,
            Prec::Postfix => Prec::Atom,
            Prec::Atom => Prec::Atom,
        }
    }
}

fn print_float(out: &mut String, x: f64) {
    // Prefer the shortest representation that round-trips, with a
    // trailing `.0` for whole numbers so the source still parses
    // as a Float (not an Int). `{x:?}` does this naturally:
    // `1.0_f64` prints as `"1.0"`, `1.5_f64` as `"1.5"`.
    out.push_str(&format!("{x:?}"));
}

fn print_string_literal(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn print_interp(out: &mut String, parts: &[String], exprs: &[Expr]) {
    out.push('"');
    for (i, p) in parts.iter().enumerate() {
        for ch in p.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '{' => out.push_str("\\{"),
                '}' => out.push_str("\\}"),
                c => out.push(c),
            }
        }
        if let Some(e) = exprs.get(i) {
            out.push('{');
            print_expr(out, e, Prec::Lowest);
            out.push('}');
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer;
    use crate::parser;

    fn fmt(src: &str) -> String {
        let tokens = lexer::lex(&format!("{src}\n")).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        print_program(&program)
    }

    #[test]
    fn formats_let() {
        assert_eq!(fmt("let x = 5"), "let x = 5\n");
    }

    #[test]
    fn formats_arithmetic_with_precedence() {
        // Multiplication binds tighter than addition; no parens needed.
        assert_eq!(fmt("let x = 1 + 2 * 3"), "let x = 1 + 2 * 3\n");
        // Parenthesized addition under multiplication needs parens to round-trip.
        assert_eq!(fmt("let x = (1 + 2) * 3"), "let x = (1 + 2) * 3\n");
    }

    #[test]
    fn formats_string_and_interpolation() {
        assert_eq!(fmt("print(\"hi\")"), "print(\"hi\")\n");
        assert_eq!(
            fmt("let n = 5\nprint(\"n = {n}\")"),
            "let n = 5\nprint(\"n = {n}\")\n"
        );
    }

    #[test]
    fn formats_if_elif_else() {
        let src = "if x < 3:\n    print(1)\nelif x < 6:\n    print(2)\nelse:\n    print(3)";
        assert_eq!(
            fmt(src),
            "if x < 3:\n    print(1)\nelif x < 6:\n    print(2)\nelse:\n    print(3)\n"
        );
    }

    #[test]
    fn formats_for_over_range() {
        assert_eq!(
            fmt("for i in 0..<5:\n    print(i)"),
            "for i in 0..<5:\n    print(i)\n"
        );
    }

    #[test]
    fn formats_function_decl() {
        let src = "function add(a, b):\n    return a + b";
        assert_eq!(fmt(src), "function add(a, b):\n    return a + b\n");
    }

    #[test]
    fn formats_method_with_self_field() {
        let src = "item Counter:\n    n: 0\n\n    bump(amount):\n        self.n = self.n + amount";
        // Field-then-method gets a blank line between.
        let want =
            "item Counter:\n    n: 0\n\n    bump(amount):\n        self.n = self.n + amount\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn formats_scene_with_state_and_every() {
        let src = "scene S:\n    var n: int = 0\n\n    initial: a\n\n    state a:\n        every 100ms:\n            n += 1\n";
        // Phase 6 session 4: field annotations are now kept on
        // the AST, so the formatter round-trips `n: int = 0`
        // verbatim instead of stripping the type.
        let got = fmt(src);
        assert!(got.contains("scene S:\n"), "got: {got}");
        assert!(got.contains("    n: int = 0\n"), "got: {got}");
        assert!(got.contains("    initial: a\n"), "got: {got}");
        assert!(got.contains("    state a:\n"), "got: {got}");
        assert!(got.contains("        every 100ms:\n"), "got: {got}");
        assert!(got.contains("            n += 1\n"), "got: {got}");
    }

    #[test]
    fn formats_tuple_and_list_literals() {
        assert_eq!(fmt("let p = (3, 4)"), "let p = (3, 4)\n");
        assert_eq!(fmt("let xs = [1, 2, 3]"), "let xs = [1, 2, 3]\n");
        // Single-element tuple needs trailing comma.
        assert_eq!(fmt("let t = (1,)"), "let t = (1,)\n");
    }

    #[test]
    fn formats_keyword_args() {
        assert_eq!(
            fmt("rect(at: (10, 20), size: (3, 4))"),
            "rect(at: (10, 20), size: (3, 4))\n"
        );
    }

    #[test]
    fn formats_short_circuit_and_or_with_parens_when_needed() {
        // `a and b or c` — `and` binds tighter than `or`, no paren needed.
        assert_eq!(fmt("let x = a and b or c"), "let x = a and b or c\n");
        // `(a or b) and c` — paren needed because `or` < `and`.
        assert_eq!(fmt("let x = (a or b) and c"), "let x = (a or b) and c\n");
    }

    #[test]
    fn idempotent_on_method_program() {
        let src = "item Counter:\n    value: 0\n\n    bump(amount):\n        self.value = self.value + amount\n\nlet c = Counter()\nprint(c.value)\nc.bump(5)\nprint(c.value)";
        let once = fmt(src);
        let twice = fmt(&once);
        assert_eq!(once, twice, "fmt should be idempotent");
    }
}
