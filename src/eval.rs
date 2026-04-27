use std::rc::Rc;

use crate::ast::{BinOp, Expr, Program, Stmt, UnOp};
use crate::stdlib;
use crate::value::{Env, RuntimeError, Value};

pub fn run(program: &Program) -> Result<String, RuntimeError> {
    let mut env = Env::new();
    stdlib::install(&mut env);
    for stmt in &program.stmts {
        eval_stmt(&mut env, stmt)?;
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
        Stmt::Expr(e) => {
            eval_expr(env, e)?;
            Ok(())
        }
    }
}

fn eval_expr(env: &mut Env, expr: &Expr) -> Result<Value, RuntimeError> {
    match expr {
        Expr::Str { value, .. } => Ok(Value::Str(Rc::new(value.clone()))),
        Expr::Int { value, .. } => Ok(Value::Int(*value)),
        Expr::Bool { value, .. } => Ok(Value::Bool(*value)),
        Expr::Ident { name, line, col } => env.get(name).cloned().ok_or_else(|| RuntimeError {
            line: *line,
            col: *col,
            message: format!("name '{name}' is not defined"),
            help: Some(format!("declare it with `let {name} = ...` before use")),
        }),
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

fn eval_binary(
    env: &mut Env,
    op: BinOp,
    left: &Expr,
    right: &Expr,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    // Short-circuit logical operators.
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
        BinOp::Add => match (&l, &r) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
            _ => bin_type_error(&l, &r, "+", line, col),
        },
        BinOp::Sub => match (&l, &r) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
            _ => bin_type_error(&l, &r, "-", line, col),
        },
        BinOp::Mul => match (&l, &r) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
            _ => bin_type_error(&l, &r, "*", line, col),
        },
        BinOp::Div => match (&l, &r) {
            (Value::Int(_), Value::Int(0)) => Err(RuntimeError {
                line,
                col,
                message: "division by zero".to_string(),
                help: Some("guard the divisor with `if b != 0:` before dividing".to_string()),
            }),
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a / b)),
            _ => bin_type_error(&l, &r, "/", line, col),
        },
        BinOp::Eq => Ok(Value::Bool(values_equal(&l, &r))),
        BinOp::Neq => Ok(Value::Bool(!values_equal(&l, &r))),
        BinOp::Lt => cmp_int(&l, &r, |a, b| a < b, "<", line, col),
        BinOp::Gt => cmp_int(&l, &r, |a, b| a > b, ">", line, col),
        BinOp::Lte => cmp_int(&l, &r, |a, b| a <= b, "<=", line, col),
        BinOp::Gte => cmp_int(&l, &r, |a, b| a >= b, ">=", line, col),
        BinOp::And | BinOp::Or => unreachable!("handled above"),
    }
}

fn cmp_int(
    l: &Value,
    r: &Value,
    f: fn(i64, i64) -> bool,
    op_str: &str,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(f(*a, *b))),
        _ => bin_type_error(l, r, op_str, line, col),
    }
}

fn bin_type_error(
    l: &Value,
    r: &Value,
    op: &str,
    line: u32,
    col: u32,
) -> Result<Value, RuntimeError> {
    Err(RuntimeError {
        line,
        col,
        message: format!(
            "operator '{op}' is not defined on {} and {}",
            l.type_name(),
            r.type_name()
        ),
        help: None,
    })
}

fn values_equal(l: &Value, r: &Value) -> bool {
    match (l, r) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Str(a), Value::Str(b)) => a == b,
        _ => false,
    }
}

/// Per docs/03-runtime.md, only `false` is falsy; nil and everything else are truthy.
fn is_truthy(v: &Value) -> bool {
    !matches!(v, Value::Bool(false))
}
