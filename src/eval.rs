use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{
    AssignOp, AssignTarget, BinOp, DeclKind, DeclMember, Expr, Program, Stmt, UnOp,
};
use crate::stdlib;
use crate::value::{
    ClassDef, Env, FunctionDef, Instance, MethodDef, OnUpdateHandler, RuntimeError, Value,
};

pub fn run(program: &Program) -> Result<String, RuntimeError> {
    run_with_frames(program, 0, 1.0 / 60.0)
}

pub fn run_with_frames(
    program: &Program,
    frames: u32,
    dt: f64,
) -> Result<String, RuntimeError> {
    let mut env = Env::new();
    stdlib::install(&mut env);
    run_block(&mut env, &program.stmts)?;
    if env.returning.take().is_some() {
        // Top-level `return` — drop the value, stop here.
        return Ok(env.out);
    }
    if let Some(handler) = env.on_update.clone() {
        for _ in 0..frames {
            env.set(handler.param.clone(), Value::Float(dt));
            run_block(&mut env, &handler.body)?;
            if env.returning.take().is_some() {
                break;
            }
        }
    }
    Ok(env.out)
}

fn run_block(env: &mut Env, stmts: &[Stmt]) -> Result<(), RuntimeError> {
    for stmt in stmts {
        eval_stmt(env, stmt)?;
        if env.returning.is_some() || env.breaking || env.continuing {
            return Ok(());
        }
    }
    Ok(())
}

fn eval_stmt(env: &mut Env, stmt: &Stmt) -> Result<(), RuntimeError> {
    match stmt {
        Stmt::Let { name, value, .. } => {
            let v = eval_expr(env, value)?;
            env.set(name.clone(), v);
            Ok(())
        }
        Stmt::Assign {
            target,
            op,
            value,
            line,
            col,
        } => eval_assign(env, target, *op, value, *line, *col),
        Stmt::If {
            cond,
            then_body,
            elifs,
            else_body,
            ..
        } => {
            let cond_val = eval_expr(env, cond)?;
            if is_truthy(&cond_val) {
                return run_block(env, then_body);
            }
            for (elif_cond, elif_body) in elifs {
                let v = eval_expr(env, elif_cond)?;
                if is_truthy(&v) {
                    return run_block(env, elif_body);
                }
            }
            if let Some(eb) = else_body {
                run_block(env, eb)?;
            }
            Ok(())
        }
        Stmt::FunctionDecl { name, params, body, .. } => {
            env.set(
                name.clone(),
                Value::Function(Rc::new(FunctionDef {
                    name: name.clone(),
                    params: params.clone(),
                    body: body.clone(),
                })),
            );
            Ok(())
        }
        Stmt::Return { value, line, col } => {
            if env.call_depth == 0 {
                return Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: "`return` is only valid inside a function or method body"
                        .to_string(),
                    help: None,
                });
            }
            let v = match value {
                Some(e) => eval_expr(env, e)?,
                None => Value::Nil,
            };
            env.returning = Some(v);
            Ok(())
        }
        Stmt::While { cond, body, .. } => {
            env.loop_depth += 1;
            let result = run_while(env, cond, body);
            env.loop_depth -= 1;
            result
        }
        Stmt::For {
            var, iter, body, line, col,
        } => {
            env.loop_depth += 1;
            let result = run_for(env, var, iter, body, *line, *col);
            env.loop_depth -= 1;
            result
        }
        Stmt::Break { line, col } => {
            if env.loop_depth == 0 {
                return Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: "`break` is only valid inside a loop".to_string(),
                    help: None,
                });
            }
            env.breaking = true;
            Ok(())
        }
        Stmt::Continue { line, col } => {
            if env.loop_depth == 0 {
                return Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: "`continue` is only valid inside a loop".to_string(),
                    help: None,
                });
            }
            env.continuing = true;
            Ok(())
        }
        Stmt::OnUpdate { param, body, .. } => {
            env.on_update = Some(OnUpdateHandler {
                param: param.clone(),
                body: body.clone(),
            });
            Ok(())
        }
        Stmt::Decl {
            kind,
            name,
            parent,
            members,
            line,
            col,
        } => eval_decl(env, *kind, name, parent.as_deref(), members, *line, *col),
        Stmt::Expr(e) => {
            eval_expr(env, e)?;
            Ok(())
        }
    }
}

