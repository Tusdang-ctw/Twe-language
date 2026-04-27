use std::rc::Rc;

use crate::ast::{AssignOp, AssignTarget, BinOp, Expr, Program, Stmt, UnOp};
use crate::stdlib;
use crate::value::{Env, OnUpdateHandler, RuntimeError, Value};

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
    for stmt in &program.stmts {
        eval_stmt(&mut env, stmt)?;
    }
    if let Some(handler) = env.on_update.clone() {
        for _ in 0..frames {
            env.set(handler.param.clone(), Value::Float(dt));
            for stmt in &handler.body {
                eval_stmt(&mut env, stmt)?;
            }
        }
    }
    Ok(env.out)
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
                for s in then_body {
                    eval_stmt(env, s)?;
                }
                return Ok(());
            }
            for (elif_cond, elif_body) in elifs {
                let v = eval_expr(env, elif_cond)?;
                if is_truthy(&v) {
                    for s in elif_body {
                        eval_stmt(env, s)?;
                    }
                    return Ok(());
                }
            }
            if let Some(eb) = else_body {
                for s in eb {
                    eval_stmt(env, s)?;
                }
            }
            Ok(())
        }
        Stmt::OnUpdate { param, body, .. } => {
            env.on_update = Some(OnUpdateHandler {
                param: param.clone(),
                body: body.clone(),
            });
            Ok(())
        }
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
            let Value::Object(rc) = obj_val else {
                return Err(RuntimeError {
                    line,
                    col,
                    message: format!("cannot assign field on value of type {}", obj_val.type_name()),
                    help: Some("only objects support field assignment".to_string()),
                });
            };
            let final_value = if matches!(op, AssignOp::Set) {
                new_value
            } else {
                let current = rc.borrow().fields.get(name).cloned().ok_or_else(|| RuntimeError {
                    line,
                    col,
                    message: format!("field '{name}' is not defined on this object"),
                    help: Some(format!("set it first with `obj.{name} = ...`")),
                })?;
                compound(op, &current, &new_value, line, col)?
            };
            // Special case: assigning .pos = (x, y) on a sprite-shaped object
            // also updates .x and .y. Mirrors Example 1's tuple-as-Vector2
            // behavior. See docs/01-examples.md Example 1 implied decisions.
            if name == "pos" {
                if let Value::Tuple(elems) = &final_value {
                    if elems.len() >= 2 {
                        let mut o = rc.borrow_mut();
                        o.fields.insert("x".to_string(), elems[0].clone());
                        o.fields.insert("y".to_string(), elems[1].clone());
                    }
                }
            }
            // And the converse: assigning .x or .y refreshes .pos.
            rc.borrow_mut().fields.insert(name.clone(), final_value);
            if name == "x" || name == "y" {
                refresh_pos(&rc);
            }
            Ok(())
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
        Expr::Int { value, .. } => Ok(Value::Int(*value)),
        Expr::Float { value, .. } => Ok(Value::Float(*value)),
        Expr::Bool { value, .. } => Ok(Value::Bool(*value)),
        Expr::Ident { name, line, col } => env.get(name).cloned().ok_or_else(|| RuntimeError {
            line: *line,
            col: *col,
            message: format!("name '{name}' is not defined"),
            help: Some(format!("declare it with `let {name} = ...` before use")),
        }),
        Expr::Tuple { elems, .. } => {
            let mut vals = Vec::with_capacity(elems.len());
            for e in elems {
                vals.push(eval_expr(env, e)?);
            }
            Ok(Value::Tuple(Rc::new(vals)))
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
        } => {
            let f = eval_expr(env, callee)?;
            let mut arg_vals = Vec::with_capacity(args.len());
            for a in args {
                arg_vals.push(eval_expr(env, a)?);
            }
            match f {
                Value::Builtin { func, .. } => func(env, &arg_vals),
                other => Err(RuntimeError {
                    line: *line,
                    col: *col,
                    message: format!("cannot call value of type {}", other.type_name()),
                    help: Some(
                        "only functions are callable; check that the name resolves to a function"
                            .to_string(),
                    ),
                }),
            }
        }
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
        Value::Object(rc) => rc.borrow().fields.get(name).cloned().ok_or_else(|| {
            RuntimeError {
                line,
                col,
                message: format!("field '{name}' is not defined on this object"),
                help: Some(format!("set it first with `obj.{name} = ...`")),
            }
        }),
        _ => Err(RuntimeError {
            line,
            col,
            message: format!("cannot read field on value of type {}", obj.type_name()),
            help: None,
        }),
    }
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
        BinOp::And | BinOp::Or => unreachable!("handled above"),
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
