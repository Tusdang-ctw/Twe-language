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
        Stmt::Let { name, value, line, col } => {
            s.push_str("{\"kind\":\"Let\",\"name\":");
            write_str_value(s, name);
            s.push_str(",\"value\":");
            write_expr(s, value);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Assign { target, op, value, line, col } => {
            s.push_str("{\"kind\":\"Assign\",\"op\":");
            write_str_value(s, assign_op_str(*op));
            s.push_str(",\"target\":");
            write_target(s, target);
            s.push_str(",\"value\":");
            write_expr(s, value);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::If { cond, then_body, elifs, else_body, line, col } => {
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
        Stmt::OnUpdate { param, body, line, col } => {
            s.push_str("{\"kind\":\"OnUpdate\",\"param\":");
            write_str_value(s, param);
            s.push_str(",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::Decl { kind, name, parent, members, line, col } => {
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
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::FunctionDecl { name, params, body, line, col } => {
            s.push_str("{\"kind\":\"FunctionDecl\",\"name\":");
            write_str_value(s, name);
            s.push_str(",\"params\":");
            write_str_array(s, params);
            s.push_str(",\"body\":");
            write_block(s, body);
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
        Stmt::While { cond, body, line, col } => {
            s.push_str("{\"kind\":\"While\",\"cond\":");
            write_expr(s, cond);
            s.push_str(",\"body\":");
            write_block(s, body);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Stmt::For { var, iter, body, line, col } => {
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
        Stmt::Expr(e) => {
            s.push_str("{\"kind\":\"ExprStmt\",\"expr\":");
            write_expr(s, e);
            s.push('}');
        }
    }
}

fn write_member(s: &mut String, m: &DeclMember) {
    match m {
        DeclMember::Field { name, value, line, col } => {
            s.push_str("{\"kind\":\"Field\",\"name\":");
            write_str_value(s, name);
            s.push_str(",\"value\":");
            write_expr(s, value);
            write_pos(s, *line, *col);
            s.push('}');
        }
        DeclMember::Method { name, params, body, line, col } => {
            s.push_str("{\"kind\":\"Method\",\"name\":");
            write_str_value(s, name);
            s.push_str(",\"params\":");
            write_str_array(s, params);
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
        DeclMember::State { name, members, line, col } => {
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
        StateMember::Every { interval, body, line, col } => {
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
        StateMember::OnKeyPress { key, body, line, col } => {
            s.push_str("{\"kind\":\"OnKeyPress\",\"key\":");
            write_str_value(s, key);
            s.push_str(",\"body\":");
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
        Expr::Interp { parts, exprs, line, col } => {
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
        Expr::Quantity { value, unit, line, col } => {
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
        Expr::Index { object, index, line, col } => {
            s.push_str("{\"kind\":\"Index\",\"object\":");
            write_expr(s, object);
            s.push_str(",\"index\":");
            write_expr(s, index);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Range { start, end, exclusive, line, col } => {
            s.push_str("{\"kind\":\"Range\",\"start\":");
            write_expr(s, start);
            s.push_str(",\"end\":");
            write_expr(s, end);
            s.push_str(",\"exclusive\":");
            s.push_str(if *exclusive { "true" } else { "false" });
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Field { object, name, line, col } => {
            s.push_str("{\"kind\":\"Field\",\"object\":");
            write_expr(s, object);
            s.push_str(",\"name\":");
            write_str_value(s, name);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Call { callee, args, line, col } => {
            s.push_str("{\"kind\":\"Call\",\"callee\":");
            write_expr(s, callee);
            s.push_str(",\"args\":");
            write_expr_array(s, args);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Unary { op, operand, line, col } => {
            s.push_str("{\"kind\":\"Unary\",\"op\":");
            write_str_value(s, un_op_str(*op));
            s.push_str(",\"operand\":");
            write_expr(s, operand);
            write_pos(s, *line, *col);
            s.push('}');
        }
        Expr::Binary { op, left, right, line, col } => {
            s.push_str("{\"kind\":\"Binary\",\"op\":");
            write_str_value(s, bin_op_str(*op));
            s.push_str(",\"left\":");
            write_expr(s, left);
            s.push_str(",\"right\":");
            write_expr(s, right);
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