fn eval_assign(
    env: &mut Env,
    target: &AssignTarget,
    op: AssignOp,
    value: &Expr,
    line: u32,
    col: u32,
) -> Result<(), RuntimeError> {
    let new_value = eval_expr(env, value)?;
    match target {
        AssignTarget::Name(name) => {
            if matches!(op, AssignOp::Set) {
                env.set(name.clone(), new_value);
                return Ok(());
            }
            let current = env.get(name).cloned().ok_or_else(|| RuntimeError {
                line,
                col,
                message: format!("name '{name}' is not defined"),
                help: Some(format!("declare it with `let {name} = ...` before use")),
            })?;
            let combined = compound(op, &current, &new_value, line, col)?;
            env.set(name.clone(), combined);
            Ok(())
        }
        AssignTarget::Field { object, name } => {
            let obj_val = eval_expr(env, object)?;
            match obj_val {
                Value::Object(rc) => {
                    let final_value = if matches!(op, AssignOp::Set) {
                        new_value
                    } else {
                        let current = rc.borrow().fields.get(name).cloned().ok_or_else(|| {
                            RuntimeError {
                                line,
                                col,
                                message: format!("field '{name}' is not defined on this object"),
                                help: Some(format!("set it first with `obj.{name} = ...`")),
                            }
                        })?;
                        compound(op, &current, &new_value, line, col)?
                    };
                    // Special case: `.pos = (x, y)` on a sprite-shaped object
                    // also updates `.x` and `.y`. Mirrors Example 1's
                    // tuple-as-Vector2 behavior.
                    if name == "pos" {
                        if let Value::Tuple(elems) = &final_value {
                            if elems.len() >= 2 {
                                let mut o = rc.borrow_mut();
                                o.fields.insert("x".to_string(), elems[0].clone());
                                o.fields.insert("y".to_string(), elems[1].clone());
                            }
                        }
                    }
                    rc.borrow_mut().fields.insert(name.clone(), final_value);
                    if name == "x" || name == "y" {
                        refresh_pos(&rc);
                    }
                    Ok(())
                }
                Value::Instance(rc) => {
                    let final_value = if matches!(op, AssignOp::Set) {
                        new_value
                    } else {
                        let current = rc.borrow().fields.get(name).cloned().ok_or_else(|| {
                            RuntimeError {
                                line,
                                col,
                                message: format!(
                                    "field '{name}' is not defined on instance of {}",
                                    rc.borrow().class.name
                                ),
                                help: None,
                            }
                        })?;
                        compound(op, &current, &new_value, line, col)?
                    };
                    rc.borrow_mut().fields.insert(name.clone(), final_value);
                    Ok(())
                }
                other => Err(RuntimeError {
                    line,
                    col,
                    message: format!(
                        "cannot assign field on value of type {}",
                        other.type_name()
                    ),
                    help: Some(
                        "only objects and class instances support field assignment".to_string(),
                    ),
                }),
            }
        }
    }
}

fn refresh_pos(rc: &Rc<std::cell::RefCell<crate::value::Object>>) {
    let (x, y) = {
        let o = rc.borrow();
        (
            o.fields.get("x").cloned().unwrap_or(Value::Nil),
            o.fields.get("y").cloned().unwrap_or(Value::Nil),
        )
    };
    rc.borrow_mut()
        .fields
        .insert("pos".to_string(), Value::Tuple(Rc::new(vec![x, y])));
}

fn compound(
    op: AssignOp,
    current: &Value,
    rhs: &Value,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    let bop = match op {
        AssignOp::Set => unreachable!("handled above"),
        AssignOp::AddAssign => BinOp::Add,
        AssignOp::SubAssign => BinOp::Sub,
        AssignOp::MulAssign => BinOp::Mul,
        AssignOp::DivAssign => BinOp::Div,
    };
    apply_arith(bop, current, rhs, line, col)
}

