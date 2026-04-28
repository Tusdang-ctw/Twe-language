//! AST → bytecode compiler. Phase-3 session 6 scope: literal,
//! unary, binary, and comparison expressions only. Identifiers,
//! short-circuit `and` / `or`, control flow, function calls, and
//! the heap types (lists, tuples, ranges, strings beyond the
//! constant pool) come in subsequent sessions.
//!
//! The output is a `Chunk` ending in `OP_RETURN`; in this session
//! the chunk is unrunnable (no VM yet) and tested via the
//! disassembler. Session 7 adds the dispatch loop and the same
//! chunks become runnable.

use std::rc::Rc;

use crate::ast::{BinOp, Expr, UnOp};
use crate::bytecode::{Chunk, OpCode};
use crate::value::Value;

/// What the compiler refuses to compile in the current session's
/// scope, with a source position. Distinct from `RuntimeError` and
/// `ParseError` because compile failures point at "a feature this
/// VM session doesn't support yet" rather than user error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

impl std::error::Error for CompileError {}

/// Compile a single expression into a fresh chunk that ends in
/// `OP_RETURN`. The chunk evaluates the expression and leaves its
/// value on the stack at the moment `OP_RETURN` fires (the dispatch
/// loop in session 7 will return that as the program result).
pub fn compile_expr(expr: &Expr) -> Result<Chunk, CompileError> {
    let mut chunk = Chunk::new();
    emit_expr(&mut chunk, expr)?;
    let line = expr.line();
    chunk.write_op(OpCode::Return, line);
    Ok(chunk)
}

fn emit_expr(chunk: &mut Chunk, expr: &Expr) -> Result<(), CompileError> {
    match expr {
        Expr::Int { value, line, .. } => {
            let idx = chunk.add_constant(Value::Int(*value));
            chunk.write_op(OpCode::Constant, *line);
            chunk.write_byte(idx, *line);
        }
        Expr::Float { value, line, .. } => {
            let idx = chunk.add_constant(Value::Float(*value));
            chunk.write_op(OpCode::Constant, *line);
            chunk.write_byte(idx, *line);
        }
        Expr::Bool { value, line, .. } => {
            chunk.write_op(if *value { OpCode::True } else { OpCode::False }, *line);
        }
        Expr::Str { value, line, .. } => {
            let idx = chunk.add_constant(Value::Str(Rc::new(value.clone())));
            chunk.write_op(OpCode::Constant, *line);
            chunk.write_byte(idx, *line);
        }
        Expr::Unary { op, operand, line, .. } => {
            emit_expr(chunk, operand)?;
            match op {
                UnOp::Neg => chunk.write_op(OpCode::Neg, *line),
                UnOp::Not => chunk.write_op(OpCode::Not, *line),
            }
        }
        Expr::Binary { op, left, right, line, .. } => {
            emit_expr(chunk, left)?;
            emit_expr(chunk, right)?;
            let op = match op {
                BinOp::Add => OpCode::Add,
                BinOp::Sub => OpCode::Sub,
                BinOp::Mul => OpCode::Mul,
                BinOp::Div => OpCode::Div,
                BinOp::Eq => OpCode::Equal,
                BinOp::Neq => OpCode::NotEqual,
                BinOp::Lt => OpCode::Less,
                BinOp::Lte => OpCode::LessEqual,
                BinOp::Gt => OpCode::Greater,
                BinOp::Gte => OpCode::GreaterEqual,
                BinOp::And | BinOp::Or => {
                    return Err(unsupported(
                        "short-circuit `and` / `or`",
                        *line,
                        expr.col(),
                    ));
                }
                BinOp::In | BinOp::NotIn => {
                    return Err(unsupported(
                        "`in` / `not in`",
                        *line,
                        expr.col(),
                    ));
                }
            };
            chunk.write_op(op, *line);
        }
        // Everything below is out of scope for session 6 — sessions 7
        // (dispatch), 8 (locals + control flow), 9 (functions), and
        // 10 (heap types) close these one at a time.
        Expr::Ident { name, line, col } => {
            return Err(unsupported(
                &format!("identifier `{name}` (locals land in session 8)"),
                *line,
                *col,
            ));
        }
        Expr::SelfRef { line, col } => {
            return Err(unsupported("`self` (lands with method dispatch)", *line, *col));
        }
        Expr::Tuple { line, col, .. } => {
            return Err(unsupported("tuple literal (heap; session 10)", *line, *col));
        }
        Expr::List { line, col, .. } => {
            return Err(unsupported("list literal (heap; session 10)", *line, *col));
        }
        Expr::Range { line, col, .. } => {
            return Err(unsupported("range literal (heap; session 10)", *line, *col));
        }
        Expr::Index { line, col, .. } => {
            return Err(unsupported("indexing (heap; session 10)", *line, *col));
        }
        Expr::Field { line, col, .. } => {
            return Err(unsupported(
                "field access (lands with method dispatch)",
                *line,
                *col,
            ));
        }
        Expr::Call { line, col, .. } => {
            return Err(unsupported("function call (session 9)", *line, *col));
        }
        Expr::Interp { line, col, .. } => {
            return Err(unsupported(
                "string interpolation (heap; session 10)",
                *line,
                *col,
            ));
        }
        Expr::Percent { value, line, .. } => {
            // Percent literals stay numeric for now; v0.2 distinguishes
            // them via the type system. Treat as Float for the bytecode
            // VM until then.
            let idx = chunk.add_constant(Value::Float(*value));
            chunk.write_op(OpCode::Constant, *line);
            chunk.write_byte(idx, *line);
        }
        Expr::Quantity { value, unit, line, col } => {
            return Err(unsupported(
                &format!("quantity literal `{value}{unit}` (units in Phase 4)"),
                *line,
                *col,
            ));
        }
    }
    Ok(())
}

