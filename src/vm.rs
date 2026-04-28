//! Bytecode dispatch loop. Phase-3 session 7.
//!
//! Reads a `Chunk` produced by `crate::compiler` and evaluates it
//! against an internal value stack. Semantics match the tree-walker
//! in `crate::eval` for the subset that's compiled so far: numeric
//! arithmetic with int/float coercion, string `+` concatenation,
//! structural `==` / `!=`, and numeric comparisons. Tuple
//! arithmetic, list/range membership, and unit-aware ops stay in
//! eval until the bytecode compiler grows them (sessions 9–10).
//!
//! No globals, no functions, no control flow, no GC — this session
//! is just "run an expression chunk and return the top of stack."

use std::rc::Rc;

use crate::bytecode::{Chunk, OpCode};
use crate::value::{RuntimeError, Value};

/// Stack-based VM state. One instance is short-lived: build it, run
/// a chunk, read the result. Persistent state (globals, heap) lands
/// in later sessions.
pub struct VM {
    stack: Vec<Value>,
    /// Captured `print` output, mirroring `Env::out` from the tree
    /// walker so test harnesses can compare them.
    pub out: String,
}

impl Default for VM {
    fn default() -> Self {
        Self::new()
    }
}

impl VM {
    pub fn new() -> Self {
        Self {
            stack: Vec::with_capacity(256),
            out: String::new(),
        }
    }

    /// Run a chunk to completion. Returns the value left on top of
    /// the stack at `OP_RETURN` (or `Nil` if the stack is empty
    /// there). Treats `OP_RETURN` as program-exit since this session
    /// only has a single top-level frame.
    pub fn run(&mut self, chunk: &Chunk) -> Result<Value, RuntimeError> {
        let mut ip: usize = 0;
        while ip < chunk.code.len() {
            let line = chunk.lines[ip];
            let op = OpCode::from_u8(chunk.code[ip]);
            ip += 1;
            match op {
                OpCode::Constant => {
                    let idx = chunk.code[ip] as usize;
                    ip += 1;
                    let value = chunk.constants[idx].clone();
                    self.push(value);
                }
                OpCode::Nil => self.push(Value::Nil),
                OpCode::True => self.push(Value::Bool(true)),
                OpCode::False => self.push(Value::Bool(false)),
                OpCode::Pop => {
                    self.pop()?;
                }
                OpCode::Add => self.binary_arith("+", line, |a, b| a + b, |a, b| a + b)?,
                OpCode::Sub => self.binary_arith("-", line, |a, b| a - b, |a, b| a - b)?,
                OpCode::Mul => self.binary_arith("*", line, |a, b| a * b, |a, b| a * b)?,
                OpCode::Div => self.binary_div(line)?,
                OpCode::Mod => self.binary_mod(line)?,
                OpCode::Neg => self.unary_neg(line)?,
                OpCode::Not => {
                    let v = self.pop()?;
                    self.push(Value::Bool(!is_truthy(&v)));
                }
                OpCode::Equal => {
                    let r = self.pop()?;
                    let l = self.pop()?;
                    self.push(Value::Bool(values_equal(&l, &r)));
                }
                OpCode::NotEqual => {
                    let r = self.pop()?;
                    let l = self.pop()?;
                    self.push(Value::Bool(!values_equal(&l, &r)));
                }
                OpCode::Less => self.compare("<", line, |a, b| a < b, |a, b| a < b)?,
                OpCode::LessEqual => {
                    self.compare("<=", line, |a, b| a <= b, |a, b| a <= b)?
                }
                OpCode::Greater => self.compare(">", line, |a, b| a > b, |a, b| a > b)?,
                OpCode::GreaterEqual => {
                    self.compare(">=", line, |a, b| a >= b, |a, b| a >= b)?
                }
                OpCode::Print => {
                    let v = self.pop()?;
                    self.out.push_str(&v.display());
                    self.out.push('\n');
                }
                OpCode::Return => {
                    return Ok(self.stack.pop().unwrap_or(Value::Nil));
                }
            }
        }
        Ok(self.stack.pop().unwrap_or(Value::Nil))
    }

    fn push(&mut self, v: Value) {
        self.stack.push(v);
    }

    fn pop(&mut self) -> Result<Value, RuntimeError> {
        self.stack.pop().ok_or_else(|| RuntimeError {
            line: 0,
            col: 0,
            message: "vm: stack underflow".to_string(),
            help: Some("compiler bug — every consumer should push before pop".to_string()),
        })
    }