fn eval_expr(env: &mut Env, expr: &Expr) -> Result<Value, RuntimeError> {
    match expr {
        Expr::Str { value, .. } => Ok(Value::Str(Rc::new(value.clone()))),
        Expr::Interp { parts, exprs, .. } => {
            // parts.len() == exprs.len() + 1 by construction in lex_string.
            let mut out = String::new();
            for (i, p) in parts.iter().enumerate() {
                out.push_str(p);
                if let Some(e) = exprs.get(i) {
                    let v = eval_expr(env, e)?;
                    out.push_str(&v.display());
                }
            }
            Ok(Value::Str(Rc::new(out)))
        }
        Expr::Int { value, .. } => Ok(Value::Int(*value)),
        Expr::Float { value, .. } => Ok(Value::Float(*value)),
        Expr::Bool { value, .. } => Ok(Value::Bool(*value)),
        Expr::Percent { value, .. } => Ok(Value::Percent(*value)),
        Expr::Quantity { value, unit, .. } => Ok(Value::Quantity {
            value: *value,
            unit: Rc::new(unit.clone()),
        }),
        Expr::Ident { name, line, col } => env.get(name).cloned().ok_or_else(|| RuntimeError {
            line: *line,
            col: *col,
            message: format!("name '{name}' is not defined"),
            help: Some(format!("declare it with `let {name} = ...` before use")),
        }),
        Expr::SelfRef { line, col } => env.self_value.clone().ok_or_else(|| RuntimeError {
            line: *line,
            col: *col,
            message: "`self` is only valid inside a method body".to_string(),
            help: None,
        }),
        Expr::Tuple { elems, .. } => {
            let mut vals = Vec::with_capacity(elems.len());
            for e in elems {
                vals.push(eval_expr(env, e)?);
            }
            Ok(Value::Tuple(Rc::new(vals)))
        }
        Expr::List { elems, .. } => {
            let mut vals = Vec::with_capacity(elems.len());
            for e in elems {
                vals.push(eval_expr(env, e)?);
            }
            Ok(Value::List(Rc::new(RefCell::new(vals))))
        }
        Expr::Index { object, index, line, col } => {
            let obj = eval_expr(env, object)?;
            let idx = eval_expr(env, index)?;
            index_get(&obj, &idx, *line, *col)
        }
        Expr::Range {
            start,
            end,
            exclusive,
            line,
            col,
        } => {
            let s = eval_expr(env, start)?;
            let e = eval_expr(env, end)?;
            match (&s, &e) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Range {
                    start: *a,
                    end: *b,
                    exclusive: *exclusive,
                }),
                _ => Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: format!(
                        "range bounds must be ints, got {} and {}",
                        s.type_name(),
                        e.type_name()
                    ),
                    help: Some("v0.1 supports only integer ranges; float ranges ship later".to_string()),
                }),
            }
        }
        Expr::Field {
            object,
            name,
            line,
            col,
        } => {
            let obj = eval_expr(env, object)?;
            field_get(&obj, name, *line, *col)
        }
        Expr::Call {
            callee,
            args,
            line,
            col,
        } => eval_call(env, callee, args, *line, *col),
        Expr::Unary {
            op,
            operand,
            line,
            col,
        } => {
            let v = eval_expr(env, operand)?;
            match (op, &v) {
                (UnOp::Neg, Value::Int(n)) => Ok(Value::Int(-n)),
                (UnOp::Neg, Value::Float(x)) => Ok(Value::Float(-x)),
                (UnOp::Neg, _) => Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: format!("cannot negate value of type {}", v.type_name()),
                    help: Some("`-` is defined on int and float".to_string()),
                }),
                (UnOp::Not, _) => Ok(Value::Bool(!is_truthy(&v))),
            }
        }
        Expr::Binary {
            op,
            left,
            right,
            line,
            col,
        } => eval_binary(env, *op, left, right, *line, *col),
    }
}

