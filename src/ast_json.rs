use crate::ast::{
    AssignOp, AssignTarget, BinOp, DeclKind, DeclMember, Expr, Program, StateMember, Stmt, UnOp,
};

pub fn to_json(program: &Program) -> String {
    let mut s = String::with_capacity(program.stmts.len() * 64);
    s.push_str("{\"kind\":\"Program\",\"stmts\":[");
    for (i, stmt) in program.stmts.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_stmt(&mut s, stmt);
    }
    s.push_str("]}");
    s
}

fn write_stmt(s: &mut String, stmt: &Stmt) {
    match stmt {
        Stmt::Let {
            name,
            value,
            ty,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Let\",\"name\":");
            write_str_value(s, name);
            s.push_str(",\"value\":");
            write_expr(s, value);
            s.push_str(",\"annotation\":");
            match ty {
                Some(t) => write_str_value(s, &t.to_string()),
                None => s.push_str("null"),
            }
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Assign {
            target,
            op,
            value,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Assign\",\"op\":");
            write_str_value(s, assign_op_str(*op));
            s.push_str(",\"target\":");
            write_target(s, target);
            s.push_str(",\"value\":");
            write_expr(s, value);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::If {
            cond,
            then_body,
            elifs,
            else_body,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"If\",\"cond\":");
            write_expr(s, cond);
            s.push_str(",\"then\":");
            write_block(s, then_body);
            s.push_str(",\"elifs\":[");
            for (i, (c, b)) in elifs.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str("{\"cond\":");
                write_expr(s, c);
                s.push_str(",\"body\":");
                write_block(s, b);
                s.push('}');
            }
            s.push(']');
            if let Some(eb) = else_body {
                s.push_str(",\"else\":");
                write_block(s, eb);
            } else {
                s.push_str(",\"else\":null");
            }
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::OnUpdate {
            param,
            body,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"OnUpdate\",\"param\":");
            write_str_value(s, param);
            s.push_str(",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::OnRender { body, line, col } => {
            s.push_str("{\"kind\":\"OnRender\",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::OnClassEvent {
            class,
            event,
            param,
            body,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"OnClassEvent\",\"class\":");
            write_str_value(s, class);
            s.push_str(",\"event\":");
            write_str_value(s, event);
            s.push_str(",\"param\":");
            write_str_value(s, param);
            s.push_str(",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Decl {
            kind,
            name,
            parent,
            members,
            deprecation,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Decl\",\"declKind\":");
            write_str_value(s, decl_kind_str(*kind));
            s.push_str(",\"name\":");
            write_str_value(s, name);
            s.push_str(",\"parent\":");
            match parent {
                Some(p) => write_str_value(s, p),
                None => s.push_str("null"),
            }
            s.push_str(",\"members\":[");
            for (i, m) in members.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_member(s, m);
            }
            s.push(']');
            write_deprecation(s, deprecation);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::FunctionDecl {
            name,
            params,
            ret,
            body,
            deprecation,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"FunctionDecl\",\"name\":");
            write_str_value(s, name);
            s.push_str(",\"params\":[");
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str("{\"name\":");
                write_str_value(s, &p.name);
                s.push_str(",\"annotation\":");
                match &p.ty {
                    Some(t) => write_str_value(s, &t.to_string()),
                    None => s.push_str("null"),
                }
                s.push('}');
            }
            s.push_str("],\"returnAnnotation\":");
            match ret {
                Some(t) => write_str_value(s, &t.to_string()),
                None => s.push_str("null"),
            }
            s.push_str(",\"body\":");
            write_block(s, body);
            write_deprecation(s, deprecation);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Return { value, line, col } => {
            s.push_str("{\"kind\":\"Return\",\"value\":");
            match value {
                Some(e) => write_expr(s, e),
                None => s.push_str("null"),
            }
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::While {
            cond,
            body,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"While\",\"cond\":");
            write_expr(s, cond);
            s.push_str(",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::For {
            var,
            iter,
            body,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"For\",\"var\":");
            write_str_value(s, var);
            s.push_str(",\"iter\":");
            write_expr(s, iter);
            s.push_str(",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Break { line, col } => {
            s.push_str("{\"kind\":\"Break\"");
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Continue { line, col } => {
            s.push_str("{\"kind\":\"Continue\"");
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Transition { target, line, col } => {
            s.push_str("{\"kind\":\"Transition\",\"target\":");
            write_str_value(s, target);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Spawn {
            class,
            at,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Spawn\",\"class\":");
            write_str_value(s, class);
            s.push_str(",\"at\":");
            match at {
                Some(e) => write_expr(s, e),
                None => s.push_str("null"),
            }
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Despawn { target, line, col } => {
            s.push_str("{\"kind\":\"Despawn\",\"target\":");
            write_expr(s, target);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Wait {
            duration,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Wait\",\"duration\":");
            write_expr(s, duration);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::DialogueDecl {
            name,
            body,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"DialogueDecl\",\"name\":");
            write_str_value(s, name);
            s.push_str(",\"body\":[");
            for (i, st) in body.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_stmt(s, st);
            }
            s.push(']');
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Say {
            actor,
            text,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Say\",\"actor\":");
            match actor {
                Some(a) => write_expr(s, a),
                None => s.push_str("null"),
            }
            s.push_str(",\"text\":");
            write_expr(s, text);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Choice {
            branches,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Choice\",\"branches\":[");
            for (i, (label, body)) in branches.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str("{\"label\":");
                write_expr(s, label);
                s.push_str(",\"body\":[");
                for (j, st) in body.iter().enumerate() {
                    if j > 0 {
                        s.push(',');
                    }
                    write_stmt(s, st);
                }
                s.push_str("]}");
            }
            s.push(']');
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Import {
            path,
            alias,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Import\",\"path\":");
            write_str_value(s, path);
            s.push_str(",\"alias\":");
            match alias {
                Some(name) => write_str_value(s, name),
                None => s.push_str("null"),
            }
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Expr(e) => {
            s.push_str("{\"kind\":\"ExprStmt\",\"expr\":");
            write_expr(s, e);
            s.push('}');
        }
    }
}

fn write_member(s: &mut String, m: &DeclMember) {
    match m {
        DeclMember::Field {
            name,
            value,
            ty,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Field\",\"name\":");
            write_str_value(s, name);
            s.push_str(",\"value\":");
            write_expr(s, value);
            s.push_str(",\"annotation\":");
            match ty {
                Some(t) => write_str_value(s, &t.to_string()),
                None => s.push_str("null"),
            }
            write_pos(s, *line, *col);
            s.push('}');
        }
        DeclMember::Method {
            name,
            params,
            ret,
            body,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Method\",\"name\":");
            write_str_value(s, name);
            s.push_str(",\"params\":[");
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str("{\"name\":");
                write_str_value(s, &p.name);
                s.push_str(",\"annotation\":");
                match &p.ty {
                    Some(t) => write_str_value(s, &t.to_string()),
                    None => s.push_str("null"),
                }
                s.push('}');
            }
            s.push_str("],\"returnAnnotation\":");
            match ret {
                Some(t) => write_str_value(s, &t.to_string()),
                None => s.push_str("null"),
            }
            s.push_str(",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        DeclMember::InitialState { name, line, col } => {
            s.push_str("{\"kind\":\"InitialState\",\"name\":");
            write_str_value(s, name);
            write_pos(s, *line, *col);
            s.push('}');
        }
        DeclMember::State {
            name,
            members,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"State\",\"name\":");
            write_str_value(s, name);
            s.push_str(",\"members\":[");
            for (i, sm) in members.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_state_member(s, sm);
            }
            s.push(']');
            write_pos(s, *line, *col);
            s.push('}');
        }
    }
}

