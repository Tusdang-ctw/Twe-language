//! AST → bytecode compiler.
//!
//! Phase-3 session 6: literal, unary, binary, comparison expressions.
//! Phase-3 session 8: locals, `let`/`var`, plain-name assignment,
//! `if`/`elif`/`else`, `while`, `break`, `continue`, short-circuit
//! `and` / `or`, top-level `print(<expr>)` lowered to `OP_PRINT`.
//!
//! Identifiers, function calls, classes, `for`, and the heap types
//! (lists, tuples, ranges, full strings, instance fields) come in
//! sessions 9 and 10. Until then, those produce `CompileError`s
//! that name the session where the feature lands.

use std::rc::Rc;

use crate::ast::{AssignOp, AssignTarget, BinOp, Expr, Program, Stmt, UnOp};
use crate::bytecode::{Chunk, OpCode};
use crate::value::Value;

/// What the compiler refuses to compile in the current session's
/// scope, with a source position.
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
/// value on the stack at the moment `OP_RETURN` fires.
pub fn compile_expr(expr: &Expr) -> Result<Chunk, CompileError> {
    let mut c = Compiler::new();
    c.emit_expr(expr)?;
    let line = expr.line();
    c.chunk.write_op(OpCode::Return, line);
    Ok(c.chunk)
}

/// Compile a whole program into a chunk that ends in `OP_RETURN`.
/// Top-level `let` / `var` declarations live as locals on the
/// stack; the chunk's "result" is `Nil` (the OP_RETURN at the end
/// pops nothing, so the VM returns its stack-empty default).
pub fn compile_program(program: &Program) -> Result<Chunk, CompileError> {
    let mut c = Compiler::new();
    for stmt in &program.stmts {
        c.emit_stmt(stmt)?;
    }
    let line = program.stmts.last().map(stmt_line).unwrap_or(1);
    c.chunk.write_op(OpCode::Return, line);
    Ok(c.chunk)
}

fn stmt_line(s: &Stmt) -> u32 {
    match s {
        Stmt::Let { line, .. }
        | Stmt::Assign { line, .. }
        | Stmt::If { line, .. }
        | Stmt::OnUpdate { line, .. }
        | Stmt::Decl { line, .. }
        | Stmt::FunctionDecl { line, .. }
        | Stmt::Return { line, .. }
        | Stmt::While { line, .. }
        | Stmt::For { line, .. }
        | Stmt::Break { line, .. }
        | Stmt::Continue { line, .. }
        | Stmt::Transition { line, .. }
        | Stmt::Spawn { line, .. }
        | Stmt::Despawn { line, .. } => *line,
        Stmt::Expr(e) => e.line(),
    }
}

struct Local {
    name: String,
    /// Block-scope depth at which this local was declared. -1 marks
    /// "in flight" (declared but the initialiser hasn't finished
    /// pushing its value); we forbid `let x = x` shadowing during
    /// that window.
    depth: i32,
}

struct LoopFrame {
    /// Byte offset to jump back to on `continue` and on the natural
    /// loop-end `OP_LOOP`.
    loop_start: usize,
    /// Stack-slot count when the loop began; `break`/`continue` must
    /// pop locals declared inside the loop before jumping.
    locals_at_entry: usize,
    /// `OP_JUMP` patch sites for `break` — patched to point past the
    /// loop on `end_loop`.
    breaks: Vec<usize>,
}

struct Compiler {
    chunk: Chunk,
    locals: Vec<Local>,
    scope_depth: i32,
    loops: Vec<LoopFrame>,
}

impl Compiler {
    fn new() -> Self {
        Self {
            chunk: Chunk::new(),
            locals: Vec::new(),
            scope_depth: 0,
            loops: Vec::new(),
        }
    }