fn index_get(obj: &Value, idx: &Value, line: u32, col: u32) -> Result<Value, RuntimeError> {
    match (obj, idx) {
        (Value::List(rc), Value::Int(i)) => {
            let v = rc.borrow();
            let len = v.len() as i64;
            let actual = if *i < 0 { *i + len } else { *i };
            if actual < 0 || actual >= len {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!("list index {i} out of bounds (length {len})"),
                    help: Some(
                        "lists are 0-indexed; negative indices count from the end".to_string(),
                    ),
                });
            }
            Ok(v[actual as usize].clone())
        }
        (Value::Tuple(elems), Value::Int(i)) => {
            let len = elems.len() as i64;
            let actual = if *i < 0 { *i + len } else { *i };
            if actual < 0 || actual >= len {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!("tuple index {i} out of bounds (length {len})"),
                    help: None,
                });
            }
            Ok(elems[actual as usize].clone())
        }
        (Value::List(_) | Value::Tuple(_), other) => Err(RuntimeError {
            line,
            col,
            message: format!("index must be int, got {}", other.type_name()),
            help: None,
        }),
        (other, _) => Err(RuntimeError {
            line,
            col,
            message: format!("cannot index value of type {}", other.type_name()),
            help: Some("indexing works on lists and tuples".to_string()),
        }),
    }
}

fn field_get(obj: &Value, name: &str, line: u32, col: u32) -> Result<Value, RuntimeError> {
    match obj {
        Value::Tuple(elems) => match name {
            "x" if !elems.is_empty() => Ok(elems[0].clone()),
            "y" if elems.len() >= 2 => Ok(elems[1].clone()),
            "z" if elems.len() >= 3 => Ok(elems[2].clone()),
            _ => Err(RuntimeError {
                line,
                col,
                message: format!("tuple has no field '{name}'"),
                help: Some(
                    "tuples expose .x, .y, .z (and only those for the leading components)"
                        .to_string(),
                ),
            }),
        },
        Value::List(rc) => match name {
            "length" => Ok(Value::Int(rc.borrow().len() as i64)),
            _ => Err(RuntimeError {
                line,
                col,
                message: format!("list has no field '{name}'"),
                help: Some(
                    "lists expose .length; methods are .append, .prepend, .pop_back, \
                     .pop_front, .contains"
                        .to_string(),
                ),
            }),
        },
        Value::Object(rc) => rc.borrow().fields.get(name).cloned().ok_or_else(|| {
            RuntimeError {
                line,
                col,
                message: format!("field '{name}' is not defined on this object"),
                help: Some(format!("set it first with `obj.{name} = ...`")),
            }
        }),
        Value::Instance(rc) => {
            let inst = rc.borrow();
            if let Some(v) = inst.fields.get(name) {
                return Ok(v.clone());
            }
            // Methods are not values yet — `obj.method` outside a call site is
            // not supported in this commit. The Call path resolves them
            // directly. Falling through to "field not defined" is correct.
            Err(RuntimeError {
                line,
                col,
                message: format!(
                    "field '{name}' is not defined on instance of {}",
                    inst.class.name
                ),
                help: None,
            })
        }
        _ => Err(RuntimeError {
            line,
            col,
            message: format!("cannot read field on value of type {}", obj.type_name()),
            help: None,
        }),
    }
}

fn eval_call(
    env: &mut Env,
    callee: &Expr,
    args: &[Expr],
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    // Method call: `recv.method(args)`. Resolved here (not via field_get)
    // because methods aren't first-class values yet.
    if let Expr::Field { object, name, .. } = callee {
        let recv = eval_expr(env, object)?;
        // List built-in methods.
        if let Value::List(rc) = &recv {
            if let Some(v) = list_method_call(env, rc, name, args, line, col)? {
                return Ok(v);
            }
        }
        if let Value::Range { start, end, exclusive } = &recv {
            if let Some(v) = range_method_call(
                env,
                *start,
                *end,
                *exclusive,
                name,
                args,
                line,
                col,
            )? {
                return Ok(v);
            }
        }
        if let Value::Instance(rc) = &recv {
            let class = rc.borrow().class.clone();
            if let Some(method) = find_method(&class, name) {
                let mut arg_vals = Vec::with_capacity(args.len());
                for a in args {
                    arg_vals.push(eval_expr(env, a)?);
                }
                return call_method(env, recv, &method, &arg_vals, line, col);
            }
            // Fall through to a normal field_get -> Call path, which will
            // produce a "field not defined" error below.
        }
        // Re-create the field-get + call path for non-instance receivers.
        let mut arg_vals = Vec::with_capacity(args.len());
        for a in args {
            arg_vals.push(eval_expr(env, a)?);
        }
        let f = field_get(&recv, name, line, col)?;
        return apply_call(env, f, &arg_vals, line, col);
    }
    let f = eval_expr(env, callee)?;
    let mut arg_vals = Vec::with_capacity(args.len());
    for a in args {
        arg_vals.push(eval_expr(env, a)?);
    }
    apply_call(env, f, &arg_vals, line, col)
}

