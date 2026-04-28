//! Bytecode dispatch loop. Phase-3 sessions 7–9.
//!
//! Reads a `Chunk` produced by `crate::compiler` and evaluates it
//! against an internal value stack. Semantics match the tree-walker
//! in `crate::eval` for the subset that's compiled so far: numeric
//! arithmetic with int/float coercion, string `+` concatenation,
//! structural `==` / `!=`, numeric comparisons, control flow,
//! globals, and (session 9) user-defined functions with calls and
//! returns.
//!
//! Frame layout per *Crafting Interpreters* §24.4:
//! - The top-level script is wrapped in a synthetic `BcFunction`
//!   named `<script>` with arity 0. Its frame's slot 0 holds the
//!   script-function value.
//! - Each `OP_CALL` creates a new `CallFrame { function, ip,
//!   slot_base }`. `slot_base` points at the function value on the
//!   value stack; arguments naturally live at slots 1..=arity.
//! - `OP_RETURN` collapses the callee's slots back to `slot_base`
//!   and pushes the return value, leaving the caller's stack as if
//!   the call expression evaluated to that value.

use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::{BcFunction, Chunk, OpCode};
use crate::value::{RuntimeError, Value};

const FRAMES_MAX: usize = 256;

/// One activation record: which compiled function is running, where
/// in its chunk we are, and where on the value stack its locals begin.
struct CallFrame {
    function: Rc<BcFunction>,
    ip: usize,
    slot_base: usize,
}

