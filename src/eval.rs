use std::rc::Rc;

use crate::ast::{Expr, Program, Stmt};
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
        Expr::Ident { name, line, col } => env.get(name).cloned().ok_or_else(|| RuntimeError {
            line: *line,
            col: *col,
            message: format!("name '{name}' is not defined"),
            help: Some("declare it with `let {name} = ...` before use".replace("{name}", name)),
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
    }
}