#[allow(clippy::too_many_arguments)]
fn list_method_call(
    env: &mut Env,
    rc: &Rc<RefCell<Vec<Value>>>,
    name: &str,
    args: &[Expr],
    line: u32,
    col: u32,
) -> Result<Option<Value>, RuntimeError> {
    let arity_check = |expected: usize| -> Result<(), RuntimeError> {
        if args.len() != expected {
            Err(RuntimeError {
                line,
                col,
                message: format!(
                    "list.{name} expected {expected} argument{}, got {}",
                    if expected == 1 { "" } else { "s" },
                    args.len()
                ),
                help: None,
            })
        } else {
            Ok(())
        }
    };
    match name {
        "append" => {
            arity_check(1)?;
            let v = eval_expr(env, &args[0])?;
            rc.borrow_mut().push(v);
            Ok(Some(Value::Nil))
        }
        "prepend" => {
            arity_check(1)?;
            let v = eval_expr(env, &args[0])?;
            rc.borrow_mut().insert(0, v);
            Ok(Some(Value::Nil))
        }
        "pop_back" => {
            arity_check(0)?;
            rc.borrow_mut().pop().ok_or_else(|| RuntimeError {
                line,
                col,
                message: "pop_back on an empty list".to_string(),
                help: Some("guard with `if list.length > 0:` before popping".to_string()),
            }).map(Some)
        }
        "pop_front" => {
            arity_check(0)?;
            let mut v = rc.borrow_mut();
            if v.is_empty() {
                return Err(RuntimeError {
                    line,
                    col,
                    message: "pop_front on an empty list".to_string(),
                    help: Some("guard with `if list.length > 0:` before popping".to_string()),
                });
            }
            Ok(Some(v.remove(0)))
        }
        "contains" => {
            arity_check(1)?;
            let needle = eval_expr(env, &args[0])?;
            let found = rc.borrow().iter().any(|v| values_equal(v, &needle));
            Ok(Some(Value::Bool(found)))
        }
        _ => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
fn range_method_call(
    env: &mut Env,
    start: i64,
    end: i64,
    exclusive: bool,
    name: &str,
    args: &[Expr],
    line: u32,
    col: u32,
) -> Result<Option<Value>, RuntimeError> {
    match name {
        "roll" => {
            if !args.is_empty() {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!("range.roll expected 0 arguments, got {}", args.len()),
                    help: None,
                });
            }
            let upper = if exclusive { end } else { end + 1 };
            if upper <= start {
                return Err(RuntimeError {
                    line,
                    col,
                    message: "range.roll on an empty range".to_string(),
                    help: None,
                });
            }
            let n = env.next_random_u64();
            let span = (upper - start) as u64;
            Ok(Some(Value::Int(start + (n % span) as i64)))
        }
        "contains" => {
            if args.len() != 1 {
                return Err(RuntimeError {
                    line,
                    col,
                    message: "range.contains expected 1 argument".to_string(),
                    help: None,
                });
            }
            let v = eval_expr(env, &args[0])?;
            let upper = if exclusive { end } else { end + 1 };
            let result = match v {
                Value::Int(n) => n >= start && n < upper,
                _ => false,
            };
            Ok(Some(Value::Bool(result)))
        }
        _ => Ok(None),
    }
}

fn apply_call(
    env: &mut Env,
    f: Value,
    args: &[Value],
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    match f {
        Value::Builtin { func, .. } => func(env, args),
        Value::Function(def) => call_function(env, &def, args, line, col),
        Value::Class(class) => {
            if !args.is_empty() {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!(
                        "constructor for {} takes no arguments yet (got {})",
                        class.name,
                        args.len()
                    ),
                    help: Some(
                        "v0.1 constructors initialise from field defaults; \
                         positional/keyword args ship later"
                            .to_string(),
                    ),
                });
            }
            Ok(instantiate(class))
        }
        other => Err(RuntimeError {
            line,
            col,
            message: format!("cannot call value of type {}", other.type_name()),
            help: Some(
                "only functions, builtins, and class constructors are callable".to_string(),
            ),
        }),
    }
}