    // --- statement emission ---

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        match stmt {
            Stmt::Let { name, value, line, .. } => {
                self.emit_expr(value)?;
                self.declare_local(name, *line, expr_col(value))?;
                // Local now occupies the top stack slot; mark it
                // initialised so further references resolve.
                self.mark_initialised();
            }
            Stmt::Assign { target, op, value, line, col } => {
                self.emit_assign(target, *op, value, *line, *col)?;
            }
            Stmt::Expr(e) => {
                // Top-level call to the bare `print` builtin lowers to
                // a single OP_PRINT — keeps Phase-3 programs printable
                // without yet routing through the full builtin pipeline.
                if let Expr::Call { callee, args, kwargs, line, col } = e {
                    if kwargs.is_empty() {
                        if let Expr::Ident { name, .. } = callee.as_ref() {
                            if name == "print" && args.len() == 1 {
                                self.emit_expr(&args[0])?;
                                self.chunk.write_op(OpCode::Print, *line);
                                return Ok(());
                            }
                            if name == "print" && args.is_empty() {
                                let idx = self
                                    .chunk
                                    .add_constant(Value::Str(Rc::new(String::new())));
                                self.chunk.write_op(OpCode::Constant, *line);
                                self.chunk.write_byte(idx, *line);
                                self.chunk.write_op(OpCode::Print, *line);
                                return Ok(());
                            }
                            if name == "print" {
                                return Err(CompileError {
                                    line: *line,
                                    col: *col,
                                    message:
                                        "bytecode `print(...)` shorthand takes 0 or 1 args; \
                                         multi-arg print lands with full builtin dispatch in session 9"
                                            .to_string(),
                                });
                            }
                        }
                    }
                }
                self.emit_expr(e)?;
                self.chunk.write_op(OpCode::Pop, e.line());
            }
            Stmt::If { cond, then_body, elifs, else_body, line, .. } => {
                self.emit_if(cond, then_body, elifs, else_body.as_deref(), *line)?;
            }
            Stmt::While { cond, body, line, .. } => {
                self.emit_while(cond, body, *line)?;
            }
            Stmt::Break { line, col } => {
                if self.loops.is_empty() {
                    return Err(CompileError {
                        line: *line,
                        col: *col,
                        message: "`break` outside of a loop".to_string(),
                    });
                }
                let pop_count = self.locals.len() - self.loops.last().unwrap().locals_at_entry;
                for _ in 0..pop_count {
                    self.chunk.write_op(OpCode::Pop, *line);
                }
                let jmp = self.emit_jump(OpCode::Jump, *line);
                self.loops.last_mut().unwrap().breaks.push(jmp);
            }
            Stmt::Continue { line, col } => {
                if self.loops.is_empty() {
                    return Err(CompileError {
                        line: *line,
                        col: *col,
                        message: "`continue` outside of a loop".to_string(),
                    });
                }
                let frame = self.loops.last().unwrap();
                let pop_count = self.locals.len() - frame.locals_at_entry;
                let loop_start = frame.loop_start;
                for _ in 0..pop_count {
                    self.chunk.write_op(OpCode::Pop, *line);
                }
                self.emit_loop(loop_start, *line)?;
            }
            Stmt::Return { value: _, line, col } => {
                return Err(self.unsupported(
                    "`return` (functions land in session 9)",
                    *line,
                    *col,
                ));
            }
            Stmt::FunctionDecl { line, col, .. } => {
                return Err(self.unsupported("`function` (session 9)", *line, *col));
            }
            Stmt::Decl { line, col, kind, .. } => {
                return Err(self.unsupported(
                    &format!("`{}` declarative block (session 11)", kind.as_str()),
                    *line,
                    *col,
                ));
            }
            Stmt::OnUpdate { line, col, .. } => {
                return Err(self.unsupported(
                    "top-level `on update(dt):` (session 11)",
                    *line,
                    *col,
                ));
            }
            Stmt::For { line, col, .. } => {
                return Err(self.unsupported(
                    "`for` (depends on Range; session 10)",
                    *line,
                    *col,
                ));
            }
            Stmt::Transition { line, col, .. }
            | Stmt::Spawn { line, col, .. }
            | Stmt::Despawn { line, col, .. } => {
                return Err(self.unsupported(
                    "scene / entity flow control (session 11)",
                    *line,
                    *col,
                ));
            }
        }
        Ok(())
    }

    fn emit_if(
        &mut self,
        cond: &Expr,
        then_body: &[Stmt],
        elifs: &[(Expr, Vec<Stmt>)],
        else_body: Option<&[Stmt]>,
        line: u32,
    ) -> Result<(), CompileError> {
        // Strategy: emit cond, OP_JUMP_IF_FALSE → next branch, then-body,
        // OP_JUMP → end. Repeat for each elif. Finally else-body.
        // Patch all the end-jumps to past the whole if-chain.
        self.emit_expr(cond)?;
        let jump_to_next = self.emit_jump(OpCode::JumpIfFalse, line);
        self.begin_scope();
        for s in then_body {
            self.emit_stmt(s)?;
        }
        self.end_scope(line);
        let mut end_jumps = Vec::new();
        end_jumps.push(self.emit_jump(OpCode::Jump, line));
        let mut next_patch = Some(jump_to_next);

        for (cond, body) in elifs {
            if let Some(p) = next_patch.take() {
                self.patch_jump(p)?;
            }
            self.emit_expr(cond)?;
            next_patch = Some(self.emit_jump(OpCode::JumpIfFalse, cond.line()));
            self.begin_scope();
            for s in body {
                self.emit_stmt(s)?;
            }
            self.end_scope(cond.line());
            end_jumps.push(self.emit_jump(OpCode::Jump, cond.line()));
        }

        if let Some(p) = next_patch.take() {
            self.patch_jump(p)?;
        }
        if let Some(else_body) = else_body {
            self.begin_scope();
            for s in else_body {
                self.emit_stmt(s)?;
            }
            self.end_scope(line);
        }
        for j in end_jumps {
            self.patch_jump(j)?;
        }
        Ok(())
    }

    fn emit_while(
        &mut self,
        cond: &Expr,
        body: &[Stmt],
        line: u32,
    ) -> Result<(), CompileError> {
        let loop_start = self.chunk.code.len();
        self.loops.push(LoopFrame {
            loop_start,
            locals_at_entry: self.locals.len(),
            breaks: Vec::new(),
        });
        self.emit_expr(cond)?;
        let exit_jump = self.emit_jump(OpCode::JumpIfFalse, line);
        self.begin_scope();
        for s in body {
            self.emit_stmt(s)?;
        }
        self.end_scope(line);
        self.emit_loop(loop_start, line)?;
        self.patch_jump(exit_jump)?;
        let frame = self.loops.pop().expect("loop frame missing");
        for b in frame.breaks {
            self.patch_jump(b)?;
        }
        Ok(())
    }

    fn emit_assign(
        &mut self,
        target: &AssignTarget,
        op: AssignOp,
        value: &Expr,
        line: u32,
        col: u32,
    ) -> Result<(), CompileError> {
        match target {
            AssignTarget::Name(name) => {
                let slot = self.resolve_local(name).ok_or_else(|| CompileError {
                    line,
                    col,
                    message: format!("name `{name}` is not defined"),
                })?;
                if matches!(op, AssignOp::Set) {
                    self.emit_expr(value)?;
                } else {
                    // Compound: load current, evaluate RHS, apply op.
                    self.chunk.write_op(OpCode::GetLocal, line);
                    self.chunk.write_byte(slot, line);
                    self.emit_expr(value)?;
                    let arith = match op {
                        AssignOp::AddAssign => OpCode::Add,
                        AssignOp::SubAssign => OpCode::Sub,
                        AssignOp::MulAssign => OpCode::Mul,
                        AssignOp::DivAssign => OpCode::Div,
                        AssignOp::Set => unreachable!(),
                    };
                    self.chunk.write_op(arith, line);
                }
                self.chunk.write_op(OpCode::SetLocal, line);
                self.chunk.write_byte(slot, line);
                // Treat assignment as a statement: discard the produced
                // value left on top of stack by SetLocal.
                self.chunk.write_op(OpCode::Pop, line);
            }
            AssignTarget::Field { .. } => {
                return Err(self.unsupported(
                    "assignment to a field (heap; session 10/11)",
                    line,
                    col,
                ));
            }
        }
        Ok(())
    }

    // --- expression emission ---

    fn emit_expr(&mut self, expr: &Expr) -> Result<(), CompileError> {
        match expr {
            Expr::Int { value, line, .. } => {
                let idx = self.chunk.add_constant(Value::Int(*value));
                self.chunk.write_op(OpCode::Constant, *line);
                self.chunk.write_byte(idx, *line);
            }
            Expr::Float { value, line, .. } => {
                let idx = self.chunk.add_constant(Value::Float(*value));
                self.chunk.write_op(OpCode::Constant, *line);
                self.chunk.write_byte(idx, *line);
            }
            Expr::Bool { value, line, .. } => {
                self.chunk.write_op(
                    if *value { OpCode::True } else { OpCode::False },
                    *line,
                );
            }
            Expr::Str { value, line, .. } => {
                let idx = self
                    .chunk
                    .add_constant(Value::Str(Rc::new(value.clone())));
                self.chunk.write_op(OpCode::Constant, *line);
                self.chunk.write_byte(idx, *line);
            }
            Expr::Percent { value, line, .. } => {
                let idx = self.chunk.add_constant(Value::Float(*value));
                self.chunk.write_op(OpCode::Constant, *line);
                self.chunk.write_byte(idx, *line);
            }
            Expr::Ident { name, line, col } => {
                let slot = self.resolve_local(name).ok_or_else(|| CompileError {
                    line: *line,
                    col: *col,
                    message: format!("name `{name}` is not defined"),
                })?;
                self.chunk.write_op(OpCode::GetLocal, *line);
                self.chunk.write_byte(slot, *line);
            }
            Expr::Unary { op, operand, line, .. } => {
                self.emit_expr(operand)?;
                match op {
                    UnOp::Neg => self.chunk.write_op(OpCode::Neg, *line),
                    UnOp::Not => self.chunk.write_op(OpCode::Not, *line),
                }
            }
            Expr::Binary { op, left, right, line, col } => {
                if matches!(op, BinOp::And) {
                    return self.emit_and(left, right, *line);
                }
                if matches!(op, BinOp::Or) {
                    return self.emit_or(left, right, *line);
                }
                self.emit_expr(left)?;
                self.emit_expr(right)?;
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
                    BinOp::And | BinOp::Or => unreachable!("handled above"),
                    BinOp::In | BinOp::NotIn => {
                        return Err(self.unsupported(
                            "`in` / `not in` (heap; session 10)",
                            *line,
                            *col,
                        ));
                    }
                };
                self.chunk.write_op(op, *line);
            }
            // Heap / future-session features.
            Expr::SelfRef { line, col } => {
                return Err(self.unsupported("`self` (with method dispatch)", *line, *col));
            }
            Expr::Tuple { line, col, .. } => {
                return Err(self.unsupported("tuple literal (session 10)", *line, *col));
            }
            Expr::List { line, col, .. } => {
                return Err(self.unsupported("list literal (session 10)", *line, *col));
            }
            Expr::Range { line, col, .. } => {
                return Err(self.unsupported("range literal (session 10)", *line, *col));
            }
            Expr::Index { line, col, .. } => {
                return Err(self.unsupported("indexing (session 10)", *line, *col));
            }
            Expr::Field { line, col, .. } => {
                return Err(self.unsupported(
                    "field access (with method dispatch)",
                    *line,
                    *col,
                ));
            }
            Expr::Call { line, col, .. } => {
                return Err(self.unsupported("function call (session 9)", *line, *col));
            }
            Expr::Interp { line, col, .. } => {
                return Err(self.unsupported(
                    "string interpolation (session 10)",
                    *line,
                    *col,
                ));
            }
            Expr::Quantity { value, unit, line, col } => {
                return Err(self.unsupported(
                    &format!("quantity literal `{value}{unit}` (Phase 4)"),
                    *line,
                    *col,
                ));
            }
        }
        Ok(())
    }

    fn emit_and(&mut self, left: &Expr, right: &Expr, line: u32) -> Result<(), CompileError> {
        // `a and b`: emit a; if a is falsy, leave a as the result and
        // skip b. Otherwise pop a, evaluate b, and let b be the result.
        self.emit_expr(left)?;
        let end_jump = self.emit_jump(OpCode::JumpIfFalsePeek, line);
        self.chunk.write_op(OpCode::Pop, line);
        self.emit_expr(right)?;
        self.patch_jump(end_jump)?;
        Ok(())
    }

    fn emit_or(&mut self, left: &Expr, right: &Expr, line: u32) -> Result<(), CompileError> {
        // `a or b`: emit a; if a is truthy, leave a as the result and
        // skip b. Otherwise pop a, evaluate b, and let b be the result.
        self.emit_expr(left)?;
        let end_jump = self.emit_jump(OpCode::JumpIfTruePeek, line);
        self.chunk.write_op(OpCode::Pop, line);
        self.emit_expr(right)?;
        self.patch_jump(end_jump)?;
        Ok(())
    }

    // --- locals + scopes ---

    fn declare_local(
        &mut self,
        name: &str,
        line: u32,
        col: u32,
    ) -> Result<(), CompileError> {
        if self.locals.len() >= 256 {
            return Err(CompileError {
                line,
                col,
                message: "too many local variables in this function (max 256)".to_string(),
            });
        }
        // Same-scope shadowing is forbidden; outer-scope shadowing is fine.
        for local in self.locals.iter().rev() {
            if local.depth != -1 && local.depth < self.scope_depth {
                break;
            }
            if local.name == name {
                return Err(CompileError {
                    line,
                    col,
                    message: format!(
                        "name `{name}` is already declared in this scope"
                    ),
                });
            }
        }
        self.locals.push(Local {
            name: name.to_string(),
            depth: -1,
        });
        Ok(())
    }

    fn mark_initialised(&mut self) {
        if let Some(last) = self.locals.last_mut() {
            last.depth = self.scope_depth;
        }
    }

    fn resolve_local(&self, name: &str) -> Option<u8> {
        for (i, local) in self.locals.iter().enumerate().rev() {
            if local.name == name && local.depth != -1 {
                return Some(i as u8);
            }
        }
        None
    }

    fn begin_scope(&mut self) {
        self.scope_depth += 1;
    }

    fn end_scope(&mut self, line: u32) {
        self.scope_depth -= 1;
        while let Some(last) = self.locals.last() {
            if last.depth > self.scope_depth {
                self.chunk.write_op(OpCode::Pop, line);
                self.locals.pop();
            } else {
                break;
            }
        }
    }

    // --- jump emission and patching ---

    fn emit_jump(&mut self, op: OpCode, line: u32) -> usize {
        self.chunk.write_op(op, line);
        self.chunk.write_byte(0xFF, line);
        self.chunk.write_byte(0xFF, line);
        self.chunk.code.len() - 2
    }

    fn patch_jump(&mut self, offset: usize) -> Result<(), CompileError> {
        let jump = self.chunk.code.len() - offset - 2;
        if jump > u16::MAX as usize {
            return Err(CompileError {
                line: self.chunk.lines[offset],
                col: 0,
                message: format!(
                    "jump distance {jump} exceeds 16-bit limit"
                ),
            });
        }
        self.chunk.code[offset] = ((jump >> 8) & 0xFF) as u8;
        self.chunk.code[offset + 1] = (jump & 0xFF) as u8;
        Ok(())
    }

    fn emit_loop(&mut self, loop_start: usize, line: u32) -> Result<(), CompileError> {
        self.chunk.write_op(OpCode::Loop, line);
        // Distance = current_ip + 2 (operand) - loop_start, then the
        // VM subtracts that from post-operand IP to land at loop_start.
        let jump = self.chunk.code.len() - loop_start + 2;
        if jump > u16::MAX as usize {
            return Err(CompileError {
                line,
                col: 0,
                message: format!(
                    "loop body too large for 16-bit OP_LOOP offset ({jump})"
                ),
            });
        }
        self.chunk.write_byte(((jump >> 8) & 0xFF) as u8, line);
        self.chunk.write_byte((jump & 0xFF) as u8, line);
        Ok(())
    }

    fn unsupported(&self, what: &str, line: u32, col: u32) -> CompileError {
        CompileError {
            line,
            col,
            message: format!("bytecode compiler doesn't yet support {what}"),
        }
    }
}