    fn binary_arith(
        &mut self,
        op_str: &str,
        line: u32,
        int_op: fn(i64, i64) -> i64,
        float_op: fn(f64, f64) -> f64,
    ) -> Result<(), RuntimeError> {
        let r = self.pop()?;
        let l = self.pop()?;
        // String concatenation via `+`.
        if op_str == "+" {
            if let (Value::Str(a), Value::Str(b)) = (&l, &r) {
                let mut s = String::with_capacity(a.len() + b.len());
                s.push_str(a);
                s.push_str(b);
                self.push(Value::Str(Rc::new(s)));
                return Ok(());
            }
        }
        let result = match (&l, &r) {
            (Value::Int(a), Value::Int(b)) => Value::Int(int_op(*a, *b)),
            (Value::Float(a), Value::Float(b)) => Value::Float(float_op(*a, *b)),
            (Value::Int(a), Value::Float(b)) => Value::Float(float_op(*a as f64, *b)),
            (Value::Float(a), Value::Int(b)) => Value::Float(float_op(*a, *b as f64)),
            _ => {
                return Err(type_error(op_str, &l, &r, line));
            }
        };
        self.push(result);
        Ok(())
    }

    fn binary_div(&mut self, line: u32) -> Result<(), RuntimeError> {
        let r = self.pop()?;
        let l = self.pop()?;
        let result = match (&l, &r) {
            (Value::Int(_), Value::Int(0)) => {
                return Err(division_by_zero(line));
            }
            (Value::Int(a), Value::Int(b)) => Value::Int(a / b),
            (Value::Float(a), Value::Float(b)) => Value::Float(a / b),
            (Value::Int(a), Value::Float(b)) => Value::Float(*a as f64 / *b),
            (Value::Float(a), Value::Int(b)) => Value::Float(*a / *b as f64),
            _ => return Err(type_error("/", &l, &r, line)),
        };
        self.push(result);
        Ok(())
    }

    fn binary_mod(&mut self, line: u32) -> Result<(), RuntimeError> {
        let r = self.pop()?;
        let l = self.pop()?;
        let result = match (&l, &r) {
            (Value::Int(_), Value::Int(0)) => {
                return Err(division_by_zero(line));
            }
            (Value::Int(a), Value::Int(b)) => Value::Int(a % b),
            _ => return Err(type_error("%", &l, &r, line)),
        };
        self.push(result);
        Ok(())
    }

    fn unary_neg(&mut self, line: u32) -> Result<(), RuntimeError> {
        let v = self.pop()?;
        let result = match v {
            Value::Int(n) => Value::Int(-n),
            Value::Float(f) => Value::Float(-f),
            other => {
                return Err(RuntimeError {
                    line,
                    col: 0,
                    message: format!("unary `-` is not defined on {}", other.type_name()),
                    help: None,
                });
            }
        };
        self.push(result);
        Ok(())
    }

    fn compare(
        &mut self,
        op_str: &str,
        line: u32,
        int_cmp: fn(i64, i64) -> bool,
        float_cmp: fn(f64, f64) -> bool,
    ) -> Result<(), RuntimeError> {
        let r = self.pop()?;
        let l = self.pop()?;
        let result = match (&l, &r) {
            (Value::Int(a), Value::Int(b)) => Value::Bool(int_cmp(*a, *b)),
            (Value::Float(a), Value::Float(b)) => Value::Bool(float_cmp(*a, *b)),
            (Value::Int(a), Value::Float(b)) => Value::Bool(float_cmp(*a as f64, *b)),
            (Value::Float(a), Value::Int(b)) => Value::Bool(float_cmp(*a, *b as f64)),
            _ => return Err(type_error(op_str, &l, &r, line)),
        };
        self.push(result);
        Ok(())
    }
}

fn type_error(op: &str, l: &Value, r: &Value, line: u32) -> RuntimeError {
    RuntimeError {
        line,
        col: 0,
        message: format!(
            "operator '{op}' is not defined on {} and {}",
            l.type_name(),
            r.type_name()
        ),
        help: None,
    }
}

fn division_by_zero(line: u32) -> RuntimeError {
    RuntimeError {
        line,
        col: 0,
        message: "division by zero".to_string(),
        help: Some("guard the divisor with `if b != 0:` before dividing".to_string()),
    }
}

/// Twe truthiness rule (Principle 3): only `false` is falsy.
fn is_truthy(v: &Value) -> bool {
    !matches!(v, Value::Bool(false))
}