fn call_function(
    env: &mut Env,
    def: &FunctionDef,
    args: &[Value],
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    if args.len() != def.params.len() {
        return Err(RuntimeError {
            line,
            col,
            message: format!(
                "function '{}' expected {} arguments, got {}",
                def.name,
                def.params.len(),
                args.len()
            ),
            help: None,
        });
    }
    let saved_returning = env.returning.take();
    let saved_params: Vec<(String, Option<Value>)> = def
        .params
        .iter()
        .map(|p| (p.clone(), env.get(p).cloned()))
        .collect();
    for (param, arg) in def.params.iter().zip(args.iter()) {
        env.set(param.clone(), arg.clone());
    }
    env.call_depth += 1;
    let body_result = run_block(env, &def.body);
    env.call_depth -= 1;
    let return_value = env.returning.take().unwrap_or(Value::Nil);
    env.returning = saved_returning;
    for (name, prev) in saved_params {
        match prev {
            Some(v) => env.set(name, v),
            None => env.remove(&name),
        }
    }
    body_result?;
    Ok(return_value)
}

fn instantiate(class: Rc<ClassDef>) -> Value {
    let mut fields = HashMap::new();
    // Walk the parent chain, oldest first, so child overrides win.
    let mut chain: Vec<Rc<ClassDef>> = Vec::new();
    let mut cur = Some(class.clone());
    while let Some(c) = cur {
        chain.push(c.clone());
        cur = c.parent.clone();
    }
    for c in chain.iter().rev() {
        for (k, v) in &c.field_defaults {
            fields.insert(k.clone(), v.clone());
        }
    }
    Value::Instance(Rc::new(RefCell::new(Instance { class, fields })))
}

fn find_method(class: &ClassDef, name: &str) -> Option<Rc<MethodDef>> {
    if let Some(m) = class.methods.get(name) {
        return Some(m.clone());
    }
    class.parent.as_ref().and_then(|p| find_method(p, name))
}

fn call_method(
    env: &mut Env,
    recv: Value,
    method: &MethodDef,
    args: &[Value],
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    if args.len() != method.params.len() {
        return Err(RuntimeError {
            line,
            col,
            message: format!(
                "method expected {} arguments, got {}",
                method.params.len(),
                args.len()
            ),
            help: None,
        });
    }
    let saved_self = env.self_value.replace(recv);
    let saved_returning = env.returning.take();
    let saved_params: Vec<(String, Option<Value>)> = method
        .params
        .iter()
        .map(|p| (p.clone(), env.get(p).cloned()))
        .collect();
    for (param, arg) in method.params.iter().zip(args.iter()) {
        env.set(param.clone(), arg.clone());
    }
    env.call_depth += 1;
    let body_result = run_block(env, &method.body);
    env.call_depth -= 1;
    let return_value = env.returning.take().unwrap_or(Value::Nil);
    env.returning = saved_returning;
    env.self_value = saved_self;
    for (name, prev) in saved_params {
        match prev {
            Some(v) => env.set(name, v),
            None => env.remove(&name),
        }
    }
    body_result?;
    Ok(return_value)
}

fn run_while(env: &mut Env, cond: &Expr, body: &[Stmt]) -> Result<(), RuntimeError> {
    loop {
        let v = eval_expr(env, cond)?;
        if !is_truthy(&v) {
            break;
        }
        run_block(env, body)?;
        if env.returning.is_some() {
            break;
        }
        if env.breaking {
            env.breaking = false;
            break;
        }
        if env.continuing {
            env.continuing = false;
        }
    }
    Ok(())
}