/// Stack-based VM state. One instance is short-lived: build it, run
/// a chunk, read the result. The `globals` HashMap persists across
/// statements within a single `run` but does not survive a fresh
/// `VM::new()`.
pub struct VM {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    globals: HashMap<String, Value>,
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
            frames: Vec::with_capacity(64),
            globals: HashMap::new(),
            out: String::new(),
        }
    }

    /// Run a chunk to completion. The chunk is wrapped in a synthetic
    /// `<script>` function for uniform frame dispatch. Returns the
    /// value left on top of the stack at the script's `OP_RETURN`
    /// (or `Nil` if the stack is empty there).
    pub fn run(&mut self, chunk: &Chunk) -> Result<Value, RuntimeError> {
        // Wrap the script chunk in a BcFunction so the dispatch loop
        // is uniformly frame-based. The arity is 0; slot 0 holds the
        // script function itself (unused by the body but reserved by
        // the compiler so local indices line up).
        let script = Rc::new(BcFunction::new("<script>", 0, chunk.clone()));
        self.stack.push(Value::BcFunction(script.clone()));
        self.frames.push(CallFrame {
            function: script,
            ip: 0,
            slot_base: 0,
        });
        self.dispatch()
    }

    fn dispatch(&mut self) -> Result<Value, RuntimeError> {
        loop {
            // Borrow info from the active frame without holding the
            // borrow across the match arms — the dispatch arms need
            // mutable access to `self.frames` (for push/pop) and to
            // `self.stack`, so we copy the bytes we need each tick.
            let (op, line, ip_after_op) = {
                let frame = self.frames.last().expect("at least one frame");
                let chunk = &frame.function.chunk;
                if frame.ip >= chunk.code.len() {
                    // Bytecode without an explicit OP_RETURN at the end
                    // would land here; the compiler always emits one,
                    // so this is a defensive fallthrough.
                    return Ok(self.stack.pop().unwrap_or(Value::Nil));
                }
                let line = chunk.lines[frame.ip];
                let op = OpCode::from_u8(chunk.code[frame.ip]);
                (op, line, frame.ip + 1)
            };
            self.frames.last_mut().unwrap().ip = ip_after_op;

            match op {
                OpCode::Constant => {
                    let idx = self.read_byte() as usize;
                    let value = self.read_constant(idx);
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
                    let result = self.pop()?;
                    let frame = self.frames.pop().expect("frame to return from");
                    if self.frames.is_empty() {
                        // Script frame ended. Drop the synthetic script
                        // function value sitting at slot_base and exit.
                        self.stack.truncate(frame.slot_base);
                        return Ok(result);
                    }
                    // Caller frame: collapse the callee's slots, push
                    // the return value as the call expression's result.
                    self.stack.truncate(frame.slot_base);
                    self.push(result);
                }
                OpCode::GetLocal => {
                    let slot = self.read_byte() as usize;
                    let abs = self.frames.last().unwrap().slot_base + slot;
                    let v = self.stack.get(abs).cloned().ok_or_else(|| RuntimeError {
                        line,
                        col: 0,
                        message: format!(
                            "vm: local slot {slot} (abs {abs}) out of range \
                             (stack has {})",
                            self.stack.len()
                        ),
                        help: Some(
                            "compiler bug — slot was emitted past the live frame".to_string(),
                        ),
                    })?;
                    self.push(v);
                }
                OpCode::SetLocal => {
                    let slot = self.read_byte() as usize;
                    let abs = self.frames.last().unwrap().slot_base + slot;
                    if abs >= self.stack.len() {
                        return Err(RuntimeError {
                            line,
                            col: 0,
                            message: format!(
                                "vm: SetLocal slot {slot} (abs {abs}) past stack top ({})",
                                self.stack.len()
                            ),
                            help: None,
                        });
                    }
                    // Peek, don't pop: the assignment expression keeps
                    // its value on the stack so the producer's OP_POP
                    // drops it cleanly.
                    let v = self.stack.last().cloned().unwrap();
                    self.stack[abs] = v;
                }
                OpCode::JumpIfFalse => {
                    let offset = self.read_u16();
                    let v = self.pop()?;
                    if !is_truthy(&v) {
                        self.frames.last_mut().unwrap().ip += offset as usize;
                    }
                }
                OpCode::JumpIfFalsePeek => {
                    let offset = self.read_u16();
                    let truthy = self
                        .stack
                        .last()
                        .map(is_truthy)
                        .ok_or_else(|| RuntimeError {
                            line,
                            col: 0,
                            message: "vm: stack underflow on JumpIfFalsePeek".to_string(),
                            help: None,
                        })?;
                    if !truthy {
                        self.frames.last_mut().unwrap().ip += offset as usize;
                    }
                }
                OpCode::JumpIfTruePeek => {
                    let offset = self.read_u16();
                    let truthy = self
                        .stack
                        .last()
                        .map(is_truthy)
                        .ok_or_else(|| RuntimeError {
                            line,
                            col: 0,
                            message: "vm: stack underflow on JumpIfTruePeek".to_string(),
                            help: None,
                        })?;
                    if truthy {
                        self.frames.last_mut().unwrap().ip += offset as usize;
                    }
                }
                OpCode::Jump => {
                    let offset = self.read_u16();
                    self.frames.last_mut().unwrap().ip += offset as usize;
                }
                OpCode::Loop => {
                    let offset = self.read_u16();
                    let frame = self.frames.last_mut().unwrap();
                    frame.ip = frame.ip.checked_sub(offset as usize).ok_or_else(|| {
                        RuntimeError {
                            line,
                            col: 0,
                            message: "vm: OP_LOOP offset underflow".to_string(),
                            help: None,
                        }
                    })?;
                }
                OpCode::DefineGlobal => {
                    let idx = self.read_byte() as usize;
                    let name = self.read_string_constant(idx, line)?;
                    let value = self.pop()?;
                    self.globals.insert(name, value);
                }
                OpCode::GetGlobal => {
                    let idx = self.read_byte() as usize;
                    let name = self.read_string_constant(idx, line)?;
                    let value = self.globals.get(&name).cloned().ok_or_else(|| {
                        RuntimeError {
                            line,
                            col: 0,
                            message: format!("name `{name}` is not defined"),
                            help: Some(format!(
                                "declare it with `let {name} = ...` before using it"
                            )),
                        }
                    })?;
                    self.push(value);
                }
                OpCode::SetGlobal => {
                    let idx = self.read_byte() as usize;
                    let name = self.read_string_constant(idx, line)?;
                    if !self.globals.contains_key(&name) {
                        return Err(RuntimeError {
                            line,
                            col: 0,
                            message: format!("name `{name}` is not defined"),
                            help: Some(format!(
                                "declare it with `let {name} = ...` before assigning"
                            )),
                        });
                    }
                    // Peek, don't pop — assignment is an expression;
                    // its statement caller will OP_POP the value.
                    let v = self
                        .stack
                        .last()
                        .cloned()
                        .ok_or_else(|| RuntimeError {
                            line,
                            col: 0,
                            message: "vm: stack underflow on SetGlobal".to_string(),
                            help: None,
                        })?;
                    self.globals.insert(name, v);
                }
                OpCode::Call => {
                    let arg_count = self.read_byte() as usize;
                    self.call_value(arg_count, line)?;
                }
            }
        }
    }

    /// Look up the value being called (sitting at `stack[top - arg_count]`),
    /// validate arity, push a fresh CallFrame so dispatch resumes inside
    /// the callee's chunk. Args remain on the stack as the new frame's
    /// locals 1..=arg_count, and the function value itself becomes the
    /// new frame's local 0 (per *Crafting Interpreters* §24.5).
    fn call_value(&mut self, arg_count: usize, line: u32) -> Result<(), RuntimeError> {
        let callee_idx = self
            .stack
            .len()
            .checked_sub(arg_count + 1)
            .ok_or_else(|| RuntimeError {
                line,
                col: 0,
                message: format!(
                    "vm: stack underflow on Call (arg_count={arg_count}, stack={})",
                    self.stack.len()
                ),
                help: None,
            })?;
        let callee = self.stack[callee_idx].clone();
        match callee {
            Value::BcFunction(func) => {
                if func.arity as usize != arg_count {
                    return Err(RuntimeError {
                        line,
                        col: 0,
                        message: format!(
                            "function `{}` expected {} arguments, got {}",
                            func.name, func.arity, arg_count
                        ),
                        help: None,
                    });
                }
                if self.frames.len() >= FRAMES_MAX {
                    return Err(RuntimeError {
                        line,
                        col: 0,
                        message: "stack overflow".to_string(),
                        help: Some(format!(
                            "call stack exceeded {FRAMES_MAX} frames — likely \
                             unbounded recursion"
                        )),
                    });
                }
                self.frames.push(CallFrame {
                    function: func,
                    ip: 0,
                    slot_base: callee_idx,
                });
                Ok(())
            }
            other => Err(RuntimeError {
                line,
                col: 0,
                message: format!(
                    "tried to call a {} (only functions are callable in bytecode v0.1)",
                    other.type_name()
                ),
                help: None,
            }),
        }
    }

    /// Read a single byte from the active frame's chunk and bump IP.
    fn read_byte(&mut self) -> u8 {
        let frame = self.frames.last_mut().expect("frame");
        let byte = frame.function.chunk.code[frame.ip];
        frame.ip += 1;
        byte
    }

    /// Read a big-endian u16 operand from the active frame's chunk
    /// and bump IP by 2.
    fn read_u16(&mut self) -> u16 {
        let frame = self.frames.last_mut().expect("frame");
        let chunk = &frame.function.chunk;
        let hi = chunk.code[frame.ip] as u16;
        let lo = chunk.code[frame.ip + 1] as u16;
        frame.ip += 2;
        (hi << 8) | lo
    }

    fn read_constant(&self, idx: usize) -> Value {
        let frame = self.frames.last().expect("frame");
        frame.function.chunk.constants[idx].clone()
    }

    fn read_string_constant(&self, idx: usize, line: u32) -> Result<String, RuntimeError> {
        match &self.frames.last().expect("frame").function.chunk.constants[idx] {
            Value::Str(s) => Ok(s.as_ref().clone()),
            other => Err(RuntimeError {
                line,
                col: 0,
                message: format!(
                    "vm: expected a string constant for global name, got {}",
                    other.type_name()
                ),
                help: Some("compiler bug — global ops must point at a Value::Str".to_string()),
            }),
        }
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

    fn run_program(src: &str) -> Result<String, RuntimeError> {
        let tokens = lexer::lex(&format!("{src}\n")).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let chunk =
            crate::compiler::compile_program(&program).expect("compile");
        let mut vm = VM::new();
        vm.run(&chunk)?;
        Ok(std::mem::take(&mut vm.out))
    }

    #[test]
    fn vm_runs_let_then_print() {
        let out = run_program("let x = 5\nprint(x)").expect("ok");
        assert_eq!(out, "5\n");
    }

    #[test]
    fn vm_runs_compound_assignment() {
        let out = run_program("let x = 10\nx += 5\nprint(x)").expect("ok");
        assert_eq!(out, "15\n");
    }

    #[test]
    fn vm_runs_if_else() {
        let out = run_program("if 1 < 2:\n    print(1)\nelse:\n    print(2)").expect("ok");
        assert_eq!(out, "1\n");
        let out = run_program("if 2 < 1:\n    print(1)\nelse:\n    print(2)").expect("ok");
        assert_eq!(out, "2\n");
    }

    #[test]
    fn vm_runs_elif_chain() {
        let out = run_program(
            "let x = 5\nif x < 3:\n    print(\"small\")\nelif x < 10:\n    print(\"medium\")\nelse:\n    print(\"large\")",
        )
        .expect("ok");
        assert_eq!(out, "medium\n");
    }

    #[test]
    fn vm_runs_while_loop() {
        let out = run_program("let n = 0\nwhile n < 3:\n    print(n)\n    n = n + 1").expect("ok");
        assert_eq!(out, "0\n1\n2\n");
    }

    #[test]
    fn vm_runs_break_inside_while() {
        let out = run_program(
            "let n = 0\nwhile n < 100:\n    if n == 3:\n        break\n    print(n)\n    n = n + 1",
        )
        .expect("ok");
        assert_eq!(out, "0\n1\n2\n");
    }

    #[test]
    fn vm_runs_continue_inside_while() {
        let out = run_program(
            "let n = 0\nwhile n < 5:\n    n = n + 1\n    if n == 2:\n        continue\n    print(n)",
        )
        .expect("ok");
        assert_eq!(out, "1\n3\n4\n5\n");
    }

    #[test]
    fn vm_short_circuits_and() {
        // Twe `and` is value-returning: true and 42 → 42.
        // The VM should produce the same result.
        let out = run_program("print(true and 42)").expect("ok");
        assert_eq!(out, "42\n");
        let out = run_program("print(false and 42)").expect("ok");
        assert_eq!(out, "false\n");
    }

    #[test]
    fn vm_short_circuits_or() {
        let out = run_program("print(false or 99)").expect("ok");
        assert_eq!(out, "99\n");
        let out = run_program("print(0 or \"default\")").expect("ok");
        // 0 is truthy in Twe — Principle 3.
        assert_eq!(out, "0\n");
    }

    #[test]
    fn vm_undefined_global_errors_at_runtime() {
        // Session 9 change: the compiler doesn't know whether a name is
        // a global or undefined; the VM errors at OP_GET_GLOBAL.
        let err = run_program("print(missing)").expect_err("should fail");
        assert!(err.message.contains("`missing`"), "got: {}", err.message);
        assert!(err.message.contains("not defined"), "got: {}", err.message);
    }

    #[test]
    fn vm_assign_to_undeclared_global_errors() {
        // Bare `x = 1` (no prior `let`) should error — matches the
        // tree-walker's behaviour of refusing to invent a binding.
        let err = run_program("x = 1").expect_err("should fail");
        assert!(err.message.contains("`x`"), "got: {}", err.message);
    }

    #[test]
    fn vm_calls_a_zero_arg_function() {
        let out = run_program(
            "function greet():\n    print(\"hi\")\n\ngreet()\n",
        )
        .expect("ok");
        assert_eq!(out, "hi\n");
    }

    #[test]
    fn vm_calls_a_function_with_args_and_return() {
        let out = run_program(
            "function add(a, b):\n    return a + b\n\nlet r = add(2, 3)\nprint(r)\n",
        )
        .expect("ok");
        assert_eq!(out, "5\n");
    }

    #[test]
    fn vm_function_without_explicit_return_returns_nil() {
        let out = run_program(
            "function noop():\n    let x = 1\n\nlet r = noop()\nprint(r)\n",
        )
        .expect("ok");
        assert_eq!(out, "nil\n");
    }

    #[test]
    fn vm_recursion_factorial() {
        // factorial(6) = 720. Direct recursion exercises the frame
        // stack: each call pushes a new CallFrame, OP_RETURN pops it.
        let src = "function fact(n):\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n\nprint(fact(6))\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "720\n");
    }

    #[test]
    fn vm_recursion_fibonacci() {
        // fib(10) = 55. Two recursive call sites per call exercises
        // the return-value-into-arg-slot collapse twice per fire.
        let src = "function fib(n):\n    if n < 2:\n        return n\n    return fib(n - 1) + fib(n - 2)\n\nprint(fib(10))\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "55\n");
    }

    #[test]
    fn vm_mutual_recursion_even_odd() {
        // is_even(4) → is_odd(3) → is_even(2) → is_odd(1) →
        // is_even(0) → true. Both functions need to be visible by
        // name from each other's body, which is the purpose of the
        // global table — they're bound before either is called.
        let src = "function is_even(n):\n    if n == 0:\n        return true\n    return is_odd(n - 1)\n\nfunction is_odd(n):\n    if n == 0:\n        return false\n    return is_even(n - 1)\n\nprint(is_even(4))\nprint(is_odd(7))\n";
        let out = run_program(src).expect("ok");
        assert_eq!(out, "true\ntrue\n");
    }

    #[test]
    fn vm_unbounded_recursion_overflows_with_helpful_message() {
        // No base case → the call-frame guard should fire long before
        // a real stack overflow.
        let src = "function loop():\n    return loop()\n\nloop()\n";
        let err = run_program(src).expect_err("should overflow");
        assert!(err.message.contains("stack overflow"), "got: {}", err.message);
    }

    #[test]
    fn vm_arity_mismatch_errors() {
        let src = "function takes_two(a, b):\n    return a + b\n\ntakes_two(1)\n";
        let err = run_program(src).expect_err("should fail");
        assert!(
            err.message.contains("expected 2"),
            "got: {}",
            err.message
        );
        assert!(err.message.contains("got 1"), "got: {}", err.message);
    }

    #[test]
    fn vm_calling_a_non_function_errors() {
        let src = "let x = 5\nx()\n";
        let err = run_program(src).expect_err("should fail");
        assert!(
            err.message.contains("tried to call"),
            "got: {}",
            err.message
        );
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

    #[test]
    fn vm_matches_eval_on_factorial_and_fibonacci() {
        // The session-9 cross-check: real recursive programs should
        // produce identical output on the bytecode VM and the tree-
        // walker. If these ever diverge, that's the canary for a
        // semantic drift between the two implementations.
        let cases = [
            "function fact(n):\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n\nprint(fact(5))\nprint(fact(8))\n",
            "function fib(n):\n    if n < 2:\n        return n\n    return fib(n - 1) + fib(n - 2)\n\nprint(fib(7))\nprint(fib(12))\n",
            "function is_even(n):\n    if n == 0:\n        return true\n    return is_odd(n - 1)\n\nfunction is_odd(n):\n    if n == 0:\n        return false\n    return is_even(n - 1)\n\nprint(is_even(10))\nprint(is_odd(10))\n",
        ];
        for src in cases {
            let bytecode_out = run_program(src)
                .unwrap_or_else(|e| panic!("bytecode failed on `{src}`: {e}"));
            let walker_out = crate::eval::run(
                &parser::parse(&lexer::lex(src).expect("lex")).expect("parse"),
            )
            .unwrap_or_else(|e| panic!("walker failed on `{src}`: {e}"));
            assert_eq!(
                bytecode_out, walker_out,
                "results diverge on `{src}`",
            );
        }
    }
}