fn write_state_member(s: &mut String, m: &StateMember) {
    match m {
        StateMember::Stmt(st) => {
            s.push_str("{\"kind\":\"Stmt\",\"stmt\":");
            write_stmt(s, st);
            s.push('}');
        }
        StateMember::Every {
            interval,
            body,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Every\",\"interval\":");
            write_expr(s, interval);
            s.push_str(",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        StateMember::OnRender { body, line, col } => {
            s.push_str("{\"kind\":\"OnRender\",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        StateMember::OnKeyPress {
            key,
            body,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"OnKeyPress\",\"key\":");
            write_str_value(s, key);
            s.push_str(",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        StateMember::OnUpdate {
            param,
            body,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"OnUpdate\",\"param\":");
            write_str_value(s, param);
            s.push_str(",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        StateMember::OnPredicate {
            predicate,
            body,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"OnPredicate\",\"predicate\":");
            write_expr(s, predicate);
            s.push_str(",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        StateMember::OnEnter { body, line, col } => {
            s.push_str("{\"kind\":\"OnEnter\",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        StateMember::OnExit { body, line, col } => {
            s.push_str("{\"kind\":\"OnExit\",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
    }
}

fn write_target(s: &mut String, target: &AssignTarget) {
    match target {
        AssignTarget::Name(n) => {
            s.push_str("{\"kind\":\"Name\",\"name\":");
            write_str_value(s, n);
            s.push('}');
        }
        AssignTarget::Field { object, name } => {
            s.push_str("{\"kind\":\"Field\",\"object\":");
            write_expr(s, object);
            s.push_str(",\"name\":");
            write_str_value(s, name);
            s.push('}');
        }
    }
}

fn write_expr(s: &mut String, expr: &Expr) {
    match expr {
        Expr::Str { value, line, col } => {
            s.push_str("{\"kind\":\"Str\",\"value\":");
            write_str_value(s, value);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Interp {
            parts,
            exprs,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Interp\",\"parts\":");
            write_str_array(s, parts);
            s.push_str(",\"exprs\":");
            write_expr_array(s, exprs);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Int { value, line, col } => {
            s.push_str("{\"kind\":\"Int\",\"value\":");
            s.push_str(&value.to_string());
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Float { value, line, col } => {
            s.push_str("{\"kind\":\"Float\",\"value\":");
            write_float_lit(s, *value);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Bool { value, line, col } => {
            s.push_str("{\"kind\":\"Bool\",\"value\":");
            s.push_str(if *value { "true" } else { "false" });
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Percent { value, line, col } => {
            s.push_str("{\"kind\":\"Percent\",\"value\":");
            write_float_lit(s, *value);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Quantity {
            value,
            unit,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Quantity\",\"value\":");
            write_float_lit(s, *value);
            s.push_str(",\"unit\":");
            write_str_value(s, unit);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Ident { name, line, col } => {
            s.push_str("{\"kind\":\"Ident\",\"name\":");
            write_str_value(s, name);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::SelfRef { line, col } => {
            s.push_str("{\"kind\":\"Self\"");
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Tuple { elems, line, col } => {
            s.push_str("{\"kind\":\"Tuple\",\"elems\":");
            write_expr_array(s, elems);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::List { elems, line, col } => {
            s.push_str("{\"kind\":\"List\",\"elems\":");
            write_expr_array(s, elems);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Index {
            object,
            index,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Index\",\"object\":");
            write_expr(s, object);
            s.push_str(",\"index\":");
            write_expr(s, index);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Range {
            start,
            end,
            exclusive,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Range\",\"start\":");
            write_expr(s, start);
            s.push_str(",\"end\":");
            write_expr(s, end);
            s.push_str(",\"exclusive\":");
            s.push_str(if *exclusive { "true" } else { "false" });
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Field {
            object,
            name,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Field\",\"object\":");
            write_expr(s, object);
            s.push_str(",\"name\":");
            write_str_value(s, name);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Call {
            callee,
            args,
            kwargs,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Call\",\"callee\":");
            write_expr(s, callee);
            s.push_str(",\"args\":");
            write_expr_array(s, args);
            s.push_str(",\"kwargs\":[");
            for (i, (k, v)) in kwargs.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str("{\"name\":");
                write_str_value(s, k);
                s.push_str(",\"value\":");
                write_expr(s, v);
                s.push('}');
            }
            s.push(']');
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Unary {
            op,
            operand,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Unary\",\"op\":");
            write_str_value(s, un_op_str(*op));
            s.push_str(",\"operand\":");
            write_expr(s, operand);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Binary {
            op,
            left,
            right,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"Binary\",\"op\":");
            write_str_value(s, bin_op_str(*op));
            s.push_str(",\"left\":");
            write_expr(s, left);
            s.push_str(",\"right\":");
            write_expr(s, right);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::IfExpr {
            cond,
            then_expr,
            elifs,
            else_expr,
            line,
            col,
        } => {
            s.push_str("{\"kind\":\"IfExpr\",\"cond\":");
            write_expr(s, cond);
            s.push_str(",\"then\":");
            write_expr(s, then_expr);
            s.push_str(",\"elifs\":[");
            for (i, (c, e)) in elifs.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str("{\"cond\":");
                write_expr(s, c);
                s.push_str(",\"expr\":");
                write_expr(s, e);
                s.push('}');
            }
            s.push_str("],\"else\":");
            write_expr(s, else_expr);
            write_pos(s, *line, *col);
            s.push('}');
        }
        // Phase 33 session 9: typed hole.
        Expr::Hole { line, col } => {
            s.push_str("{\"kind\":\"Hole\"");
            write_pos(s, *line, *col);
            s.push('}');
        }
    }
}

fn write_block(s: &mut String, stmts: &[Stmt]) {
    s.push('[');
    for (i, st) in stmts.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_stmt(s, st);
    }
    s.push(']');
}

fn write_expr_array(s: &mut String, exprs: &[Expr]) {
    s.push('[');
    for (i, e) in exprs.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_expr(s, e);
    }
    s.push(']');
}

fn write_str_array(s: &mut String, items: &[String]) {
    s.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_str_value(s, item);
    }
    s.push(']');
}

fn write_pos(s: &mut String, line: u32, col: u32) {
    s.push_str(",\"line\":");
    s.push_str(&line.to_string());
    s.push_str(",\"col\":");
    s.push_str(&col.to_string());
}

/// Phase 13 session 9: emit the optional `@deprecated` annotation
/// onto a Stmt::Decl / Stmt::FunctionDecl JSON object. Omitted
/// when None so older AST consumers see the same shape.
fn write_deprecation(s: &mut String, dep: &Option<crate::ast::Deprecation>) {
    if let Some(d) = dep {
        s.push_str(",\"deprecation\":{\"since\":");
        match &d.since {
            Some(v) => write_str_value(s, v),
            None => s.push_str("null"),
        }
        s.push_str(",\"line\":");
        s.push_str(&d.line.to_string());
        s.push_str(",\"col\":");
        s.push_str(&d.col.to_string());
        s.push('}');
    }
}

fn write_str_value(s: &mut String, value: &str) {
    s.push('"');
    for c in value.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                s.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => s.push(c),
        }
    }
    s.push('"');
}

fn write_float_lit(s: &mut String, value: f64) {
    if value.is_finite() {
        // {f:?} produces "1.0" rather than "1", and "1.5" rather than "1.5e0".
        // Matches JSON spec for finite floats; non-finite need string fallback.
        s.push_str(&format!("{value:?}"));
    } else if value.is_nan() {
        s.push_str("\"NaN\"");
    } else if value > 0.0 {
        s.push_str("\"Infinity\"");
    } else {
        s.push_str("\"-Infinity\"");
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

fn un_op_str(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "-",
        UnOp::Not => "not",
    }
}

fn bin_op_str(op: BinOp) -> &'static str {
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

fn decl_kind_str(k: DeclKind) -> &'static str {
    k.as_str()
}