fn run_for(
    env: &mut Env,
    var: &str,
    iter: &Expr,
    body: &[Stmt],
    line: u32,
    col: u32,
) -> Result<(), RuntimeError> {
    let iter_val = eval_expr(env, iter)?;
    let (start, end, exclusive) = match iter_val {
        Value::Range { start, end, exclusive } => (start, end, exclusive),
        other => {
            return Err(RuntimeError {
                line,
                col,
                message: format!(
                    "for-loop iterable must be a range, got {}",
                    other.type_name()
                ),
                help: Some("v0.1 only supports `for i in <range>:`; lists ship later".to_string()),
            });
        }
    };
    let saved = env.get(var).cloned();
    let limit = if exclusive { end } else { end + 1 };
    let mut i = start;
    while i < limit {
        env.set(var.to_string(), Value::Int(i));
        run_block(env, body)?;
        if env.returning.is_some() {
            break;
        }
        if env.breaking {
            env.breaking = false;
            break;
        }
        if env.continuing {
            env.continuing = false;
        }
        i += 1;
    }
    match saved {
        Some(v) => env.set(var.to_string(), v),
        None => env.remove(var),
    }
    Ok(())
}

fn eval_decl(
    env: &mut Env,
    kind: DeclKind,
    name: &str,
    parent: Option<&str>,
    members: &[DeclMember],
    line: u32,
    col: u32,
) -> Result<(), RuntimeError> {
    let parent_class = if let Some(p) = parent {
        match env.get(p) {
            Some(Value::Class(c)) => Some(c.clone()),
            Some(other) => {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!(
                        "cannot extend `{p}`: it is a {}, not a class",
                        other.type_name()
                    ),
                    help: None,
                });
            }
            None => {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!("parent `{p}` is not defined"),
                    help: Some(format!(
                        "declare `{p}` with `entity {p}:` or `item {p}:` before extending it"
                    )),
                });
            }
        }
    } else {
        None
    };

    let mut field_defaults = HashMap::new();
    let mut methods = HashMap::new();
    for member in members {
        match member {
            DeclMember::Field { name: fname, value, .. } => {
                let v = eval_expr(env, value)?;
                field_defaults.insert(fname.clone(), v);
            }
            DeclMember::Method {
                name: mname,
                params,
                body,
                ..
            } => {
                methods.insert(
                    mname.clone(),
                    Rc::new(MethodDef {
                        params: params.clone(),
                        body: body.clone(),
                    }),
                );
            }
        }
    }

    let class = Rc::new(ClassDef {
        kind: kind.as_str(),
        name: name.to_string(),
        parent: parent_class,
        field_defaults,
        methods,
    });
    env.set(name.to_string(), Value::Class(class));
    Ok(())
}

fn eval_binary(
    env: &mut Env,
    op: BinOp,
    left: &Expr,
    right: &Expr,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    if matches!(op, BinOp::And) {
        let l = eval_expr(env, left)?;
        return if is_truthy(&l) { eval_expr(env, right) } else { Ok(l) };
    }
    if matches!(op, BinOp::Or) {
        let l = eval_expr(env, left)?;
        return if is_truthy(&l) { Ok(l) } else { eval_expr(env, right) };
    }
    let l = eval_expr(env, left)?;
    let r = eval_expr(env, right)?;
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => apply_arith(op, &l, &r, line, col),
        BinOp::Eq => Ok(Value::Bool(values_equal(&l, &r))),
        BinOp::Neq => Ok(Value::Bool(!values_equal(&l, &r))),
        BinOp::Lt => cmp_int(&l, &r, |a, b| a < b, |a, b| a < b, "<", line, col),
        BinOp::Gt => cmp_int(&l, &r, |a, b| a > b, |a, b| a > b, ">", line, col),
        BinOp::Lte => cmp_int(&l, &r, |a, b| a <= b, |a, b| a <= b, "<=", line, col),
        BinOp::Gte => cmp_int(&l, &r, |a, b| a >= b, |a, b| a >= b, ">=", line, col),
        BinOp::In => Ok(Value::Bool(value_in(&l, &r, line, col)?)),
        BinOp::NotIn => Ok(Value::Bool(!value_in(&l, &r, line, col)?)),
        BinOp::And | BinOp::Or => unreachable!("handled above"),
    }
}