fn expr_col(e: &Expr) -> u32 {
    e.col()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::disassemble;
    use crate::lexer;
    use crate::parser;

    fn parse_one_expr(src: &str) -> Expr {
        let with_newline = if src.ends_with('\n') {
            src.to_string()
        } else {
            format!("{src}\n")
        };
        let tokens = lexer::lex(&with_newline).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        match program.stmts.into_iter().next().expect("at least one stmt") {
            Stmt::Expr(e) => e,
            other => panic!("expected expression statement, got {other:?}"),
        }
    }

    fn parse_program(src: &str) -> Program {
        let with_newline = if src.ends_with('\n') {
            src.to_string()
        } else {
            format!("{src}\n")
        };
        let tokens = lexer::lex(&with_newline).expect("lex");
        parser::parse(&tokens).expect("parse")
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
    fn compile_unary_negation_then_not() {
        let chunk = compile_expr(&parse_one_expr("-5")).expect("compile");
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
    fn compile_arithmetic_emits_left_right_op() {
        let expr = parse_one_expr("1 + 2 * 3");
        let chunk = compile_expr(&expr).expect("compile");
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
    }

    #[test]
    fn compile_let_then_use_local() {
        let prog = parse_program("let x = 5\nprint(x)\n");
        let chunk = compile_program(&prog).expect("compile");
        // Emit: const 0 (5), get_local 0, print, return.
        assert_eq!(
            chunk.code,
            vec![
                OpCode::Constant as u8, 0,
                OpCode::GetLocal as u8, 0,
                OpCode::Print as u8,
                OpCode::Return as u8,
            ]
        );
    }

    #[test]
    fn compile_undefined_name_errors_with_session_8_message() {
        let err = compile_program(&parse_program("print(missing)\n")).expect_err("err");
        assert!(err.message.contains("`missing`"), "got: {}", err.message);
        assert!(err.message.contains("not defined"), "got: {}", err.message);
    }

    #[test]
    fn compile_short_circuit_emits_jump_pattern() {
        // `a and b` → emit a, JUMP_IF_FALSE_PEEK over (POP + b), patch.
        let prog = parse_program("let a = true\nlet b = false\nprint(a and b)\n");
        let chunk = compile_program(&prog).expect("compile");
        // Find the OP_JUMP_IF_FALSE_PEEK byte.
        let has_peek = chunk.code.contains(&(OpCode::JumpIfFalsePeek as u8));
        assert!(has_peek, "expected JumpIfFalsePeek for `and`: {:?}", chunk.code);
        // And `or` uses JumpIfTruePeek.
        let prog = parse_program("let a = true\nlet b = false\nprint(a or b)\n");
        let chunk = compile_program(&prog).expect("compile");
        let has_peek = chunk.code.contains(&(OpCode::JumpIfTruePeek as u8));
        assert!(has_peek, "expected JumpIfTruePeek for `or`: {:?}", chunk.code);
    }

    #[test]
    fn compile_if_emits_two_jumps() {
        let prog = parse_program("if 1 < 2:\n    print(1)\nelse:\n    print(2)\n");
        let chunk = compile_program(&prog).expect("compile");
        let n_if_false = chunk
            .code
            .iter()
            .filter(|b| **b == OpCode::JumpIfFalse as u8)
            .count();
        let n_jump = chunk
            .code
            .iter()
            .filter(|b| **b == OpCode::Jump as u8)
            .count();
        assert_eq!(n_if_false, 1, "one JumpIfFalse for the cond");
        assert_eq!(n_jump, 1, "one Jump after the then branch");
    }

    #[test]
    fn compile_while_emits_loop_back() {
        let prog = parse_program("let n = 0\nwhile n < 3:\n    n = n + 1\n");
        let chunk = compile_program(&prog).expect("compile");
        let has_loop = chunk.code.contains(&(OpCode::Loop as u8));
        assert!(has_loop, "expected OP_LOOP: {:?}", chunk.code);
    }

    #[test]
    fn compile_break_outside_loop_errors() {
        let err = compile_program(&parse_program("break\n")).expect_err("err");
        assert!(err.message.contains("`break`"), "got: {}", err.message);
    }

    #[test]
    fn compile_function_call_other_than_print_is_unsupported() {
        let err = compile_program(&parse_program("foo(1)\n")).expect_err("err");
        assert!(
            err.message.contains("function call") || err.message.contains("not defined"),
            "got: {}",
            err.message
        );
    }
}