fn unsupported(what: &str, line: u32, col: u32) -> CompileError {
    CompileError {
        line,
        col,
        message: format!("bytecode compiler doesn't yet support {what}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::disassemble;
    use crate::lexer;
    use crate::parser;

    fn parse_one_expr(src: &str) -> Expr {
        // Parse the whole file, expect a single ExprStmt at the top level.
        let with_newline = if src.ends_with('\n') {
            src.to_string()
        } else {
            format!("{src}\n")
        };
        let tokens = lexer::lex(&with_newline).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        match program.stmts.into_iter().next().expect("at least one stmt") {
            crate::ast::Stmt::Expr(e) => e,
            other => panic!("expected expression statement, got {other:?}"),
        }
    }

    #[test]
    fn compile_int_literal_emits_constant_then_return() {
        let expr = parse_one_expr("42");
        let chunk = compile_expr(&expr).expect("compile");
        let dis = disassemble(&chunk, "int");
        assert_eq!(
            dis,
            "== int ==\n0000    1 OP_CONSTANT         0 '42'\n0002    | OP_RETURN\n"
        );
    }

    #[test]
    fn compile_bool_literals_use_dedicated_opcodes() {
        let chunk = compile_expr(&parse_one_expr("true")).expect("compile true");
        assert_eq!(chunk.code, vec![OpCode::True as u8, OpCode::Return as u8]);
        let chunk = compile_expr(&parse_one_expr("false")).expect("compile false");
        assert_eq!(chunk.code, vec![OpCode::False as u8, OpCode::Return as u8]);
    }

    #[test]
    fn compile_arithmetic_emits_left_right_op() {
        let expr = parse_one_expr("1 + 2 * 3");
        let chunk = compile_expr(&expr).expect("compile");
        // Postfix order: 1, 2, 3, *, +, return
        assert_eq!(
            chunk.code,
            vec![
                OpCode::Constant as u8, 0,
                OpCode::Constant as u8, 1,
                OpCode::Constant as u8, 2,
                OpCode::Mul as u8,
                OpCode::Add as u8,
                OpCode::Return as u8,
            ]
        );
        // Constant pool is in source order (1, 2, 3).
        assert!(matches!(chunk.constants[0], Value::Int(1)));
        assert!(matches!(chunk.constants[1], Value::Int(2)));
        assert!(matches!(chunk.constants[2], Value::Int(3)));
    }

    #[test]
    fn compile_unary_negation_then_not() {
        let chunk = compile_expr(&parse_one_expr("-5")).expect("compile");
        // 5 (constant), Neg, Return
        assert_eq!(
            chunk.code,
            vec![OpCode::Constant as u8, 0, OpCode::Neg as u8, OpCode::Return as u8]
        );
        let chunk = compile_expr(&parse_one_expr("not true")).expect("compile");
        assert_eq!(
            chunk.code,
            vec![OpCode::True as u8, OpCode::Not as u8, OpCode::Return as u8]
        );
    }

    #[test]
    fn compile_comparison_emits_op_per_operator() {
        let cases = [
            ("1 == 2", OpCode::Equal),
            ("1 != 2", OpCode::NotEqual),
            ("1 <  2", OpCode::Less),
            ("1 <= 2", OpCode::LessEqual),
            ("1 >  2", OpCode::Greater),
            ("1 >= 2", OpCode::GreaterEqual),
        ];
        for (src, expected_op) in cases {
            let chunk = compile_expr(&parse_one_expr(src)).expect("compile");
            // 4-byte sequence: constant 0, constant 1, op, return.
            // Skip-checking constant indices and just look at the op byte.
            let op_byte_idx = 4;
            assert_eq!(
                chunk.code[op_byte_idx], expected_op as u8,
                "src `{src}` should emit {expected_op:?}"
            );
        }
    }

    #[test]
    fn compile_short_circuit_is_unsupported_in_session_6() {
        let err = compile_expr(&parse_one_expr("true or false")).expect_err("err");
        assert!(err.message.contains("`and` / `or`"), "got: {}", err.message);
    }

    #[test]
    fn compile_identifier_is_unsupported_in_session_6() {
        let err = compile_expr(&parse_one_expr("x")).expect_err("err");
        assert!(err.message.contains("identifier"), "got: {}", err.message);
    }

    #[test]
    fn compile_call_is_unsupported_in_session_6() {
        // `print(1)` parses as Call(Ident "print", [Int 1]). The Call
        // arm fires before the inner Ident, but either error message
        // is fine for this assertion.
        let err = compile_expr(&parse_one_expr("print(1)")).expect_err("err");
        assert!(
            err.message.contains("function call") || err.message.contains("identifier"),
            "got: {}",
            err.message
        );
    }
}