fn value_in(
    needle: &Value,
    haystack: &Value,
    line: u32,
    col: u32,
) -> Result<bool, RuntimeError> {
    match haystack {
        Value::List(rc) => Ok(rc.borrow().iter().any(|v| values_equal(v, needle))),
        Value::Tuple(elems) => Ok(elems.iter().any(|v| values_equal(v, needle))),
        Value::Range { start, end, exclusive } => match needle {
            Value::Int(n) => {
                let upper = if *exclusive { *end } else { *end + 1 };
                Ok(*n >= *start && *n < upper)
            }
            _ => Ok(false),
        },
        Value::Str(s) => match needle {
            Value::Str(sub) => Ok(s.contains(sub.as_ref())),
            _ => Ok(false),
        },
        other => Err(RuntimeError {
            line,
            col,
            message: format!(
                "`in` expects a list, tuple, range, or string, got {}",
                other.type_name()
            ),
            help: None,
        }),
    }
}

fn apply_arith(
    op: BinOp,
    l: &Value,
    r: &Value,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    let op_str = match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        _ => unreachable!(),
    };
    // String concatenation via `+`.
    if matches!(op, BinOp::Add) {
        if let (Value::Str(a), Value::Str(b)) = (l, r) {
            let mut s = String::with_capacity(a.len() + b.len());
            s.push_str(a);
            s.push_str(b);
            return Ok(Value::Str(Rc::new(s)));
        }
    }
    let pair = match (l, r) {
        (Value::Int(a), Value::Int(b)) => NumPair::Ints(*a, *b),
        (Value::Float(a), Value::Float(b)) => NumPair::Floats(*a, *b),
        (Value::Int(a), Value::Float(b)) => NumPair::Floats(*a as f64, *b),
        (Value::Float(a), Value::Int(b)) => NumPair::Floats(*a, *b as f64),
        _ => {
            return Err(RuntimeError {
                line,
                col,
                message: format!(
                    "operator '{op_str}' is not defined on {} and {}",
                    l.type_name(),
                    r.type_name()
                ),
                help: None,
            })
        }
    };
    match (op, pair) {
        (BinOp::Add, NumPair::Ints(a, b)) => Ok(Value::Int(a + b)),
        (BinOp::Sub, NumPair::Ints(a, b)) => Ok(Value::Int(a - b)),
        (BinOp::Mul, NumPair::Ints(a, b)) => Ok(Value::Int(a * b)),
        (BinOp::Div, NumPair::Ints(_, 0)) => Err(RuntimeError {
            line,
            col,
            message: "division by zero".to_string(),
            help: Some("guard the divisor with `if b != 0:` before dividing".to_string()),
        }),
        (BinOp::Div, NumPair::Ints(a, b)) => Ok(Value::Int(a / b)),
        (BinOp::Add, NumPair::Floats(a, b)) => Ok(Value::Float(a + b)),
        (BinOp::Sub, NumPair::Floats(a, b)) => Ok(Value::Float(a - b)),
        (BinOp::Mul, NumPair::Floats(a, b)) => Ok(Value::Float(a * b)),
        (BinOp::Div, NumPair::Floats(a, b)) => Ok(Value::Float(a / b)),
        _ => unreachable!(),
    }
}

enum NumPair {
    Ints(i64, i64),
    Floats(f64, f64),
}

fn cmp_int(
    l: &Value,
    r: &Value,
    int_cmp: fn(i64, i64) -> bool,
    float_cmp: fn(f64, f64) -> bool,
    op_str: &str,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(int_cmp(*a, *b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(float_cmp(*a, *b))),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Bool(float_cmp(*a as f64, *b))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(float_cmp(*a, *b as f64))),
        _ => Err(RuntimeError {
            line,
            col,
            message: format!(
                "operator '{op_str}' is not defined on {} and {}",
                l.type_name(),
                r.type_name()
            ),
            help: None,
        }),
    }
}

fn values_equal(l: &Value, r: &Value) -> bool {
    match (l, r) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
        (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
        (Value::Percent(a), Value::Percent(b)) => a == b,
        (
            Value::Quantity { value: a, unit: u1 },
            Value::Quantity { value: b, unit: u2 },
        ) => a == b && u1 == u2,
        (
            Value::Range {
                start: s1,
                end: e1,
                exclusive: x1,
            },
            Value::Range {
                start: s2,
                end: e2,
                exclusive: x2,
            },
        ) => s1 == s2 && e1 == e2 && x1 == x2,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Tuple(a), Value::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| values_equal(x, y))
        }
        _ => false,
    }
}

fn is_truthy(v: &Value) -> bool {
    !matches!(v, Value::Bool(false))
}