fn values_equal(l: &Value, r: &Value) -> bool {
    // Mirror the tree-walker's `values_equal` in eval.rs for the
    // value types the bytecode VM currently sees: Nil, Bool, Int,
    // Float, Str. Heap types and instances arrive in later sessions
    // and will need to be added here at the same time they're added
    // to the compiler.
    match (l, r) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
        (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
        (Value::Str(a), Value::Str(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::compile_expr;
    use crate::lexer;
    use crate::parser;

    fn run_expr(src: &str) -> Result<Value, RuntimeError> {
        let tokens = lexer::lex(&format!("{src}\n")).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let expr = match program.stmts.into_iter().next().expect("at least one") {
            crate::ast::Stmt::Expr(e) => e,
            other => panic!("expected expression statement, got {other:?}"),
        };
        let chunk = compile_expr(&expr).expect("compile");
        let mut vm = VM::new();
        vm.run(&chunk)
    }

    #[test]
    fn vm_evaluates_int_arithmetic() {
        assert!(matches!(run_expr("1 + 2"), Ok(Value::Int(3))));
        assert!(matches!(run_expr("7 - 4"), Ok(Value::Int(3))));
        assert!(matches!(run_expr("3 * 4"), Ok(Value::Int(12))));
        assert!(matches!(run_expr("10 / 3"), Ok(Value::Int(3))));
        assert!(matches!(run_expr("1 + 2 * 3"), Ok(Value::Int(7))));
    }

    #[test]
    fn vm_evaluates_float_arithmetic_with_int_promotion() {
        assert!(matches!(run_expr("1.5 + 0.5"), Ok(Value::Float(v)) if v == 2.0));
        assert!(matches!(run_expr("1 + 0.5"), Ok(Value::Float(v)) if v == 1.5));
        assert!(matches!(run_expr("3.0 / 2"), Ok(Value::Float(v)) if v == 1.5));
    }

    #[test]
    fn vm_evaluates_unary_neg_and_not() {
        assert!(matches!(run_expr("-7"), Ok(Value::Int(-7))));
        assert!(matches!(run_expr("not true"), Ok(Value::Bool(false))));
        assert!(matches!(run_expr("not false"), Ok(Value::Bool(true))));
        // Twe truthiness: 0 is truthy; `not 0` is false.
        assert!(matches!(run_expr("not 0"), Ok(Value::Bool(false))));
    }

    #[test]
    fn vm_evaluates_comparisons() {
        assert!(matches!(run_expr("1 < 2"), Ok(Value::Bool(true))));
        assert!(matches!(run_expr("2 < 1"), Ok(Value::Bool(false))));
        assert!(matches!(run_expr("3 == 3"), Ok(Value::Bool(true))));
        assert!(matches!(run_expr("3 != 3"), Ok(Value::Bool(false))));
        assert!(matches!(run_expr("3 >= 3"), Ok(Value::Bool(true))));
        assert!(matches!(run_expr("2 <= 1"), Ok(Value::Bool(false))));
        // Cross-type numeric: int 3 vs float 3.0 is equal.
        assert!(matches!(run_expr("3 == 3.0"), Ok(Value::Bool(true))));
    }

    #[test]
    fn vm_concatenates_strings_with_plus() {
        let v = run_expr(r#""hello, " + "world""#).expect("ok");
        match v {
            Value::Str(s) => assert_eq!(s.as_ref(), "hello, world"),
            other => panic!("want Str, got {other:?}"),
        }
    }

    #[test]
    fn vm_division_by_zero_errors() {
        let err = run_expr("1 / 0").expect_err("should fail");
        assert!(err.message.contains("division by zero"), "got: {}", err.message);
    }

    #[test]
    fn vm_type_mismatch_errors() {
        let err = run_expr(r#"1 + "x""#).expect_err("should fail");
        assert!(err.message.contains("'+'"), "got: {}", err.message);
        assert!(err.message.contains("string"), "got: {}", err.message);
    }

    #[test]
    fn vm_matches_eval_results_on_arithmetic_corpus() {
        // Cross-check: every expression here should produce the same
        // result on the bytecode VM as on the tree-walker. This is
        // the key correctness gate for sessions 8-17 — every new
        // feature added to the bytecode VM should keep matching.
        let cases = [
            "1 + 2 + 3",
            "10 - 4 - 2",
            "2 * 3 * 4",
            "100 / 4 / 5",
            "1.5 + 2.5",
            "0.1 + 0.2",
            "-(1 + 2)",
            "1 == 2",
            "not (1 == 2)",
            "(1 + 2) * 3",
        ];
        for src in cases {
            let bytecode_result = run_expr(src).unwrap_or_else(|e| {
                panic!("bytecode failed on `{src}`: {e}")
            });
            // Run the same expression through the tree-walker by
            // wrapping it in `print(...)` and parsing the output.
            let walker_out = crate::eval::run(
                &parser::parse(
                    &lexer::lex(&format!("print({src})\n")).expect("lex"),
                )
                .expect("parse"),
            )
            .unwrap_or_else(|e| panic!("walker failed on `{src}`: {e}"));
            let walker_str = walker_out.trim_end_matches('\n').to_string();
            assert_eq!(
                bytecode_result.display(),
                walker_str,
                "results diverge on `{src}`: bytecode={bytecode_result:?}",
            );
        }
    }
}
